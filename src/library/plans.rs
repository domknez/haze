//! Programmes still to come that the viewer made plans for: to be reminded of
//! them (kept in `reminders.json`), or to record them (`recordings.json`).

use crate::iptv::model::Programme;
use serde_json::{Value, json};
use std::path::PathBuf;

/// How long before a programme starts its reminder comes, or its recording begins.
pub const LEAD: i64 = 60;

/// A programme on a channel of a source, to be reminded of or recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub source: String,
    pub channel: String,
    pub title: String,
    pub start: i64,
    pub stop: i64,
}

impl Plan {
    fn to_json(&self) -> Value {
        json!({"source": self.source, "channel": self.channel, "title": self.title, "start": self.start, "stop": self.stop})
    }

    fn from_json(v: &Value) -> Option<Plan> {
        Some(Plan {
            source: v["source"].as_str()?.to_string(),
            channel: v["channel"].as_str()?.to_string(),
            title: v["title"].as_str().unwrap_or_default().to_string(),
            start: v["start"].as_i64()?,
            stop: v["stop"].as_i64()?,
        })
    }

    /// The programme it's for, as far as the plan knows it.
    pub fn programme(&self) -> Programme {
        Programme { start: self.start, stop: self.stop, title: self.title.clone(), desc: String::new() }
    }

    /// Whether it's for the programme starting at `start` on `channel` of `source`.
    pub fn is(&self, source: &str, channel: &str, start: i64) -> bool {
        self.source == source && self.channel == channel && self.start == start
    }
}

/// Plans of one kind, soonest first.
#[derive(Default)]
pub struct Plans {
    list: Vec<Plan>,
    file: Option<PathBuf>,
    /// What the file calls them.
    kind: &'static str,
}

impl Plans {
    /// The reminders saved, but for programmes that are over.
    pub fn reminders(now: i64) -> Plans {
        Plans::load("reminders", now)
    }

    /// The recordings to be made, but for programmes that are over.
    pub fn recordings(now: i64) -> Plans {
        Plans::load("recordings", now)
    }

    fn load(kind: &'static str, now: i64) -> Plans {
        let file = super::settings_file(&format!("{kind}.json"));
        let v = super::read_json(file.as_deref());
        let mut list: Vec<Plan> = v[kind].as_array().into_iter().flatten().filter_map(Plan::from_json).collect();
        list.retain(|r| r.stop > now);
        list.sort_by_key(|r| r.start);
        Plans { list, file, kind }
    }

    fn changed(&mut self) {
        let list: Vec<Value> = self.list.iter().map(Plan::to_json).collect();
        super::write_json(self.file.as_deref(), &json!({ self.kind: list }));
    }

    pub fn is_set(&self, source: &str, channel: &str, start: i64) -> bool {
        self.list.iter().any(|r| r.is(source, channel, start))
    }

    /// Plan `programme` on `channel`, or not any more. True when it's set.
    pub fn toggle(&mut self, source: &str, channel: &str, programme: &Programme) -> bool {
        let set = if self.is_set(source, channel, programme.start) {
            self.list.retain(|r| !r.is(source, channel, programme.start));
            false
        } else {
            let at = self.list.partition_point(|r| r.start <= programme.start);
            let r = Plan {
                source: source.to_string(),
                channel: channel.to_string(),
                title: programme.title.clone(),
                start: programme.start,
                stop: programme.stop,
            };
            self.list.insert(at, r);
            true
        };
        self.changed();
        set
    }

    /// The plans of `source`, soonest first.
    pub fn of<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a Plan> {
        self.list.iter().filter(move |r| r.source == source)
    }

    /// The plans whose time has come at `now`, for programmes not over yet.
    pub fn due(&self, now: i64) -> impl Iterator<Item = &Plan> {
        self.list.iter().take_while(move |r| r.start - LEAD <= now).filter(move |r| r.stop > now)
    }

    /// Take the plans whose time has come at `now`; those for programmes already
    /// over (the computer slept through them) go without a word.
    pub fn take_due(&mut self, now: i64) -> Vec<Plan> {
        if !self.list.iter().any(|r| r.start - LEAD <= now) {
            return Vec::new();
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.list).into_iter().partition(|r| r.start - LEAD <= now);
        self.list = later;
        self.changed();
        due.into_iter().filter(|r| r.stop > now).collect()
    }

    /// Drop the plan for the programme starting at `start` on `channel` of `source`.
    pub fn remove(&mut self, source: &str, channel: &str, start: i64) {
        self.list.retain(|r| !r.is(source, channel, start));
        self.changed();
    }

    /// When the next plan comes due, after `now`.
    pub fn next_at(&self, now: i64) -> Option<i64> {
        self.list.iter().map(|r| r.start - LEAD).find(|&at| at > now)
    }

    /// Give a source's plans new channel keys (the ones `rekey` has one for).
    pub fn rekey(&mut self, source: &str, rekey: impl Fn(&str) -> Option<String>) {
        for r in self.list.iter_mut().filter(|r| r.source == source) {
            if let Some(new) = rekey(&r.channel) {
                r.channel = new;
            }
        }
        self.changed();
    }

    pub fn forget_source(&mut self, source: &str) {
        self.list.retain(|r| r.source != source);
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme(title: &str, start: i64) -> Programme {
        Programme { start, stop: start + 1800, title: title.into(), desc: String::new() }
    }

    #[test]
    fn come_a_minute_before_and_once() {
        let mut r = Plans::default();
        assert!(r.toggle("a", "1", &programme("Late", 5000)));
        assert!(r.toggle("a", "2", &programme("Early", 1000)));
        assert!(r.toggle("b", "1", &programme("Elsewhere", 3000)));
        assert_eq!(r.next_at(0), Some(1000 - LEAD));
        assert_eq!(r.of("a").map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Early", "Late"]);
        assert!(r.take_due(1000 - LEAD - 1).is_empty());
        assert_eq!(r.take_due(1000 - LEAD).iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Early"]);
        assert!(r.take_due(1000).is_empty(), "only once");
        // Slept through one: it's over by the time the computer wakes.
        assert!(r.take_due(3000 + 1800).is_empty());
        assert_eq!(r.of("a").count(), 1);
    }

    #[test]
    fn due_ones_stay_until_removed() {
        let mut r = Plans::default();
        r.toggle("a", "1", &programme("News", 1000));
        r.toggle("a", "2", &programme("Film", 5000));
        let titles = |r: &Plans, now| r.due(now).map(|x| x.title.clone()).collect::<Vec<_>>();
        assert!(titles(&r, 1000 - LEAD - 1).is_empty());
        assert_eq!(titles(&r, 1000 - LEAD), ["News"]);
        assert_eq!(titles(&r, 2000), ["News"], "still due while it's on");
        assert!(titles(&r, 1000 + 1800).is_empty(), "not once it's over");
        r.remove("a", "1", 1000);
        assert!(titles(&r, 2000).is_empty());
        assert_eq!(r.next_at(2000), Some(5000 - LEAD));
    }

    #[test]
    fn toggle_takes_it_back() {
        let mut r = Plans::default();
        let p = programme("News", 1000);
        assert!(r.toggle("a", "1", &p));
        assert!(r.is_set("a", "1", 1000) && !r.is_set("b", "1", 1000));
        assert!(!r.toggle("a", "1", &p));
        assert!(!r.is_set("a", "1", 1000));
        r.toggle("a", "1", &p);
        r.forget_source("a");
        assert_eq!(r.next_at(0), None);
    }
}
