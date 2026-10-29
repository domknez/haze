//! Opening a stream and handing its packets to the audio and video threads.

use super::{AudioCmd, DemuxCmd, Shared, Status, StreamInfo, Track, audio, codec_label, video};
use ffmpeg_next as ffmpeg;
use ffmpeg::media::Type;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Once};

/// Packets that may wait for the video thread. The picture waits for the sound,
/// which is two seconds behind on an AirPlay speaker, so this must hold more than
/// that (at 60 fps, plus slack for streams that interleave loosely).
const VIDEO_QUEUE: usize = 1024;
const AUDIO_QUEUE: usize = 1024;

pub enum Packet {
    Data(ffmpeg::Packet),
    /// Drop what's decoded and queued (after a seek).
    Flush,
    /// Switch to another audio track.
    Decoder(ffmpeg::decoder::Audio, ffmpeg::Rational),
    End,
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
    if let Err(e) = play(&shared, &request, audio, &commands) {
        if !shared.stopped() {
            shared.set_status(Status::Failed(e));
        }
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

fn play(shared: &Arc<Shared>, request: &super::Request, settings: audio::Settings, commands: &Receiver<DemuxCmd>) -> Result<(), String> {
    let stop = shared.clone();
    let mut input = ffmpeg::format::input_with_interrupt_and_dictionary(&request.url, move || stop.stopped(), options(request))
        .map_err(|e| describe(&e))?;
    let video_index = input.streams().best(Type::Video).map(|s| s.index());
    let mut audio_index = input.streams().best(Type::Audio).map(|s| s.index());
    if video_index.is_none() && audio_index.is_none() {
        return Err("the stream has neither picture nor sound".into());
    }

    let mut info = StreamInfo {
        duration: (input.duration() > 0).then(|| input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)),
        audio_tracks: input
            .streams()
            .filter(|s| s.parameters().medium() == Type::Audio)
            .enumerate()
            .map(|(n, s)| Track { stream: s.index(), label: track_label(&s, n) })
            .collect(),
        ..StreamInfo::default()
    };
    info.audio_track = info.audio_tracks.iter().position(|t| Some(t.stream) == audio_index);

    let video_tx = match video_index.and_then(|i| input.stream(i)) {
        Some(stream) => {
            let decoder = video_decoder(&stream)?;
            info.video_codec = decoder.codec().map(|c| codec_label(c.name()));
            (info.width, info.height) = (decoder.width(), decoder.height());
            let fps = stream.avg_frame_rate();
            info.fps = (fps.numerator() > 0 && fps.denominator() > 0).then(|| fps.numerator() as f32 / fps.denominator() as f32);
            let (tx, rx) = sync_channel(VIDEO_QUEUE);
            let (s, tb, has_audio) = (shared.clone(), stream.time_base(), audio_index.is_some());
            std::thread::Builder::new()
                .name("video".into())
                .spawn(move || video::run(s, decoder, tb, rx, has_audio))
                .map_err(|e| e.to_string())?;
            Some(tx)
        }
        None => None,
    };

    let mut audio_settings = Some(settings);
    let mut audio_tx: Option<SyncSender<Packet>> = None;
    let mut pending_audio_cmds = None;
    match audio_index.and_then(|i| input.stream(i)) {
        Some(stream) => {
            let decoder = audio_decoder(&stream)?;
            info.audio_codec = decoder.codec().map(|c| codec_label(c.name()));
            info.audio_channels = decoder.channels();
            let (tx, rx) = sync_channel(AUDIO_QUEUE);
            let (s, tb, settings) = (shared.clone(), stream.time_base(), audio_settings.take().expect("audio settings"));
            std::thread::Builder::new()
                .name("audio".into())
                .spawn(move || audio::run(s, decoder, tb, rx, settings))
                .map_err(|e| e.to_string())?;
            audio_tx = Some(tx);
        }
        // No sound: commands for the output have nowhere to go.
        None => pending_audio_cmds = audio_settings.take().map(|s| s.commands),
    }
    *shared.info.lock().unwrap_or_else(|e| e.into_inner()) = info;
    shared.set_status(Status::Buffering);

    let mut ended = false;
    loop {
        if shared.stopped() {
            return Ok(());
        }
        if let Some(rx) = &pending_audio_cmds {
            while let Ok(AudioCmd::Volume(_) | AudioCmd::Output(_)) = rx.try_recv() {}
        }
        match commands.try_recv() {
            Ok(DemuxCmd::Seek(t)) => {
                let ts = (t * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
                if let Err(e) = input.seek(ts, ..ts) {
                    log_warn!("seek to {t:.1}s: {e}");
                } else {
                    shared.epoch.fetch_add(1, Ordering::Relaxed);
                    shared.clock.reset();
                    for tx in [&video_tx, &audio_tx].into_iter().flatten() {
                        let _ = tx.send(Packet::Flush);
                    }
                    ended = false;
                }
            }
            Ok(DemuxCmd::AudioTrack(i)) => {
                if let (Some(tx), Some(stream)) = (&audio_tx, input.stream(i))
                    && stream.parameters().medium() == Type::Audio
                {
                    match audio_decoder(&stream) {
                        Ok(d) => {
                            let _ = tx.send(Packet::Decoder(d, stream.time_base()));
                            audio_index = Some(i);
                            let mut info = shared.info.lock().unwrap_or_else(|e| e.into_inner());
                            info.audio_track = info.audio_tracks.iter().position(|t| t.stream == i);
                        }
                        Err(e) => log_warn!("audio track {i}: {e}"),
                    }
                }
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
        }
        if ended {
            // Played to the end: wait for a seek or the stop.
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }

        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                let index = packet.stream();
                let target = if Some(index) == video_index {
                    &video_tx
                } else if Some(index) == audio_index {
                    &audio_tx
                } else {
                    continue;
                };
                // Blocks while the queue is full: that's how the stream is paced.
                if let Some(tx) = target
                    && tx.send(Packet::Data(packet)).is_err()
                {
                    return Ok(());
                }
            }
            Err(ffmpeg::Error::Eof) => {
                // The decoding threads play out what's queued; the last one to get
                // to the end marks the stream ended.
                let consumers: Vec<_> = [&video_tx, &audio_tx].into_iter().flatten().collect();
                shared.finishing.store(consumers.len() as u8, Ordering::Release);
                for tx in consumers {
                    let _ = tx.send(Packet::End);
                }
                ended = true;
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
            Err(e) => {
                if shared.stopped() {
                    return Ok(());
                }
                return Err(describe(&e));
            }
        }
    }
}

fn video_decoder(stream: &ffmpeg::Stream) -> Result<ffmpeg::decoder::Video, String> {
    let mut ctx = ffmpeg::codec::Context::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    // Frame threads: 1080p50 H.264 on one core is too much for older machines.
    ctx.set_threading(ffmpeg::codec::threading::Config { kind: ffmpeg::codec::threading::Type::Frame, count: 0, ..Default::default() });
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
        ffmpeg::Error::HttpUnauthorized | ffmpeg::Error::HttpForbidden => "the provider refused the stream (login or connection limit)".into(),
        ffmpeg::Error::HttpNotFound => "the stream was not found".into(),
        ffmpeg::Error::HttpServerError => "the provider's server failed".into(),
        ffmpeg::Error::Exit => "stopped".into(),
        other => other.to_string(),
    }
}
