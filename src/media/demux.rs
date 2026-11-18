//! Opening a stream and handing its packets to the audio and video threads.

use super::{AudioCmd, DemuxCmd, Shared, Status, StreamInfo, Track, audio, codec_label, subtitle, video};
use crate::sync::Lock;
use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Once};
use std::thread::JoinHandle;

/// Packets that may wait for the video thread. The picture waits for the sound,
/// which is two seconds behind on an AirPlay speaker, so this must hold more than
/// that (at 60 fps, plus slack for streams that interleave loosely).
const VIDEO_QUEUE: usize = 1024;
const AUDIO_QUEUE: usize = 1024;

/// What the decoding threads are sent. Data and the end carry the seek epoch they
/// were read in: what was queued before a seek is dropped undecoded.
pub enum Packet {
    Data(ffmpeg::Packet, u64),
    /// Drop what's decoded and queued (after a seek).
    Flush,
    /// Switch to another audio track.
    Decoder(ffmpeg::decoder::Audio, ffmpeg::Rational),
    End(u64),
}

fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if let Err(e) = ffmpeg::init() {
            log_error!("ffmpeg: {e}");
        }
        ffmpeg::util::log::set_level(if crate::log::enabled(crate::log::Level::Trace) {
            ffmpeg::util::log::Level::Info
        } else {
            ffmpeg::util::log::Level::Fatal
        });
    });
}

pub fn run(shared: Arc<Shared>, request: super::Request, audio: audio::Settings, commands: Receiver<DemuxCmd>) {
    init();
    if let Err(e) = play(&shared, &request, audio, &commands)
        && !shared.stopped()
    {
        shared.set_status(Status::Failed(e));
    }
}

fn options(request: &super::Request) -> ffmpeg::Dictionary<'static> {
    let mut o = ffmpeg::Dictionary::new();
    o.set("user_agent", request.user_agent.as_deref().unwrap_or(crate::iptv::http::USER_AGENT));
    if let Some(r) = &request.referrer {
        o.set("referer", r);
    }
    // Give up on a silent connection after 15 s (in microseconds).
    o.set("rw_timeout", "15000000");
    o.set("reconnect", "1");
    o.set("reconnect_on_network_error", "1");
    o.set("reconnect_delay_max", "4");
    if request.live {
        o.set("reconnect_streamed", "1");
        // Start quickly: a live stream is probed for 1.5 s, not the default 5.
        o.set("analyzeduration", "1500000");
        o.set("probesize", "2000000");
    }
    o
}

/// The subtitle track shown: its index, decoder and time base.
type SubtitleTrack = (usize, ffmpeg::decoder::Subtitle, ffmpeg::Rational);

/// An open stream being read: which of its streams play, and where their packets go.
struct Demuxer<'a> {
    shared: &'a Arc<Shared>,
    live: bool,
    input: ffmpeg::format::context::Input,
    video_index: Option<usize>,
    audio_index: Option<usize>,
    video_tx: Option<SyncSender<Packet>>,
    audio_tx: Option<SyncSender<Packet>>,
    /// Output commands when there's no sound to take them.
    unheard: Option<Receiver<AudioCmd>>,
    subtitles: Option<SubtitleTrack>,
    /// Read to the end; waiting for a seek or the stop.
    ended: bool,
    /// The decoding threads, which end once their packets stop coming.
    threads: Vec<JoinHandle<()>>,
}

