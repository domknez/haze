//! The app's state, what happens each frame, and what the viewer's actions do.

pub use super::browse::Tab;
pub use super::session::Playing;

use super::browse::Browse;
use super::load::Load;
use super::logos::Logos;
use super::session::{Next, Session};
use super::style::{self, colors, metrics};
use super::video::Video;
use super::views::add_source::SourceForm;
use crate::iptv::model::{Catalog, Channel, Guide, Programme, Shelf};
use crate::iptv::time;
use crate::library::{self, Marks, Progress, Source};
use crate::media::{Player, Request, Status};
use crate::output::Output;
use crate::output::airplay::Discovery;
use crate::settings::{Appearance, Prefs};
use eframe::egui::{self, Key, Modifiers, ViewportCommand};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Popup {
    Outputs,
    Sources,
    AudioTracks,
    Settings,
}

/// What the viewer did this frame; applied after drawing.
pub enum Act {
    PlayChannel(String),
    PlayMovie(String),
    /// Show the episodes of a series (its index in the catalog), or go back to the
    /// series.
    OpenSeries(Option<usize>),
    /// An episode of the open series, by its index there.
    PlayEpisode(usize),
    PlayCatchup(String, Programme),
    GoLive,
    TogglePause,
    /// Pause (true) or play on (false); nothing when it already is.
    Pause(bool),
    /// Step through the list on screen: -1 previous, +1 next.
    Step(i32),
    ToggleFavorite(String),
    Tab(Tab),
    Group(Option<String>),
    /// Put a playlist group with the channels, movies or series; None leaves it to
    /// what it looks like.
    Shelve(String, Option<Shelf>),
    /// Hide a group (true) or show it again.
    Hide(String, bool),
    /// Show hidden groups, or hide them again.
    ShowHidden(bool),
    SwitchSource(usize),
    Popup(Option<Popup>),
    Output(Output),
    Volume(f32),
    /// Change the picture delay of the output in use by this many milliseconds;
    /// None sets it back to zero.
    PictureDelay(Option<i32>),
    ToggleMute,
    ToggleFullscreen,
    ToggleRail,
    /// Show or hide the column of groups.
    ToggleGroups,
    Theme(Appearance),
    AddSource,
    EditSource(usize),
    RemoveSource(usize),
    /// A source checked and ready to save, with what it offers.
    SourceSaved(Source, Box<Catalog>),
    AudioTrack(usize),
    Seek(f64),
    Toast(String),
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
    pub rail_open: bool,
    /// The column of groups beside the channels.
    pub groups_open: bool,
    pub popup: Option<Popup>,
    pub form: Option<SourceForm>,
    pub toast: Option<(String, Instant)>,
    /// Bumped when favourites or recents change.
    pub marks_serial: u64,
    last_motion: Instant,
    last_pointer: Option<egui::Pos2>,
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
    /// Playlist sources that turned out to be Xtream logins while loading.
    upgrades: (Sender<library::Upgrade>, Receiver<library::Upgrade>),
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
            rail_open: true,
            groups_open: true,
            popup: None,
            form: None,
            toast: None,
            marks_serial: 0,
            last_motion: Instant::now(),
            last_pointer: None,
            focus_search: false,
            anchors: Default::default(),
            applied_dark: None,
            remote,
            now_playing: super::now_playing::NowPlaying::start(cc),
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

    /// Start using source `i`; `catalog` when it was just loaded (a new source).
    fn switch_source(&mut self, ctx: &egui::Context, i: usize, catalog: Option<Catalog>) {
        let Some(source) = self.sources.get(i).cloned() else { return };
        self.active = Some(i);
        self.prefs.update(|p| p.source = Some(source.id.clone()));
        self.browse.reset();
        self.browse.hidden = source.hidden.clone();
        self.guide = Load::Idle;
        match catalog {
            Some(c) => {
                self.catalog = Load::Ready(c);
                self.catalog_arrived(ctx);
            }
            None => {
                let upgrades = self.upgrades.0.clone();
                self.catalog = Load::start(ctx, move || {
                    let (catalog, upgrade) = library::fetch_upgrading(&source)?;
                    if let Some(u) = upgrade {
                        let _ = upgrades.send(u);
                    }
                    Ok(catalog)
                });
            }
        }
    }

