//! The guide: the channels of the group picked against the time, with what's on
//! each before, now and after. A click on a channel or on what's on now plays the
//! channel; one on a programme from its archive plays that.

use crate::iptv::model::{Channel, Programme};
use crate::iptv::time;
use crate::ui::app::{Act, App, Popup};
use crate::ui::icons::{self, Icon};
use crate::ui::style::{colors, metrics, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};

/// Width of half an hour.
const HALF_HOUR: f32 = 170.0;
const ROW: f32 = 50.0;
/// The column of channel names, which stays put while the times scroll.
const NAMES: f32 = 210.0;
/// The row of times along the top.
const RULER: f32 = 26.0;
/// How far back (about as far as archives reach) and ahead the guide goes.
const BEFORE: i64 = 12 * 3600;
const AFTER: i64 = 24 * 3600;

impl App {
    pub fn guide_view(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let top = screen.top() + metrics::GAP - 4.0 + metrics::TOP_BAR + metrics::GAP;
        let rect = Rect::from_min_max(pos2(screen.left() + metrics::GAP, top), screen.max - vec2(metrics::GAP, metrics::GAP));
        if rect.height() < 160.0 {
            return;
        }
        egui::Area::new(egui::Id::unique("guide")).order(egui::Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
            widgets::glass(metrics::PANEL_RADIUS).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                let inner = rect.size() - vec2(20.0, 20.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                self.guide_header(ui, acts);
                self.guide_grid(ui, acts);
            });
        });
    }

    fn guide_header(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.add_space(6.0);
            ui.label(egui::RichText::new("GUIDE").font(typography::meta()).color(p.faint));
            ui.add_space(6.0);
            // What it shows: click to pick the favourites or another group.
            let label = match &self.browse.channels.picked {
                _ if self.guide_favorites => "Favorites".to_string(),
                Some(g) => g.clone(),
                None => "All channels".into(),
            };
            let text = widgets::ellipsize(ui, &label, typography::small_strong(), p.text, 320.0);
            let (r, pick) = ui.allocate_exact_size(vec2(text.size().x + 40.0, 30.0), Sense::click());
            let open = self.popup == Some(Popup::GuideChannels);
            if open || pick.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(9), if open { p.hl_strong } else { p.hl });
            }
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(13.0, 13.0)), Icon::List, p.dim);
            ui.painter().galley(pos2(r.left() + 28.0, r.center().y - text.size().y / 2.0), text, p.text);
            self.anchor(Popup::GuideChannels, r);
            if pick.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show another group").clicked() {
                acts.push(Act::Popup(if open { None } else { Some(Popup::GuideChannels) }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Close, 28.0, p.faint, "Close the guide (G)").clicked() {
                    acts.push(Act::ToggleGuide);
                }
                if widgets::pill(ui, None, "Now", false).clicked() {
                    self.guide_jump = true;
                }
            });
        });
        ui.add_space(4.0);
    }

    fn guide_grid(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(catalog) = self.catalog.ready() else { return };
        let Some(guide) = self.guide.ready() else {
            let text = if self.guide.is_loading() { "Loading the guide…" } else { "This source has no guide." };
            ui.add_space(16.0);
            ui.label(egui::RichText::new(text).font(typography::body()).color(p.dim));
            return;
        };
        let channels = if self.guide_favorites {
            let source = self.source_id();
            let by_key = |k: &str| catalog.channels.iter().position(|c| c.key == k);
            self.marks.favorites(&source).filter_map(by_key).filter(|&i| self.browse.shows(&catalog.channels[i].group)).collect()
        } else {
            self.browse.channels_in_group(catalog)
        };
        let now = time::now();
        let t0 = (now - BEFORE) / 1800 * 1800;
        let x_of = |t: i64| NAMES + (t - t0) as f32 / 1800.0 * HALF_HOUR;
        let size = vec2(x_of(now + AFTER), RULER + channels.len() as f32 * ROW);
        let playing = self.playing_channel().map(|c| c.key.clone());

        let mut area = egui::ScrollArea::both().id_salt("guide-grid").auto_shrink([false, false]);
        if std::mem::take(&mut self.guide_jump) {
            let row = playing.as_ref().and_then(|k| channels.iter().position(|&i| &catalog.channels[i].key == k)).unwrap_or(0);
            area = area.scroll_offset(vec2(x_of(now - 1800) - NAMES, (row as f32 - 2.0).max(0.0) * ROW));
        }
        area.show_viewport(ui, |ui, view| {
            let (_, _) = ui.allocate_exact_size(size, Sense::hover());
            let origin = ui.min_rect().min;
            let at = |x: f32, y: f32| origin + vec2(x, y);
            let left = origin.x + view.left();
            let top = origin.y + view.top();
            let first = ((view.top() - RULER) / ROW).floor().max(0.0) as usize;
            let last = (((view.bottom() - RULER) / ROW).ceil().max(0.0) as usize).min(channels.len());
            let painter = ui.painter().clone();

            // What's on, row by row; then the names and times kept in view over it.
            for (row, &index) in channels.iter().enumerate().take(last).skip(first) {
                let channel = &catalog.channels[index];
                let y = RULER + row as f32 * ROW;
                let programmes = guide.for_channel(channel);
                if programmes.is_empty() {
                    let r = Rect::from_min_size(at(view.left() + NAMES + 4.0, y + 4.0), vec2(view.width() - NAMES - 8.0, ROW - 8.0));
                    painter.text(
                        r.left_center() + vec2(12.0, 0.0),
                        Align2::LEFT_CENTER,
                        "No guide for this channel",
                        typography::secondary(),
                        p.faint,
                    );
                    continue;
                }
                for prog in programmes.iter().filter(|pr| x_of(pr.stop) > view.left() + NAMES && x_of(pr.start) < view.right()) {
                    let x0 = x_of(prog.start).max(view.left() + NAMES);
                    let r = Rect::from_min_max(at(x0 + 2.0, y + 3.0), at(x_of(prog.stop) - 2.0, y + ROW - 3.0));
                    if r.width() < 2.0 {
                        continue;
                    }
                    programme_cell(ui, r, channel, prog, now, acts);
                }
            }

            // The time now, as a line down the grid.
            let x_now = origin.x + x_of(now);
            if x_now > left + NAMES {
                painter.line_segment([pos2(x_now, top + RULER), pos2(x_now, origin.y + view.bottom())], Stroke::new(1.5, p.accent));
            }

            // Channel names.
            let names_bg = Rect::from_min_max(pos2(left, top), pos2(left + NAMES - 4.0, origin.y + view.bottom()));
            painter.rect_filled(names_bg, CornerRadius::ZERO, p.glass_strong);
            for (row, &index) in channels.iter().enumerate().take(last).skip(first) {
                let channel = &catalog.channels[index];
                let r = Rect::from_min_size(pos2(left, origin.y + RULER + row as f32 * ROW), vec2(NAMES - 8.0, ROW));
                if channel_name(ui, &mut self.logos, r, channel, playing.as_deref() == Some(channel.key.as_str())).clicked() {
                    acts.push(Act::PlayChannel(channel.key.clone()));
                }
            }

            // Times along the top.
            let ruler = Rect::from_min_max(pos2(left, top), pos2(origin.x + view.right(), top + RULER));
            painter.rect_filled(ruler, CornerRadius::ZERO, p.glass_strong);
            let mut t = t0 + ((view.left() / HALF_HOUR).floor() as i64) * 1800;
            while origin.x + x_of(t) < origin.x + view.right() {
                let x = origin.x + x_of(t);
                if x > left + NAMES - 2.0 {
                    painter.line_segment([pos2(x, top + RULER - 6.0), pos2(x, top + RULER)], Stroke::new(1.0, p.faint));
                    painter.text(pos2(x + 5.0, top + RULER / 2.0), Align2::LEFT_CENTER, super::bottom::hhmm(t), typography::meta(), p.dim);
                }
                t += 1800;
            }
            painter.text(pos2(left + 10.0, top + RULER / 2.0), Align2::LEFT_CENTER, day(t0 + BEFORE, now), typography::meta(), p.faint);
            if x_now > left + NAMES {
                painter.text(pos2(x_now, top + RULER / 2.0), Align2::CENTER_CENTER, "now", typography::meta(), p.accent);
            }
        });
    }
}