impl Drop for Demuxer<'_> {
    /// Let the decoding threads go and wait for them: the player counts as stopped
    /// once its output is closed.
    fn drop(&mut self) {
        (self.video_tx, self.audio_tx) = (None, None);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn play(shared: &Arc<Shared>, request: &super::Request, settings: audio::Settings, commands: &Receiver<DemuxCmd>) -> Result<(), String> {
    let Some(mut demuxer) = Demuxer::open(shared, request)? else { return Ok(()) };
    demuxer.start(settings)?;
    if let Some(at) = request.start_at {
        demuxer.seek(demuxer.info_start() + at);
    }
    let result = demuxer.pump(commands);
    // Say so before the demuxer waits for its threads on the way out: the app
    // reconnects on it.
    if let Err(e) = &result
        && !shared.stopped()
    {
        shared.set_status(Status::Failed(e.clone()));
    }
    result
}

impl<'a> Demuxer<'a> {
    /// Open the stream (the best quality of an HLS playlist) and pick what plays;
    /// None when stopped meanwhile.
    fn open(shared: &'a Arc<Shared>, request: &super::Request) -> Result<Option<Self>, String> {
        let user_agent = request.user_agent.as_deref().unwrap_or(crate::iptv::http::USER_AGENT);
        let url = super::hls::pick_variant(&request.url, user_agent, request.referrer.as_deref()).unwrap_or_else(|| request.url.clone());
        if shared.stopped() {
            return Ok(None);
        }
        let stop = shared.clone();
        let mut input = ffmpeg::format::input_with_interrupt_and_dictionary(&url, move || stop.stopped(), options(request))
            .map_err(|e| describe(&e))?;
        let video_index = best_video(&input);
        let audio_index = best_audio(&mut input, video_index);
        if video_index.is_none() && audio_index.is_none() {
            return Err("the channel isn't sending anything right now".into());
        }
        // Leave out every other stream: an HLS playlist offers each quality as streams of
        // its own, and the ones left in are all downloaded.
        for i in 0..input.nb_streams() as usize {
            set_discard(&mut input, i, Some(i) != video_index && Some(i) != audio_index);
        }
        Ok(Some(Demuxer {
            shared,
            live: request.live,
            input,
            video_index,
            audio_index,
            video_tx: None,
            audio_tx: None,
            unheard: None,
            subtitles: None,
            ended: false,
            threads: Vec::new(),
        }))
    }

    /// Start the decoding threads and say what the stream is.
    fn start(&mut self, settings: audio::Settings) -> Result<(), String> {
        let mut info = self.describe_streams();
        if let Some(stream) = self.video_index.and_then(|i| self.input.stream(i)) {
            let decoder = video_decoder(&stream)?;
            info.video_codec = decoder.codec().map(|c| codec_label(c.name()));
            (info.width, info.height) = (decoder.width(), decoder.height());
            let fps = stream.avg_frame_rate();
            info.fps = (fps.numerator() > 0 && fps.denominator() > 0).then(|| fps.numerator() as f32 / fps.denominator() as f32);
            let (tx, rx) = sync_channel(VIDEO_QUEUE);
            let (s, tb, has_audio) = (self.shared.clone(), stream.time_base(), self.audio_index.is_some());
            self.threads.push(spawn("video", move || video::run(s, decoder, tb, rx, has_audio))?);
            self.video_tx = Some(tx);
        }
        match self.audio_index.and_then(|i| self.input.stream(i)) {
            Some(stream) => {
                let decoder = audio_decoder(&stream)?;
                info.audio_codec = decoder.codec().map(|c| codec_label(c.name()));
                info.audio_channels = decoder.channels();
                let (tx, rx) = sync_channel(AUDIO_QUEUE);
                let (s, tb) = (self.shared.clone(), stream.time_base());
                self.threads.push(spawn("audio", move || audio::run(s, decoder, tb, rx, settings))?);
                self.audio_tx = Some(tx);
            }
            None => self.unheard = Some(settings.commands),
        }
        *self.shared.info.locked() = info;
        self.shared.set_status(Status::Buffering);
        Ok(())
    }

    /// Read and pass on packets, and obey commands, until stopped.
    fn pump(&mut self, commands: &Receiver<DemuxCmd>) -> Result<(), String> {
        while !self.shared.stopped() {
            self.drop_unheard();
            if let Some(command) = self.next_command(commands) {
                self.obey(command);
            }
            if self.ended {
                std::thread::sleep(std::time::Duration::from_millis(50));
            } else if !self.read_packet()? {
                break;
            }
        }
        Ok(())
    }

    /// The length and the audio and subtitle tracks there are to pick from.
    fn describe_streams(&self) -> StreamInfo {
        let tracks = |medium: Type, usable: &dyn Fn(&ffmpeg::Stream) -> bool| -> Vec<Track> {
            self.input
                .streams()
                .filter(|s| s.parameters().medium() == medium && usable(s))
                .enumerate()
                .map(|(n, s)| Track { stream: s.index(), label: track_label(&s, n) })
                .collect()
        };
        let duration = self.input.duration();
        // SAFETY: plain read of a field of the open input.
        let start = unsafe { (*self.input.as_ptr()).start_time };
        let audio_tracks = tracks(Type::Audio, &|_| true);
        StreamInfo {
            duration: (duration > 0).then(|| duration as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)),
            start: (start != ffmpeg::ffi::AV_NOPTS_VALUE).then(|| start as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)),
            audio_track: audio_tracks.iter().position(|t| Some(t.stream) == self.audio_index),
            audio_tracks,
            subtitle_tracks: tracks(Type::Subtitle, &|s| subtitle::is_text(s.parameters().id())),
            ..StreamInfo::default()
        }
    }

    /// The stream time the stream starts at (0 when it doesn't say).
    fn info_start(&self) -> f64 {
        self.shared.info.locked().start.unwrap_or(0.0)
    }

    /// With no sound, commands for the output have nowhere to go.
    fn drop_unheard(&self) {
        if let Some(rx) = &self.unheard {
            while rx.try_recv().is_ok() {}
        }
    }

    fn next_command(&self, commands: &Receiver<DemuxCmd>) -> Option<DemuxCmd> {
        // A speaker that threw its buffer away on pause: carry on from where it paused.
        let resume = self.shared.resume_at.locked().take();
        match resume {
            Some(t) if !self.live => Some(DemuxCmd::Seek(t)),
            _ => commands.try_recv().ok(),
        }
    }

    fn obey(&mut self, command: DemuxCmd) {
        match command {
            DemuxCmd::Seek(t) => {
                // A track's cues come again from where it's read; a file's stay.
                if self.seek(t) && self.subtitles.is_some() {
                    self.shared.cues.locked().clear();
                }
            }
            DemuxCmd::AudioTrack(i) => self.switch_audio(i),
            DemuxCmd::Subtitles(track) => self.show_subtitles(track),
        }
    }

    /// Go to `t` seconds in; false when the stream can't.
    fn seek(&mut self, t: f64) -> bool {
        let done = seek(&mut self.input, t, self.shared, [&self.video_tx, &self.audio_tx]);
        if done {
            self.ended = false;
        }
        done
    }

    fn switch_audio(&mut self, i: usize) {
        let Some(tx) = &self.audio_tx else { return };
        let Some(stream) = self.input.stream(i).filter(|s| s.parameters().medium() == Type::Audio) else { return };
        match audio_decoder(&stream) {
            Ok(d) => {
                let _ = tx.send(Packet::Decoder(d, stream.time_base()));
                if let Some(old) = self.audio_index {
                    set_discard(&mut self.input, old, true);
                }
                set_discard(&mut self.input, i, false);
                self.audio_index = Some(i);
                let mut info = self.shared.info.locked();
                info.audio_track = info.audio_tracks.iter().position(|t| t.stream == i);
            }
            Err(e) => log_warn!("audio track {i}: {e}"),
        }
    }

    fn show_subtitles(&mut self, track: Option<usize>) {
        if let Some((old, ..)) = self.subtitles.take() {
            set_discard(&mut self.input, old, true);
            self.shared.cues.locked().clear();
        }
        if let Some((i, stream)) = track.and_then(|i| Some((i, self.input.stream(i)?))) {
            match ffmpeg::codec::Context::from_parameters(stream.parameters()).and_then(|c| c.decoder().subtitle()) {
                Ok(d) => {
                    let tb = stream.time_base();
                    set_discard(&mut self.input, i, false);
                    self.subtitles = Some((i, d, tb));
                    // What's been read ahead went by without its subtitles: read it
                    // again from what's playing.
                    if !self.live
                        && let Some(t) = self.shared.clock.now()
                    {
                        self.seek(t);
                    }
                }
                Err(e) => log_warn!("subtitle track {i}: {e}"),
            }
        }
        let shown = self.subtitles.as_ref().map(|(i, ..)| *i);
        let mut info = self.shared.info.locked();
        info.subtitle_track = shown.and_then(|i| info.subtitle_tracks.iter().position(|t| t.stream == i));
        (self.shared.repaint)();
    }

    /// Read a packet and pass it on; false when the decoders are gone.
    fn read_packet(&mut self) -> Result<bool, String> {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut self.input) {
            Ok(()) => Ok(self.pass_on(packet)),
            Err(ffmpeg::Error::Eof) => {
                self.end();
                Ok(true)
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => Ok(true),
            Err(_) if self.shared.stopped() => Ok(false),
            Err(e) => Err(describe(&e)),
        }
    }

    /// Subtitles become cues here; sound and picture go to their threads, blocking
    /// while a queue is full: that's how the stream is paced. False when a decoder
    /// is gone.
    fn pass_on(&mut self, packet: ffmpeg::Packet) -> bool {
        let index = packet.stream();
        if let Some((i, decoder, tb)) = &mut self.subtitles
            && *i == index
        {
            let mut sub = ffmpeg::Subtitle::new();
            if let (Some(pts), Ok(true)) = (packet.pts(), decoder.decode(&packet, &mut sub)) {
                let seconds = |t: i64| t as f64 * f64::from(*tb);
                let duration = (packet.duration() > 0).then(|| seconds(packet.duration()));
                let mut cues = self.shared.cues.locked();
                for cue in subtitle::cues(&sub, seconds(pts), duration) {
                    cues.add(cue);
                }
            }
            return true;
        }
        let target = if Some(index) == self.video_index {
            &self.video_tx
        } else if Some(index) == self.audio_index {
            &self.audio_tx
        } else {
            return true;
        };
        let epoch = self.shared.epoch.load(Ordering::Relaxed);
        target.as_ref().is_none_or(|tx| tx.send(Packet::Data(packet, epoch)).is_ok())
    }

    /// The decoding threads play out what's queued; the last one to get to the end
    /// marks the stream ended.
    fn end(&mut self) {
        let consumers: Vec<_> = [&self.video_tx, &self.audio_tx].into_iter().flatten().collect();
        self.shared.finishing.store(consumers.len() as u8, Ordering::Release);
        let epoch = self.shared.epoch.load(Ordering::Relaxed);
        for tx in consumers {
            let _ = tx.send(Packet::End(epoch));
        }
        self.ended = true;
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, String> {
    std::thread::Builder::new().name(name.into()).spawn(f).map_err(|e| e.to_string())
}

/// Go to `t` seconds in and drop what the decoders have queued; false when the
/// stream can't seek there.
fn seek(input: &mut ffmpeg::format::context::Input, t: f64, shared: &Shared, decoders: [&Option<SyncSender<Packet>>; 2]) -> bool {
    let ts = (t * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    if let Err(e) = input.seek(ts, ..ts) {
        log_warn!("seek to {t:.1}s: {e}");
        return false;
    }
    shared.epoch.fetch_add(1, Ordering::Relaxed);
    shared.clock.reset();
    for tx in decoders.into_iter().flatten() {
        let _ = tx.send(Packet::Flush);
    }
    true
}

/// The picture to show: of an HLS playlist's qualities, the best one.
fn best_video(input: &ffmpeg::format::context::Input) -> Option<usize> {
    let bitrate = |s: &ffmpeg::Stream| s.metadata().get("variant_bitrate").and_then(|b| b.parse::<u64>().ok()).unwrap_or(0);
    input
        .streams()
        .filter(|s| s.parameters().medium() == Type::Video && !s.disposition().contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC))
        .max_by_key(|s| {
            // SAFETY: the parameters belong to a stream of this open input.
            let (w, h) = unsafe { ((*s.parameters().as_ptr()).width, (*s.parameters().as_ptr()).height) };
            (bitrate(s), i64::from(w) * i64::from(h))
        })
        .map(|s| s.index())
}

/// The sound that goes with `video` (in the same program), as ffmpeg judges it.
fn best_audio(input: &mut ffmpeg::format::context::Input, video: Option<usize>) -> Option<usize> {
    let related = video.map_or(-1, |v| v as i32);
    // SAFETY: plain query on the open input.
    let i = unsafe {
        ffmpeg::ffi::av_find_best_stream(
            input.as_mut_ptr(),
            ffmpeg::ffi::AVMediaType::AVMEDIA_TYPE_AUDIO,
            -1,
            related,
            std::ptr::null_mut(),
            0,
        )
    };
    (i >= 0).then_some(i as usize)
}

fn set_discard(input: &mut ffmpeg::format::context::Input, index: usize, discard: bool) {
    // SAFETY: index is below nb_streams; only the discard field is written.
    unsafe {
        let ctx = input.as_mut_ptr();
        if index < (*ctx).nb_streams as usize {
            let stream = *(*ctx).streams.add(index);
            (*stream).discard = if discard { ffmpeg::ffi::AVDiscard::AVDISCARD_ALL } else { ffmpeg::ffi::AVDiscard::AVDISCARD_DEFAULT };
        }
    }
}

fn video_decoder(stream: &ffmpeg::Stream) -> Result<ffmpeg::decoder::Video, String> {
    let mut ctx = ffmpeg::codec::Context::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    if !super::hwaccel::attach(&mut ctx) {
        // In software, a few frame threads: 1080p50 H.264 on one core is too much for
        // older machines, and every thread holds frames of its own.
        ctx.set_threading(ffmpeg::codec::threading::Config { kind: ffmpeg::codec::threading::Type::Frame, count: 4 });
    }
    ctx.decoder().video().map_err(|e| format!("can't decode the picture: {e}"))
}

fn audio_decoder(stream: &ffmpeg::Stream) -> Result<ffmpeg::decoder::Audio, String> {
    let ctx = ffmpeg::codec::Context::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    ctx.decoder().audio().map_err(|e| format!("can't decode the sound: {e}"))
}

fn track_label(stream: &ffmpeg::Stream, n: usize) -> String {
    let meta = stream.metadata();
    let lang = meta.get("language").filter(|l| !l.is_empty() && *l != "und");
    let title = meta.get("title").filter(|t| !t.is_empty());
    match (title, lang) {
        (Some(t), Some(l)) => format!("{t} ({})", l.to_ascii_uppercase()),
        (Some(t), None) => t.to_string(),
        (None, Some(l)) => l.to_ascii_uppercase(),
        (None, None) => format!("Track {}", n + 1),
    }
}

/// ffmpeg's errors in words for the screen.
fn describe(e: &ffmpeg::Error) -> String {
    match e {
        ffmpeg::Error::HttpUnauthorized | ffmpeg::Error::HttpForbidden => {
            "the provider refused the stream (login or connection limit)".into()
        }
        ffmpeg::Error::HttpNotFound => "the stream was not found".into(),
        ffmpeg::Error::HttpServerError => "the provider's server failed".into(),
        ffmpeg::Error::Exit => "stopped".into(),
        other => other.to_string(),
    }
}
