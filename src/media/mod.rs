//! Playing a stream: [`demux`] reads it and hands packets to the [`audio`] and
//! [`video`] threads. Audio goes to an output (sound card or AirPlay speaker) as fast
//! as it takes it and drives the [`clock`]; video waits for the clock, so the picture
//! matches the sound wherever it is heard, even two seconds later on a speaker.
//! What plays can be [`record`]ed as it is read.

mod audio;
mod clock;
mod demux;
pub mod frame;
mod hwaccel;
mod manifest;
mod record;
pub mod subtitle;
mod video;

use crate::output::Output;
use crate::sync::Lock;
pub use frame::VideoFrame;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What to play.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub url: String,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    /// A live channel (no seeking, reconnect on drop-outs) rather than a recording.
    pub live: bool,
    /// Where to begin, in seconds from the start: a movie carried on.
    pub start_at: Option<f64>,
}

/// Why a stream stopped playing.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("the provider refused the stream (login or connection limit)")]
    Refused,
    #[error("the stream was not found")]
    NotFound,
    #[error("the provider's server failed")]
    ServerFailed,
    #[error("the channel isn't sending anything right now")]
    Empty,
    /// The connection or the stream itself (a network error, a broken container).
    #[error("{0}")]
    Stream(String),
    #[error("can't decode {0}")]
    Decode(String),
    #[error("couldn't start playing: {0}")]
    Start(String),
}

impl From<ffmpeg_next::Error> for Error {
    fn from(e: ffmpeg_next::Error) -> Self {
        use ffmpeg_next::Error as E;
        match e {
            E::HttpUnauthorized | E::HttpForbidden => Error::Refused,
            E::HttpNotFound => Error::NotFound,
            E::HttpServerError => Error::ServerFailed,
            other => Error::Stream(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Opening,
    Playing,
    /// Waiting for more of the stream.
    Buffering,
    Ended,
    Failed(Error),
}

/// What the stream is, for the info line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamInfo {
    /// e.g. `H.264`.
    pub video_codec: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: Option<f32>,
    /// e.g. `AAC`.
    pub audio_codec: Option<String>,
    pub audio_channels: u16,
    pub audio_tracks: Vec<Track>,
    /// Index into `audio_tracks` of the one playing.
    pub audio_track: Option<usize>,
    /// Length in seconds, for recordings.
    pub duration: Option<f64>,
    /// The stream time the stream starts at, when it says: positions count from here.
    pub start: Option<f64>,
    /// Subtitle tracks with text in them.
    pub subtitle_tracks: Vec<Track>,
    /// Index into `subtitle_tracks` of the one shown, if any.
    pub subtitle_track: Option<usize>,
}

/// Whether what plays is being recorded.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Recording {
    #[default]
    Off,
    /// Into this file.
    To(PathBuf),
    /// It stopped, for this reason.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The stream's index in the container.
    pub stream: usize,
    pub label: String,
}

/// State the threads share with the UI.
pub struct Shared {
    status: Mutex<Status>,
    info: Mutex<StreamInfo>,
    frame: Mutex<Option<Arc<VideoFrame>>>,
    /// Bumped with every new frame, so the UI uploads each only once.
    frame_serial: AtomicU64,
    clock: clock::Clock,
    stop: AtomicBool,
    paused: AtomicBool,
    /// Bumped on every seek: a frame waiting for its moment gives up.
    epoch: AtomicU64,
    /// Threads still playing out the end of the stream; the last one to finish
    /// marks it ended.
    finishing: AtomicU8,
    /// How much later than the output's own account the picture shows, in
    /// milliseconds: for speakers that take longer than they say.
    picture_delay_ms: AtomicI32,
    /// Where to carry on after a pause the output didn't keep its audio through.
    resume_at: Mutex<Option<f64>>,
    /// A problem worth telling about that didn't stop playback (e.g. the speaker
    /// couldn't be reached, so sound plays here).
    notice: Mutex<Option<String>>,
    /// The subtitles of the track shown, or of a file.
    cues: Mutex<subtitle::Cues>,
    recording: Mutex<Recording>,
    repaint: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn set_status(&self, s: Status) {
        let mut cur = self.status.locked();
        if *cur != s {
            log_debug!("player: {s:?}");
            *cur = s;
            drop(cur);
            (self.repaint)();
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn publish(&self, frame: VideoFrame) {
        *self.frame.locked() = Some(Arc::new(frame));
        self.frame_serial.fetch_add(1, Ordering::Release);
        (self.repaint)();
    }

    /// A decoding thread played everything up to the end of the stream.
    fn finished(&self) {
        if self.finishing.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.set_status(Status::Ended);
        }
    }

    fn notify(&self, notice: String) {
        log_warn!("player: {notice}");
        *self.notice.locked() = Some(notice);
        (self.repaint)();
    }
}

/// Requests to the audio thread.
enum AudioCmd {
    Volume(f32),
    Output(Output),
}

/// Requests to the demuxer.
enum DemuxCmd {
    Seek(f64),
    AudioTrack(usize),
    /// Show the subtitle track at this stream index, or none.
    Subtitles(Option<usize>),
    /// Record into this file, or stop recording.
    Record(Option<PathBuf>),
}

/// A stream being played. Dropping it stops playback.
pub struct Player {
    shared: Arc<Shared>,
    audio: Sender<AudioCmd>,
    demux: Sender<DemuxCmd>,
    pub request: Request,
    /// The demuxer, which ends after the decoding threads it started.
    thread: Option<JoinHandle<()>>,
}

/// A player told to stop, whose threads may still be closing the connection and
/// the output. The next player waits for them (on its own thread): providers often
/// allow one connection, and a speaker takes one sender at a time.
pub struct Stopping(Option<JoinHandle<()>>);

impl Stopping {
    /// Wait until the player has stopped and closed what it had open.
    pub fn wait(self) {
        if let Some(thread) = self.0 {
            let _ = thread.join();
        }
    }
}

impl Player {
    /// Start playing `request`, once `after` (the player before, if any) has stopped.
    pub fn open(
        request: Request,
        output: Output,
        volume: f32,
        after: Option<Stopping>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Player {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::Opening),
            info: Mutex::default(),
            frame: Mutex::new(None),
            frame_serial: AtomicU64::new(0),
            clock: clock::Clock::default(),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            finishing: AtomicU8::new(0),
            picture_delay_ms: AtomicI32::new(0),
            resume_at: Mutex::new(None),
            notice: Mutex::new(None),
            cues: Mutex::default(),
            recording: Mutex::default(),
            repaint: Box::new(repaint),
        });
        let (audio_tx, audio_rx) = channel();
        let (demux_tx, demux_rx) = channel();
        let settings = audio::Settings { output, volume, commands: audio_rx };
        let (s, r) = (shared.clone(), request.clone());
        let thread = std::thread::Builder::new().name("demux".into()).spawn(move || {
            if let Some(previous) = after {
                previous.wait();
            }
            demux::run(s, r, settings, demux_rx);
        });
        let thread = thread.inspect_err(|e| shared.set_status(Status::Failed(Error::Start(e.to_string())))).ok();
        Player { shared, audio: audio_tx, demux: demux_tx, request, thread }
    }

