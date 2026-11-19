//! Playing: opening what was picked, zapping, the player's settings, and what the
//! player reports back.

use super::{App, Playing, delay_text};
use crate::iptv::model::Episode;
use crate::library::{self, Source};
use crate::media::{Player, Request, Status};
use crate::output::Output;
use crate::ui::load::Load;
use crate::ui::session::Next;
use eframe::egui;
use std::time::{Duration, Instant};

impl App {
    fn request_for(&self, playing: &Playing) -> Option<Request> {
        let catalog = self.catalog.ready()?;
        match playing {
            Playing::Live { key } => {
                let c = catalog.channels.iter().find(|c| &c.key == key)?;
                Some(Request {
                    url: c.url.clone(),
                    user_agent: c.user_agent.clone(),
                    referrer: c.referrer.clone(),
                    live: true,
                    start_at: None,
                })
            }
            Playing::Catchup { key, programme } => {
                let c = catalog.channels.iter().find(|c| &c.key == key)?;
                let login = self.source().and_then(Source::login);
                let offset = catalog.account.as_ref().map_or(0, |a| a.server_offset);
                let url = library::catchup_url(c, programme, login.as_ref(), offset)?;
                Some(Request { url, user_agent: c.user_agent.clone(), referrer: c.referrer.clone(), live: false, start_at: None })
            }
            Playing::Movie { key } => {
                let m = catalog.movies.iter().find(|m| &m.key == key)?;
                Some(Request { url: m.url.clone(), user_agent: None, referrer: None, live: false, start_at: None })
            }
            Playing::Episode { episode, .. } => {
                Some(Request { url: episode.url.clone(), user_agent: None, referrer: None, live: false, start_at: None })
            }
        }
    }

    /// A player for `playing`, which starts once the one before has stopped (see
    /// [`crate::media::Stopping`]); None when it can't be played.
    fn open(&mut self, ctx: &egui::Context, playing: &Playing, start_at: Option<f64>) -> Option<Player> {
        self.request_for(playing)?;
        self.keep_progress();
        let previous = self.session.player.take().map(Player::stop);
        self.video.clear();
        let player = self.new_player(ctx, playing, start_at, self.output.clone(), self.effective_volume(), previous)?;
        player.set_picture_delay(self.picture_delay());
        Some(player)
    }

    /// A player for `playing` on `output`, which starts once `after` has stopped.
    pub(super) fn new_player(
        &self,
        ctx: &egui::Context,
        playing: &Playing,
        start_at: Option<f64>,
        output: Output,
        volume: f32,
        after: Option<crate::media::Stopping>,
    ) -> Option<Player> {
        let request = Request { start_at, ..self.request_for(playing)? };
        log_info!("playing {}", crate::iptv::http::redact(&request.url));
        let repaint = ctx.clone();
        // A zero-delay request draws twice (egui lets layouts settle); a frame of video
        // needs one pass.
        Some(Player::open(request, output, volume, after, move || repaint.request_repaint_after(Duration::from_millis(1))))
    }

    pub fn play(&mut self, ctx: &egui::Context, playing: Playing) {
        // In multiview a channel takes the place of the one in focus; anything else
        // is watched alone.
        if self.multi.on() {
            match &playing {
                Playing::Live { key } if self.session.live() => {
                    if !self.retune_focus(key) {
                        return;
                    }
                }
                _ => self.leave_multi(),
            }
        }
        let source = self.source_id();
        let resume = match &playing {
            Playing::Movie { key } | Playing::Episode { episode: Episode { key, .. }, .. } => self.progress.resume_at(&source, key),
            Playing::Live { .. } | Playing::Catchup { .. } => None,
        };
        let Some(player) = self.open(ctx, &playing, resume) else {
            self.show_toast("That can't be played");
            return;
        };
        if let Playing::Live { key } = &playing {
            self.marks.watched(&source, key);
            self.prefs.update(|p| p.channel = Some(key.clone()));
        }
        if let Some(at) = resume {
            self.show_toast(&format!("Carrying on from {} (← goes back)", crate::ui::views::duration_text(at)));
        }
        self.progress_at = Instant::now();
        self.session.start(playing, player);
    }