    /// The catalog came in: count its groups, load its guide, resume the last channel.
    fn catalog_arrived(&mut self, ctx: &egui::Context) {
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
            Playing::Episode { episode, .. } => Some(Request { url: episode.url.clone(), user_agent: None, referrer: None, live: false }),
        }
    }

    /// A player for `playing`, opened once the old one stopped (providers often allow
    /// one connection); None when it can't be played.
    fn open(&mut self, ctx: &egui::Context, playing: &Playing) -> Option<Player> {
        let request = self.request_for(playing)?;
        log_info!("playing {}", request.url.split('?').next().unwrap_or(""));
        self.keep_progress();
        self.session.player = None;
        self.video.clear();
        let repaint = ctx.clone();
        // A zero-delay request draws twice (egui lets layouts settle); a frame of video
        // needs one pass.
        let player = Player::open(request, self.output.clone(), self.effective_volume(), move || {
            repaint.request_repaint_after(Duration::from_millis(1))
        });
        player.set_picture_delay(self.picture_delay());
        Some(player)
    }

    pub fn play(&mut self, ctx: &egui::Context, playing: Playing) {
        let Some(player) = self.open(ctx, &playing) else {
            self.show_toast("That can't be played");
            return;
        };
        let source = self.source_id();
        match &playing {
            Playing::Live { key } => {
                self.marks.watched(&source, key);
                self.marks_serial += 1;
                self.prefs.update(|p| p.channel = Some(key.clone()));
            }
            Playing::Movie { key } | Playing::Episode { episode: crate::iptv::model::Episode { key, .. }, .. } => {
                if let Some(at) = self.progress.resume_at(&source, key) {
                    player.seek(at);
                    self.show_toast(&format!("Carrying on from {} (← goes back)", super::views::duration_text(at)));
                }
            }
            Playing::Catchup { .. } => {}
        }
        self.progress_at = Instant::now();
        self.session.start(playing, player);
    }

    /// Keep how far the movie or episode playing got.
    fn keep_progress(&mut self) {
        let Some(player) = &self.session.player else { return };
        let (Some(key), Some(position), Some(duration)) = (self.session.recording_key(), player.position(), player.info().duration) else {
            return;
        };
        let position = position - self.session.start_pts.unwrap_or(0.0);
        self.progress.record(&self.source_id(), &key, position, duration);
        self.progress_at = Instant::now();
    }

    /// Open what plays again, after the provider dropped it.
    fn reopen(&mut self, ctx: &egui::Context) {
        if let Some(playing) = self.session.playing.clone()
            && let Some(player) = self.open(ctx, &playing)
        {
            self.session.restart(player);
        }
    }

    fn stop(&mut self) {
        self.keep_progress();
        self.session.stop();
        self.video.clear();
    }

    /// The picture delay set for the output in use, in milliseconds.
    pub fn picture_delay(&self) -> i32 {
        self.prefs.picture_delays.get(self.output.name()).copied().unwrap_or(0)
    }

    fn effective_volume(&self) -> f32 {
        if self.prefs.muted { 0.0 } else { self.prefs.volume }
    }

    pub fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    /// What the rail lists, worked out again only when something it depends on changed.
    pub fn refresh_list(&mut self) {
        let source = self.source_id();
        self.browse.refresh(self.catalog.ready(), &self.marks, &source, self.marks_serial);
    }

    /// Show the episodes of series `i`: the playlist lists them, or the provider is
    /// asked.
    fn open_series(&mut self, ctx: &egui::Context, i: usize) {
        let Some(series) = self.catalog.ready().and_then(|c| c.series.get(i)) else { return };
        self.browse.open_series = Some(i);
        self.browse.episodes = match self.source().and_then(Source::login) {
            Some(login) if series.episodes.is_empty() => {
                let key = series.key.clone();
                Load::start(ctx, move || login.episodes(&key).map_err(|e| e.to_string()))
            }
            _ => Load::Ready(series.episodes.clone()),
        };
    }

    /// Whether the panels over the picture show: always, unless something plays and
    /// the pointer has been still for a while.
    fn controls_visible(&self, ctx: &egui::Context) -> bool {
        let playing = self.session.player.as_ref().is_some_and(|p| p.status() == Status::Playing && !p.paused());
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
        self.poll_loads(ctx);
        for act in self.presses() {
            self.apply(ctx, act);
        }
        self.announce();
        self.use_saved_output();
        self.poll_player(ctx);
    }

    /// Take what the background loads brought.
    fn poll_loads(&mut self, ctx: &egui::Context) {
        // Before the catalog it came with: the source is a login by then.
        while let Ok((source, password)) = self.upgrades.1.try_recv() {
            let Some(i) = self.sources.iter().position(|s| s.id == source.id) else { continue };
            log_info!("{}: signing in to the panel's API in place of its playlist link", source.name);
            crate::credentials::save(&source.id, &password);
            // Channels are known by their stream id now, not their link.
            self.marks.rekey(&source.id, crate::iptv::xtream::stream_id);
            self.marks_serial += 1;
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
            self.apply(ctx, Act::SourceSaved(source, Box::new(catalog)));
        }
    }

    /// Presses on the AirPlay speaker's buttons, media keys and the system's Now
    /// Playing controls.
    fn presses(&self) -> Vec<Act> {
        use super::now_playing::Press;
        use crate::output::airplay::dacp::Remote;
        let speaker = self.remote.iter().flat_map(|(_, rx)| rx.try_iter()).map(|r| match r {
            Remote::PlayPause => Act::TogglePause,
            Remote::Play => Act::Pause(false),
            Remote::Pause => Act::Pause(true),
            Remote::Next => Act::Step(1),
            Remote::Prev => Act::Step(-1),
            Remote::Volume(v) => Act::Volume(v),
            Remote::VolumeStep(d) => Act::Volume(self.prefs.volume + d),
        });
        let keys = self.now_playing.iter().flat_map(|n| n.presses()).map(|p| match p {
            Press::Toggle => Act::TogglePause,
            Press::Play => Act::Pause(false),
            Press::Pause => Act::Pause(true),
            Press::Next => Act::Step(1),
            Press::Prev => Act::Step(-1),
            Press::Seek(d) => Act::Seek(d),
            Press::Volume(v) => Act::Volume(v),
        });
        speaker.chain(keys).collect()
    }

    /// Speakers come and go; the one picked last time is used once it shows up.
    fn use_saved_output(&mut self) {
        if let (Some(name), Output::Local, Some(d)) = (&self.prefs.output, &self.output, &self.discovery)
            && let Some(device) = d.devices().into_iter().find(|d| &d.name == name)
        {
            self.output = Output::AirPlay(device);
            if let Some(p) = &self.session.player {
                p.set_output(self.output.clone());
            }
        }
    }

    /// The player's newest frame, what it has to say, and what its status calls for.
    fn poll_player(&mut self, ctx: &egui::Context) {
        let Some(player) = &self.session.player else { return };
        let (serial, frame) = player.frame();
        let (notice, status, duration) = (player.take_notice(), player.status(), player.info().duration);
        if self.session.start_pts.is_none() {
            self.session.start_pts = frame.as_ref().map(|f| f.pts);
        }
        self.video.set_frame(serial, frame);
        if let Some(notice) = notice {
            self.show_toast(&notice);
        }
        if status == Status::Playing && self.progress_at.elapsed() >= Duration::from_secs(10) {
            self.keep_progress();
        }
        match self.session.next(&status, Instant::now()) {
            Next::Nothing => {}
            Next::Wait(at) => ctx.request_repaint_after(at.saturating_duration_since(Instant::now())),
            Next::Reopen => self.reopen(ctx),
            Next::Ended => {
                if let (Some(key), Some(duration)) = (self.session.recording_key(), duration) {
                    self.progress.record(&self.source_id(), &key, duration, duration);
                }
                let next = self.catalog.ready().zip(self.session.playing.as_ref()).and_then(|(c, p)| self.browse.next_episode(c, p));
                if let Some(next) = next {
                    self.play(ctx, next);
                }
            }
        }
    }

    /// Tell the OS what's on.
    fn announce(&mut self) {
        if self.now_playing.is_none() {
            return;
        }
        let name = self.playing_name();
        let channel = self.playing_channel().cloned();
        let title = match &self.session.playing {
            Some(Playing::Catchup { programme, .. }) => programme.title.clone(),
            Some(Playing::Live { .. }) => channel.as_ref().and_then(|c| self.now_on(c)).map_or_else(|| name.clone(), |p| p.title.clone()),
            _ => name.clone(),
        };
        let subtitle = if title == name { self.source().map(|s| s.name.clone()).unwrap_or_default() } else { name };
        let logo = channel.and_then(|c| c.logo);
        let playing = self.session.player.as_ref().is_some_and(|p| !p.paused());
        if let Some(n) = &mut self.now_playing {
            n.update(if self.session.playing.is_some() { &title } else { "" }, &subtitle, logo.as_deref(), playing);
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
            for (n, tab) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5].into_iter().zip(Tab::ALL) {
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
            if i.consume_key(Modifiers::NONE, Key::CloseBracket) {
                acts.push(Act::PictureDelay(Some(50)));
            }
            if i.consume_key(Modifiers::NONE, Key::OpenBracket) {
                acts.push(Act::PictureDelay(Some(-50)));
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
            Act::OpenSeries(Some(i)) => self.open_series(ctx, i),
            Act::OpenSeries(None) => self.browse.close_series(),
            Act::PlayEpisode(i) => {
                if let Some(episode) = self.catalog.ready().and_then(|c| self.browse.episode(c, i)) {
                    self.play(ctx, episode);
                }
            }
            Act::PlayCatchup(key, programme) => self.play(ctx, Playing::Catchup { key, programme }),
            Act::GoLive => {
                if let Some(key) = self.session.playing.as_ref().map(|p| p.key().to_string()) {
                    self.play(ctx, Playing::Live { key });
                }
            }
            Act::TogglePause => self.apply(ctx, Act::Pause(!self.session.paused())),
            Act::Pause(paused) => {
                if let Some(p) = &self.session.player {
                    p.set_paused(paused);
                }
            }
            Act::Step(d) => self.step(ctx, d),
            Act::ToggleFavorite(key) => {
                let source = self.source_id();
                self.marks.toggle_favorite(&source, &key);
                self.marks_serial += 1;
            }
            Act::Tab(t) => {
                self.prefs.update(|p| p.tab = Some(t.key().to_string()));
                self.browse.tab = t;
                self.rail_open = true;
                self.popup = None;
            }
            Act::Group(g) => {
                if !matches!(self.browse.tab, Tab::Movies | Tab::Series) {
                    self.prefs.update(|p| p.group = g.clone());
                }
                self.browse.pick_group(g);
            }
            Act::Shelve(group, shelf) => self.shelve(ctx, group, shelf),
            Act::Hide(group, hide) => self.hide(group, hide),
            Act::ShowHidden(show) => {
                self.browse.show_hidden = show;
                self.popup = None;
                if let Some(c) = self.catalog.ready() {
                    self.browse.count_groups(c);
                }
            }
            Act::ToggleGroups => self.groups_open = !self.groups_open,
            Act::SwitchSource(i) => {
                self.popup = None;
                if self.active != Some(i) {
                    self.stop();
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
                self.output = o;
                if let Some(p) = &self.session.player {
                    p.set_output(self.output.clone());
                    p.set_picture_delay(self.picture_delay());
                }
            }
            Act::Volume(v) => {
                self.prefs.update(|p| {
                    p.volume = v.clamp(0.0, 1.0);
                    p.muted = false;
                });
                self.update_volume();
            }
            Act::ToggleMute => {
                self.prefs.update(|p| p.muted = !p.muted);
                self.update_volume();
            }
            Act::PictureDelay(change) => self.change_picture_delay(change),
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
            Act::RemoveSource(i) => self.remove_source(ctx, i),
            Act::SourceSaved(source, catalog) => self.save_source(ctx, source, *catalog),
            Act::AudioTrack(stream) => {
                if let Some(p) = &self.session.player {
                    p.set_audio_track(stream);
                }
                self.popup = None;
            }
            Act::Seek(delta) => {
                if let (Some(p), false) = (&self.session.player, self.session.live()) {
                    let now = p.position().unwrap_or(0.0);
                    p.seek(now + delta);
                }
            }
            Act::Toast(t) => self.show_toast(&t),
        }
    }

    fn update_volume(&self) {
        if let Some(p) = &self.session.player {
            p.set_volume(self.effective_volume());
        }
    }

    /// Change the picture delay of the output in use by `change` milliseconds, or set
    /// it back to zero.
    fn change_picture_delay(&mut self, change: Option<i32>) {
        let name = self.output.name().to_string();
        let ms = change.map_or(0, |d| (self.picture_delay() + d).clamp(-2000, 3000));
        self.prefs.update(|p| {
            if ms == 0 {
                p.picture_delays.remove(&name);
            } else {
                p.picture_delays.insert(name.clone(), ms);
            }
        });
        if let Some(p) = &self.session.player {
            p.set_picture_delay(ms);
        }
        self.show_toast(&format!("Picture delay on {name}: {}", delay_text(ms)));
    }

    /// Put a playlist group with the channels, movies or series (None: where it looks
    /// like it belongs), and read the source again; what plays goes on.
    fn shelve(&mut self, ctx: &egui::Context, group: String, shelf: Option<Shelf>) {
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

    fn hide(&mut self, group: String, hide: bool) {
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

    fn remove_source(&mut self, ctx: &egui::Context, i: usize) {
        if i >= self.sources.len() {
            return;
        }
        let removed = self.sources.remove(i);
        crate::credentials::clear(&removed.id);
        self.marks.forget_source(&removed.id);
        self.progress.forget_source(&removed.id);
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

    /// A source checked and ready: keep it (in place of the one edited) and use it.
    fn save_source(&mut self, ctx: &egui::Context, source: Source, catalog: Catalog) {
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

    /// The next or previous entry of the list on screen (channel zapping).
    fn step(&mut self, ctx: &egui::Context, d: i32) {
        self.refresh_list();
        let current = self.session.playing.as_ref().map(Playing::key);
        if let Some(next) = self.catalog.ready().and_then(|c| self.browse.step(c, current, d)) {
            self.play(ctx, next);
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
    }

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
        if self.session.player.as_ref().is_some_and(|p| p.paused() || p.status() != Status::Playing || p.info().video_codec.is_none()) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}