    /// Stop playing; the threads finish in the background.
    pub fn stop(mut self) -> Stopping {
        Stopping(self.thread.take())
    }

    pub fn status(&self) -> Status {
        self.shared.status.locked().clone()
    }

    pub fn info(&self) -> StreamInfo {
        self.shared.info.locked().clone()
    }

    /// The latest frame and its serial number.
    pub fn frame(&self) -> (u64, Option<Arc<VideoFrame>>) {
        let serial = self.shared.frame_serial.load(Ordering::Acquire);
        (serial, self.shared.frame.locked().clone())
    }

    pub fn take_notice(&self) -> Option<String> {
        self.shared.notice.locked().take()
    }

    /// Stream time being heard (seconds from the stream's start for recordings).
    pub fn position(&self) -> Option<f64> {
        self.shared.clock.now()
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
        self.shared.clock.set_paused(paused);
        (self.shared.repaint)();
    }

    /// Show the picture `ms` later (or earlier, if negative) than the output says
    /// its sound is heard.
    pub fn set_picture_delay(&self, ms: i32) {
        self.shared.picture_delay_ms.store(ms, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self.audio.send(AudioCmd::Volume(volume));
    }

    pub fn set_output(&self, output: Output) {
        let _ = self.audio.send(AudioCmd::Output(output));
    }

    pub fn seek(&self, seconds: f64) {
        if !self.request.live {
            let _ = self.demux.send(DemuxCmd::Seek(seconds.max(0.0)));
        }
    }

    pub fn set_audio_track(&self, stream: usize) {
        let _ = self.demux.send(DemuxCmd::AudioTrack(stream));
    }

    /// Show the subtitle track at stream index `stream`, or none.
    pub fn set_subtitle_track(&self, stream: Option<usize>) {
        let _ = self.demux.send(DemuxCmd::Subtitles(stream));
    }

    /// Show the subtitles of a file, `offset` seconds in (where the stream's time
    /// starts), in place of a track's.
    pub fn use_subtitles(&self, cues: Vec<subtitle::Cue>, offset: f64) {
        let _ = self.demux.send(DemuxCmd::Subtitles(None));
        let shifted = cues.into_iter().map(|c| subtitle::Cue { start: c.start + offset, end: c.end + offset, ..c }).collect();
        self.shared.cues.locked().replace(shifted);
        (self.shared.repaint)();
    }

    /// Record what plays into a new file at `path`, or stop recording.
    pub fn record(&self, path: Option<PathBuf>) {
        *self.shared.recording.locked() = path.clone().map_or(Recording::Off, Recording::To);
        let _ = self.demux.send(DemuxCmd::Record(path));
    }

    pub fn recording(&self) -> Recording {
        self.shared.recording.locked().clone()
    }

    /// The subtitles on screen now.
    pub fn subtitles(&self) -> Option<String> {
        let t = self.position()?;
        self.shared.cues.locked().at(t)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The threads notice, close the connection and the output, and end.
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// Codec names as people know them.
fn codec_label(name: &str) -> String {
    match name {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "mpeg2video" => "MPEG-2",
        "mpeg4" => "MPEG-4",
        "vp9" => "VP9",
        "av1" | "libdav1d" => "AV1",
        "aac" | "aac_latm" => "AAC",
        "ac3" => "AC-3",
        "eac3" => "E-AC-3",
        "mp2" => "MP2",
        "mp3" | "mp3float" => "MP3",
        "opus" => "Opus",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Two seconds of a test picture without sound (so no sound card is needed), in
    /// MPEG-TS, with the stream's clock starting at 1001.4 s.
    const PICTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/media/testdata/picture.ts");

    fn play(url: &str) -> Player {
        let request = Request { url: url.into(), user_agent: None, referrer: None, live: true, start_at: None };
        Player::open(request, Output::Local, 0.0, None, || {})
    }

    /// Wait until `done` says so, for a few seconds at most.
    fn wait_for(player: &Player, done: impl Fn(&Player) -> bool) {
        let start = Instant::now();
        while !done(player) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("haze-player-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The pictures in the recording at `path`, and how long it says it is.
    fn recorded(path: &Path) -> (usize, f64) {
        let mut input = ffmpeg_next::format::input(path).unwrap();
        let duration = input.duration() as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE);
        (input.packets().count(), duration)
    }

    #[test]
    fn records_what_plays_until_it_ends() {
        let path = folder("ends").join("recording.mkv");
        let player = play(PICTURE);
        player.record(Some(path.clone()));
        assert_eq!(player.recording(), Recording::To(path.clone()));
        wait_for(&player, |p| p.status() == Status::Ended);
        assert_eq!(player.status(), Status::Ended);
        player.stop().wait();
        let (pictures, duration) = recorded(&path);
        assert_eq!(pictures, 50);
        // The length is written as the file is finished.
        assert!((1.9..2.1).contains(&duration), "{duration}");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn stops_recording_when_told() {
        let dir = folder("told");
        let (first, second) = (dir.join("first.mkv"), dir.join("second.mkv"));
        let player = play(PICTURE);
        player.record(Some(first.clone()));
        wait_for(&player, |p| p.status() == Status::Playing);
        player.record(None);
        assert_eq!(player.recording(), Recording::Off);
        // A new file starts, and the one before is finished. (The clip is read to its
        // end well before it has played, so what follows may hold nothing more.)
        player.record(Some(second.clone()));
        assert_eq!(player.recording(), Recording::To(second.clone()));
        wait_for(&player, |p| p.status() == Status::Ended);
        player.stop().wait();
        let (pictures, duration) = recorded(&first);
        assert!(pictures > 0 && duration > 0.0, "{pictures} pictures, {duration} s");
        assert!(second.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn says_why_recording_failed() {
        let nowhere = std::env::temp_dir().join("haze-no-such-folder").join("deeper").join("x.mkv");
        let player = play(PICTURE);
        player.record(Some(nowhere));
        wait_for(&player, |p| matches!(p.recording(), Recording::Failed(_)));
        assert!(matches!(player.recording(), Recording::Failed(why) if why.starts_with("couldn't create the file")));
        // Playing goes on.
        wait_for(&player, |p| p.status() == Status::Ended);
        assert_eq!(player.status(), Status::Ended);
    }
}