    /// Keep how far the movie or episode playing got.
    pub(super) fn keep_progress(&mut self) {
        let Some(player) = &self.session.player else { return };
        let (Some((key, series)), Some(position), Some(duration)) =
            (self.session.recording_key(), player.position(), player.info().duration)
        else {
            return;
        };
        let position = position - self.session.start_pts.unwrap_or(0.0);
        self.progress.record(&self.source_id(), &key, series.as_deref(), position, duration);
        self.progress_at = Instant::now();
    }

    /// Open what plays again, after the provider dropped it.
    fn reopen(&mut self, ctx: &egui::Context) {
        if let Some(playing) = self.session.playing.clone()
            && let Some(player) = self.open(ctx, &playing, None)
        {
            self.session.restart(player);
        }
    }

    pub(super) fn stop(&mut self) {
        self.leave_multi();
        self.keep_progress();
        self.session.stop();
        self.video.clear();
    }

    /// The next or previous entry of the list on screen (channel zapping).
    pub(super) fn step(&mut self, ctx: &egui::Context, d: i32) {
        self.refresh_list();
        let current = self.session.playing.as_ref().map(Playing::key);
        if let Some(next) = self.catalog.ready().and_then(|c| self.browse.step(c, current, d)) {
            self.play(ctx, next);
        }
    }

