//! Sample-rate conversion: files that aren't at the output's 44.1 kHz (podcasts,
//! server files) and sound cards that don't run at it. One linear resampler for
//! both, over i16 (decoded audio) or f32 (the sound card's samples).

/// A sample type the resampler interpolates.
pub trait Sample: Copy {
    /// The value `t` (0..1) of the way from `a` to `b`.
    fn lerp(a: Self, b: Self, t: f64) -> Self;
}

impl Sample for i16 {
    fn lerp(a: i16, b: i16, t: f64) -> i16 {
        (a as f64 + (b as f64 - a as f64) * t).round() as i16
    }
}

impl Sample for f32 {
    fn lerp(a: f32, b: f32, t: f64) -> f32 {
        a + (b - a) * t as f32
    }
}

/// Streaming linear interpolating resampler for interleaved stereo. Plenty for a
/// music player, where the rates rarely differ, and it keeps no more state than the
/// last frame.
pub struct Resampler<T> {
    /// Input frames per output frame.
    step: f64,
    /// Position of the next output frame, in input frames relative to the start of
    /// the next block (-1.0 is `last`).
    pos: f64,
    /// Last frame of the previous block.
    last: Option<[T; 2]>,
}

impl<T: Sample> Resampler<T> {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self { step: from_rate as f64 / to_rate as f64, pos: 0.0, last: None }
    }

    /// Whether the rates are the same (output equals input).
    pub fn is_identity(&self) -> bool {
        self.step == 1.0
    }

    /// Resample a block of interleaved stereo samples, appending to `out`.
    pub fn process(&mut self, input: &[T], out: &mut Vec<T>) {
        let frames = input.len() / 2;
        if frames == 0 {
            return;
        }
        let at = |i: isize| -> [T; 2] {
            if i < 0 {
                self.last.unwrap_or([input[0], input[1]])
            } else {
                let i = i as usize * 2;
                [input[i], input[i + 1]]
            }
        };
        // Interpolate between frame floor(pos) and the one after, both available.
        while self.pos < frames as f64 - 1.0 {
            let i = self.pos.floor();
            let frac = self.pos - i;
            let (a, b) = (at(i as isize), at(i as isize + 1));
            out.push(T::lerp(a[0], b[0], frac));
            out.push(T::lerp(a[1], b[1], frac));
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        self.last = Some(at(frames as isize - 1));
    }

    /// Forget the previous block (the next one doesn't follow on from it: a seek).
    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resample a ramp of `frames` frames (left rises by `slope` per frame, right
    /// falls), fed in blocks of `block` frames.
    fn run(from: u32, to: u32, frames: usize, block: usize, slope: f64) -> Vec<i16> {
        let input: Vec<i16> = (0..frames).map(|i| (i as f64 * slope) as i16).flat_map(|x| [x, -x]).collect();
        let mut r = Resampler::new(from, to);
        let mut out = Vec::new();
        for chunk in input.chunks(block * 2) {
            r.process(chunk, &mut out);
        }
        out
    }

    #[test]
    fn doubles_by_interpolating() {
        let out = run(22_050, 44_100, 4, 4, 10.0);
        assert_eq!(out, vec![0, 0, 5, -5, 10, -10, 15, -15, 20, -20, 25, -25]);
    }

    #[test]
    fn rate_and_continuity_hold_across_blocks() {
        // 1 s at 48 kHz becomes ~1 s at 44.1 kHz, the same in one block or many.
        let whole = run(48_000, 44_100, 48_000, 48_000, 0.5);
        let pieces = run(48_000, 44_100, 48_000, 1_152, 0.5);
        assert!((whole.len() as i64 / 2 - 44_100).abs() <= 2, "{} frames", whole.len() / 2);
        assert_eq!(whole, pieces);
        // A ramp stays a ramp: no jumps at block edges.
        assert!(pieces.chunks(2).zip(pieces.chunks(2).skip(1)).all(|(a, b)| (b[0] - a[0]).abs() <= 1));
    }

    #[test]
    fn same_rate_passes_through() {
        assert_eq!(run(44_100, 44_100, 5, 2, 10.0), vec![0, 0, 10, -10, 20, -20, 30, -30]);
        assert!(Resampler::<i16>::new(44_100, 44_100).is_identity());
    }

    #[test]
    fn float_samples_upsample_for_a_sound_card() {
        // 1 s at 44.1 kHz fills ~1 s at 48 kHz, in the player's packet-sized blocks.
        let mut r = Resampler::new(44_100, 48_000);
        let input = vec![0.5f32; 44_100 * 2];
        let mut out = Vec::new();
        for chunk in input.chunks(1152 * 2) {
            r.process(chunk, &mut out);
        }
        assert!((47_990..=48_000).contains(&(out.len() / 2)), "{}", out.len() / 2);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6), "a constant stays constant");
    }

    #[test]
    fn reset_starts_over_without_the_old_block() {
        let mut r = Resampler::new(22_050, 44_100);
        let mut out = Vec::new();
        r.process(&[1000i16, 1000, 2000, 2000], &mut out);
        r.reset();
        out.clear();
        r.process(&[0i16, 0, 10, 10], &mut out);
        // Starts at the new block's first frame, not interpolated from the old one.
        assert_eq!(&out[..4], &[0, 0, 5, 5]);
    }
}
