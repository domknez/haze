//! The app's state and what happens each frame. What the viewer's actions do is in
//! [`act`]; playing in [`playback`]; sources, groups and favourites in [`sources`];
//! keys, the pointer and remote controls in [`input`]; the mini player in [`mini`];
//! keeping the channels and the guide fresh in [`refresh`]; the sleep timer in
//! [`sleep`]; several channels at once in [`multi`]; reminders from the guide in
//! [`remind`].

mod act;
mod input;
mod mini;
mod multi;
mod playback;
mod refresh;
mod remind;
mod sleep;
mod sources;

pub use super::browse::Tab;
pub use super::session::Playing;
pub use act::{Act, Popup};
pub use multi::slots as multi_slots;
pub use refresh::ago;
pub use sleep::{Timer, left as sleep_left};

use super::browse::Browse;
use super::load::Load;
use super::logos::Logos;
use super::session::Session;
use super::style::{self, colors};
use super::video::Video;
use super::views::add_source::SourceForm;
use super::views::pin::{Outcome as PinOutcome, PinDialog};
use crate::iptv::model::{Catalog, Channel, Guide, Programme};
use crate::iptv::time;
use crate::library::{self, Marks, Progress, Reminder, Reminders, Source};
use crate::media::Status;
use crate::output::Output;
use crate::output::airplay::Discovery;
use crate::settings::{Appearance, Prefs};
use eframe::egui;
use input::{Pointer, Remotes};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// What shows over the picture, and where.
pub struct Panels {
    /// The channel list.
    pub rail_open: bool,
    /// The column of groups beside the channels.
    pub groups_open: bool,
    /// The guide covers the picture.
    pub guide_open: bool,
    /// The guide should scroll to now and the channel playing.
    pub guide_jump: bool,
    /// The guide shows the favourites rather than the group picked.
    pub guide_favorites: bool,
    /// The settings page, open at a section.
    pub settings: Option<crate::ui::views::SettingsPage>,
    pub popup: Option<Popup>,
    /// Where the buttons that open menus are, for placing the menus.
    pub anchors: std::cell::RefCell<std::collections::HashMap<Popup, egui::Rect>>,
    /// The search field should take the keyboard.
    pub focus_search: bool,
    /// The panels showed last frame (subtitles keep clear of them).
    pub controls_shown: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Panels {
            rail_open: true,
            groups_open: true,
            guide_open: false,
            guide_jump: false,
            guide_favorites: false,
            settings: None,
            popup: None,
            anchors: Default::default(),
            focus_search: false,
            controls_shown: true,
        }
    }
}

pub struct App {
    pub prefs: Prefs,
    pub sources: Vec<Source>,
    pub active: Option<usize>,
    pub catalog: Load<Catalog>,
    pub guide: Load<Guide>,
    pub marks: Marks,
    pub progress: Progress,
    /// When the progress of what plays was kept last.
    progress_at: Instant,
    pub session: Session,
    pub video: Video,
    pub logos: Logos,
    pub discovery: Option<Discovery>,
    pub output: Output,
    pub browse: Browse,
    pub panels: Panels,
    pub form: Option<SourceForm>,
    pub pin: Option<PinDialog>,
    pub toast: Option<(String, Instant)>,
    pointer: Pointer,
    /// Resume the last channel once the catalog is in.
    resume: Option<String>,
    /// The theme the style was last set up for.
    applied_dark: Option<bool>,
    remotes: Remotes,
    /// Playlist sources that turned out to be Xtream logins while loading.
    upgrades: (Sender<library::Upgrade>, Receiver<library::Upgrade>),
    pub updates: super::updates::Updates,
    /// Held while a picture plays, so the display doesn't dim or sleep under it.
    screen: Option<crate::awake::Awake>,
    /// The picture alone in a small window above the others.
    mini: mini::Mini,
    /// Loading the channels and the guide again, every few hours.
    pub refresh: refresh::Refresh,
    pub guide_search: crate::ui::views::GuideSearch,
    pub multi: multi::Multi,
    pub sleep: sleep::Sleep,
    /// Programmes to be reminded of.
    pub reminders: Reminders,
    /// Reminders whose time came, shown until answered or the programme ends.
    pub reminding: Vec<Reminder>,
    /// The section of the settings shown last.
    settings_page: crate::ui::views::SettingsPage,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> App {
        let ctx = &cc.egui_ctx;
        style::install_fonts(ctx);
        let prefs = Prefs::load();
        let sources = library::load_sources();
        let active = prefs.source.as_ref().and_then(|id| sources.iter().position(|s| &s.id == id)).or((!sources.is_empty()).then_some(0));
        let repaint = ctx.clone();
        let discovery =
            Discovery::start(move || repaint.request_repaint()).inspect_err(|e| log_warn!("AirPlay discovery unavailable: {e}")).ok();
        let mut app = App {
            updates: super::updates::Updates::new(prefs.auto_update),
            screen: None,
            mini: mini::Mini::Off,
            refresh: refresh::Refresh::default(),
            guide_search: Default::default(),
            multi: multi::Multi::default(),
            sleep: sleep::Sleep::default(),
            reminders: Reminders::load(time::now()),
            reminding: Vec::new(),
            settings_page: Default::default(),
            resume: prefs.channel.clone(),
            browse: Browse::new(prefs.tab.as_deref().and_then(Tab::from_key).unwrap_or(Tab::Live)),
            prefs,
            sources,
            active: None,
            catalog: Load::Idle,
            guide: Load::Idle,
            marks: Marks::load(),
            progress: Progress::load(),
            progress_at: Instant::now(),
            session: Session::default(),
            video: Video::new(),
            logos: Logos::new(ctx),
            discovery,
            output: Output::Local,
            panels: Panels::default(),
            form: None,
            pin: None,
            toast: None,
            pointer: Pointer::default(),
            applied_dark: None,
            remotes: Remotes::start(cc),
            upgrades: std::sync::mpsc::channel(),
        };
        if let Some(i) = active {
            app.switch_source(ctx, i, None);
        }
        app
    }

