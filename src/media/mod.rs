//! Playing a stream: [`demux`] reads it and hands packets to the [`audio`] and
//! [`video`] threads. Audio goes to an output (sound card or AirPlay speaker) as fast
//! as it takes it and drives the [`clock`]; video waits for the clock, so the picture
//! matches the sound wherever it is heard, even two seconds later on a speaker.

mod audio;
mod clock;
mod demux;
pub mod frame;
mod hls;
mod hwaccel;
pub mod subtitle;
mod video;

use crate::output::Output;
pub use frame::VideoFrame;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// What to play.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub url: String,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    /// A live channel (no seeking, reconnect on drop-outs) rather than a recording.
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Opening,
    Playing,
    /// Waiting for more of the stream.
    Buffering,
    Ended,
    Failed(String),
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
    /// Subtitle tracks with text in them.
    pub subtitle_tracks: Vec<Track>,
    /// Index into `subtitle_tracks` of the one shown, if any.
    pub subtitle_track: Option<usize>,
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
    repaint: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn set_status(&self, s: Status) {
        let mut cur = self.status.lock().unwrap_or_else(|e| e.into_inner());
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
        *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(frame));
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
        *self.notice.lock().unwrap_or_else(|e| e.into_inner()) = Some(notice);
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
}

/// A stream being played. Dropping it stops playback.
pub struct Player {
    shared: Arc<Shared>,
    audio: Sender<AudioCmd>,
    demux: Sender<DemuxCmd>,
    pub request: Request,
}

impl Player {
    pub fn open(request: Request, output: Output, volume: f32, repaint: impl Fn() + Send + Sync + 'static) -> Player {
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
            repaint: Box::new(repaint),
        });
        let (audio_tx, audio_rx) = channel();
        let (demux_tx, demux_rx) = channel();
        let settings = audio::Settings { output, volume, commands: audio_rx };
        let (s, r) = (shared.clone(), request.clone());
        std::thread::Builder::new().name("demux".into()).spawn(move || demux::run(s, r, settings, demux_rx)).expect("spawn demux thread");
        Player { shared, audio: audio_tx, demux: demux_tx, request }
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn info(&self) -> StreamInfo {
        self.shared.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The latest frame and its serial number.
    pub fn frame(&self) -> (u64, Option<Arc<VideoFrame>>) {
        let serial = self.shared.frame_serial.load(Ordering::Acquire);
        (serial, self.shared.frame.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    pub fn take_notice(&self) -> Option<String> {
        self.shared.notice.lock().unwrap_or_else(|e| e.into_inner()).take()
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
        self.shared.cues.lock().unwrap_or_else(|e| e.into_inner()).replace(shifted);
        (self.shared.repaint)();
    }

    /// The subtitles on screen now.
    pub fn subtitles(&self) -> Option<String> {
        let t = self.position()?;
        self.shared.cues.lock().unwrap_or_else(|e| e.into_inner()).at(t)
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