/// A channel in the column of names: its logo and name.
fn channel_name(ui: &mut egui::Ui, logos: &mut crate::ui::logos::Logos, r: Rect, channel: &Channel, on: bool) -> egui::Response {
    let p = colors();
    let resp = ui.interact(r, ui.scope_id().with(("guide-name", &channel.key)), Sense::click());
    if on || resp.hovered() {
        ui.painter().rect_filled(r.shrink2(vec2(2.0, 3.0)), CornerRadius::same(10), if on { p.hl_strong } else { p.hl });
    }
    let logo = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 15.0), vec2(30.0, 30.0));
    let tex = channel.logo.as_deref().and_then(|u| logos.get(u));
    widgets::logo_tile(ui.painter(), logo, &channel.name, tex.map(|t| t.0), tex.map(|t| t.1));
    let name = widgets::ellipsize(ui, &channel.name, typography::small_strong(), p.text, r.right() - logo.right() - 14.0);
    ui.painter().galley(pos2(logo.right() + 10.0, r.center().y - name.size().y / 2.0), name, p.text);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A programme in the grid: on now (plays the channel), from the archive (plays it),
/// or one that's over or still to come.
fn programme_cell(ui: &mut egui::Ui, r: Rect, channel: &Channel, prog: &Programme, now: i64, acts: &mut Vec<Act>) {
    let p = colors();
    let on = prog.is_on(now);
    let archived = prog.stop <= now && crate::library::catchup_url(channel, prog, None, 0).is_some();
    let resp = ui.interact(
        r,
        ui.scope_id().with(("guide-programme", &channel.key, prog.start)),
        if on || archived { Sense::click() } else { Sense::hover() },
    );
    let fill = if (on || archived) && resp.hovered() { p.hl_strong } else { p.hl };
    ui.painter().rect_filled(r, CornerRadius::same(8), fill);
    if on {
        ui.painter().rect_stroke(r, CornerRadius::same(8), Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
    }
    let faded = prog.stop <= now && !archived;
    let w = r.width() - 16.0;
    if w > 12.0 {
        let title = widgets::ellipsize(ui, &prog.title, typography::small_strong(), if faded { p.faint } else { p.text }, w);
        ui.painter().galley(pos2(r.left() + 8.0, r.top() + 7.0), title, p.text);
        let when = widgets::ellipsize(ui, &super::bottom::hhmm(prog.start), typography::meta(), if on { p.accent } else { p.faint }, w);
        let at = pos2(r.left() + 8.0, r.bottom() - 7.0 - when.size().y);
        let time = Rect::from_min_size(at, when.size());
        ui.painter().galley(at, when, p.faint);
        if archived && time.right() + 16.0 < r.right() {
            super::bottom::archive_mark(ui.painter(), time, p.faint);
        }
    }
    let resp = resp.on_hover_ui(|ui| {
        ui.set_max_width(340.0);
        ui.label(egui::RichText::new(&prog.title).font(typography::small_strong()).color(p.text));
        let mut when = format!("{}–{}", super::bottom::hhmm(prog.start), super::bottom::hhmm(prog.stop));
        if archived {
            when.push_str(" · click to watch from the archive");
        } else if on {
            when.push_str(" · on now");
        }
        ui.label(egui::RichText::new(when).font(typography::meta()).color(p.dim));
        if !prog.desc.is_empty() {
            ui.label(egui::RichText::new(&prog.desc).font(typography::secondary()).color(p.dim));
        }
    });
    if resp.clicked() {
        if on {
            acts.push(Act::PlayChannel(channel.key.clone()));
        } else if archived {
            acts.push(Act::PlayCatchup(channel.key.clone(), prog.clone()));
        }
    }
}

/// `Today`, `Yesterday`, `Tomorrow` or the date of `t`, as of `now`.
fn day(t: i64, now: i64) -> String {
    let local = |t: i64| t + super::bottom::local_offset(t);
    let (d, today) = (local(t).div_euclid(86_400), local(now).div_euclid(86_400));
    match d - today {
        0 => "Today".into(),
        -1 => "Yesterday".into(),
        1 => "Tomorrow".into(),
        _ => {
            let c = time::civil(local(t));
            format!("{}.{}.", c.day, c.month)
        }
    }
}
