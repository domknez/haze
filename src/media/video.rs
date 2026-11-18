//! Decoding pictures and showing each when the sound reaches it.

use super::demux::Packet;
use super::{Shared, Status, VideoFrame};
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A frame this far ahead of the clock means the stream jumped (a new programme
/// segment, a restarted encoder): show it rather than wait.
const JUMP: f64 = 5.0;
/// Frames this late are skipped …
const LATE: f64 = 0.15;
/// … but at least one in this many is shown, so a slow machine still moves.
const SHOW_EVERY: u32 = 8;
/// How long to wait for the sound to start before the picture runs on its own.
const NO_CLOCK: Duration = Duration::from_secs(3);

pub fn run(
    shared: Arc<Shared>,
    mut decoder: ffmpeg::decoder::Video,
    time_base: ffmpeg::Rational,
    packets: Receiver<Packet>,
    has_audio: bool,
) {
    let mut frame = ffmpeg::frame::Video::empty();
    let mut converter = Converter::default();
    let mut ending = false;
    let mut presenter = Presenter { shared: shared.clone(), has_audio, shown: false, skipped: 0, waiting_since: None };

    while !shared.stopped() {
        let packet = match packets.recv_timeout(Duration::from_millis(50)) {
            Ok(p) => p,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match packet {
            Packet::Data(_, epoch) | Packet::End(epoch) if epoch != shared.epoch.load(Ordering::Relaxed) => continue,
            Packet::Data(p, _) => {
                if decoder.send_packet(&p).is_err() {
                    continue;
                }
            }
            Packet::Flush => {
                decoder.flush();
                presenter.shown = false;
                continue;
            }
            Packet::Decoder(..) => continue,
            Packet::End(_) => {
                let _ = decoder.send_eof();
                ending = true;
            }
        }
        while decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame.timestamp().or(frame.pts()).map_or(0.0, |t| t as f64 * f64::from(time_base));
            let Some(picture) = converter.picture(&frame, pts) else { continue };
            presenter.present(picture);
            if shared.stopped() {
                return;
            }
        }
        if std::mem::take(&mut ending) {
            shared.finished();
        }
    }
}

/// Brings decoded frames to what the screen takes: back from the GPU (as NV12), and
/// other pixel formats (10-bit, 4:2:2, …) to 8-bit 4:2:0.
struct Converter {
    downloaded: ffmpeg::frame::Video,
    converted: ffmpeg::frame::Video,
    /// The scaler, and the format and size it was made for.
    scaler: Option<(ffmpeg::software::scaling::Context, (Pixel, u32, u32))>,
}

impl Default for Converter {
    fn default() -> Self {
        Converter { downloaded: ffmpeg::frame::Video::empty(), converted: ffmpeg::frame::Video::empty(), scaler: None }
    }
}

impl Converter {
    fn picture(&mut self, frame: &ffmpeg::frame::Video, pts: f64) -> Option<VideoFrame> {
        let frame = match super::hwaccel::download(frame, &mut self.downloaded) {
            Some(true) => &self.downloaded,
            Some(false) => return None,
            None => frame,
        };
        if let Some(picture) = VideoFrame::from_decoded(frame, pts) {
            return Some(picture);
        }
        let key = (frame.format(), frame.width(), frame.height());
        if self.scaler.as_ref().is_none_or(|(_, k)| *k != key) {
            let flags = ffmpeg::software::scaling::Flags::FAST_BILINEAR;
            self.scaler = ffmpeg::software::scaling::Context::get(key.0, key.1, key.2, Pixel::YUV420P, key.1, key.2, flags)
                .map_err(|e| log_warn!("can't convert {:?} pictures: {e}", key.0))
                .ok()
                .map(|s| (s, key));
        }
        let (scaler, _) = self.scaler.as_mut()?;
        scaler.run(frame, &mut self.converted).ok()?;
        self.converted.set_color_space(frame.color_space());
        self.converted.set_color_range(frame.color_range());
        VideoFrame::from_decoded(&self.converted, pts)
    }
}

/// What to do with a frame `ahead` seconds ahead of the clock (behind when negative).
#[derive(Debug, PartialEq)]
enum Timing {
    Show,
    /// Not yet: look again after this long.
    Wait(Duration),
    /// Too late: drop it (`skipped` frames in a row were dropped before it).
    Skip,
    /// The stream jumped and nothing else keeps the time: set the clock to it, show.
    Resync,
}

fn timing(ahead: f64, has_audio: bool, skipped: u32) -> Timing {
    if ahead > JUMP {
        return match has_audio {
            // Far ahead for good: the sound isn't coming along, show it anyway.
            true if ahead > JUMP * 4.0 => Timing::Show,
            // Wait for the sound to jump too; meanwhile keep this one.
            true => Timing::Wait(Duration::from_millis(10)),
            false => Timing::Resync,
        };
    }
    if ahead > 0.002 {
        return Timing::Wait(Duration::from_secs_f64(ahead.min(0.01)));
    }
    if ahead < -LATE {
        if ahead < -JUMP && !has_audio {
            return Timing::Resync;
        }
        if skipped + 1 < SHOW_EVERY {
            return Timing::Skip;
        }
    }
    Timing::Show
}

struct Presenter {
    shared: Arc<Shared>,
    has_audio: bool,
    /// A frame was shown since the start or the last seek.
    shown: bool,
    skipped: u32,
    /// Since when frames wait for a clock that hasn't started.
    waiting_since: Option<Instant>,
}

impl Presenter {
    fn present(&mut self, frame: VideoFrame) {
        let epoch = self.shared.epoch.load(Ordering::Relaxed);
        if !self.shown {
            // The first picture shows at once, as a still until the sound starts.
            self.shown = true;
            if !self.has_audio {
                self.shared.clock.set(frame.pts);
                self.shared.set_status(Status::Playing);
            }
            self.shared.publish(frame);
            return;
        }
        loop {
            if self.shared.stopped() || self.shared.epoch.load(Ordering::Relaxed) != epoch {
                return;
            }
            let Some(now) = self.shared.clock.now() else {
                // Without sound the picture keeps the time (after a seek, too).
                if !self.has_audio {
                    self.shared.clock.set(frame.pts);
                    continue;
                }
                let since = *self.waiting_since.get_or_insert_with(Instant::now);
                if since.elapsed() > NO_CLOCK {
                    // No sound after all (or it can't be decoded): run on the wall clock.
                    log_info!("no audio clock, the picture runs on its own");
                    self.shared.clock.set(frame.pts);
                    self.has_audio = false;
                    self.shared.set_status(Status::Playing);
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
                continue;
            };
            self.waiting_since = None;
            match timing(frame.pts - now, self.has_audio, self.skipped) {
                Timing::Wait(d) => std::thread::sleep(d),
                Timing::Skip => {
                    self.skipped += 1;
                    return;
                }
                Timing::Resync => {
                    self.shared.clock.set(frame.pts);
                    break;
                }
                Timing::Show => break,
            }
        }
        self.skipped = 0;
        self.shared.publish(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_wait_show_or_skip_against_the_clock() {
        assert_eq!(timing(0.0, true, 0), Timing::Show);
        assert_eq!(timing(0.005, true, 0), Timing::Wait(Duration::from_secs_f64(0.005)));
        assert_eq!(timing(1.0, true, 0), Timing::Wait(Duration::from_millis(10)), "waits in small steps");
        assert_eq!(timing(-0.1, true, 0), Timing::Show, "a little late still shows");
        assert_eq!(timing(-0.5, true, 0), Timing::Skip);
    }

    #[test]
    fn a_slow_machine_still_shows_one_frame_in_a_few() {
        assert_eq!(timing(-0.5, true, SHOW_EVERY - 2), Timing::Skip);
        assert_eq!(timing(-0.5, true, SHOW_EVERY - 1), Timing::Show);
    }

    #[test]
    fn a_jump_in_the_stream_waits_for_the_sound_or_resets_the_clock() {
        assert_eq!(timing(JUMP + 1.0, true, 0), Timing::Wait(Duration::from_millis(10)));
        assert_eq!(timing(JUMP * 5.0, true, 0), Timing::Show);
        assert_eq!(timing(JUMP + 1.0, false, 0), Timing::Resync);
        assert_eq!(timing(-JUMP - 1.0, false, 0), Timing::Resync);
        assert_eq!(timing(-JUMP - 1.0, true, 0), Timing::Skip, "the sound keeps the time");
    }
}
