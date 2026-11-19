//! The mini player: the picture alone in a small window that stays above the
//! others, to watch while doing something else. P, Esc or a double-click put the
//! full window back where it was.

use super::App;
use crate::ui::style::metrics;
use eframe::egui::{self, Pos2, Rect, Vec2, ViewportCommand, WindowLevel, pos2, vec2};
use std::time::{Duration, Instant};

/// How wide the mini window opens.
const WIDTH: f32 = 400.0;
/// The smallest it can be made.
const MIN_SIZE: Vec2 = vec2(200.0, 112.0);
/// How long leaving full screen takes: macOS animates it, and a window resized
/// meanwhile is put back to its old size when the animation ends.
const UNFULLSCREEN: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub enum Mini {
    #[default]
    Off,
    /// Asked for in full screen: waits for full screen to end first.
    Waiting(Instant),
    On(Back),
}

/// Where the full window was and how big, to put it back.
#[derive(Clone, Copy)]
pub struct Back {
    pos: Option<Pos2>,
    size: Option<Vec2>,
    maximized: bool,
}

/// The mini window for a picture of `aspect`: its top right corner where the full
/// window's was, so it stays on the same screen and in reach.
fn frame(window: Rect, aspect: f32) -> Rect {
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect.clamp(1.0, 2.5) } else { 16.0 / 9.0 };
    let size = vec2(WIDTH, (WIDTH / aspect).round()).max(MIN_SIZE);
    Rect::from_min_size(pos2(window.right() - size.x, window.top()), size)
}

impl App {
    pub fn mini_on(&self) -> bool {
        matches!(self.mini, Mini::On(_))
    }

    /// Go to the mini player, or back to the full window. Only something playing
    /// goes mini.
    pub(super) fn toggle_mini(&mut self, ctx: &egui::Context) {
        match self.mini {
            Mini::Off if self.session.player.is_some() => {
                self.panels.popup = None;
                self.panels.guide_open = false;
                if ctx.input(|i| i.viewport().fullscreen == Some(true)) {
                    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
                    self.mini = Mini::Waiting(Instant::now());
                } else {
                    self.enter_mini(ctx);
                }
            }
            Mini::Off => {}
            Mini::Waiting(_) => self.mini = Mini::Off,
            Mini::On(back) => self.leave_mini(ctx, back),
        }
    }

    /// Go mini once full screen has ended.
    pub(super) fn settle_mini(&mut self, ctx: &egui::Context) {
        let Mini::Waiting(since) = self.mini else { return };
        let full = ctx.input(|i| i.viewport().fullscreen == Some(true));
        if !full && since.elapsed() >= UNFULLSCREEN {
            self.enter_mini(ctx);
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn enter_mini(&mut self, ctx: &egui::Context) {
        let (outer, inner, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized == Some(true))
        });
        let back = Back { pos: outer.map(|r| r.min), size: inner.map(|r| r.size()), maximized };
        let aspect = self.picture_aspect();
        let window = outer.or(inner).unwrap_or(Rect::from_min_size(Pos2::ZERO, Vec2::from(metrics::WINDOW_SIZE)));
        let mini = frame(window, aspect);
        if maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        }
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(MIN_SIZE));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(mini.size()));
        if outer.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(mini.min));
        }
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        self.mini = Mini::On(back);
    }

    fn leave_mini(&mut self, ctx: &egui::Context, back: Back) {
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(Vec2::from(metrics::MIN_WINDOW_SIZE)));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(back.size.unwrap_or(Vec2::from(metrics::WINDOW_SIZE))));
        if let Some(pos) = back.pos {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        }
        if back.maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
        self.mini = Mini::Off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mini_window_takes_the_full_windows_top_right_corner() {
        let window = Rect::from_min_size(pos2(100.0, 50.0), vec2(1280.0, 760.0));
        let mini = frame(window, 16.0 / 9.0);
        assert_eq!(mini.size(), vec2(400.0, 225.0));
        assert_eq!((mini.right(), mini.top()), (window.right(), window.top()));
    }

    #[test]
    fn odd_pictures_still_get_a_sensible_window() {
        let window = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 760.0));
        assert_eq!(frame(window, 4.0 / 3.0).size(), vec2(400.0, 300.0));
        // Tall, unknown or very wide pictures don't make a tower or a sliver.
        assert_eq!(frame(window, 9.0 / 16.0).size(), vec2(400.0, 400.0));
        assert_eq!(frame(window, 0.0).size(), vec2(400.0, 225.0));
        assert_eq!(frame(window, f32::NAN).size(), vec2(400.0, 225.0));
        assert_eq!(frame(window, 10.0).size(), vec2(400.0, 160.0));
    }
}
