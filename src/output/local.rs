//! Local sound card output via cpal, fed through a lock-free ring buffer.

use super::resample::Resampler;
use super::{RATE, Sink};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// State the sink and the device callback share.
struct Shared {
    volume: AtomicU32,
    paused: AtomicBool,
    /// Set by the sink to drop what is queued; cleared by the callback once it has.
    flush: AtomicBool,
}

/// The sink's end of the ring buffer.
struct Feed {
    producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
}

/// The device callback's end of the ring buffer.
struct Drain {
    consumer: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
}

/// A ring buffer of `samples` interleaved stereo samples, both ends.
fn ring(samples: usize, volume: f32) -> (Feed, Drain) {
    let (producer, consumer) = rtrb::RingBuffer::new(samples);
    let shared =
        Arc::new(Shared { volume: AtomicU32::new(volume.to_bits()), paused: AtomicBool::new(false), flush: AtomicBool::new(false) });
    (Feed { producer, shared: shared.clone() }, Drain { consumer, shared })
}

impl Feed {
    /// Samples that can be written now: none while a flush waits for the callback.
    fn room(&self) -> usize {
        if self.flushing() { 0 } else { self.producer.slots() }
    }

    /// Samples waiting for the device; none while a flush waits (they are dropped).
    fn queued(&self) -> usize {
        if self.flushing() { 0 } else { self.producer.buffer().capacity() - self.producer.slots() }
    }

    fn push(&mut self, samples: &[f32]) {
        if let Ok(chunk) = self.producer.write_chunk_uninit(samples.len()) {
            chunk.fill_from_iter(samples.iter().copied());
        }
    }

    fn flush(&self) {
        self.shared.flush.store(true, Ordering::Release);
    }

    fn flushing(&self) -> bool {
        self.shared.flush.load(Ordering::Acquire)
    }
}

impl Drain {
    /// The device callback: fill `out` (frames of `channels`) from the ring, silence
    /// when paused or short. Runs on the realtime audio thread, so no allocation.
    fn fill<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], channels: usize) {
        let shared = &self.shared;
        if shared.flush.load(Ordering::Acquire) {
            let n = self.consumer.slots();
            if let Ok(c) = self.consumer.read_chunk(n) {
                c.commit_all();
            }
            // Only now: the sink writes nothing while the flag is set, so all that
            // was just dropped came before the flush.
            shared.flush.store(false, Ordering::Release);
        }
        let silent = shared.paused.load(Ordering::Relaxed);
        let v = f32::from_bits(shared.volume.load(Ordering::Relaxed));
        let gain = v * v; // rough perceptual curve
        for frame in out.chunks_mut(channels) {
            let (l, r) = if !silent && self.consumer.slots() >= 2 {
                (self.consumer.pop().unwrap_or(0.0) * gain, self.consumer.pop().unwrap_or(0.0) * gain)
            } else {
                (0.0, 0.0)
            };
            for (i, s) in frame.iter_mut().enumerate() {
                *s = T::from_sample(match i {
                    0 => l,
                    1 => r,
                    _ => 0.0,
                });
            }
        }
    }
}

pub struct LocalSink {
    _stream: cpal::Stream,
    feed: Feed,
    resampler: Resampler<f32>,
    rate: u32,
    scratch: Vec<f32>,
    resampled: Vec<f32>,
}

