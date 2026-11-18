//! What the viewer did this frame, and what it does: [`App::apply`] sends each
//! action to the part of the app it concerns.

use super::{App, Playing, Tab};
use crate::iptv::model::{Catalog, Programme, Shelf};
use crate::library::Source;
use crate::output::Output;
use crate::settings::Appearance;
use crate::ui::views::add_source::SourceForm;
use crate::ui::views::pin::{PinDialog, Purpose as PinPurpose};
use eframe::egui::{self, ViewportCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Popup {
    Outputs,
    Sources,
    AudioTracks,
    Settings,
    /// Which channels the guide shows.
    GuideChannels,
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
    /// Move a favourite (the first key) just before the second, or after it.
    MoveFavorite(String, String, bool),
    /// Show the favourites in the guide (None), or a group of channels (all of them
    /// when that's None too).
    GuideChannels(Option<Option<String>>),
    Tab(Tab),
    Group(Option<String>),
    /// Put a playlist group with the channels, movies or series; None leaves it to
    /// what it looks like.
    Shelve(String, Option<Shelf>),
    /// Hide a group (true) or show it again.
    Hide(String, bool),
    /// Show hidden groups (asking for the PIN, if one is set), or hide them again.
    ShowHidden(bool),
    /// Set a PIN for hidden groups, or remove it (asking for it).
    Pin(bool),
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
    /// Open or close the guide.
    ToggleGuide,
    /// Show or hide the column of groups.
    ToggleGroups,
    Theme(Appearance),
    AddSource,
    EditSource(usize),
    RemoveSource(usize),
    /// A source checked and ready to save, with what it offers.
    SourceSaved(Source, Box<Catalog>),
    AudioTrack(usize),
    /// Show the subtitle track at this stream index, or none.
    SubtitleTrack(Option<usize>),
    Seek(f64),
    Toast(String),
    AutoUpdate(bool),
    CheckForUpdates,
    /// The top bar's update button: install and restart, or open the download page.
    InstallUpdate,
}

impl App {
    pub fn apply(&mut self, ctx: &egui::Context, act: Act) {
        match act {
            Act::PlayChannel(key) => self.play(ctx, Playing::Live { key }),
            Act::PlayMovie(key) => self.play(ctx, Playing::Movie { key }),
            Act::OpenSeries(Some(i)) => self.open_series(ctx, i),
            Act::OpenSeries(None) => self.browse.close_series(),
            Act::PlayEpisode(i) => self.play_episode(ctx, i),
            Act::PlayCatchup(key, programme) => self.play(ctx, Playing::Catchup { key, programme }),
            Act::GoLive => self.go_live(ctx),
            Act::TogglePause => self.set_paused(!self.session.paused()),
            Act::Pause(paused) => self.set_paused(paused),
            Act::Step(d) => self.step(ctx, d),
            Act::Seek(delta) => self.seek(delta),
            Act::AudioTrack(stream) => self.audio_track(stream),
            Act::SubtitleTrack(stream) => self.subtitle_track(stream),
            Act::Output(o) => self.set_output(o),
            Act::Volume(v) => self.set_volume(v),
            Act::ToggleMute => self.toggle_mute(),
            Act::PictureDelay(change) => self.change_picture_delay(change),
            Act::ToggleFavorite(key) => self.toggle_favorite(&key),
            Act::MoveFavorite(key, target, after) => self.move_favorite(&key, &target, after),
            Act::Tab(t) => self.show_tab(t),
            Act::Group(g) => self.pick_group(g),
            Act::Shelve(group, shelf) => self.shelve(ctx, group, shelf),
            Act::Hide(group, hide) => self.hide(group, hide),
            Act::ShowHidden(show) => self.ask_show_hidden(show),
            Act::Pin(set) => self.ask_pin(set),
            Act::SwitchSource(i) => self.pick_source(ctx, i),
            Act::AddSource => self.open_form(SourceForm::new()),
            Act::EditSource(i) => {
                if let Some(form) = self.sources.get(i).map(SourceForm::edit) {
                    self.open_form(form);
                }
            }
            Act::RemoveSource(i) => self.remove_source(ctx, i),
            Act::SourceSaved(source, catalog) => self.save_source(ctx, source, *catalog),
            Act::Popup(p) => self.toggle_popup(p),
            Act::ToggleRail => self.panels.rail_open = !self.panels.rail_open,
            Act::ToggleGroups => self.panels.groups_open = !self.panels.groups_open,
            Act::ToggleGuide => self.toggle_guide(),
            Act::GuideChannels(pick) => self.guide_channels(pick),
            Act::ToggleFullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!full));
            }
            Act::Theme(t) => self.prefs.update(|p| p.theme = t),
            Act::AutoUpdate(on) => {
                self.updates.auto = on;
                self.prefs.update(|p| p.auto_update = on);
            }
            Act::CheckForUpdates => self.updates.check_now(ctx),
            Act::InstallUpdate => self.install_update(ctx),
            Act::Toast(t) => self.show_toast(&t),
        }
    }

    /// Open a menu, or close it when it's the one open. None closes any.
    fn toggle_popup(&mut self, popup: Option<Popup>) {
        let panels = &mut self.panels;
        panels.popup = if panels.popup == popup { None } else { popup };
        // Look again for speakers that went quiet (after sleep, a network change).
        if panels.popup == Some(Popup::Outputs)
            && let Some(d) = &self.discovery
        {
            d.refresh();
        }
    }

    fn open_form(&mut self, form: SourceForm) {
        self.panels.popup = None;
        self.form = Some(form);
    }

    fn show_tab(&mut self, tab: Tab) {
        self.prefs.update(|p| p.tab = Some(tab.key().to_string()));
        self.browse.tab = tab;
        self.panels.rail_open = true;
        self.panels.popup = None;
    }

    fn toggle_guide(&mut self) {
        let panels = &mut self.panels;
        panels.guide_open = !panels.guide_open;
        panels.guide_jump = panels.guide_open;
        panels.popup = None;
    }

    /// Show the favourites in the guide (None), or a group of channels.
    fn guide_channels(&mut self, pick: Option<Option<String>>) {
        self.panels.popup = None;
        self.panels.guide_jump = true;
        self.panels.guide_favorites = pick.is_none();
        self.browse.group_filter.clear();
        if let Some(g) = pick {
            self.prefs.update(|p| p.group = g.clone());
            self.browse.channels.picked = g;
        }
    }

    /// Show hidden groups, asking for the PIN first when there is one.
    fn ask_show_hidden(&mut self, show: bool) {
        self.panels.popup = None;
        if show && self.prefs.pin.is_some() {
            self.pin = Some(PinDialog::new(PinPurpose::Unlock));
        } else {
            self.show_hidden(show);
        }
    }

    /// Set a PIN, or remove the one set.
    fn ask_pin(&mut self, set: bool) {
        self.panels.popup = None;
        self.pin = Some(PinDialog::new(if set { PinPurpose::Set } else { PinPurpose::Remove }));
    }

    /// Install the update and restart, or open the download page.
    fn install_update(&mut self, ctx: &egui::Context) {
        if let Some(restart) = self.updates.act(ctx) {
            self.keep_progress();
            crate::update::relaunch(&restart);
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}