    pub fn source(&self) -> Option<&Source> {
        self.active.and_then(|i| self.sources.get(i))
    }

    pub fn source_id(&self) -> String {
        self.source().map(|s| s.id.clone()).unwrap_or_default()
    }

    pub fn channel(&self, key: &str) -> Option<&Channel> {
        self.catalog.ready()?.channels.iter().find(|c| c.key == key)
    }

    pub fn playing_channel(&self) -> Option<&Channel> {
        match &self.session.playing {
            Some(Playing::Live { key } | Playing::Catchup { key, .. }) => self.channel(key),
            _ => None,
        }
    }

    pub fn now_on(&self, channel: &Channel) -> Option<&Programme> {
        self.guide.ready().and_then(|g| g.now_on(channel, time::now())).map(|(_, p)| p)
    }

    pub fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    /// What the rail lists, worked out again only when something it depends on changed.
    pub fn refresh_list(&mut self) {
        let source = self.source_id();
        self.browse.refresh(self.catalog.ready(), &self.marks, &self.progress, &source);
    }

    fn sync_appearance(&mut self, ctx: &egui::Context) {
        let dark = match self.prefs.theme {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark),
        };
        if self.applied_dark != Some(dark) {
            style::apply(ctx, dark);
            self.applied_dark = Some(dark);
        }
    }

    /// Everything that happened since the last frame.
    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_loads(ctx);
        self.poll_refresh(ctx);
        self.refresh_when_due(ctx);
        self.sleep_when_due(ctx);
        self.remind_when_due(ctx);
        for act in self.remotes.presses(self.prefs.volume) {
            self.apply(ctx, act);
        }
        self.announce();
        self.use_saved_output();
        self.poll_player(ctx);
        self.poll_tiles(ctx);
        self.keep_screen_on();
        self.updates.poll(ctx);
    }

    /// The source form and the PIN dialog, when one is open.
    fn dialogs(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        if let Some(form) = &mut self.form
            && form.show(ctx, screen) == Some(false)
        {
            self.form = None;
        }
        if let Some(dialog) = &mut self.pin {
            match dialog.show(ctx, screen, self.prefs.pin.as_deref()) {
                PinOutcome::Open => {}
                PinOutcome::Cancelled => self.pin = None,
                PinOutcome::Done(hash) => {
                    let purpose = dialog.purpose;
                    self.pin = None;
                    self.pin_done(purpose, hash);
                }
            }
        }
    }
}

/// `+150 ms`, `-50 ms`, `none`.
pub fn delay_text(ms: i32) -> String {
    if ms == 0 { "none".into() } else { format!("{ms:+} ms") }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.keep_progress();
        self.updates.apply_on_exit();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        log_trace!("frame {}: {:?}", ctx.cumulative_pass_nr(), ctx.repaint_causes());
        self.sync_appearance(&ctx);
        self.logos.begin_frame();
        self.poll(&ctx);
        self.settle_mini(&ctx);
        self.pointer.track(&ctx);
        let mut acts = Vec::new();
        self.shortcuts(&ctx, &mut acts);
        self.dropped_files(&ctx);
        self.refresh_list();

        let screen = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(if self.video.has_picture() { egui::Color32::BLACK } else { colors().backdrop }))
            .show(ui, |ui| {
                if self.sources.is_empty() {
                    self.welcome_glow(ui, ui.max_rect());
                }
                self.stage(ui, &mut acts)
            })
            .response
            .rect;

        if self.sources.is_empty() && self.form.is_none() {
            self.welcome(&ctx, screen, &mut acts);
        } else if self.mini_on() {
            self.panels.controls_shown = false;
            if self.controls_visible(&ctx) {
                self.mini_controls(&ctx, screen, &mut acts);
            } else {
                ctx.set_cursor_icon(egui::CursorIcon::None);
            }
        } else if self.controls_visible(&ctx) {
            self.panels.controls_shown = true;
            self.top_bar(&ctx, screen, &mut acts);
            if self.panels.guide_open {
                self.guide_view(&ctx, screen, &mut acts);
            } else {
                if self.panels.rail_open {
                    self.rail(&ctx, screen, &mut acts);
                }
                self.bottom(&ctx, screen, &mut acts);
            }
            self.popups(&ctx, screen, &mut acts);
            if let Some(page) = self.panels.settings {
                self.settings_view(&ctx, screen, page, &mut acts);
            }
        } else {
            self.panels.controls_shown = false;
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        self.reminder_view(&ctx, screen, &mut acts);
        self.toast_view(&ctx, screen);
        self.dialogs(&ctx, screen);
        for act in acts {
            self.apply(&ctx, act);
        }
        // The programme line and times move on even when no new frames come (paused,
        // or sound only).
        if self.session.player.as_ref().is_some_and(|p| p.paused() || p.status() != Status::Playing || p.info().video_codec.is_none()) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}