    /// Show the episodes of series `i`: the playlist lists them, or the provider is
    /// asked.
    pub(super) fn open_series(&mut self, ctx: &egui::Context, i: usize) {
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

    /// Open series `i` and play the episode of it watched last, from where it was
    /// left, once the episodes are in.
    pub(super) fn carry_on_series(&mut self, ctx: &egui::Context, i: usize) {
        let Some(series) = self.catalog.ready().and_then(|c| c.series.get(i)).map(|s| s.key.clone()) else { return };
        let episode = self.progress.left(&self.source_id()).into_iter().find_map(|l| match l {
            crate::library::Left::Episode { series: s, key } if s == series => Some(key),
            _ => None,
        });
        self.open_series(ctx, i);
        self.browse.carry_on = episode;
        self.carry_on(ctx);
    }

    /// Play the episode to carry on with, once the open series' episodes are in.
    pub(super) fn carry_on(&mut self, ctx: &egui::Context) {
        if self.browse.carry_on.is_none() || self.browse.episodes.is_loading() {
            return;
        }
        let key = self.browse.carry_on.take();
        let at = self.browse.episodes.ready().and_then(|l| l.iter().position(|e| Some(&e.key) == key.as_ref()));
        if let Some(i) = at {
            self.play_episode(ctx, i);
        }
    }

    pub(super) fn play_episode(&mut self, ctx: &egui::Context, i: usize) {
        if let Some(episode) = self.catalog.ready().and_then(|c| self.browse.episode(c, i)) {
            self.play(ctx, episode);
        }
    }

    /// From the archive (or a programme's start) back to the channel, live.
    pub(super) fn go_live(&mut self, ctx: &egui::Context) {
        if let Some(key) = self.session.playing.as_ref().map(|p| p.key().to_string()) {
            self.play(ctx, Playing::Live { key });
        }
    }

    pub(super) fn set_paused(&self, paused: bool) {
        if let Some(p) = &self.session.player {
            p.set_paused(paused);
        }
    }

    /// Skip `delta` seconds in a recording; live streams don't seek.
    pub(super) fn seek(&self, delta: f64) {
        if let (Some(p), false) = (&self.session.player, self.session.live()) {
            p.seek(p.position().unwrap_or(0.0) + delta);
        }
    }

    pub(super) fn audio_track(&mut self, stream: usize) {
        if let Some(p) = &self.session.player {
            p.set_audio_track(stream);
        }
        self.panels.popup = None;
    }

    pub(super) fn subtitle_track(&mut self, stream: Option<usize>) {
        if let Some(p) = &self.session.player {
            p.set_subtitle_track(stream);
        }
        self.panels.popup = None;
    }

    pub(super) fn set_output(&mut self, output: Output) {
        self.prefs.update(|p| p.output = matches!(output, Output::AirPlay(_)).then(|| output.name().to_string()));
        self.output = output;
        if let Some(p) = &self.session.player {
            p.set_output(self.output.clone());
            p.set_picture_delay(self.picture_delay());
        }
    }

    pub(super) fn set_volume(&mut self, volume: f32) {
        self.prefs.update(|p| {
            p.volume = volume.clamp(0.0, 1.0);
            p.muted = false;
        });
        self.update_volume();
    }

    pub(super) fn toggle_mute(&mut self) {
        self.prefs.update(|p| p.muted = !p.muted);
        self.update_volume();
    }

    /// The picture delay set for the output in use, in milliseconds.
    pub fn picture_delay(&self) -> i32 {
        self.prefs.picture_delays.get(self.output.name()).copied().unwrap_or(0)
    }

    fn effective_volume(&self) -> f32 {
        if self.prefs.muted { 0.0 } else { self.prefs.volume * self.sleep_loudness() }
    }

    pub(super) fn update_volume(&self) {
        if let Some(p) = &self.session.player {
            p.set_volume(self.effective_volume());
        }
    }

    /// Change the picture delay of the output in use by `change` milliseconds, or set
    /// it back to zero.
    pub(super) fn change_picture_delay(&mut self, change: Option<i32>) {
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

    /// Speakers come and go; the one picked last time is used once it shows up.
    pub(super) fn use_saved_output(&mut self) {
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
    pub(super) fn poll_player(&mut self, ctx: &egui::Context) {
        let Some(player) = &self.session.player else { return };
        let (serial, frame) = player.frame();
        let (notice, status, duration) = (player.take_notice(), player.status(), player.info().duration);
        // Positions count from where the stream starts; a stream that doesn't say
        // starts at its first picture.
        if self.session.start_pts.is_none() {
            self.session.start_pts = frame.as_ref().map(|f| player.info().start.unwrap_or(f.pts));
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
                if let (Some((key, series)), Some(duration)) = (self.session.recording_key(), duration) {
                    self.progress.record(&self.source_id(), &key, series.as_deref(), duration, duration);
                }
                let next = self.catalog.ready().zip(self.session.playing.as_ref()).and_then(|(c, p)| self.browse.next_episode(c, p));
                if let Some(next) = next {
                    self.play(ctx, next);
                }
            }
        }
    }

    /// Tell the OS what's on.
    pub(super) fn announce(&mut self) {
        if self.remotes.now_playing.is_none() {
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
        if let Some(n) = &mut self.remotes.now_playing {
            n.update(if self.session.playing.is_some() { &title } else { "" }, &subtitle, logo.as_deref(), playing);
        }
    }

    /// Let the display sleep only when nothing is playing: paused, ended or failed.
    pub(super) fn keep_screen_on(&mut self) {
        let playing = self
            .session
            .player
            .as_ref()
            .is_some_and(|p| !p.paused() && matches!(p.status(), Status::Opening | Status::Playing | Status::Buffering));
        if playing != self.screen.is_some() {
            self.screen = playing.then(crate::awake::Awake::screen);
        }
    }

    /// Show the subtitles in a file over the movie or episode playing.
    pub(super) fn subtitles_from(&mut self, path: &std::path::Path) {
        let Some(player) = self.session.player.as_ref().filter(|_| self.session.recording_key().is_some()) else {
            self.show_toast("Subtitles go with a movie or an episode: start one, then drop the file");
            return;
        };
        let cues = std::fs::read(path).map(|b| crate::media::subtitle::parse_srt(&crate::media::subtitle::decode_text(&b)));
        match cues {
            Ok(cues) if !cues.is_empty() => {
                let n = cues.len();
                player.use_subtitles(cues, self.session.start_pts.unwrap_or(0.0));
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                log_info!("subtitles: {n} lines from {name}");
                self.show_toast(&format!("Subtitles from {name}"));
            }
            Ok(_) => self.show_toast("That file has no subtitles in it"),
            Err(e) => self.show_toast(&format!("Couldn't read the subtitles: {e}")),
        }
    }
}
