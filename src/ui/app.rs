//! The app's state, what happens each frame, and what the viewer's actions do.

use super::load::Load;
use super::logos::Logos;
use super::style::{self, colors, metrics};
use super::video::Video;
use super::views::add_source::SourceForm;
use crate::iptv::model::{Catalog, Channel, Guide, Programme};
use crate::iptv::time;
use crate::library::{self, Marks, Source};
use crate::media::{Player, Request, Status};
use crate::output::Output;
use crate::output::airplay::Discovery;
use crate::settings::{Appearance, Prefs};
use eframe::egui::{self, Key, Modifiers, ViewportCommand};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Live,
    Favorites,
    Recent,
    Movies,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Live, Tab::Favorites, Tab::Recent, Tab::Movies];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Live => "Live",
            Tab::Favorites => "Favorites",
            Tab::Recent => "Recent",
            Tab::Movies => "Movies",
        }
    }
}

/// What's playing.
#[derive(Clone, Debug, PartialEq)]
pub enum Playing {
    Live {
        key: String,
    },
    /// A programme from a channel's archive.
    Catchup {
        key: String,
        programme: Programme,
    },
    Movie {
        key: String,
    },
}

impl Playing {
    pub fn key(&self) -> &str {
        match self {
            Playing::Live { key } | Playing::Catchup { key, .. } | Playing::Movie { key } => key,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Popup {
    Outputs,
    Sources,
    Groups,
    AudioTracks,
    Settings,
}

/// What the viewer did this frame; applied after drawing.
pub enum Act {
    PlayChannel(String),
    PlayMovie(String),
    PlayCatchup(String, Programme),
    GoLive,
    TogglePause,
    /// Step through the list on screen: -1 previous, +1 next.
    Step(i32),
    ToggleFavorite(String),
    Tab(Tab),
    Group(Option<String>),
    SwitchSource(usize),
    Popup(Option<Popup>),
    Output(Output),
    Volume(f32),
    ToggleMute,
    ToggleFullscreen,
    ToggleRail,
    Theme(Appearance),
    AddSource,
    EditSource(usize),
    RemoveSource(usize),
    /// A source checked and ready to save, with what it offers.
    SourceSaved(Source, Catalog),
    AudioTrack(usize),
    Seek(f64),
    Toast(String),
    Retry,
}

pub struct App {
    pub prefs: Prefs,
    pub sources: Vec<Source>,
    pub active: Option<usize>,
    pub catalog: Load<Catalog>,
    pub guide: Load<Guide>,
    pub marks: Marks,
    pub player: Option<Player>,
    pub playing: Option<Playing>,
    pub video: Video,
    pub logos: Logos,
    pub discovery: Option<Discovery>,
    pub output: Output,
    pub tab: Tab,
    pub group: Option<String>,
    pub movie_group: Option<String>,
    pub search: String,
    pub rail_open: bool,
    pub popup: Option<Popup>,
    pub form: Option<SourceForm>,
    pub toast: Option<(String, Instant)>,
    /// Indices of what the rail lists, and what they were worked out for.
    pub list: Vec<usize>,
    list_key: Option<(Tab, Option<String>, String, usize, u64)>,
    /// Bumped when favourites or recents change.
    pub marks_serial: u64,
    last_motion: Instant,
    last_pointer: Option<egui::Pos2>,
    /// Reconnect attempts for a live channel that dropped.
    retries: u32,
    retry_at: Option<Instant>,
    /// Stream time of the first frame of a recording, so positions start at 0.
    pub start_pts: Option<f64>,
    /// The search field should take the keyboard.
    pub focus_search: bool,
    /// Resume the last channel once the catalog is in.
    resume: Option<String>,
    /// Where the buttons that open menus are, for placing the menus.
    pub anchors: std::cell::RefCell<std::collections::HashMap<Popup, egui::Rect>>,
    /// The theme the style was last set up for.
    applied_dark: Option<bool>,
    /// Presses on the AirPlay speaker's buttons.
    remote: Option<(crate::output::airplay::dacp::Server, std::sync::mpsc::Receiver<crate::output::airplay::dacp::Remote>)>,
    now_playing: Option<super::now_playing::NowPlaying>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> App {
        let ctx = &cc.egui_ctx;
        style::install_fonts(ctx);
        let prefs = Prefs::load();
        let sources = library::load_sources();
        let active = prefs.source.as_ref().and_then(|id| sources.iter().position(|s| &s.id == id)).or(if sources.is_empty() {
            None
        } else {
            Some(0)
        });
        let repaint = ctx.clone();
        let discovery =
            Discovery::start(move || repaint.request_repaint()).inspect_err(|e| log_warn!("AirPlay discovery unavailable: {e}")).ok();
        let (remote_tx, remote_rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        let remote = crate::output::airplay::dacp::start(move |r| {
            let _ = remote_tx.send(r);
            repaint.request_repaint();
        })
        .inspect_err(|e| log_warn!("speaker remote control unavailable: {e}"))
        .ok()
        .map(|server| (server, remote_rx));
        let mut app = App {
            resume: prefs.channel.clone(),
            prefs,
            sources,
            active: None,
            catalog: Load::Idle,
            guide: Load::Idle,
            marks: Marks::load(),
            player: None,
            playing: None,
            video: Video::new(),
            logos: Logos::new(ctx),
            discovery,
            output: Output::Local,
            tab: Tab::Live,
            group: None,
            movie_group: None,
            search: String::new(),
            rail_open: true,
            popup: None,
            form: None,
            toast: None,
            list: Vec::new(),
            list_key: None,
            marks_serial: 0,
            last_motion: Instant::now(),
            last_pointer: None,
            retries: 0,
            retry_at: None,
            start_pts: None,
            focus_search: false,
            anchors: Default::default(),
            applied_dark: None,
            remote,
            now_playing: super::now_playing::NowPlaying::start(cc),
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
        match &self.playing {
            Some(Playing::Live { key } | Playing::Catchup { key, .. }) => self.channel(key),
            _ => None,
        }
    }

    pub fn now_on(&self, channel: &Channel) -> Option<&Programme> {
        self.guide.ready().and_then(|g| g.now_on(channel, time::now())).map(|(_, p)| p)
    }

    /// Start using source `i`; `catalog` when it was just loaded (a new source).
    fn switch_source(&mut self, ctx: &egui::Context, i: usize, catalog: Option<Catalog>) {
        let Some(source) = self.sources.get(i).cloned() else { return };
        self.active = Some(i);
        self.prefs.update(|p| p.source = Some(source.id.clone()));
        self.group = None;
        self.movie_group = None;
        self.list_key = None;
        self.guide = Load::Idle;
        match catalog {
            Some(c) => {
                self.catalog = Load::Ready(c);
                self.catalog_arrived(ctx);
            }
            None => self.catalog = Load::start(ctx, move || library::fetch(&source)),
        }
    }

    /// The catalog came in: load its guide, resume the last channel.
    fn catalog_arrived(&mut self, ctx: &egui::Context) {
        let Some(c) = self.catalog.ready() else { return };
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

    fn request_for(&self, playing: &Playing) -> Option<Request> {
        let catalog = self.catalog.ready()?;
        match playing {
            Playing::Live { key } => {
                let c = catalog.channels.iter().find(|c| &c.key == key)?;
                Some(Request { url: c.url.clone(), user_agent: c.user_agent.clone(), referrer: c.referrer.clone(), live: true })
            }
            Playing::Catchup { key, programme } => {
                let c = catalog.channels.iter().find(|c| &c.key == key)?;
                let login = self.source().and_then(Source::login);
                let offset = catalog.account.as_ref().map_or(0, |a| a.server_offset);
                let url = library::catchup_url(c, programme, login.as_ref(), offset)?;
                Some(Request { url, user_agent: c.user_agent.clone(), referrer: c.referrer.clone(), live: false })
            }
            Playing::Movie { key } => {
                let m = catalog.movies.iter().find(|m| &m.key == key)?;
                Some(Request { url: m.url.clone(), user_agent: None, referrer: None, live: false })
            }
        }
    }

    pub fn play(&mut self, ctx: &egui::Context, playing: Playing) {
        let Some(request) = self.request_for(&playing) else {
            self.show_toast("That can't be played");
            return;
        };
        log_info!("playing {}", request.url.split('?').next().unwrap_or(""));
        // The old stream stops first: providers often allow one connection.
        self.player = None;
        self.video.clear();
        self.start_pts = None;
        let repaint = ctx.clone();
        // A zero-delay request draws twice (egui lets layouts settle); a frame of video
        // needs one pass.
        let player = Player::open(request, self.output.clone(), self.effective_volume(), move || {
            repaint.request_repaint_after(Duration::from_millis(1))
        });
        self.player = Some(player);
        if let Playing::Live { key } = &playing {
            let source = self.source_id();
            self.marks.watched(&source, key);
            self.marks_serial += 1;
            self.prefs.update(|p| p.channel = Some(key.clone()));
        }
        self.playing = Some(playing);
        self.retries = 0;
        self.retry_at = None;
    }

    fn effective_volume(&self) -> f32 {
        if self.prefs.muted { 0.0 } else { self.prefs.volume }
    }

    pub fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    /// What the rail lists, worked out again only when something it depends on changed.
    pub fn refresh_list(&mut self) {
        let Some(catalog) = self.catalog.ready() else {
            self.list.clear();
            return;
        };
        let group = if self.tab == Tab::Movies { self.movie_group.clone() } else { self.group.clone() };
        let key = (self.tab, group.clone(), self.search.clone(), catalog.channels.len() + catalog.movies.len(), self.marks_serial);
        if self.list_key.as_ref() == Some(&key) {
            return;
        }
        let needle = self.search.trim().to_lowercase();
        let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
        let source = self.source_id();
        let by_key = |k: &str| catalog.channels.iter().position(|c| c.key == k);
        self.list = match self.tab {
            Tab::Live => catalog
                .channels
                .iter()
                .enumerate()
                .filter(|(_, c)| group.as_ref().is_none_or(|g| &c.group == g) && matches(&c.name))
                .map(|(i, _)| i)
                .collect(),
            Tab::Favorites => self.marks.favorites(&source).filter_map(by_key).filter(|&i| matches(&catalog.channels[i].name)).collect(),
            Tab::Recent => self.marks.recent(&source).filter_map(by_key).filter(|&i| matches(&catalog.channels[i].name)).collect(),
            Tab::Movies => catalog
                .movies
                .iter()
                .enumerate()
                .filter(|(_, m)| group.as_ref().is_none_or(|g| &m.group == g) && matches(&m.name))
                .map(|(i, _)| i)
                .collect(),
        };
        self.list_key = Some(key);
    }

    /// Whether the panels over the picture show: always, unless something plays and
    /// the pointer has been still for a while.
    fn controls_visible(&self, ctx: &egui::Context) -> bool {
        let playing = self.player.as_ref().is_some_and(|p| p.status() == Status::Playing && !p.paused());
        if !playing || self.popup.is_some() || self.form.is_some() || self.focus_search {
            return true;
        }
        let idle = self.last_motion.elapsed().as_secs_f32();
        if idle < metrics::IDLE_HIDE {
            ctx.request_repaint_after(Duration::from_secs_f32(metrics::IDLE_HIDE - idle + 0.05));
            return true;
        }
        false
    }

    fn track_pointer(&mut self, ctx: &egui::Context) {
        let (pos, any) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.any_down()
                    || !i.events.is_empty()
                        && i.events.iter().any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::MouseWheel { .. })),
            )
        });
        if pos != self.last_pointer || any {
            self.last_pointer = pos;
            self.last_motion = Instant::now();
        }
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

    fn poll(&mut self, ctx: &egui::Context) {
        if self.catalog.poll() {
            if let Load::Failed(e) = &self.catalog {
                log_warn!("catalog: {e}");
            }
            self.list_key = None;
            self.catalog_arrived(ctx);
        }
        if self.guide.poll()
            && let Load::Failed(e) = &self.guide
        {
            log_warn!("guide: {e}");
            self.show_toast("The guide couldn't be loaded");
        }
        if let Some(form) = &mut self.form
            && let Some((source, catalog)) = form.poll()
        {
            self.form = None;
            self.apply(ctx, Act::SourceSaved(source, catalog));
        }
        // The speaker's own buttons.
        let presses: Vec<_> = self.remote.as_ref().map(|(_, rx)| rx.try_iter().collect()).unwrap_or_default();
        for press in presses {
            use crate::output::airplay::dacp::Remote;
            let paused = self.player.as_ref().is_some_and(Player::paused);
            let act = match press {
                Remote::PlayPause => Act::TogglePause,
                Remote::Play if paused => Act::TogglePause,
                Remote::Pause if !paused => Act::TogglePause,
                Remote::Play | Remote::Pause => continue,
                Remote::Next => Act::Step(1),
                Remote::Prev => Act::Step(-1),
                Remote::Volume(v) => Act::Volume(v),
                Remote::VolumeStep(d) => Act::Volume(self.prefs.volume + d),
            };
            self.apply(ctx, act);
        }
        // Media keys and the system's Now Playing controls.
        let keys = self.now_playing.as_ref().map(|n| n.presses()).unwrap_or_default();
        for key in keys {
            use super::now_playing::Press;
            let paused = self.player.as_ref().is_some_and(Player::paused);
            let act = match key {
                Press::Toggle => Act::TogglePause,
                Press::Play if paused => Act::TogglePause,
                Press::Pause if !paused => Act::TogglePause,
                Press::Play | Press::Pause => continue,
                Press::Next => Act::Step(1),
                Press::Prev => Act::Step(-1),
                Press::Seek(d) => Act::Seek(d),
                Press::Volume(v) => Act::Volume(v),
            };
            self.apply(ctx, act);
        }
        self.announce();
        // Speakers come and go; the one picked last time is used once it shows up.
        if let (Some(name), Output::Local, Some(d)) = (&self.prefs.output, &self.output, &self.discovery)
            && let Some(device) = d.devices().into_iter().find(|d| &d.name == name)
        {
            self.output = Output::AirPlay(device);
            if let Some(p) = &self.player {
                p.set_output(self.output.clone());
            }
        }
        let Some(player) = &self.player else { return };
        let (serial, frame) = player.frame();
        let (notice, status) = (player.take_notice(), player.status());
        if self.start_pts.is_none() {
            self.start_pts = frame.as_ref().map(|f| f.pts);
        }
        self.video.set_frame(serial, frame);
        if let Some(notice) = notice {
            self.show_toast(&notice);
        }
        let live = matches!(self.playing, Some(Playing::Live { .. }));
        match status {
            Status::Failed(e) if live && self.retries < 5 => {
                let at = *self.retry_at.get_or_insert_with(|| Instant::now() + Duration::from_secs(1 + u64::from(self.retries)));
                if Instant::now() >= at {
                    log_info!("reconnecting ({e})");
                    self.apply(ctx, Act::Retry);
                } else {
                    ctx.request_repaint_after(at - Instant::now());
                }
            }
            Status::Ended if live => self.apply(ctx, Act::Retry),
            Status::Playing => self.retries = 0,
            _ => {}
        }
    }

    /// Tell the OS what's on.
    fn announce(&mut self) {
        if self.now_playing.is_none() {
            return;
        }
        let name = self.playing_name();
        let channel = self.playing_channel().cloned();
        let title = match &self.playing {
            Some(Playing::Catchup { programme, .. }) => programme.title.clone(),
            Some(Playing::Live { .. }) => channel.as_ref().and_then(|c| self.now_on(c)).map_or_else(|| name.clone(), |p| p.title.clone()),
            _ => name.clone(),
        };
        let subtitle = if title == name { self.source().map(|s| s.name.clone()).unwrap_or_default() } else { name };
        let logo = channel.and_then(|c| c.logo);
        let playing = self.player.as_ref().is_some_and(|p| !p.paused());
        if let Some(n) = &mut self.now_playing {
            n.update(if self.playing.is_some() { &title } else { "" }, &subtitle, logo.as_deref(), playing);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let typing = ctx.memory(|m| m.focused().is_some());
        ctx.input_mut(|i| {
            let cmd = Modifiers::COMMAND;
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                if self.popup.is_some() || self.form.is_some() {
                    acts.push(Act::Popup(None));
                    self.form = None;
                } else if i.viewport().fullscreen == Some(true) {
                    acts.push(Act::ToggleFullscreen);
                }
            }
            for (n, tab) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4].into_iter().zip(Tab::ALL) {
                if i.consume_key(cmd, n) {
                    acts.push(Act::Tab(tab));
                }
            }
            if i.consume_key(cmd, Key::F) {
                self.rail_open = true;
                self.focus_search = true;
            }
            if i.consume_key(cmd, Key::L) {
                acts.push(Act::ToggleRail);
            }
            if typing {
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Space) {
                acts.push(Act::TogglePause);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::PageDown) {
                acts.push(Act::Step(1));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::NONE, Key::PageUp) {
                acts.push(Act::Step(-1));
            }
            if i.consume_key(Modifiers::NONE, Key::F) {
                acts.push(Act::ToggleFullscreen);
            }
            if i.consume_key(Modifiers::NONE, Key::M) {
                acts.push(Act::ToggleMute);
            }
            if i.consume_key(Modifiers::NONE, Key::L) {
                acts.push(Act::ToggleRail);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                acts.push(Act::Seek(10.0));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                acts.push(Act::Seek(-10.0));
            }
        });
        // Files dropped on the window: playlists to add.
        let dropped: Vec<_> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(path) = dropped.into_iter().next() {
            self.form = Some(SourceForm::for_file(&path.to_string_lossy()));
        }
    }

    pub fn apply(&mut self, ctx: &egui::Context, act: Act) {
        match act {
            Act::PlayChannel(key) => self.play(ctx, Playing::Live { key }),
            Act::PlayMovie(key) => self.play(ctx, Playing::Movie { key }),
            Act::PlayCatchup(key, programme) => self.play(ctx, Playing::Catchup { key, programme }),
            Act::GoLive => {
                if let Some(key) = self.playing.as_ref().map(|p| p.key().to_string()) {
                    self.play(ctx, Playing::Live { key });
                }
            }
            Act::Retry => {
                if let Some(playing) = self.playing.clone() {
                    let retries = self.retries + 1;
                    self.play(ctx, playing);
                    self.retries = retries;
                }
            }
            Act::TogglePause => {
                if let Some(p) = &self.player {
                    p.set_paused(!p.paused());
                }
            }
            Act::Step(d) => self.step(ctx, d),
            Act::ToggleFavorite(key) => {
                let source = self.source_id();
                self.marks.toggle_favorite(&source, &key);
                self.marks_serial += 1;
            }
            Act::Tab(t) => {
                self.tab = t;
                self.rail_open = true;
                self.popup = None;
            }
            Act::Group(g) => {
                if self.tab == Tab::Movies {
                    self.movie_group = g;
                } else {
                    self.group = g;
                }
                self.popup = None;
            }
            Act::SwitchSource(i) => {
                self.popup = None;
                if self.active != Some(i) {
                    self.player = None;
                    self.playing = None;
                    self.video.clear();
                    self.switch_source(ctx, i, None);
                }
            }
            Act::Popup(p) => {
                self.popup = if self.popup == p { None } else { p };
                // Look again for speakers that went quiet (after sleep, a network change).
                if self.popup == Some(Popup::Outputs)
                    && let Some(d) = &self.discovery
                {
                    d.refresh();
                }
            }
            Act::Output(o) => {
                self.prefs.update(|p| p.output = matches!(o, Output::AirPlay(_)).then(|| o.name().to_string()));
                if let Some(p) = &self.player {
                    p.set_output(o.clone());
                }
                self.output = o;
                self.popup = None;
            }
            Act::Volume(v) => {
                self.prefs.update(|p| {
                    p.volume = v.clamp(0.0, 1.0);
                    p.muted = false;
                });
                if let Some(p) = &self.player {
                    p.set_volume(self.effective_volume());
                }
            }
            Act::ToggleMute => {
                self.prefs.update(|p| p.muted = !p.muted);
                if let Some(p) = &self.player {
                    p.set_volume(self.effective_volume());
                }
            }
            Act::ToggleFullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!full));
            }
            Act::ToggleRail => self.rail_open = !self.rail_open,
            Act::Theme(t) => self.prefs.update(|p| p.theme = t),
            Act::AddSource => {
                self.popup = None;
                self.form = Some(SourceForm::new());
            }
            Act::EditSource(i) => {
                self.popup = None;
                if let Some(s) = self.sources.get(i) {
                    self.form = Some(SourceForm::edit(s));
                }
            }
            Act::RemoveSource(i) => {
                if i >= self.sources.len() {
                    return;
                }
                let removed = self.sources.remove(i);
                crate::credentials::clear(&removed.id);
                self.marks.forget_source(&removed.id);
                library::save_sources(&self.sources);
                if self.active == Some(i) {
                    self.player = None;
                    self.playing = None;
                    self.video.clear();
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
            Act::SourceSaved(source, catalog) => {
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
                self.player = None;
                self.playing = None;
                self.video.clear();
                self.switch_source(ctx, i, Some(catalog));
                self.show_toast(&format!("{} is ready", source.name));
            }
            Act::AudioTrack(stream) => {
                if let Some(p) = &self.player {
                    p.set_audio_track(stream);
                }
                self.popup = None;
            }
            Act::Seek(delta) => {
                if let (Some(p), false) = (&self.player, matches!(self.playing, Some(Playing::Live { .. }))) {
                    let now = p.position().unwrap_or(0.0);
                    p.seek(now + delta);
                }
            }
            Act::Toast(t) => self.show_toast(&t),
        }
    }

