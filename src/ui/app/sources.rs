//! Sources and what they offer: switching between them, adding, editing and removing
//! them, where their groups go, hidden groups, and favourites.

use super::{Act, App, Playing, Tab};
use crate::iptv::model::{Catalog, Shelf};
use crate::library::{self, Checked};
use crate::ui::load::Load;
use crate::ui::views::pin::Purpose as PinPurpose;
use eframe::egui;

impl App {
    /// Start using source `i`; `catalog` when it was just loaded (a new source).
    pub(super) fn switch_source(&mut self, ctx: &egui::Context, i: usize, catalog: Option<Catalog>) {
        let Some(source) = self.sources.get(i).cloned() else { return };
        self.active = Some(i);
        self.prefs.update(|p| p.source = Some(source.id.clone()));
        self.browse.reset();
        self.browse.hidden = source.hidden.clone();
        self.guide = Load::Idle;
        self.refresh = Default::default();
        match catalog {
            Some(c) => {
                self.catalog = Load::Ready(c);
                self.catalog_arrived(ctx);
            }
            None => self.catalog = self.load_catalog(ctx, source),
        }
    }

    /// The catalog came in: count its groups, load its guide, resume the last channel.
    fn catalog_arrived(&mut self, ctx: &egui::Context) {
        if self.catalog.ready().is_none() {
            return;
        }
        self.first_loaded();
        let Some(c) = self.catalog.ready() else { return };
        self.browse.catalog_arrived(c, self.prefs.group.as_ref());
        if let Some(url) = c.epg_url.clone() {
            let channels = c.channels.clone();
            self.guide = Load::start(ctx, move || library::fetch_guide(&url, &channels));
        }
        if let Some(key) = self.resume.take()
            && self.channel(&key).is_some()
        {
            self.play(ctx, Playing::Live { key });
        }
    }

    /// Use source `i` from the source menu.
    pub(super) fn pick_source(&mut self, ctx: &egui::Context, i: usize) {
        self.panels.popup = None;
        if self.active != Some(i) {
            self.stop();
            self.switch_source(ctx, i, None);
        }
    }

    /// Take what the background loads brought.
    pub(super) fn poll_loads(&mut self, ctx: &egui::Context) {
        // Before the catalog it came with: the source is a login by then.
        while let Ok((source, password)) = self.upgrades.1.try_recv() {
            let Some(i) = self.sources.iter().position(|s| s.id == source.id) else { continue };
            log_info!("{}: signing in to the panel's API in place of its playlist link", source.name);
            crate::credentials::save(&source.id, &password);
            // Channels are known by their stream id now, not their link.
            self.marks.rekey(&source.id, crate::iptv::xtream::stream_id);
            self.reminders.rekey(&source.id, crate::iptv::xtream::stream_id);
            if self.source().is_some_and(|s| s.id == source.id)
                && let Some(key) = self.resume.as_deref().and_then(crate::iptv::xtream::stream_id)
            {
                self.resume = Some(key);
            }
            let channel = self.prefs.channel.as_deref().and_then(crate::iptv::xtream::stream_id);
            if channel.is_some() && self.prefs.source.as_ref() == Some(&source.id) {
                self.prefs.update(|p| p.channel = channel);
            }
            self.show_toast(&format!("{} now signs in to your provider: posters, plots and seasons", source.name));
            self.sources[i] = source;
            library::save_sources(&self.sources);
        }
        if self.catalog.poll() {
            if let Load::Failed(e) = &self.catalog {
                log_warn!("catalog: {e}");
            }
            self.browse.relist();
            self.catalog_arrived(ctx);
        }
        if self.browse.episodes.poll() {
            if let Load::Failed(e) = &self.browse.episodes {
                log_warn!("episodes: {e}");
            }
            self.browse.relist();
            self.carry_on(ctx);
        }
        if self.guide.poll()
            && let Load::Failed(e) = &self.guide
        {
            log_warn!("guide: {e}");
            self.show_toast("The guide couldn't be loaded");
        }
        if let Some(form) = &mut self.form
            && let Some(checked) = form.poll()
        {
            self.form = None;
            self.apply(ctx, Act::SourceSaved(Box::new(checked)));
        }
    }

