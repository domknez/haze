//! Multiview: up to four live channels at once, side by side. One of them is the
//! channel in focus, played by the session as any channel is (its sound, the
//! bottom panel, zapping); the others play beside it without sound. A click on one
//! brings it into focus, a double-click watches it alone.

use super::{App, Playing};
use crate::output::Output;
use crate::ui::session::{Next, Session};
use crate::ui::video::Video;
use eframe::egui::{self, Rect, pos2, vec2};
use std::time::Instant;

/// The most channels watched at once.
pub const MAX: usize = 4;

/// A channel playing beside the one in focus.
pub struct Tile {
    pub session: Session,
    pub video: Video,
}

/// The channels watched at once; none outside multiview.
#[derive(Default)]
pub struct Multi {
    /// The keys of the channels, in the order they're laid out, the one in focus
    /// among them.
    pub order: Vec<String>,
    /// The channels besides the one in focus.
    pub tiles: Vec<Tile>,
    /// Pictures of channels closed, to use again (each holds GPU textures).
    spare: Vec<Video>,
}

impl Multi {
    pub fn on(&self) -> bool {
        self.order.len() > 1
    }

    /// The tile playing channel `key`.
    pub fn tile(&self, key: &str) -> Option<&Tile> {
        self.tiles.iter().find(|t| t.session.playing.as_ref().is_some_and(|p| p.key() == key))
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.tiles.iter().position(|t| t.session.playing.as_ref().is_some_and(|p| p.key() == key))
    }
}

/// Where `n` pictures go in `area`: two side by side, three or four in a square of
/// four, each as big as fits.
pub fn slots(area: Rect, n: usize) -> Vec<Rect> {
    let (cols, rows) = if n <= 2 { (n.max(1), 1) } else { (2, 2) };
    let gap = 4.0;
    let size = vec2((area.width() - gap * (cols - 1) as f32) / cols as f32, (area.height() - gap * (rows - 1) as f32) / rows as f32);
    // Two side by side sit in the middle, as tall as they need.
    let top = if rows == 1 { area.center().y - (size.x * 9.0 / 16.0).min(size.y) / 2.0 } else { area.top() };
    let size = if rows == 1 { vec2(size.x, (size.x * 9.0 / 16.0).min(size.y)) } else { size };
    (0..n)
        .map(|i| {
            let (c, r) = (i % cols, i / cols);
            Rect::from_min_size(pos2(area.left() + c as f32 * (size.x + gap), top + r as f32 * (size.y + gap)), size)
        })
        .collect()
}

impl App {
    /// Watch live channel `key` beside what's on.
    pub(super) fn add_tile(&mut self, ctx: &egui::Context, key: String) {
        let Some(Playing::Live { key: focus }) = self.session.playing.clone() else {
            self.play(ctx, Playing::Live { key });
            return;
        };
        if self.multi.order.is_empty() {
            self.multi.order.push(focus);
        }
        if self.multi.order.contains(&key) {
            return self.focus_tile(key);
        }
        if self.multi.order.len() >= MAX {
            return self.show_toast(&format!("Up to {MAX} channels at once"));
        }
        if let Some(m) = self.streams_to_watch().filter(|&m| self.multi.order.len() >= m) {
            let streams = if m == 1 { "one stream".to_string() } else { format!("{m} streams") };
            let besides = if self.taped.is_empty() { "" } else { " besides those recording" };
            return self.show_toast(&format!("Your provider allows {streams} at once{besides}"));
        }
        let playing = Playing::Live { key: key.clone() };
        let Some(player) = self.new_player(ctx, &playing, None, Output::Local, 0.0, None) else {
            return self.show_toast("That can't be played");
        };
        let mut session = Session::default();
        session.start(playing, player);
        let video = self.multi.spare.pop().unwrap_or_else(Video::new);
        self.multi.tiles.push(Tile { session, video });
        self.multi.order.push(key);
        if self.multi.order.len() == 2 {
            self.show_toast("Click a channel for its sound, double-click to watch it alone");
        }
    }

