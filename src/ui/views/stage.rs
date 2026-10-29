//! The picture filling the window, what shows instead of it, the welcome screen
//! and short messages.

use crate::media::Status;
use crate::ui::app::{Act, App};
use crate::ui::style::{colors, metrics, typography};
use crate::ui::{video, widgets};
use eframe::egui::{self, Align2, Color32, Rect, Sense, vec2};
use std::time::Duration;

/// A soft radial glow: `color` at the centre, fading out at `radius`.
fn glow(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: Color32) {
    let mut mesh = egui::Mesh::default();
    let rings = [(0.0, 1.0), (0.35, 0.7), (0.65, 0.3), (1.0, 0.0)];
    let segments = 64;
    mesh.colored_vertex(center, color);
    for (r, a) in &rings[1..] {
        for i in 0..segments {
            let t = i as f32 / segments as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + vec2(t.cos(), t.sin()) * radius * *r, color.gamma_multiply(*a));
        }
    }
    let seg = segments as u32;
    for i in 0..seg {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % seg);
    }
    for ring in 0..(rings.len() as u32 - 2) {
        let (a, b) = (1 + ring * seg, 1 + (ring + 1) * seg);
        for i in 0..seg {
            let j = (i + 1) % seg;
            mesh.add_triangle(a + i, b + i, b + j);
            mesh.add_triangle(a + i, b + j, a + j);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// How long a message stays.
const TOAST: Duration = Duration::from_secs(4);

impl App {
    pub fn stage(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let area = ui.max_rect();
        let resp = ui.interact(area, ui.scope_id().with("stage"), Sense::click());
        if resp.double_clicked() {
            acts.push(Act::ToggleFullscreen);
        } else if resp.clicked() && self.popup.is_some() {
            acts.push(Act::Popup(None));
        }
        let p = colors();
        if self.video.has_picture() {
            let aspect = self.player.as_ref().and_then(|pl| pl.frame().1).map_or(16.0 / 9.0, |f| f.aspect);
            self.video.paint(ui.painter(), video::fit(area, aspect));
        }
        let Some(player) = &self.player else {
            if !self.sources.is_empty() {
                self.idle_backdrop(ui, area);
            }
            return;
        };
        let center = area.center() - vec2(0.0, 30.0);
        let name = self.playing_name();
        match player.status() {
            Status::Opening | Status::Buffering if !self.video.has_picture() => {
                widgets::spinner(ui, center, 18.0, p.accent);
                ui.painter().text(
                    center + vec2(0.0, 40.0),
                    Align2::CENTER_TOP,
                    format!("Tuning in to {name}…"),
                    typography::control(),
                    p.dim,
                );
            }
            Status::Failed(e) => {
                let retrying = matches!(self.playing, Some(crate::ui::app::Playing::Live { .. }));
                let text = if retrying { format!("{name}: {e}. Reconnecting…") } else { format!("{name}: {e}") };
                let bg = Rect::from_center_size(
                    center,
                    vec2(ui.painter().layout_no_wrap(text.clone(), typography::control(), p.text).size().x + 40.0, 48.0),
                );
                ui.painter().rect_filled(bg, 12.0, p.glass_strong);
                ui.painter().text(bg.center(), Align2::CENTER_CENTER, text, typography::control(), p.text);
            }
            _ => {}
        }
        if player.paused() {
            let r = Rect::from_center_size(area.center(), vec2(72.0, 72.0));
            ui.painter().circle_filled(r.center(), 36.0, Color32::from_black_alpha(120));
            crate::ui::icons::paint(ui.painter(), r.shrink(24.0), crate::ui::icons::Icon::Pause, p.on_video);
        }
    }

    /// What's playing, by name.
    pub fn playing_name(&self) -> String {
        let Some(playing) = &self.playing else { return String::new() };
        let catalog = self.catalog.ready();
        match playing {
            crate::ui::app::Playing::Movie { key } => {
                catalog.and_then(|c| c.movies.iter().find(|m| &m.key == key)).map(|m| m.name.clone()).unwrap_or_default()
            }
            crate::ui::app::Playing::Episode { series, .. } => {
                catalog.and_then(|c| c.series.iter().find(|s| &s.key == series)).map(|s| s.name.clone()).unwrap_or_default()
            }
            _ => self.playing_channel().map(|c| c.name.clone()).unwrap_or_default(),
        }
    }

    fn idle_backdrop(&self, ui: &egui::Ui, area: Rect) {
        let p = colors();
        let painter = ui.painter();
        let c = area.center() + vec2(if self.rail_open { self.rail_width() / 2.0 } else { 0.0 }, -10.0);
        glow(painter, c, area.width().max(area.height()) * 0.45, p.accent.gamma_multiply(0.16));
        painter.text(c, Align2::CENTER_BOTTOM, "haze_", egui::FontId::new(44.0, typography::extrabold()), p.text);
        let hint = match &self.catalog {
            crate::ui::load::Load::Loading(_) => "Loading the channels…".to_string(),
            crate::ui::load::Load::Failed(e) => format!("The channels couldn't be loaded: {e}"),
            _ => "Pick a channel — ↑ ↓ to zap, Space to pause, F for full screen".to_string(),
        };
        painter.text(c + vec2(0.0, 14.0), Align2::CENTER_TOP, hint, typography::body(), p.dim);
        if self.catalog.is_loading() {
            widgets::spinner(ui, c + vec2(0.0, 64.0), 12.0, p.accent);
        }
    }

    pub fn welcome_glow(&self, ui: &egui::Ui, area: Rect) {
        glow(ui.painter(), area.center(), area.width().max(area.height()) * 0.45, colors().accent.gamma_multiply(0.16));
    }

    pub fn welcome(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let p = colors();
        egui::Area::new(egui::Id::unique("welcome")).order(egui::Order::Middle).anchor(Align2::CENTER_CENTER, vec2(0.0, -20.0)).show(
            ctx,
            |ui| {
                ui.set_width(screen.width().min(460.0));
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("haze_").font(egui::FontId::new(56.0, typography::extrabold())).color(p.text));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Your channels, light as air.").font(typography::heading()).color(p.dim));
                    ui.add_space(18.0);
                    ui.label(
                        egui::RichText::new("Add an M3U playlist by link or file, or sign in with your provider's Xtream Codes login.")
                            .font(typography::body())
                            .color(p.dim),
                    );
                    ui.add_space(22.0);
                    if widgets::pill(ui, Some(crate::ui::icons::Icon::Plus), "Add source", true).clicked() {
                        acts.push(Act::AddSource);
                    }
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("or drop a playlist file on this window").font(typography::secondary()).color(p.faint));
                });
            },
        );
    }

    pub fn toast_view(&mut self, ctx: &egui::Context, screen: Rect) {
        let Some((text, at)) = &self.toast else { return };
        let age = at.elapsed();
        if age > TOAST {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(TOAST - age);
        let p = colors();
        let y = screen.bottom() - metrics::BOTTOM - metrics::GAP * 3.0;
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(egui::pos2(screen.center().x, y))
            .show(ctx, |ui| {
                widgets::popover().inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                    ui.label(egui::RichText::new(text.as_str()).font(typography::control()).color(p.text));
                });
            });
    }
}
