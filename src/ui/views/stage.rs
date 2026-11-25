//! The picture filling the window, what shows instead of it, the welcome screen
//! and short messages: toasts, and reminders whose time came.

use crate::media::Status;
use crate::ui::app::{Act, App};
use crate::ui::icons::Icon;
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

/// Subtitle lines centred over `picture`, their bottom at `bottom`, sized to the
/// picture, on a dark band so they read over anything.
fn subtitles(painter: &egui::Painter, text: &str, picture: Rect, bottom: f32) {
    let size = (picture.height() * 0.045).clamp(15.0, 44.0);
    let mut job = egui::text::LayoutJob::simple(
        text.to_string(),
        egui::FontId::new(size, typography::semibold()),
        Color32::WHITE,
        picture.width() * 0.8,
    );
    job.halign = egui::Align::Center;
    let galley = painter.layout_job(job);
    let pos = egui::pos2(picture.center().x, bottom - galley.size().y);
    for row in &galley.rows {
        let r = row.rect().translate(pos.to_vec2()).expand2(vec2(size * 0.35, size * 0.06));
        painter.rect_filled(r, size * 0.2, Color32::from_black_alpha(165));
    }
    painter.galley(pos, galley, Color32::WHITE);
}

/// How long a message stays.
const TOAST: Duration = Duration::from_secs(4);

impl App {
    pub fn stage(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let area = ui.max_rect();
        let mini = self.mini_on();
        // The mini window has no title bar: it moves by the picture.
        let resp = ui.interact(area, ui.scope_id().with("stage"), if mini { Sense::click_and_drag() } else { Sense::click() });
        if resp.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            acts.push(if mini { Act::ToggleMini } else { Act::ToggleFullscreen });
        } else if resp.clicked() && self.panels.popup.is_some() {
            acts.push(Act::Popup(None));
        }
        if self.multi.on() && !mini {
            return self.multi_stage(ui, area, acts);
        }
        let p = colors();
        let picture = video::place(area, self.picture_aspect(), self.prefs.fill);
        if self.video.has_picture() {
            self.video.paint(ui.painter(), picture);
        }
        let Some(player) = &self.session.player else {
            if !self.sources.is_empty() {
                self.idle_backdrop(ui, area);
            }
            return;
        };
        if let Some(text) = player.subtitles() {
            // Over the part of the picture that shows.
            let picture = picture.intersect(area);
            let floor = if self.panels.controls_shown { area.bottom() - metrics::BOTTOM - metrics::GAP * 2.0 } else { picture.bottom() };
            subtitles(ui.painter(), &text, picture, floor.min(picture.bottom()) - picture.height() * 0.04);
        }
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
                let retrying = matches!(self.session.playing, Some(crate::ui::app::Playing::Live { .. }));
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

