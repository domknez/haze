//! The sleep timer: playing stops after a while, or when the programme on ends, the
//! sound fading out over the last half minute, so the computer can sleep too.

use super::App;
use crate::iptv::time;
use eframe::egui;
use std::time::Duration;

/// How long the sound takes to fade out before playing stops.
const FADE: i64 = 30;
/// How often to look while there's long to go: the UI may otherwise sleep.
const LOOK: i64 = 60;

/// When to stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timer {
    /// In so many minutes.
    In(u32),
    /// When the programme on the channel playing ends.
    EndOfProgramme,
}

impl Timer {
    pub const MINUTES: [u32; 5] = [15, 30, 45, 60, 90];
}

/// The sleep timer set, if any.
#[derive(Default)]
pub struct Sleep {
    /// When playing stops, in seconds since the epoch.
    pub at: Option<i64>,
    /// What was picked, to mark it in the menu.
    pub timer: Option<Timer>,
}

impl Sleep {
    /// How loud to play `at` (seconds since the epoch): fading to nothing over the
    /// last half minute.
    fn loudness(&self, now: i64) -> f32 {
        match self.at {
            Some(at) if at - now < FADE => ((at - now) as f32 / FADE as f32).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }
}

/// How long until `at`, from `now`, in words: `in 25 min`, `in 1 h 30 min`.
pub fn left(at: i64, now: i64) -> String {
    let minutes = ((at - now).max(0) + 59) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("in {m} min"),
        (h, 0) => format!("in {h} h"),
        (h, m) => format!("in {h} h {m} min"),
    }
}

impl App {
    /// Set the sleep timer, or turn it off (None).
    pub(super) fn set_sleep(&mut self, timer: Option<Timer>) {
        let now = time::now();
        let at = match timer {
            None => None,
            Some(Timer::In(minutes)) => Some(now + i64::from(minutes) * 60),
            Some(Timer::EndOfProgramme) => match self.programme_playing() {
                Some(p) if p.stop > now => Some(p.stop),
                _ => {
                    self.show_toast("Nothing in the guide for this channel now");
                    return;
                }
            },
        };
        self.sleep = Sleep { at, timer: timer.filter(|_| at.is_some()) };
        self.update_volume();
        match at {
            Some(at) => self.show_toast(&format!("Stops at {} ({})", crate::ui::views::hhmm(at), left(at, now))),
            None => self.show_toast("Sleep timer off"),
        }
    }

    /// The programme on now on the channel playing live.
    pub fn programme_playing(&self) -> Option<&crate::iptv::model::Programme> {
        let Some(super::Playing::Live { key }) = &self.session.playing else { return None };
        self.now_on(self.channel(key)?)
    }

    /// How loud the sleep timer lets it play now.
    pub(super) fn sleep_loudness(&self) -> f32 {
        self.sleep.loudness(time::now())
    }

    /// Fade out and stop when the time comes.
    pub(super) fn sleep_when_due(&mut self, ctx: &egui::Context) {
        let Some(at) = self.sleep.at else { return };
        let now = time::now();
        if now >= at {
            self.sleep = Sleep::default();
            if self.session.player.is_some() {
                log_info!("sleep timer: stopping");
                if self.mini_on() {
                    self.toggle_mini(ctx);
                }
                self.stop();
                self.show_toast("Stopped by the sleep timer");
            }
            self.update_volume();
        } else if at - now <= FADE {
            self.update_volume();
            ctx.request_repaint_after(Duration::from_millis(250));
        } else {
            ctx.request_repaint_after(Duration::from_secs((at - now - FADE).min(LOOK) as u64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_out_over_the_last_half_minute() {
        let sleep = Sleep { at: Some(1000), timer: Some(Timer::In(15)) };
        assert_eq!(sleep.loudness(900), 1.0);
        assert_eq!(sleep.loudness(985), 0.5);
        assert_eq!(sleep.loudness(1000), 0.0);
        assert_eq!(Sleep::default().loudness(1000), 1.0);
    }

    #[test]
    fn says_how_long_is_left() {
        assert_eq!(left(1000, 1000 - 25 * 60), "in 25 min");
        assert_eq!(left(1000, 1000 - 20), "in 1 min");
        assert_eq!(left(1000, 1000 - 90 * 60), "in 1 h 30 min");
        assert_eq!(left(1000, 1000 - 120 * 60), "in 2 h");
    }
}
