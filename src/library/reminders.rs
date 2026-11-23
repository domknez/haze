//! Programmes the viewer asked to be reminded of, kept in `reminders.json`.

use crate::iptv::model::Programme;
use serde_json::{Value, json};
use std::path::PathBuf;

/// How long before a programme starts the reminder comes.
pub const LEAD: i64 = 60;

/// A programme to be reminded of, on a channel of a source.
#[derive(Clone, Debug, PartialEq)]
pub struct Reminder {
    pub source: String,
    pub channel: String,
    pub title: String,
    pub start: i64,
    pub stop: i64,
}

impl Reminder {
    fn to_json(&self) -> Value {
        json!({"source": self.source, "channel": self.channel, "title": self.title, "start": self.start, "stop": self.stop})
    }

    fn from_json(v: &Value) -> Option<Reminder> {
        Some(Reminder {
            source: v["source"].as_str()?.to_string(),
            channel: v["channel"].as_str()?.to_string(),
            title: v["title"].as_str().unwrap_or_default().to_string(),
            start: v["start"].as_i64()?,
            stop: v["stop"].as_i64()?,
        })
    }

    fn is(&self, source: &str, channel: &str, start: i64) -> bool {
        self.source == source && self.channel == channel && self.start == start
    }
}

/// The reminders set, soonest first.
#[derive(Default)]
pub struct Reminders {
    list: Vec<Reminder>,
    file: Option<PathBuf>,
}

impl Reminders {
    /// The reminders saved, but for programmes that are over.
    pub fn load(now: i64) -> Reminders {
        let file = super::settings_file("reminders.json");
        let v = super::read_json(file.as_deref());
        let mut list: Vec<Reminder> = v["reminders"].as_array().into_iter().flatten().filter_map(Reminder::from_json).collect();
        list.retain(|r| r.stop > now);
        list.sort_by_key(|r| r.start);
        Reminders { list, file }
    }

    fn changed(&mut self) {
        let list: Vec<Value> = self.list.iter().map(Reminder::to_json).collect();
        super::write_json(self.file.as_deref(), &json!({ "reminders": list }));
    }

    pub fn is_set(&self, source: &str, channel: &str, start: i64) -> bool {
        self.list.iter().any(|r| r.is(source, channel, start))
    }

    /// Be reminded of `programme` on `channel`, or not any more. True when it's set.
    pub fn toggle(&mut self, source: &str, channel: &str, programme: &Programme) -> bool {
        let set = if self.is_set(source, channel, programme.start) {
            self.list.retain(|r| !r.is(source, channel, programme.start));
            false
        } else {
            let at = self.list.partition_point(|r| r.start <= programme.start);
            let r = Reminder {
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

    /// The reminders of `source`, soonest first.
    pub fn of<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a Reminder> {
        self.list.iter().filter(move |r| r.source == source)
    }

    /// Take the reminders whose time has come at `now`; those for programmes already
    /// over (the computer slept through them) go without a word.
    pub fn due(&mut self, now: i64) -> Vec<Reminder> {
        if !self.list.iter().any(|r| r.start - LEAD <= now) {
            return Vec::new();
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.list).into_iter().partition(|r| r.start - LEAD <= now);
        self.list = later;
        self.changed();
        due.into_iter().filter(|r| r.stop > now).collect()
    }

    /// When the next reminder comes.
    pub fn next_at(&self) -> Option<i64> {
        self.list.first().map(|r| r.start - LEAD)
    }

    /// Give a source's reminders new channel keys (the ones `rekey` has one for).
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
        let mut r = Reminders::default();
        assert!(r.toggle("a", "1", &programme("Late", 5000)));
        assert!(r.toggle("a", "2", &programme("Early", 1000)));
        assert!(r.toggle("b", "1", &programme("Elsewhere", 3000)));
        assert_eq!(r.next_at(), Some(1000 - LEAD));
        assert_eq!(r.of("a").map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Early", "Late"]);
        assert!(r.due(1000 - LEAD - 1).is_empty());
        assert_eq!(r.due(1000 - LEAD).iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Early"]);
        assert!(r.due(1000).is_empty(), "only once");
        // Slept through one: it's over by the time the computer wakes.
        assert!(r.due(3000 + 1800).is_empty());
        assert_eq!(r.of("a").count(), 1);
    }

    #[test]
    fn toggle_takes_it_back() {
        let mut r = Reminders::default();
        let p = programme("News", 1000);
        assert!(r.toggle("a", "1", &p));
        assert!(r.is_set("a", "1", 1000) && !r.is_set("b", "1", 1000));
        assert!(!r.toggle("a", "1", &p));
        assert!(!r.is_set("a", "1", 1000));
        r.toggle("a", "1", &p);
        r.forget_source("a");
        assert_eq!(r.next_at(), None);
    }
}
