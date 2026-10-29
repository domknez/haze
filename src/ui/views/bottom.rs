//! The panel along the bottom: what's on, the controls, the programme's progress
//! and what comes next (or, for a recording, a seek bar).

use crate::iptv::model::Programme;
use crate::iptv::time;
use crate::media::Status;
use crate::ui::app::{Act, App, Playing, Popup};
use crate::ui::icons::Icon;
use crate::ui::style::{colors, metrics, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};

/// Programmes shown in the strip.
const STRIP: usize = 5;

impl App {
    pub fn bottom(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        if self.player.is_none() {
            return;
        }
        let rect = Rect::from_min_max(
            pos2(screen.left() + metrics::GAP, screen.bottom() - metrics::GAP - metrics::BOTTOM),
            pos2(screen.right() - metrics::GAP, screen.bottom() - metrics::GAP),
        );
        egui::Area::new(egui::Id::unique("bottom")).order(egui::Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
            widgets::glass(18).inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
                let inner = rect.size() - vec2(36.0, 28.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                self.now_line(ui, acts);
                ui.add_space(10.0);
                match self.playing.clone() {
                    Some(Playing::Live { key }) => self.live_progress(ui, &key, acts),
                    Some(_) => self.seek_bar(ui, acts),
                    None => {}
                }
            });
        });
    }

    fn now_line(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        let live = matches!(self.playing, Some(Playing::Live { .. }));
        let channel = self.playing_channel().cloned();
        let name = self.playing_name();
        let (title, eyebrow) = match &self.playing {
            Some(Playing::Catchup { programme, .. }) => {
                (programme.title.clone(), format!("{name} · from the archive · {}", clock_range(programme)))
            }
            Some(Playing::Movie { .. }) => (name.clone(), "Movie".to_string()),
            _ => {
                let now = channel.as_ref().and_then(|c| self.now_on(c)).cloned();
                let group = channel.as_ref().map(|c| c.group.clone()).unwrap_or_default();
                (now.map_or_else(|| name.clone(), |n| n.title), if group.is_empty() { name.clone() } else { format!("{name} · {group}") })
            }
        };
        ui.horizontal(|ui| {
            let (logo_rect, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
            let logo = channel.as_ref().and_then(|c| c.logo.as_deref()).and_then(|u| self.logos.get(u));
            widgets::logo_tile(ui.painter(), logo_rect, &name, logo.map(|l| l.0), logo.map(|l| l.1));
            ui.add_space(6.0);
            let controls_w = 420.0;
            let text_w = (ui.available_width() - controls_w).max(120.0);
            ui.vertical(|ui| {
                ui.set_width(text_w);
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(if live { 44.0 } else { 0.0 }, 18.0), Sense::hover());
                    if live {
                        widgets::tag(ui.painter(), r.left_center(), "LIVE", p.accent, p.on_accent);
                    }
                    let g = widgets::ellipsize(ui, &eyebrow, typography::secondary(), p.dim, text_w - 50.0);
                    ui.label(egui::WidgetText::Galley(g));
                });
                let g = widgets::ellipsize(ui, &title, typography::now_title(), p.text, text_w);
                ui.label(egui::WidgetText::Galley(g));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.controls(ui, live, channel.as_ref().map(|c| c.key.clone()), acts)
            });
        });
    }

    fn controls(&mut self, ui: &mut egui::Ui, live: bool, key: Option<String>, acts: &mut Vec<Act>) {
        let p = colors();
        let full = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if widgets::icon_button(ui, if full { Icon::Windowed } else { Icon::Fullscreen }, 34.0, p.dim, "Full screen (F)").clicked() {
            acts.push(Act::ToggleFullscreen);
        }
        // Volume: a slider and the mute button.
        let mut volume = if self.prefs.muted { 0.0 } else { self.prefs.volume };
        let (r, resp) = ui.allocate_exact_size(vec2(84.0, 30.0), Sense::click_and_drag());
        let track = Rect::from_center_size(r.center(), vec2(r.width() - 8.0, 4.0));
        widgets::progress(ui.painter(), track, volume, p.text, p.hl_strong);
        ui.painter().circle_filled(
            pos2(track.left() + track.width() * volume, track.center().y),
            if resp.hovered() || resp.dragged() { 6.0 } else { 0.0 },
            p.text,
        );
        if let Some(pos) = resp.interact_pointer_pos()
            && (resp.dragged() || resp.clicked())
        {
            volume = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            acts.push(Act::Volume(volume));
        }
        let vol_icon = if self.prefs.muted || volume == 0.0 { Icon::Mute } else { Icon::Volume };
        if widgets::icon_button(ui, vol_icon, 34.0, p.dim, "Mute (M)").clicked() {
            acts.push(Act::ToggleMute);
        }
        ui.add_space(6.0);
        if let Some(key) = &key {
            let fav = self.marks.is_favorite(&self.source_id(), key);
            let star = widgets::icon_button(
                ui,
                if fav { Icon::Star } else { Icon::StarOutline },
                34.0,
                if fav { p.accent } else { p.dim },
                "Favorite",
            );
            if star.clicked() {
                acts.push(Act::ToggleFavorite(key.clone()));
            }
        }
        if widgets::icon_button(ui, Icon::Next, 34.0, p.text, if live { "Next channel (↓)" } else { "Next (↓)" }).clicked() {
            acts.push(Act::Step(1));
        }
        let paused = self.player.as_ref().is_some_and(|pl| pl.paused());
        if widgets::round_button(ui, if paused { Icon::Play } else { Icon::Pause }, 44.0, p.text, p.glass_strong.to_opaque()).clicked() {
            acts.push(Act::TogglePause);
        }
        if widgets::icon_button(ui, Icon::Prev, 34.0, p.text, if live { "Previous channel (↑)" } else { "Previous (↑)" }).clicked() {
            acts.push(Act::Step(-1));
        }
        ui.add_space(6.0);
        // Back to live, or watch the programme on now from its start.
        match &self.playing {
            Some(Playing::Catchup { .. }) => {
                if widgets::pill(ui, None, "Go live", true).clicked() {
                    acts.push(Act::GoLive);
                }
            }
            Some(Playing::Live { key }) => {
                let channel = self.channel(key);
                let start = channel.and_then(|c| self.now_on(c).cloned().filter(|_| c.catchup.is_some()));
                if let Some(prog) = start
                    && widgets::icon_button(ui, Icon::FromStart, 34.0, p.dim, "Watch from the start").clicked()
                {
                    acts.push(Act::PlayCatchup(key.clone(), prog));
                }
            }
            _ => {}
        }
        let tracks = self.player.as_ref().map(|pl| pl.info().audio_tracks.len()).unwrap_or(0);
        if tracks > 1 {
            let r = widgets::icon_button(ui, Icon::Audio, 34.0, p.dim, "Audio track");
            self.anchor(Popup::AudioTracks, r.rect);
            if r.clicked() {
                acts.push(Act::Popup(Some(Popup::AudioTracks)));
            }
        }
    }

    fn live_progress(&mut self, ui: &mut egui::Ui, key: &str, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(channel) = self.channel(key).cloned() else { return };
        let now = time::now();
        let list: Vec<Programme> = self.guide.ready().map(|g| g.for_channel(&channel).to_vec()).unwrap_or_default();
        let current = list.iter().position(|pr| pr.is_on(now));
        // Progress through the programme on now.
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        match current.map(|i| &list[i]) {
            Some(prog) => {
                let left = ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, hhmm(prog.start), typography::meta(), p.faint);
                let right = ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, hhmm(prog.stop), typography::meta(), p.faint);
                let track =
                    Rect::from_min_max(pos2(left.right() + 12.0, rect.center().y - 2.5), pos2(right.left() - 12.0, rect.center().y + 2.5));
                widgets::progress(ui.painter(), track, prog.progress(now), p.accent, p.hl_strong);
            }
            None => {
                let text = if self.guide.is_loading() { "Loading the guide…" } else { "No guide for this channel" };
                ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text, typography::meta(), p.faint);
                self.stream_info(ui, rect);
                return;
            }
        }
        ui.add_space(10.0);
        // The strip: the one before, the one on, and what's next.
        let start = current.map_or(0, |i| i.saturating_sub(1));
        let shown: Vec<(usize, Programme)> = list.iter().cloned().enumerate().skip(start).take(STRIP).collect();
        if shown.is_empty() {
            return;
        }
        let gap = 8.0;
        let w = (ui.available_width() - gap * (STRIP as f32 - 1.0)) / STRIP as f32;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, prog) in shown {
                let on = Some(i) == current;
                let archived = prog.stop <= now && crate::library::catchup_url(&channel, &prog, None, 0).is_some();
                let (r, resp) = ui.allocate_exact_size(vec2(w, 40.0), if archived { Sense::click() } else { Sense::hover() });
                let fill = if resp.hovered() && archived { p.hl_strong } else { p.hl };
                ui.painter().rect_filled(r, CornerRadius::same(10), fill);
                if on {
                    ui.painter().rect_stroke(r, CornerRadius::same(10), Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
                }
                let faded = prog.stop <= now && !archived;
                let tm = if on {
                    format!("{} · now", hhmm(prog.start))
                } else if archived {
                    format!("{} · ↺", hhmm(prog.start))
                } else {
                    hhmm(prog.start)
                };
                let tc = if on { p.accent } else { p.faint };
                ui.painter().text(pos2(r.left() + 10.0, r.top() + 12.0), Align2::LEFT_CENTER, tm, typography::meta(), tc);
                let title = widgets::ellipsize(ui, &prog.title, typography::small_strong(), if faded { p.faint } else { p.text }, w - 20.0);
                ui.painter().galley(pos2(r.left() + 10.0, r.top() + 20.0), title, p.text);
                let resp = if archived {
                    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Watch from the archive")
                } else {
                    resp
                };
                if resp.clicked() {
                    acts.push(Act::PlayCatchup(channel.key.clone(), prog.clone()));
                }
            }
        });
    }

    /// Position and length of a recording, with a bar to seek on.
    fn seek_bar(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(player) = &self.player else { return };
        let info = player.info();
        let start = self.start_pts.unwrap_or(0.0);
        let position = (player.position().unwrap_or(start) - start).max(0.0);
        let duration = match &self.playing {
            Some(Playing::Catchup { programme, .. }) => Some((programme.stop - programme.start) as f64),
            _ => info.duration,
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let left = ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, duration_text(position), typography::meta(), p.faint);
        let right = ui.painter().text(
            rect.right_center(),
            Align2::RIGHT_CENTER,
            duration.map(duration_text).unwrap_or_default(),
            typography::meta(),
            p.faint,
        );
        let track = Rect::from_min_max(pos2(left.right() + 12.0, rect.center().y - 2.5), pos2(right.left() - 12.0, rect.center().y + 2.5));
        let frac = duration.map_or(0.0, |d| (position / d.max(1.0)) as f32);
        widgets::progress(ui.painter(), track, frac, p.accent, p.hl_strong);
        if resp.hovered() {
            ui.painter().circle_filled(pos2(track.left() + track.width() * frac.clamp(0.0, 1.0), track.center().y), 6.0, p.accent);
        }
        if let (Some(pos), Some(d)) = (resp.interact_pointer_pos(), duration)
            && resp.clicked()
        {
            let target = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * d;
            acts.push(Act::Seek(target - position));
        }
        ui.add_space(8.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        self.stream_info(ui, r);
    }

    /// Codec and size, quietly, at the right.
    fn stream_info(&self, ui: &egui::Ui, rect: Rect) {
        let Some(player) = &self.player else { return };
        if player.status() != Status::Playing {
            return;
        }
        let i = player.info();
        let mut parts = Vec::new();
        if i.height > 0 {
            parts.push(format!("{}p{}", i.height, i.fps.map(|f| format!("{}", f.round() as u32)).unwrap_or_default()));
        }
        parts.extend(i.video_codec.clone());
        if let Some(a) = &i.audio_codec {
            parts.push(format!(
                "{a} {}",
                match i.audio_channels {
                    1 => "mono".into(),
                    2 => "stereo".into(),
                    n => format!("{n}ch"),
                }
            ));
        }
        let p = colors();
        ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, parts.join(" · "), typography::meta(), p.faint);
    }
}

pub fn hhmm(t: i64) -> String {
    let c = time::civil(t + local_offset(t));
    format!("{:02}:{:02}", c.hour, c.minute)
}

fn clock_range(p: &Programme) -> String {
    format!("{}–{}", hhmm(p.start), hhmm(p.stop))
}

fn duration_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// The computer's time zone at `t`, seconds east of UTC (for showing guide times).
fn local_offset(t: i64) -> i64 {
    #[cfg(unix)]
    {
        // SAFETY: localtime_r only writes into the tm we pass.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&(t as libc::time_t), &mut tm).is_null() {
                return 0;
            }
            tm.tm_gmtoff as i64
        }
    }
    #[cfg(not(unix))]
    {
        let _ = t;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::duration_text;

    #[test]
    fn formats_durations() {
        assert_eq!(duration_text(5.0), "0:05");
        assert_eq!(duration_text(3725.0), "1:02:05");
    }
}
