//! The playback clock: which moment of the stream is being heard right now. Audio
//! drives it (the speaker can't be told to wait, the picture can); without audio
//! the wall clock does.

use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
pub struct Clock(Mutex<State>);

#[derive(Default, Clone, Copy)]
struct State {
    /// Stream time (seconds) heard at `at`; none before the first sample.
    base: Option<(f64, Instant)>,
    paused: bool,
}

impl Clock {
    /// Stream time now, if the clock has started.
    pub fn now(&self) -> Option<f64> {
        let s = *self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (t, at) = s.base?;
        Some(if s.paused { t } else { t + at.elapsed().as_secs_f64() })
    }

    /// Stream time `t` is being heard now.
    pub fn set(&self, t: f64) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.base = Some((t, Instant::now()));
    }

    pub fn set_paused(&self, paused: bool) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.paused == paused {
            return;
        }
        // Freeze at (or restart from) the current moment.
        if let Some((t, at)) = s.base {
            let now = if s.paused { t } else { t + at.elapsed().as_secs_f64() };
            s.base = Some((now, Instant::now()));
        }
        s.paused = paused;
    }

    pub fn reset(&self) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.base = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn runs_and_pauses() {
        let c = Clock::default();
        assert_eq!(c.now(), None);
        c.set(10.0);
        std::thread::sleep(Duration::from_millis(20));
        let t = c.now().unwrap();
        assert!(t > 10.015 && t < 10.5, "{t}");
        c.set_paused(true);
        let frozen = c.now().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(c.now().unwrap(), frozen);
        c.set_paused(false);
        assert!(c.now().unwrap() - frozen < 0.01);
    }
}