    /// The next or previous entry of the list on screen (channel zapping).
    fn step(&mut self, ctx: &egui::Context, d: i32) {
        self.refresh_list();
        if self.list.is_empty() {
            return;
        }
        let movies = self.tab == Tab::Movies;
        let current = self.playing.as_ref().map(|p| p.key().to_string());
        let Some(catalog) = self.catalog.ready() else { return };
        let key_at = |i: usize| if movies { catalog.movies[i].key.clone() } else { catalog.channels[i].key.clone() };
        let pos = current.and_then(|k| self.list.iter().position(|&i| key_at(i) == k));
        let n = self.list.len() as i32;
        let next = match pos {
            Some(p) => (p as i32 + d).rem_euclid(n),
            None => 0,
        } as usize;
        let key = key_at(self.list[next]);
        self.play(ctx, if movies { Playing::Movie { key } } else { Playing::Live { key } });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        log_trace!("frame {}: {:?}", ctx.cumulative_pass_nr(), ctx.repaint_causes());
        self.sync_appearance(&ctx);
        self.logos.begin_frame();
        self.poll(&ctx);
        self.track_pointer(&ctx);
        let mut acts = Vec::new();
        self.shortcuts(&ctx, &mut acts);
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
        } else if self.controls_visible(&ctx) {
            self.top_bar(&ctx, screen, &mut acts);
            if self.rail_open {
                self.rail(&ctx, screen, &mut acts);
            }
            self.bottom(&ctx, screen, &mut acts);
            self.popups(&ctx, screen, &mut acts);
        } else {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        self.toast_view(&ctx, screen);
        if let Some(form) = &mut self.form
            && form.show(&ctx, screen) == Some(false)
        {
            self.form = None;
        }
        for act in acts {
            self.apply(&ctx, act);
        }
        // The programme line and times move on even when no new frames come (paused,
        // or sound only).
        if self.player.as_ref().is_some_and(|p| p.paused() || p.status() != Status::Playing || p.info().video_codec.is_none()) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}
