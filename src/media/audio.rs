//! Decoding sound, converting it to what the outputs take (stereo 16-bit at
//! 44.1 kHz) and feeding the output, which sets the pace and the clock.

use super::demux::Packet;
use super::{AudioCmd, Shared, Status};
use crate::output::{self, Output, RATE, Sink};
use ffmpeg::ChannelLayout;
use ffmpeg::format::Sample;
use ffmpeg::format::sample::Type as SampleType;
use ffmpeg_next as ffmpeg;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// Samples are handed to the output in pieces of this many frames.
const CHUNK: usize = 1024;
/// The clock is corrected only when the output's account drifts this far from it,
/// so the picture doesn't jitter with every write.
const CLOCK_SLACK: f64 = 0.03;

pub struct Settings {
    pub output: Output,
    pub volume: f32,
    pub commands: Receiver<AudioCmd>,
}

struct Feeder {
    shared: Arc<Shared>,
    sink: Box<dyn Sink>,
    output: Output,
    volume: f32,
    commands: Receiver<AudioCmd>,
    /// Whether the output is paused.
    paused: bool,
    /// Keeps macOS from napping haze while sound plays.
    _awake: crate::awake::Awake,
}

pub fn run(
    shared: Arc<Shared>,
    decoder: ffmpeg::decoder::Audio,
    time_base: ffmpeg::Rational,
    packets: Receiver<Packet>,
    settings: Settings,
) {
    crate::awake::high_priority_thread();
    let (sink, output) = open_sink(&shared, &settings.output, settings.volume);
    let mut feeder = Feeder {
        shared,
        sink,
        output,
        volume: settings.volume,
        commands: settings.commands,
        paused: false,
        _awake: crate::awake::Awake::begin(),
    };
    let mut decoder = decoder;
    let mut time_base = time_base;
    let mut resampler: Option<(ffmpeg::software::resampling::Context, (Sample, u64, u32))> = None;
    let mut frame = ffmpeg::frame::Audio::empty();
    let mut converted = ffmpeg::frame::Audio::empty();
    let mut samples: Vec<i16> = Vec::new();
    let mut ending = false;

    while !feeder.shared.stopped() {
        feeder.handle_commands();
        if feeder.hold() {
            continue;
        }
        let packet = match packets.recv_timeout(Duration::from_millis(20)) {
            Ok(p) => p,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match packet {
            Packet::Data(p) => {
                if decoder.send_packet(&p).is_err() {
                    continue;
                }
            }
            Packet::Flush => {
                decoder.flush();
                feeder.sink.flush();
                continue;
            }
            Packet::Decoder(d, tb) => {
                (decoder, time_base) = (d, tb);
                resampler = None;
                continue;
            }
            Packet::End => {
                let _ = decoder.send_eof();
                ending = true;
            }
        }
        while decoder.receive_frame(&mut frame).is_ok() {
            let start = frame.timestamp().or(frame.pts()).map(|t| t as f64 * f64::from(time_base));
            let key = (frame.format(), layout_key(&frame), frame.rate());
            if resampler.as_ref().is_none_or(|(_, k)| *k != key) {
                resampler = make_resampler(&frame).map(|r| (r, key));
            }
            let Some((r, _)) = &mut resampler else { continue };
            let Some(held) = convert(r, &frame, &mut converted, &mut samples) else { continue };
            // What comes out is older than the frame that went in by what the
            // converter still holds.
            let start = start
                .map(|t| t + frame.samples() as f64 / f64::from(frame.rate().max(1)) - (held + samples.len() / 2) as f64 / f64::from(RATE));
            feeder.write(&samples, start);
            if feeder.shared.stopped() {
                break;
            }
        }
        if std::mem::take(&mut ending) {
            feeder.drain();
            feeder.shared.finished();
        }
    }
}

impl Feeder {
    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                AudioCmd::Volume(v) => {
                    self.volume = v;
                    self.sink.set_volume(v);
                }
                AudioCmd::Output(o) if o != self.output => {
                    // Close the old one first: a speaker takes one sender at a time.
                    self.sink = Box::new(NullSink);
                    let (sink, output) = open_sink(&self.shared, &o, self.volume);
                    (self.sink, self.output) = (sink, output);
                    self.paused = false;
                }
                AudioCmd::Output(_) => {}
            }
        }
        if !self.sink.healthy() {
            log_info!("audio output lost, setting it up again");
            self.sink = Box::new(NullSink);
            let (sink, output) = open_sink(&self.shared, &self.output.clone(), self.volume);
            (self.sink, self.output) = (sink, output);
        }
    }

    /// While paused, waits (true); otherwise makes sure the output plays.
    fn hold(&mut self) -> bool {
        let paused = self.shared.paused.load(Ordering::Relaxed);
        if paused != self.paused {
            self.paused = paused;
            if paused {
                self.sink.pause();
            } else {
                if self.sink.drops_buffer_on_pause()
                    && let Some(t) = self.shared.clock.now()
                {
                    // What the speaker held is gone: play it again from the pause.
                    *self.shared.resume_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
                }
                self.sink.resume();
            }
        }
        if paused {
            std::thread::sleep(Duration::from_millis(20));
        }
        paused
    }

    /// Hand `samples` (starting at stream time `start`) to the output, waiting while
    /// it is full, and keep the clock on what is heard.
    fn write(&mut self, samples: &[i16], start: Option<f64>) {
        let mut offset = 0;
        while offset < samples.len() {
            if self.shared.stopped() {
                return;
            }
            self.handle_commands();
            if self.hold() {
                continue;
            }
            let end = (offset + CHUNK * 2).min(samples.len());
            if self.sink.write(&samples[offset..end]) == 0 {
                std::thread::sleep(Duration::from_millis(5));
                self.update_clock(start, offset);
                continue;
            }
            offset = end;
            self.update_clock(start, offset);
        }
    }

    /// Wait until the output has played what it holds.
    fn drain(&mut self) {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while self.sink.pending_frames() > 0 && std::time::Instant::now() < until && !self.shared.stopped() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The clock: the end of what was written, less what the output still holds and
    /// how long it takes to be heard.
    fn update_clock(&self, start: Option<f64>, written: usize) {
        let Some(start) = start else { return };
        let delay = f64::from(self.shared.picture_delay_ms.load(Ordering::Relaxed)) / 1000.0;
        let behind = (self.sink.pending_frames() + self.sink.latency_frames()) as f64 / f64::from(RATE) + delay;
        let heard = start + (written / 2) as f64 / f64::from(RATE) - behind;
        let clock = &self.shared.clock;
        if clock.now().is_none_or(|now| (now - heard).abs() > CLOCK_SLACK) {
            clock.set(heard);
        }
        if self.shared.status.lock().is_ok_and(|s| *s == Status::Buffering) {
            self.shared.set_status(Status::Playing);
        }
    }
}