    /// A source checked and ready: keep it (in place of the one edited) and use it.
    pub(super) fn save_source(&mut self, ctx: &egui::Context, checked: Checked) {
        let Checked { source, catalog, secret } = checked;
        if let Some(secret) = secret {
            crate::credentials::save(&source.id, &secret);
        }
        let i = match self.sources.iter().position(|s| s.id == source.id) {
            Some(i) => {
                self.sources[i] = source.clone();
                i
            }
            None => {
                self.sources.push(source.clone());
                self.sources.len() - 1
            }
        };
        library::save_sources(&self.sources);
        self.stop();
        self.switch_source(ctx, i, Some(catalog));
        self.show_toast(&format!("{} is ready", source.name));
    }

    pub(super) fn remove_source(&mut self, ctx: &egui::Context, i: usize) {
        if i >= self.sources.len() {
            return;
        }
        let removed = self.sources.remove(i);
        crate::credentials::clear(&removed.id);
        self.marks.forget_source(&removed.id);
        self.progress.forget_source(&removed.id);
        self.reminders.forget_source(&removed.id);
        library::save_sources(&self.sources);
        if self.active == Some(i) {
            self.stop();
            self.catalog = Load::Idle;
            self.guide = Load::Idle;
            self.active = None;
            if !self.sources.is_empty() {
                self.switch_source(ctx, 0, None);
            }
        } else if let Some(a) = self.active
            && a > i
        {
            self.active = Some(a - 1);
        }
        self.show_toast(&format!("Removed {}", removed.name));
    }

    /// Put a playlist group with the channels, movies or series (None: where it looks
    /// like it belongs), and read the source again; what plays goes on.
    pub(super) fn shelve(&mut self, ctx: &egui::Context, group: String, shelf: Option<Shelf>) {
        let Some(i) = self.active else { return };
        let source = &mut self.sources[i];
        match shelf {
            Some(s) => source.shelves.insert(group.clone(), s),
            None => source.shelves.remove(&group),
        };
        library::save_sources(&self.sources);
        self.switch_source(ctx, i, None);
        let place = match shelf {
            Some(Shelf::Channels) => "with the channels",
            Some(Shelf::Movies) => "under Movies",
            Some(Shelf::Series) => "under Series",
            None => "where it looks like it belongs",
        };
        self.show_toast(&format!("{group} now shows {place}"));
    }

    pub(super) fn hide(&mut self, group: String, hide: bool) {
        let Some(source) = self.active.and_then(|i| self.sources.get_mut(i)) else { return };
        if hide {
            source.hidden.insert(group.clone());
        } else {
            source.hidden.remove(&group);
        }
        self.browse.hidden = source.hidden.clone();
        library::save_sources(&self.sources);
        if let Some(c) = self.catalog.ready() {
            self.browse.count_groups(c);
        }
        if hide && !self.browse.show_hidden {
            self.show_toast(&format!("{group} is hidden; Settings › Show hidden groups brings it back"));
        }
    }

    pub(super) fn show_hidden(&mut self, show: bool) {
        self.browse.show_hidden = show;
        if let Some(c) = self.catalog.ready() {
            self.browse.count_groups(c);
        }
    }

    /// The PIN dialog's answer.
    pub(super) fn pin_done(&mut self, purpose: PinPurpose, hash: Option<String>) {
        match purpose {
            PinPurpose::Unlock => self.show_hidden(true),
            PinPurpose::Set => {
                self.prefs.update(|p| p.pin = hash);
                self.show_hidden(false);
                self.show_toast("Hidden groups are locked with the PIN");
            }
            PinPurpose::Remove => {
                self.prefs.update(|p| p.pin = None);
                self.show_toast("Hidden groups show without a PIN now");
            }
        }
    }

    pub(super) fn pick_group(&mut self, group: Option<String>) {
        if !matches!(self.browse.tab, Tab::Movies | Tab::Series) {
            self.prefs.update(|p| p.group = group.clone());
        }
        self.browse.pick_group(group);
    }

    pub(super) fn toggle_favorite(&mut self, key: &str) {
        let source = self.source_id();
        self.marks.toggle_favorite(&source, key);
    }

    pub(super) fn move_favorite(&mut self, key: &str, target: &str, after: bool) {
        let source = self.source_id();
        self.marks.move_favorite(&source, key, target, after);
    }
}
