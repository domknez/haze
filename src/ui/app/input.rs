//! What comes in besides clicks on the panels: keys, files dropped on the window,
//! the pointer going still, and remote controls (the AirPlay speaker's buttons,
//! media keys, the system's Now Playing).

use super::{Act, App, Popup, Tab};
use crate::media::Status;
use crate::output::airplay::dacp::{self, Remote};
use crate::ui::now_playing::{NowPlaying, Press};
use crate::ui::style::metrics;
use crate::ui::views::add_source::SourceForm;
use eframe::egui::{self, Key, Modifiers};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Keys that act on their own (no modifier), unless a text field has the keyboard.
const KEYS: [(Key, fn() -> Act); 14] = [
    (Key::Space, || Act::TogglePause),
    (Key::ArrowDown, || Act::Step(1)),
    (Key::PageDown, || Act::Step(1)),
    (Key::ArrowUp, || Act::Step(-1)),
    (Key::PageUp, || Act::Step(-1)),
    (Key::ArrowRight, || Act::Seek(10.0)),
    (Key::ArrowLeft, || Act::Seek(-10.0)),
    (Key::F, || Act::ToggleFullscreen),
    (Key::P, || Act::ToggleMini),
    (Key::M, || Act::ToggleMute),
    (Key::L, || Act::ToggleRail),
    (Key::G, || Act::ToggleGuide),
    (Key::CloseBracket, || Act::PictureDelay(Some(50))),
    (Key::OpenBracket, || Act::PictureDelay(Some(-50))),
];

/// When the pointer last moved (or a key or the wheel was used), so the panels can
/// fade out while the picture is watched.
pub struct Pointer {
    at: Option<egui::Pos2>,
    moved: Instant,
}

impl Default for Pointer {
    fn default() -> Self {
        Pointer { at: None, moved: Instant::now() }
    }
}

impl Pointer {
    pub fn track(&mut self, ctx: &egui::Context) {
        let (at, used) = ctx.input(|i| {
            let used =
                i.pointer.any_down() || i.events.iter().any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::MouseWheel { .. }));
            (i.pointer.hover_pos(), used)
        });
        if at != self.at || used {
            self.at = at;
            self.moved = Instant::now();
        }
    }

    fn still_for(&self) -> Duration {
        self.moved.elapsed()
    }
}

/// The AirPlay speaker's buttons, and the OS's media keys and Now Playing widget.
pub struct Remotes {
    speaker: Option<(dacp::Server, Receiver<Remote>)>,
    pub now_playing: Option<NowPlaying>,
}

impl Remotes {
    pub fn start(cc: &eframe::CreationContext) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = cc.egui_ctx.clone();
        let speaker = dacp::start(move |r| {
            let _ = tx.send(r);
            repaint.request_repaint();
        })
        .inspect_err(|e| log_warn!("speaker remote control unavailable: {e}"))
        .ok()
        .map(|server| (server, rx));
        Remotes { speaker, now_playing: NowPlaying::start(cc) }
    }

    /// What was pressed since the last frame; `volume` is the one set now.
    pub fn presses(&self, volume: f32) -> Vec<Act> {
        let speaker = self.speaker.iter().flat_map(|(_, rx)| rx.try_iter()).map(|r| match r {
            Remote::PlayPause => Act::TogglePause,
            Remote::Play => Act::Pause(false),
            Remote::Pause => Act::Pause(true),
            Remote::Next => Act::Step(1),
            Remote::Prev => Act::Step(-1),
            Remote::Volume(v) => Act::Volume(v),
            Remote::VolumeStep(d) => Act::Volume(volume + d),
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
}

impl App {
    /// Whether the panels over the picture show: always, unless something plays and
    /// the pointer has been still for a while.
    pub(super) fn controls_visible(&self, ctx: &egui::Context) -> bool {
        let playing = self.session.player.as_ref().is_some_and(|p| p.status() == Status::Playing && !p.paused());
        if !playing || self.panels.popup.is_some() || self.form.is_some() || self.pin.is_some() || self.panels.focus_search {
            return true;
        }
        let hide = Duration::from_secs_f32(metrics::IDLE_HIDE);
        let still = self.pointer.still_for();
        if still < hide {
            ctx.request_repaint_after(hide - still + Duration::from_millis(50));
            return true;
        }
        false
    }

    pub(super) fn shortcuts(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let full = ctx.input(|i| i.viewport().fullscreen == Some(true));
        ctx.input_mut(|i| {
            let cmd = Modifiers::COMMAND;
            for (n, tab) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5].into_iter().zip(Tab::ALL) {
                if i.consume_key(cmd, n) {
                    acts.push(Act::Tab(tab));
                }
            }
            if i.consume_key(cmd, Key::F) {
                self.panels.rail_open = true;
                self.panels.focus_search = true;
            }
            if i.consume_key(cmd, Key::L) {
                acts.push(Act::ToggleRail);
            }
            // ⌘, on macOS, Ctrl+, elsewhere: where every app keeps its settings.
            if i.consume_key(cmd, Key::Comma) && self.panels.popup != Some(Popup::Settings) {
                acts.push(Act::Popup(Some(Popup::Settings)));
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                acts.extend(self.escape(full));
            }
            if !typing {
                acts.extend(KEYS.iter().filter(|(key, _)| i.consume_key(Modifiers::NONE, *key)).map(|(_, act)| act()));
                if i.consume_key(Modifiers::NONE, Key::Z) {
                    acts.push(Act::Fill(!self.prefs.fill));
                }
                if i.consume_key(Modifiers::NONE, Key::A) {
                    acts.push(Act::Shape(self.session.shape.next()));
                }
            }
        });
    }

    /// Esc closes what's on top: a menu or dialog, then the guide, then the mini player
    /// or full screen.
    fn escape(&mut self, full: bool) -> Option<Act> {
        if self.panels.popup.is_some() || self.form.is_some() || self.pin.is_some() {
            self.form = None;
            self.pin = None;
            Some(Act::Popup(None))
        } else if self.panels.guide_open {
            Some(Act::ToggleGuide)
        } else if self.mini_on() {
            Some(Act::ToggleMini)
        } else {
            full.then_some(Act::ToggleFullscreen)
        }
    }

    /// Files dropped on the window: subtitles for what plays, or playlists to add.
    pub(super) fn dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        let Some(path) = dropped else { return };
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
        if matches!(ext.as_deref(), Some("srt" | "vtt")) {
            self.subtitles_from(&path);
        } else {
            self.form = Some(SourceForm::for_file(&path.to_string_lossy()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_does_one_thing() {
        let mut keys: Vec<_> = KEYS.iter().map(|(k, _)| *k).collect();
        keys.sort_by_key(|k| k.name());
        keys.dedup();
        assert_eq!(keys.len(), KEYS.len());
    }

    #[test]
    fn the_speaker_steps_the_volume_from_where_it_is() {
        let (tx, rx) = std::sync::mpsc::channel();
        let remotes = Remotes { speaker: Some((dacp::Server, rx)), now_playing: None };
        tx.send(Remote::VolumeStep(-0.1)).unwrap();
        tx.send(Remote::Next).unwrap();
        let acts = remotes.presses(0.5);
        assert!(matches!(acts[..], [Act::Volume(v), Act::Step(1)] if (v - 0.4).abs() < 1e-6));
        assert!(remotes.presses(0.5).is_empty());
    }
}
