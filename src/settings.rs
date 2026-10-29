//! Tiny `key=value` settings file in the platform config directory, read and
//! written as typed [`Prefs`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// haze's folder in the platform config directory.
pub fn dir() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    }?;
    Some(base.join("haze"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    System,
    Dark,
    Light,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Dark, Appearance::Light];

    pub fn key(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Dark => "dark",
            Appearance::Light => "light",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        }
    }

    fn from_key(k: &str) -> Option<Appearance> {
        Appearance::ALL.into_iter().find(|a| a.key() == k)
    }
}

/// The viewer's preferences, as kept in the settings file. Change them with
/// [`Prefs::update`], which saves what changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub theme: Appearance,
    /// The source used last time (its id).
    pub source: Option<String>,
    /// The channel watched last (its key in that source).
    pub channel: Option<String>,
    /// The channel group picked last.
    pub group: Option<String>,
    /// The tab open last (`live`, `movies`, …).
    pub tab: Option<String>,
    /// AirPlay speaker (by name) to play on; None for this computer.
    pub output: Option<String>,
    pub volume: f32,
    pub muted: bool,
    /// Picture delay per output (by name), in milliseconds.
    pub picture_delays: BTreeMap<String, i32>,
    /// The hash of the PIN asked before hidden groups show, if one is set.
    pub pin: Option<String>,
    /// The settings file changes are saved to; None keeps them in memory.
    file: Option<PathBuf>,
}

impl Prefs {
    /// The saved preferences (defaults for anything missing or unreadable).
    pub fn load() -> Prefs {
        let file = dir().map(|d| d.join("settings.conf"));
        Prefs { file: file.clone(), ..Prefs::parse(&file.as_deref().map(read).unwrap_or_default()) }
    }

    /// Preferences from a settings file's text, not tied to the file.
    pub fn parse(text: &str) -> Prefs {
        let saved = entries(text);
        let get = |k: &str| saved.get(k).map(String::as_str).filter(|v| !v.is_empty());
        Prefs {
            theme: get("theme").and_then(Appearance::from_key).unwrap_or(Appearance::System),
            source: get("source").map(str::to_string),
            channel: get("channel").map(str::to_string),
            group: get("group").map(str::to_string),
            tab: get("tab").map(str::to_string),
            output: get("output").map(str::to_string),
            volume: get("volume").and_then(|v| v.parse::<f32>().ok()).map_or(0.8, |v| v.clamp(0.0, 1.0)),
            muted: get("muted") == Some("true"),
            picture_delays: get("picture_delays")
                .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
                .and_then(|v| v.as_object().cloned())
                .map(|o| o.into_iter().filter_map(|(k, v)| Some((k, i32::try_from(v.as_i64()?).ok()?))).collect())
                .unwrap_or_default(),
            pin: get("pin").map(str::to_string),
            file: None,
        }
    }

    /// Change preferences and save the ones that changed. Keys this version
    /// doesn't know stay in the file as they were.
    pub fn update(&mut self, change: impl FnOnce(&mut Prefs)) {
        let before = self.values();
        change(self);
        let changed: Vec<_> = self.values().into_iter().filter(|v| !before.contains(v)).collect();
        if changed.is_empty() {
            return;
        }
        let Some(p) = &self.file else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(p, merge(&read(p), &changed)) {
            log_warn!("could not save settings to {}: {e}", p.display());
        }
    }

    fn values(&self) -> [(&'static str, String); 10] {
        let delays: serde_json::Map<String, serde_json::Value> =
            self.picture_delays.iter().map(|(k, v)| (k.clone(), (*v).into())).collect();
        [
            ("picture_delays", if delays.is_empty() { String::new() } else { serde_json::Value::Object(delays).to_string() }),
            ("group", self.group.clone().unwrap_or_default()),
            ("tab", self.tab.clone().unwrap_or_default()),
            ("theme", self.theme.key().into()),
            ("source", self.source.clone().unwrap_or_default()),
            ("channel", self.channel.clone().unwrap_or_default()),
            ("output", self.output.clone().unwrap_or_default()),
            ("volume", format!("{:.3}", self.volume)),
            ("muted", if self.muted { "true" } else { "false" }.into()),
            ("pin", self.pin.clone().unwrap_or_default()),
        ]
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn entries(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}

fn merge(text: &str, changes: &[(&str, String)]) -> String {
    let mut all = entries(text);
    for (k, v) in changes {
        all.insert(k.to_string(), v.clone());
    }
    all.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVED: &str = "channel=42\ngroup=Sports\nmuted=true\noutput=Kitchen\npicture_delays={\"Kitchen\":250}\npin=ab12\nsource=a1b2\ntab=movies\ntheme=light\nvolume=0.250\n";

    #[test]
    fn reads_a_saved_file() {
        let p = Prefs::parse(SAVED);
        assert_eq!(p.theme, Appearance::Light);
        assert_eq!((p.source.as_deref(), p.channel.as_deref(), p.output.as_deref()), (Some("a1b2"), Some("42"), Some("Kitchen")));
        assert_eq!((p.volume, p.muted), (0.25, true));
    }

    #[test]
    fn missing_or_odd_values_get_defaults() {
        let p = Prefs::parse("theme=blue\nvolume=7\noutput=\n");
        assert_eq!(p.theme, Appearance::System);
        assert_eq!((p.output, p.volume, p.muted), (None, 1.0, false));
        assert!(p.picture_delays.is_empty());
    }

    #[test]
    fn keeps_picture_delays_per_output() {
        let mut p = Prefs::parse("");
        p.picture_delays.insert("Living room".into(), 350);
        p.picture_delays.insert("This computer".into(), -50);
        let text = merge("", &p.values());
        let back = Prefs::parse(&text);
        assert_eq!(back.picture_delays, p.picture_delays);
    }

    #[test]
    fn round_trips_through_the_file() {
        let text = merge("", &Prefs::parse(SAVED).values());
        assert_eq!(text, SAVED);
    }
}