impl LocalSink {
    pub fn new(volume: f32) -> Result<Self, String> {
        let device = cpal::default_host().default_output_device().ok_or("No audio output device")?;
        let default = device.default_output_config().map_err(|e| e.to_string())?;
        // Prefer native 44.1 kHz to avoid resampling.
        let config = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .filter(|c| c.sample_format() == default.sample_format() && c.channels() >= 2)
            .find(|c| c.min_sample_rate() <= RATE && RATE <= c.max_sample_rate())
            .map(|c| c.with_sample_rate(RATE))
            .unwrap_or(default);
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        // ~200 ms of stereo audio at device rate.
        let (feed, drain) = ring((rate as usize * 2 / 5) & !1, volume);
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config.into(), channels, drain),
            SampleFormat::I16 => build::<i16>(&device, config.into(), channels, drain),
            SampleFormat::U16 => build::<u16>(&device, config.into(), channels, drain),
            SampleFormat::I32 => build::<i32>(&device, config.into(), channels, drain),
            f => return Err(format!("Unsupported sample format {f}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { _stream: stream, feed, resampler: Resampler::new(RATE, rate), rate, scratch: Vec::new(), resampled: Vec::new() })
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut drain: Drain,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(config, move |out: &mut [T], _| drain.fill(out, channels), |e| log_error!("audio stream: {e}"), None)
        .map_err(|e| e.to_string())
}

impl Sink for LocalSink {
    fn write(&mut self, samples: &[i16]) -> usize {
        // Worst-case output size after resampling, rounded to whole frames.
        let needed = ((samples.len() as u64 * self.rate as u64).div_ceil(RATE as u64) as usize + 4) & !1;
        // Also refused while a flush is pending (room is 0 then): the callback
        // would throw these away with the old audio.
        if self.feed.room() < needed {
            return 0;
        }
        self.scratch.clear();
        self.scratch.extend(samples.iter().map(|&s| s as f32 / 32768.0));
        if self.resampler.is_identity() {
            let scratch = std::mem::take(&mut self.scratch);
            self.feed.push(&scratch);
            self.scratch = scratch;
        } else {
            self.resampled.clear();
            let mut out = std::mem::take(&mut self.resampled);
            self.resampler.process(&self.scratch, &mut out);
            self.feed.push(&out);
            self.resampled = out;
        }
        samples.len()
    }

    fn pending_frames(&self) -> usize {
        self.feed.queued() / 2 * RATE as usize / self.rate as usize
    }

    fn latency_frames(&self) -> usize {
        0
    }

    fn pause(&mut self) {
        self.feed.shared.paused.store(true, Ordering::Relaxed);
    }

    fn resume(&mut self) {
        self.feed.shared.paused.store(false, Ordering::Relaxed);
    }

    fn flush(&mut self) {
        self.resampler.reset();
        self.feed.flush();
    }

    fn set_volume(&mut self, volume: f32) {
        self.feed.shared.volume.store(volume.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the device plays in one callback of `frames` stereo frames.
    fn play(drain: &mut Drain, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0; frames * 2];
        drain.fill(&mut out, 2);
        out
    }

    #[test]
    fn plays_what_was_written() {
        let (mut feed, mut drain) = ring(16, 1.0);
        feed.push(&[0.5, -0.5]);
        assert_eq!(feed.queued(), 2);
        assert_eq!(play(&mut drain, 2), [0.5, -0.5, 0.0, 0.0]);
        assert_eq!(feed.queued(), 0);
    }

    #[test]
    fn audio_written_after_a_flush_survives_it() {
        let (mut feed, mut drain) = ring(16, 1.0);
        feed.push(&[0.1; 8]);
        feed.flush();
        // Until the callback has dropped the old audio, nothing is queued and
        // nothing new can go in (it would be dropped with the old).
        assert_eq!((feed.queued(), feed.room()), (0, 0));
        assert_eq!(play(&mut drain, 2), [0.0; 4], "old audio dropped");
        assert_eq!(feed.room(), 16);
        feed.push(&[0.5, 0.5]);
        assert_eq!(play(&mut drain, 1), [0.5, 0.5], "new audio plays");
    }

    #[test]
    fn paused_plays_silence_and_keeps_the_audio() {
        let (mut feed, mut drain) = ring(16, 1.0);
        feed.push(&[0.5, 0.5]);
        feed.shared.paused.store(true, Ordering::Relaxed);
        assert_eq!(play(&mut drain, 1), [0.0, 0.0]);
        assert_eq!(feed.queued(), 2);
    }
}
