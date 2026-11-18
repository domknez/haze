//! How far movies and episodes were watched, kept in `progress.json`.

use serde_json::{Value, json};
use std::path::PathBuf;

/// How far movies and episodes were watched, by source id and key, newest first.
#[derive(Default)]
pub struct Progress {
    items: Vec<Watched>,
    file: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
struct Watched {
    source: String,
    key: String,
    /// Seconds from the start.
    position: f64,
    duration: f64,
}

/// Movies and episodes whose progress is kept.
const WATCHED: usize = 500;
/// Played this far (of the length) counts as seen: what's left is the credits.
const SEEN: f64 = 0.93;
/// Less than this far in starts from the beginning next time.
const RESUME_AFTER: f64 = 30.0;

impl Progress {
    pub fn load() -> Progress {
        let file = super::settings_file("progress.json");
        let v = super::read_json(file.as_deref());
        let items = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| {
                Some(Watched {
                    source: w["source"].as_str()?.to_string(),
                    key: w["key"].as_str()?.to_string(),
                    position: w["position"].as_f64()?,
                    duration: w["duration"].as_f64()?,
                })
            })
            .collect();
        Progress { items, file }
    }

    fn save(&self) {
        let v: Value = self
            .items
            .iter()
            .map(|w| json!({"source": w.source, "key": w.key, "position": (w.position * 10.0).round() / 10.0, "duration": w.duration.round()}))
            .collect();
        super::write_json(self.file.as_deref(), &v);
    }

    fn find(&self, source: &str, key: &str) -> Option<&Watched> {
        self.items.iter().find(|w| w.source == source && w.key == key)
    }

    /// Keep how far `key` was watched, `position` seconds of `duration`.
    pub fn record(&mut self, source: &str, key: &str, position: f64, duration: f64) {
        if !(duration > 0.0 && position.is_finite()) {
            return;
        }
        let was = self.find(source, key).cloned();
        let position = position.clamp(0.0, duration);
        // Watched to the end once stays seen when it's started again and stopped early.
        let seen = was.as_ref().is_some_and(|w| w.position >= w.duration * SEEN) && position < RESUME_AFTER;
        if seen || was.as_ref().is_some_and(|w| (w.position - position).abs() < 1.0 && w.duration == duration) {
            return;
        }
        self.items.retain(|w| !(w.source == source && w.key == key));
        self.items.insert(0, Watched { source: source.into(), key: key.into(), position, duration });
        self.items.truncate(WATCHED);
        self.save();
    }

    /// Where to carry on watching `key`, if it was left part of the way through.
    pub fn resume_at(&self, source: &str, key: &str) -> Option<f64> {
        self.find(source, key).filter(|w| w.position >= RESUME_AFTER && w.position < w.duration * SEEN).map(|w| w.position)
    }

    /// How much of `key` was watched (0 to 1), 1 once it's seen.
    pub fn fraction(&self, source: &str, key: &str) -> Option<f32> {
        let w = self.find(source, key).filter(|w| w.position >= RESUME_AFTER)?;
        Some(if w.position >= w.duration * SEEN { 1.0 } else { (w.position / w.duration) as f32 })
    }

    pub fn forget_source(&mut self, source: &str) {
        self.items.retain(|w| w.source != source);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_how_far_things_were_watched() {
        let mut p = Progress::default();
        p.record("a", "m", 10.0, 6000.0);
        assert_eq!((p.resume_at("a", "m"), p.fraction("a", "m")), (None, None));
        p.record("a", "m", 1500.0, 6000.0);
        assert_eq!((p.resume_at("a", "m"), p.fraction("a", "m")), (Some(1500.0), Some(0.25)));
        assert_eq!(p.resume_at("b", "m"), None);
        p.record("a", "m", 5700.0, 6000.0);
        assert_eq!((p.resume_at("a", "m"), p.fraction("a", "m")), (None, Some(1.0)));
        // Started again and stopped at once: still seen.
        p.record("a", "m", 5.0, 6000.0);
        assert_eq!(p.fraction("a", "m"), Some(1.0));
        p.record("a", "m", 600.0, 6000.0);
        assert_eq!(p.resume_at("a", "m"), Some(600.0));
        p.forget_source("a");
        assert_eq!(p.fraction("a", "m"), None);
    }
}