    /// Several channels at once: each in its place, the one in focus (whose sound
    /// plays) outlined; a click brings one into focus, a double-click watches it
    /// alone, the cross stops it.
    fn multi_stage(&mut self, ui: &mut egui::Ui, area: Rect, acts: &mut Vec<Act>) {
        let p = colors();
        let focus = self.session.playing.as_ref().map(|pl| pl.key().to_string());
        let order = self.multi.order.clone();
        for (key, r) in order.iter().zip(crate::ui::app::multi_slots(area.shrink(4.0), order.len())) {
            let focused = focus.as_deref() == Some(key.as_str());
            let (video, player) = if focused {
                (&self.video, self.session.player.as_ref())
            } else {
                match self.multi.tile(key) {
                    Some(t) => (&t.video, t.session.player.as_ref()),
                    None => continue,
                }
            };
            ui.painter().rect_filled(r, 6.0, Color32::BLACK);
            if video.has_picture() {
                let aspect = player.and_then(|pl| pl.frame().1).map_or(16.0 / 9.0, |f| f.aspect);
                video.paint(ui.painter(), video::fit(r, aspect));
            } else if player.is_some_and(|pl| matches!(pl.status(), Status::Failed(_))) {
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "Reconnecting…", typography::control(), p.on_video);
            } else {
                widgets::spinner(ui, r.center(), 14.0, p.accent);
            }
            let resp = ui.interact(r, ui.scope_id().with(("tile", key)), Sense::click());
            if resp.double_clicked() {
                acts.push(Act::Solo(key.clone()));
            } else if resp.clicked() && !focused {
                acts.push(Act::FocusTile(key.clone()));
            }
            if focused {
                ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(2.0, p.accent), egui::StrokeKind::Inside);
            }
            // The channel's name, and a cross to stop it, while the pointer is over it.
            if resp.hovered() || focused && self.panels.controls_shown {
                let name = self.channel(key).map(|c| c.name.clone()).unwrap_or_default();
                let text = widgets::ellipsize(ui, &name, typography::small_strong(), p.text, r.width() - 80.0);
                let pill = Rect::from_min_size(r.left_bottom() + vec2(10.0, -10.0 - 28.0), vec2(text.size().x + 20.0, 28.0));
                ui.painter().rect_filled(pill, 14.0, p.glass_strong);
                ui.painter().galley(pill.center() - text.size() / 2.0, text, p.text);
            }
            if resp.hovered() || resp.contains_pointer() {
                let close = Rect::from_center_size(r.right_top() + vec2(-22.0, 22.0), vec2(28.0, 28.0));
                let x = ui.interact(close, ui.scope_id().with(("tile-close", key)), Sense::click());
                ui.painter().circle_filled(close.center(), 14.0, if x.hovered() { p.glass_strong } else { Color32::from_black_alpha(120) });
                crate::ui::icons::paint(ui.painter(), close.shrink(9.0), Icon::Close, if x.hovered() { p.text } else { p.on_video });
                if x.on_hover_text("Stop this channel").clicked() {
                    acts.push(Act::CloseTile(key.clone()));
                }
            }
        }
    }

    /// The shape the picture is drawn in: the viewer's, or the stream's.
    pub fn picture_aspect(&self) -> f32 {
        let stream = self.session.player.as_ref().and_then(|p| p.frame().1).map_or(16.0 / 9.0, |f| f.aspect);
        self.session.shape.aspect(stream)
    }

    /// What's playing, by name.
    pub fn playing_name(&self) -> String {
        let Some(playing) = &self.session.playing else { return String::new() };
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
        let c = area.center() + vec2(if self.panels.rail_open { self.rail_width() / 2.0 } else { 0.0 }, -10.0);
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
                    ui.horizontal(|ui| {
                        let gap = 10.0;
                        let w = widgets::pill_width(ui, "Add source", true) + gap + widgets::pill_width(ui, "Try free channels", true);
                        ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                        ui.spacing_mut().item_spacing.x = gap;
                        if widgets::pill(ui, Some(crate::ui::icons::Icon::Plus), "Add source", true).clicked() {
                            acts.push(Act::AddSource);
                        }
                        if widgets::pill(ui, Some(crate::ui::icons::Icon::Globe), "Try free channels", false).clicked() {
                            acts.push(Act::AddFree);
                        }
                    });
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("or drop a playlist file on this window").font(typography::secondary()).color(p.faint));
                });
            },
        );
    }

    /// Over the mini player, while the pointer moves: pause and the way back.
    pub fn mini_controls(&self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let p = colors();
        let paused = self.session.player.as_ref().is_some_and(|pl| pl.paused());
        egui::Area::new(egui::Id::unique("mini-controls"))
            .order(egui::Order::Middle)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(egui::pos2(screen.center().x, screen.bottom() - metrics::GAP))
            .show(ctx, |ui| {
                widgets::glass(18).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if widgets::round_button(
                            ui,
                            if paused { Icon::Play } else { Icon::Pause },
                            36.0,
                            p.text,
                            p.glass_strong.to_opaque(),
                        )
                        .clicked()
                        {
                            acts.push(Act::TogglePause);
                        }
                        if widgets::icon_button(ui, Icon::Mini, 34.0, p.text, "Back to the full window (P)").clicked() {
                            acts.push(Act::ToggleMini);
                        }
                    });
                });
            });
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

    /// What's being recorded, at the top right: the channel that plays (stopped
    /// with R), and programmes from the guide, each with how long it's been going.
    /// A click stops one.
    pub fn recording_badge(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let mut badges: Vec<(String, std::time::Instant, String, Act)> = Vec::new();
        if let Some(record) = &self.record {
            badges.push((String::new(), record.since, "Stop recording (R)".into(), Act::ToggleRecording));
        }
        for taped in &self.taped {
            let plan = &taped.plan;
            let stop = Act::Record(plan.channel.clone(), plan.programme());
            badges.push((format!("{} ", plan.title), taped.since, format!("Stop recording {}", plan.title), stop));
        }
        if badges.is_empty() {
            return;
        }
        let p = colors();
        let top = if self.mini_on() || !self.panels.controls_shown {
            screen.top() + 8.0
        } else {
            screen.top() + metrics::GAP * 2.0 + metrics::TOP_BAR
        };
        egui::Area::new(egui::Id::unique("recording"))
            .order(egui::Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(egui::pos2(screen.right() - metrics::GAP, top))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    for (title, since, tip, stop) in badges {
                        let elapsed = since.elapsed();
                        ctx.request_repaint_after(Duration::from_millis(1000 - u64::from(elapsed.subsec_millis())));
                        let text = format!("REC {title}{}", crate::ui::views::duration_text(elapsed.as_secs_f64()));
                        let galley = widgets::ellipsize(ui, &text, typography::small_strong(), p.text, 280.0);
                        let size = vec2(galley.size().x + 52.0, 30.0);
                        let (r, resp) = ui.allocate_exact_size(size, Sense::click());
                        let fill = if resp.hovered() { p.glass_strong.to_opaque() } else { p.glass_strong };
                        ui.painter().rect_filled(r, egui::CornerRadius::same(15), fill);
                        ui.painter().circle_filled(egui::pos2(r.left() + 16.0, r.center().y), 5.0, p.danger);
                        ui.painter().galley(egui::pos2(r.left() + 28.0, r.center().y - galley.size().y / 2.0), galley, p.text);
                        let square = Rect::from_center_size(egui::pos2(r.right() - 14.0, r.center().y), vec2(12.0, 12.0));
                        crate::ui::icons::paint(ui.painter(), square, Icon::Stop, if resp.hovered() { p.text } else { p.dim });
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked() {
                            acts.push(stop);
                        }
                    }
                });
            });
    }

    /// The reminder whose time came: its programme, and to watch it or not.
    pub fn reminder_view(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let Some(r) = self.reminding.first() else { return };
        let p = colors();
        let now = crate::iptv::time::now();
        let at = crate::ui::views::hhmm(r.start);
        let when = if r.start > now { format!("Starts at {at}") } else { format!("Started at {at}") };
        let detail = match self.reminder_channel(r) {
            Some(channel) => format!("{channel} · {when}"),
            None => {
                let source = self.sources.iter().find(|s| s.id == r.source).map_or("another source", |s| s.name.as_str());
                format!("On {source} · {when}")
            }
        };
        let more = self.reminding.len() - 1;
        let top = if self.mini_on() { screen.top() + 8.0 } else { screen.top() + metrics::GAP * 2.0 + metrics::TOP_BAR };
        let width = (screen.width() - 16.0).min(440.0);
        egui::Area::new(egui::Id::unique("reminder"))
            .order(egui::Order::Tooltip)
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(egui::pos2(screen.center().x, top))
            .show(ctx, |ui| {
                widgets::popover().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    ui.horizontal(|ui| {
                        let (bell, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                        ui.painter().rect_filled(bell, egui::CornerRadius::same(10), p.accent);
                        crate::ui::icons::paint(ui.painter(), bell.shrink(8.0), Icon::Bell, p.on_accent);
                        ui.add_space(4.0);
                        let buttons = 150.0;
                        let text_w = (ui.available_width() - buttons).max(60.0);
                        ui.vertical(|ui| {
                            ui.set_width(text_w);
                            let title = widgets::ellipsize(ui, &r.title, typography::small_strong(), p.text, text_w);
                            ui.label(title);
                            let mut detail = detail.clone();
                            if more > 0 {
                                detail.push_str(&format!(" · {more} more"));
                            }
                            ui.label(widgets::ellipsize(ui, &detail, typography::meta(), p.dim, text_w));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::Close, 28.0, p.faint, "Not now").clicked() {
                                acts.push(Act::AnswerReminder(false));
                            }
                            if widgets::pill(ui, Some(Icon::Play), "Watch", true).clicked() {
                                acts.push(Act::AnswerReminder(true));
                            }
                        });
                    });
                });
            });
    }
}
