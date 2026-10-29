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
    let mut downloaded = ffmpeg::frame::Video::empty();
    let mut converted = ffmpeg::frame::Video::empty();
    let mut scaler: Option<(ffmpeg::software::scaling::Context, (Pixel, u32, u32))> = None;
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
            // A picture decoded on the GPU comes back to memory (as NV12) first.
            let frame = match super::hwaccel::download(&frame, &mut downloaded) {
                Some(true) => &downloaded,
                Some(false) => continue,
                None => &frame,
            };
            let picture = match VideoFrame::from_decoded(frame, pts) {
                Some(p) => p,
                None => {
                    // Other pixel formats (10-bit, 4:2:2, …) become 8-bit 4:2:0.
                    let key = (frame.format(), frame.width(), frame.height());
                    if scaler.as_ref().is_none_or(|(_, k)| *k != key) {
                        scaler = ffmpeg::software::scaling::Context::get(
                            key.0,
                            key.1,
                            key.2,
                            Pixel::YUV420P,
                            key.1,
                            key.2,
                            ffmpeg::software::scaling::Flags::FAST_BILINEAR,
                        )
                        .map_err(|e| log_warn!("can't convert {:?} pictures: {e}", key.0))
                        .ok()
                        .map(|s| (s, key));
                    }
                    let Some((s, _)) = &mut scaler else { continue };
                    if s.run(frame, &mut converted).is_err() {
                        continue;
                    }
                    converted.set_color_space(frame.color_space());
                    converted.set_color_range(frame.color_range());
                    let Some(p) = VideoFrame::from_decoded(&converted, pts) else { continue };
                    p
                }
            };
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
            let ahead = frame.pts - now;
            if ahead > JUMP {
                if self.has_audio {
                    // Wait for the sound to jump too; meanwhile keep this one.
                    std::thread::sleep(Duration::from_millis(10));
                    if ahead > JUMP * 4.0 {
                        break;
                    }
                    continue;
                }
                self.shared.clock.set(frame.pts);
                break;
            }
            if ahead > 0.002 {
                std::thread::sleep(Duration::from_secs_f64(ahead.min(0.01)));
                continue;
            }
            if ahead < -LATE {
                if ahead < -JUMP && !self.has_audio {
                    self.shared.clock.set(frame.pts);
                    break;
                }
                self.skipped += 1;
                if self.skipped < SHOW_EVERY {
                    return;
                }
            }
            break;
        }
        self.skipped = 0;
        self.shared.publish(frame);
    }
}
