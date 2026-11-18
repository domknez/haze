//! Favourite and recently watched channels, kept in `marks.json`.

use serde_json::json;
use std::path::PathBuf;

/// Favourite and recently watched channels, by source id and channel key.
#[derive(Default)]
pub struct Marks {
    favorites: Vec<(String, String)>,
    recent: Vec<(String, String)>,
    file: Option<PathBuf>,
    /// Bumped on every change, so lists built from the marks know to rebuild.
    revision: u64,
}

/// Recently watched channels kept per source.
const RECENT: usize = 30;

impl Marks {
    pub fn load() -> Marks {
        let file = super::settings_file("marks.json");
        let v = super::read_json(file.as_deref());
        let pairs = |k: &str| -> Vec<(String, String)> {
            v[k].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string()))).collect()
        };
        Marks { favorites: pairs("favorites"), recent: pairs("recent"), file, revision: 0 }
    }

    /// Changes with every change to the marks.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn changed(&mut self) {
        self.revision += 1;
        super::write_json(self.file.as_deref(), &json!({"favorites": self.favorites, "recent": self.recent}));
    }

    pub fn is_favorite(&self, source: &str, key: &str) -> bool {
        self.favorites.iter().any(|(s, k)| s == source && k == key)
    }

    pub fn toggle_favorite(&mut self, source: &str, key: &str) {
        if self.is_favorite(source, key) {
            self.favorites.retain(|(s, k)| !(s == source && k == key));
        } else {
            self.favorites.push((source.to_string(), key.to_string()));
        }
        self.changed();
    }

    /// Move a favourite of `source` beside another of its favourites: just before
    /// `target`, or just after it.
    pub fn move_favorite(&mut self, source: &str, key: &str, target: &str, after: bool) {
        let at = |list: &Vec<(String, String)>, key: &str| list.iter().position(|(s, k)| s == source && k == key);
        if key == target || at(&self.favorites, target).is_none() {
            return;
        }
        let Some(from) = at(&self.favorites, key) else { return };
        let moved = self.favorites.remove(from);
        let to = at(&self.favorites, target).unwrap_or(from) + usize::from(after);
        self.favorites.insert(to, moved);
        self.changed();
    }

    pub fn favorites<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a str> {
        self.favorites.iter().filter(move |(s, _)| s == source).map(|(_, k)| k.as_str())
    }

    pub fn watched(&mut self, source: &str, key: &str) {
        self.recent.retain(|(s, k)| !(s == source && k == key));
        self.recent.insert(0, (source.to_string(), key.to_string()));
        let mut kept = 0;
        self.recent.retain(|(s, _)| {
            if s != source {
                return true;
            }
            kept += 1;
            kept <= RECENT
        });
        self.changed();
    }

    pub fn recent<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a str> {
        self.recent.iter().filter(move |(s, _)| s == source).map(|(_, k)| k.as_str())
    }

    /// Give a source's marks new keys (the ones `rekey` has one for).
    pub fn rekey(&mut self, source: &str, rekey: impl Fn(&str) -> Option<String>) {
        for (s, k) in self.favorites.iter_mut().chain(self.recent.iter_mut()) {
            if s == source
                && let Some(new) = rekey(k)
            {
                *k = new;
            }
        }
        self.changed();
    }

    pub fn forget_source(&mut self, source: &str) {
        self.favorites.retain(|(s, _)| s != source);
        self.recent.retain(|(s, _)| s != source);
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iptv::xtream;

    #[test]
    fn every_change_moves_the_revision() {
        let mut m = Marks::default();
        let mut last = m.revision();
        let mut changed = |m: &Marks| {
            let moved = m.revision() > last;
            last = m.revision();
            moved
        };
        m.toggle_favorite("a", "1");
        assert!(changed(&m));
        m.toggle_favorite("a", "2");
        m.move_favorite("a", "2", "1", false);
        assert!(changed(&m));
        m.watched("a", "1");
        assert!(changed(&m));
        m.forget_source("a");
        assert!(changed(&m));
        assert!(!changed(&m), "reading changes nothing");
    }

    #[test]
    fn favorites_move_among_their_source() {
        let mut m = Marks::default();
        for (s, k) in [("a", "1"), ("b", "x"), ("a", "2"), ("a", "3"), ("b", "y")] {
            m.toggle_favorite(s, k);
        }
        let order = |m: &Marks| m.favorites("a").collect::<Vec<_>>().join("");
        m.move_favorite("a", "3", "1", false);
        assert_eq!(order(&m), "312");
        m.move_favorite("a", "3", "2", true);
        assert_eq!(order(&m), "123");
        m.move_favorite("a", "1", "2", true);
        assert_eq!(order(&m), "213");
        // Another source's favourite, or one that isn't there, moves nothing.
        m.move_favorite("a", "2", "x", false);
        m.move_favorite("a", "9", "1", false);
        m.move_favorite("a", "2", "2", true);
        assert_eq!(order(&m), "213");
        assert_eq!(m.favorites("b").collect::<Vec<_>>(), ["x", "y"]);
    }

    #[test]
    fn marks_follow_new_keys() {
        let mut m = Marks::default();
        m.toggle_favorite("a", "http://x/me/pw/7");
        m.watched("a", "http://x/me/pw/7");
        m.watched("b", "http://x/me/pw/7");
        m.rekey("a", xtream::stream_id);
        assert_eq!(m.favorites("a").collect::<Vec<_>>(), ["7"]);
        assert_eq!(m.recent("a").collect::<Vec<_>>(), ["7"]);
        assert_eq!(m.recent("b").collect::<Vec<_>>(), ["http://x/me/pw/7"]);
    }

    #[test]
    fn keeps_recent_per_source() {
        let mut m = Marks::default();
        for i in 0..40 {
            m.watched("a", &i.to_string());
        }
        m.watched("b", "x");
        m.watched("a", "5");
        assert_eq!(m.recent("a").count(), RECENT);
        assert_eq!(m.recent("a").next(), Some("5"));
        assert_eq!(m.recent("b").collect::<Vec<_>>(), ["x"]);
        m.toggle_favorite("a", "1");
        assert!(m.is_favorite("a", "1") && !m.is_favorite("b", "1"));
        m.toggle_favorite("a", "1");
        assert!(!m.is_favorite("a", "1"));
    }
}
