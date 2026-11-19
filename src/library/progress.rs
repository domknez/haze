//! How far movies and episodes were watched, kept in `progress.json`.

use serde_json::{Value, json};
use std::path::PathBuf;

/// How far movies and episodes were watched, by source id and key, newest first.
#[derive(Default)]
pub struct Progress {
    items: Vec<Watched>,
    file: Option<PathBuf>,
    /// Moves on with every change, for what's worked out from the progress.
    revision: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Watched {
    source: String,
    key: String,
    /// The series, for an episode.
    series: Option<String>,
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
                    series: w["series"].as_str().map(str::to_string),
                    position: w["position"].as_f64()?,
                    duration: w["duration"].as_f64()?,
                })
            })
            .collect();
        Progress { items, file, revision: 0 }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn save(&mut self) {
        self.revision += 1;
        let v: Value = self
            .items
            .iter()
            .map(|w| {
                let mut v = json!({"source": w.source, "key": w.key, "position": (w.position * 10.0).round() / 10.0, "duration": w.duration.round()});
                if let Some(series) = &w.series {
                    v["series"] = json!(series);
                }
                v
            })
            .collect();
        super::write_json(self.file.as_deref(), &v);
    }

    fn find(&self, source: &str, key: &str) -> Option<&Watched> {
        self.items.iter().find(|w| w.source == source && w.key == key)
    }

    /// Keep how far `key` (an episode of `series`, or a movie) was watched,
    /// `position` seconds of `duration`.
    pub fn record(&mut self, source: &str, key: &str, series: Option<&str>, position: f64, duration: f64) {
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
        self.items.insert(0, Watched { source: source.into(), key: key.into(), series: series.map(str::to_string), position, duration });
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

    /// What to carry on watching from `source`, the latest first: movies left part
    /// of the way through (`Left::Movie`), and series whose episode watched last
    /// wasn't seen to the end, even one only just begun (the next episode starts by
    /// itself, and is often left during its opening).
    pub fn left(&self, source: &str) -> Vec<Left> {
        let mut series_seen: Vec<&str> = Vec::new();
        let mut left = Vec::new();
        for w in self.items.iter().filter(|w| w.source == source) {
            let seen = w.position >= w.duration * SEEN;
            match &w.series {
                Some(series) if !series_seen.contains(&series.as_str()) => {
                    series_seen.push(series);
                    if !seen {
                        left.push(Left::Episode { series: series.clone(), key: w.key.clone() });
                    }
                }
                Some(_) => {}
                None if !seen && w.position >= RESUME_AFTER => left.push(Left::Movie(w.key.clone())),
                None => {}
            }
        }
        left
    }

    pub fn forget_source(&mut self, source: &str) {
        self.items.retain(|w| w.source != source);
        self.save();
    }
}

/// Something to carry on watching.
#[derive(Clone, Debug, PartialEq)]
pub enum Left {
    Movie(String),
    /// An episode (`key`) of `series`.
    Episode {
        series: String,
        key: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_how_far_things_were_watched() {
        let mut p = Progress::default();
        p.record("a", "m", None, 10.0, 6000.0);
        assert_eq!((p.resume_at("a", "m"), p.fraction("a", "m")), (None, None));
        p.record("a", "m", None, 1500.0, 6000.0);
        assert_eq!((p.resume_at("a", "m"), p.fraction("a", "m")), (Some(1500.0), Some(0.25)));
        assert_eq!(p.resume_at("b", "m"), None);
        p.record("a", "m", None, 5700.0, 6000.0);
        assert_eq!((p.resume_at("a", "m"), p.fraction("a", "m")), (None, Some(1.0)));
        // Started again and stopped at once: still seen.
        p.record("a", "m", None, 5.0, 6000.0);
        assert_eq!(p.fraction("a", "m"), Some(1.0));
        p.record("a", "m", None, 600.0, 6000.0);
        assert_eq!(p.resume_at("a", "m"), Some(600.0));
        p.forget_source("a");
        assert_eq!(p.fraction("a", "m"), None);
    }

    #[test]
    fn knows_what_was_left_part_of_the_way() {
        let mut p = Progress::default();
        p.record("a", "dune", None, 1500.0, 6000.0);
        p.record("a", "glimpse", None, 10.0, 6000.0);
        p.record("a", "seen", None, 5900.0, 6000.0);
        p.record("a", "dark-1", Some("dark"), 2900.0, 3000.0);
        p.record("a", "dark-2", Some("dark"), 12.0, 3000.0);
        p.record("a", "lost-1", Some("lost"), 300.0, 3000.0);
        p.record("a", "lost-2", Some("lost"), 2950.0, 3000.0);
        p.record("b", "other", None, 1500.0, 6000.0);
        let episode = |series: &str, key: &str| Left::Episode { series: series.into(), key: key.into() };
        // Lost's last episode was seen to the end; Dark's next one was begun.
        assert_eq!(p.left("a"), [episode("dark", "dark-2"), Left::Movie("dune".into())]);
    }
}