fn open_sink(shared: &Shared, output: &Output, volume: f32) -> (Box<dyn Sink>, Output) {
    match output::open(output, volume) {
        Ok(sink) => (sink, output.clone()),
        Err(e) => {
            shared.notify(format!("Couldn't play on {}: {e}. Playing on this computer.", output.name()));
            match output::open(&Output::Local, volume) {
                Ok(sink) => (sink, Output::Local),
                Err(e) => {
                    shared.notify(format!("No sound output: {e}"));
                    (Box::new(NullSink), Output::Local)
                }
            }
        }
    }
}

/// Converts `frame` into `samples` (interleaved stereo i16 at [`RATE`]); returns how
/// many frames the converter holds back, or None if it failed.
///
/// The output frame is made big enough for all of this input every time: given one
/// sized for an earlier, shorter frame, the converter keeps the rest, and what it
/// keeps grows with every frame — the sound falls ever further behind the picture.
fn convert(
    r: &mut ffmpeg::software::resampling::Context,
    frame: &ffmpeg::frame::Audio,
    out: &mut ffmpeg::frame::Audio,
    samples: &mut Vec<i16>,
) -> Option<usize> {
    let held = r.delay().map_or(0, |d| d.output.max(0) as usize);
    let room = (frame.samples() + 1) * RATE as usize / frame.rate().max(1) as usize + held + 32;
    *out = ffmpeg::frame::Audio::new(Sample::I16(SampleType::Packed), room, ChannelLayout::STEREO);
    r.run(frame, out).ok()?;
    let n = out.samples();
    samples.clear();
    samples.extend(out.data(0)[..n * 4].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
    Some(r.delay().map_or(0, |d| d.output.max(0) as usize))
}

/// Identifies a channel layout well enough to tell when it changes.
fn layout_key(frame: &ffmpeg::frame::Audio) -> u64 {
    u64::from(frame.channels())
}

fn make_resampler(frame: &ffmpeg::frame::Audio) -> Option<ffmpeg::software::resampling::Context> {
    let mut layout = frame.channel_layout();
    if layout.channels() != i32::from(frame.channels()) || layout.channels() == 0 {
        layout = ChannelLayout::default(i32::from(frame.channels().max(1)));
    }
    let r = ffmpeg::software::resampling::Context::get(
        frame.format(),
        layout,
        frame.rate(),
        Sample::I16(SampleType::Packed),
        ChannelLayout::STEREO,
        RATE,
    );
    r.map_err(|e| log_warn!("audio conversion from {:?} {} Hz: {e}", frame.format(), frame.rate())).ok()
}

/// Nowhere: used while no output can be opened, so the stream keeps moving.
struct NullSink;

impl Sink for NullSink {
    fn write(&mut self, samples: &[i16]) -> usize {
        // Pace like a real output would.
        std::thread::sleep(Duration::from_secs_f64(samples.len() as f64 / 2.0 / f64::from(RATE)));
        samples.len()
    }
    fn pending_frames(&self) -> usize {
        0
    }
    fn latency_frames(&self) -> usize {
        0
    }
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn flush(&mut self) {}
    fn set_volume(&mut self, _: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(samples: usize, rate: u32) -> ffmpeg::frame::Audio {
        let mut f = ffmpeg::frame::Audio::new(Sample::F32(SampleType::Planar), samples, ChannelLayout::STEREO);
        f.set_rate(rate);
        for ch in 0..2 {
            for (i, s) in f.plane_mut::<f32>(ch).iter_mut().enumerate() {
                *s = (i as f32 * 0.05).sin() * 0.5;
            }
        }
        f
    }

    /// Frames that grow after the first (HE-AAC sends 2048 after 1024) all come out
    /// whole: nothing piles up in the converter.
    #[test]
    fn keeps_up_when_frames_grow() {
        ffmpeg::init().unwrap();
        let first = tone(1024, 48_000);
        let mut r = make_resampler(&first).unwrap();
        let (mut out, mut samples) = (ffmpeg::frame::Audio::empty(), Vec::new());
        let (mut fed, mut got) = (0usize, 0usize);
        let mut held = 0;
        for n in [1024, 2048, 2048, 2048, 1152, 2048, 2048, 2048, 2048, 2048] {
            let f = if n == 1024 { tone(1024, 48_000) } else { tone(n, 48_000) };
            held = convert(&mut r, &f, &mut out, &mut samples).unwrap();
            fed += n;
            got += samples.len() / 2;
        }
        let expected = fed * 44_100 / 48_000;
        assert!(held < 64, "the converter holds {held} frames");
        assert!(expected.abs_diff(got + held) < 64, "fed {expected} worth, got {got} + {held} held");
    }
}
