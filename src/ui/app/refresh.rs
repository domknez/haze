//! Keeping the channels and the guide fresh: loaded again in the background every few
//! hours, or when the viewer asks, and swapped in when they arrive, so the list never
//! empties and what plays goes on.

use super::App;
use crate::iptv::model::{Catalog, Guide};
use crate::library::{self, Source};
use crate::ui::load::Load;
use eframe::egui;
use std::time::{Duration, SystemTime};

/// How old the channels and the guide get before they're loaded again.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// When loading the channels failed (offline, just woken up), how soon to try again.
const RETRY: Duration = Duration::from_secs(15 * 60);
/// How often to look whether a refresh is due, at most: the UI may otherwise sleep
/// for hours with nothing on screen moving.
const LOOK: Duration = Duration::from_secs(60);

/// The refresh of the source in use. Times are wall-clock: the monotonic clock
/// stands still while macOS sleeps, and a refresh is due after a night's sleep.
#[derive(Default)]
pub struct Refresh {
    /// When the channels were last loaded.
    pub loaded_at: Option<SystemTime>,
    /// When to load them again; None before they first loaded.
    next_at: Option<SystemTime>,
    catalog: Load<Catalog>,
    guide: Load<Guide>,
    /// The viewer asked for it: say how it went.
    asked: bool,
}

impl Refresh {
    pub fn running(&self) -> bool {
        self.catalog.is_loading() || self.guide.is_loading()
    }

    /// The channels just loaded at `now`.
    fn loaded(&mut self, now: SystemTime) {
        self.loaded_at = Some(now);
        self.next_at = Some(now + EVERY);
    }
}

/// Whether a refresh set for `next` is due at `now`. A `next` further off than a
/// whole period means the clock was turned back: due then too.
fn due(next: Option<SystemTime>, now: SystemTime) -> bool {
    next.is_some_and(|next| next <= now || next.duration_since(now).is_ok_and(|left| left > EVERY))
}

/// How long ago, in words: `just now`, `5 min ago`, `3 h ago`, `2 days ago`.
pub fn ago(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0 => "just now".into(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        1440..2880 => "a day ago".into(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

impl App {
    /// Load the channels of `source` in the background. A playlist link that turns
    /// out to be a provider login is passed on to be saved as one.
    pub(super) fn load_catalog(&self, ctx: &egui::Context, source: Source) -> Load<Catalog> {
        let upgrades = self.upgrades.0.clone();
        Load::start(ctx, move || {
            let (catalog, upgrade) = library::fetch_upgrading(&source)?;
            if let Some(u) = upgrade {
                let _ = upgrades.send(u);
            }
            Ok(catalog)
        })
    }

    /// The source's channels came in for the first time.
    pub(super) fn first_loaded(&mut self) {
        self.refresh.loaded(SystemTime::now());
    }

    /// Start a refresh when one is due; otherwise make sure the UI looks again.
    pub(super) fn refresh_when_due(&mut self, ctx: &egui::Context) {
        if self.refresh.running() {
            return;
        }
        if due(self.refresh.next_at, SystemTime::now()) {
            self.start_refresh(ctx, false);
        } else if self.refresh.next_at.is_some() {
            ctx.request_repaint_after(LOOK);
        }
    }

    /// Load the channels of the source in use again, then its guide.
    pub(super) fn start_refresh(&mut self, ctx: &egui::Context, asked: bool) {
        let Some(source) = self.source().cloned() else { return };
        if self.refresh.running() || self.catalog.is_loading() {
            return;
        }
        if self.catalog.ready().is_none() {
            // Nothing to keep showing (the first load failed): load it as at the start.
            self.catalog = self.load_catalog(ctx, source);
            return;
        }
        log_info!("{}: refreshing the channels and the guide", source.name);
        self.refresh.asked = asked;
        self.refresh.catalog = self.load_catalog(ctx, source);
    }

    /// Take what the refresh brought.
    pub(super) fn poll_refresh(&mut self, ctx: &egui::Context) {
        if self.refresh.catalog.poll() {
            match std::mem::take(&mut self.refresh.catalog) {
                Load::Ready(catalog) => self.swap_catalog(ctx, catalog),
                Load::Failed(e) => {
                    self.refresh.next_at = Some(SystemTime::now() + RETRY);
                    self.refresh_failed("channels", &e);
                }
                _ => {}
            }
        }
        if self.refresh.guide.poll() {
            match std::mem::take(&mut self.refresh.guide) {
                Load::Ready(guide) => {
                    self.guide = Load::Ready(guide);
                    self.refresh_done();
                }
                // Not tried again soon: a guide that fails often fails for good (a
                // provider refusing it), and the channels would be loaded with it.
                Load::Failed(e) => self.refresh_failed("guide", &e),
                _ => {}
            }
        }
    }

    /// Use the channels just loaded in place of the ones shown, keeping what the
    /// viewer has open, and load the guide for them.
    fn swap_catalog(&mut self, ctx: &egui::Context, catalog: Catalog) {
        // The open series by its key: its place in the list may have moved.
        let open = self.browse.open_series.and_then(|i| Some(self.catalog.ready()?.series.get(i)?.key.clone()));
        match open.and_then(|key| catalog.series.iter().position(|s| s.key == key)) {
            Some(i) => self.browse.open_series = Some(i),
            None if self.browse.open_series.is_some() => self.browse.close_series(),
            None => {}
        }
        self.browse.count_groups(&catalog);
        self.browse.relist();
        let guide = catalog.epg_url.clone().map(|url| (url, catalog.channels.clone()));
        self.catalog = Load::Ready(catalog);
        self.refresh.loaded(SystemTime::now());
        match guide {
            Some((url, channels)) => self.refresh.guide = Load::start(ctx, move || library::fetch_guide(&url, &channels)),
            None => {
                self.guide = Load::Idle;
                self.refresh_done();
            }
        }
    }

    fn refresh_done(&mut self) {
        if std::mem::take(&mut self.refresh.asked) {
            self.show_toast("Channels and guide are up to date");
        }
    }

    fn refresh_failed(&mut self, what: &str, e: &str) {
        log_warn!("refresh: the {what} couldn't be loaded: {e}");
        if std::mem::take(&mut self.refresh.asked) {
            self.show_toast(&format!("The {what} couldn't be refreshed: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_once_the_time_comes() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(!due(None, now));
        assert!(!due(Some(now + Duration::from_secs(60)), now));
        assert!(due(Some(now), now));
        assert!(due(Some(now - Duration::from_secs(60)), now));
        // The clock was turned back a day: don't wait a day longer.
        assert!(due(Some(now + EVERY + Duration::from_secs(86_400)), now));
    }

    #[test]
    fn says_how_long_ago() {
        let mins = |m: u64| ago(Duration::from_secs(m * 60));
        assert_eq!(ago(Duration::from_secs(20)), "just now");
        assert_eq!(mins(5), "5 min ago");
        assert_eq!(mins(150), "2 h ago");
        assert_eq!(mins(1500), "a day ago");
        assert_eq!(mins(3 * 1440 + 5), "3 days ago");
    }
}