    /// Bring channel `key`, playing beside, into focus: its sound, the bottom panel.
    pub(super) fn focus_tile(&mut self, key: String) {
        let Some(i) = self.multi.position(&key) else { return };
        let tile = &mut self.multi.tiles[i];
        std::mem::swap(&mut self.session, &mut tile.session);
        std::mem::swap(&mut self.video, &mut tile.video);
        if let Some(p) = &tile.session.player {
            p.set_output(Output::Local);
            p.set_volume(0.0);
            p.set_picture_delay(0);
        }
        if let Some(p) = &self.session.player {
            p.set_output(self.output.clone());
            p.set_picture_delay(self.picture_delay());
        }
        self.update_volume();
        self.marks.watched(&self.source_id(), &key);
        self.prefs.update(|p| p.channel = Some(key));
    }

    /// Stop watching channel `key` among the others.
    pub(super) fn close_tile(&mut self, key: &str) {
        let focused = self.session.playing.as_ref().is_some_and(|p| p.key() == key);
        if focused {
            // Another takes the focus: the one after it, or before.
            let at = self.multi.order.iter().position(|k| k == key).unwrap_or(0);
            let next = self.multi.order.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| self.multi.order.get(i))).cloned();
            if let Some(next) = next {
                self.focus_tile(next);
            }
        }
        if let Some(i) = self.multi.position(key) {
            let mut tile = self.multi.tiles.remove(i);
            drop(tile.session.player.take().map(crate::media::Player::stop));
            tile.video.clear();
            self.multi.spare.push(tile.video);
        }
        self.multi.order.retain(|k| k != key);
        if self.multi.order.len() < 2 {
            self.multi.order.clear();
        }
    }

    /// Watch channel `key` alone, the others stopped.
    pub(super) fn solo(&mut self, key: String) {
        self.focus_tile(key);
        self.leave_multi();
    }

    /// Stop the channels playing beside the one in focus.
    pub(super) fn leave_multi(&mut self) {
        for mut tile in std::mem::take(&mut self.multi.tiles) {
            drop(tile.session.player.take().map(crate::media::Player::stop));
            tile.video.clear();
            self.multi.spare.push(tile.video);
        }
        self.multi.order.clear();
    }

    /// The channel in focus changes to `key` (zapping, a pick from the list): it
    /// takes the same place. False when `key` already plays beside, and comes into
    /// focus instead.
    pub(super) fn retune_focus(&mut self, key: &str) -> bool {
        if self.multi.tile(key).is_some() {
            self.focus_tile(key.to_string());
            return false;
        }
        let focus = self.session.playing.as_ref().map(|p| p.key().to_string());
        if let Some(slot) = self.multi.order.iter_mut().find(|k| Some(k.as_str()) == focus.as_deref()) {
            *slot = key.to_string();
        }
        true
    }

    /// Take the pictures of the channels beside, and open again those that dropped.
    pub(super) fn poll_tiles(&mut self, ctx: &egui::Context) {
        let mut reopen = Vec::new();
        for (i, tile) in self.multi.tiles.iter_mut().enumerate() {
            let Some(player) = &tile.session.player else { continue };
            let (serial, frame) = player.frame();
            tile.video.set_frame(serial, frame);
            match tile.session.next(&player.status(), Instant::now()) {
                Next::Wait(at) => ctx.request_repaint_after(at.saturating_duration_since(Instant::now())),
                Next::Reopen => reopen.push(i),
                Next::Nothing | Next::Ended => {}
            }
        }
        for i in reopen {
            let Some(playing) = self.multi.tiles[i].session.playing.clone() else { continue };
            let previous = self.multi.tiles[i].session.player.take().map(crate::media::Player::stop);
            if let Some(player) = self.new_player(ctx, &playing, None, Output::Local, 0.0, previous) {
                self.multi.tiles[i].session.restart(player);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_two_side_by_side_and_more_in_a_square() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1604.0, 904.0));
        let two = slots(area, 2);
        assert_eq!(two[0].size(), vec2(800.0, 450.0));
        assert_eq!((two[0].center().y, two[1].left()), (452.0, 804.0));
        let four = slots(area, 4);
        assert_eq!(four[3], Rect::from_min_size(pos2(804.0, 454.0), vec2(800.0, 450.0)));
        assert_eq!(slots(area, 3).len(), 3);
    }
}
