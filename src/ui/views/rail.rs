//! The channel list down the left: search, the group, and what's on each channel.

use crate::iptv::time;
use crate::ui::app::{Act, App, Popup, Tab};
use crate::ui::icons::{self, Icon};
use crate::ui::load::Load;
use crate::ui::style::{colors, metrics, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};

impl App {
    pub fn rail(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let top = screen.top() + metrics::GAP - 4.0 + metrics::TOP_BAR + metrics::GAP;
        let bottom =
            if self.player.is_some() { screen.bottom() - metrics::BOTTOM - metrics::GAP * 2.0 } else { screen.bottom() - metrics::GAP };
        let rect =
            Rect::from_min_max(pos2(screen.left() + metrics::GAP, top), pos2(screen.left() + metrics::GAP + metrics::RAIL_WIDTH, bottom));
        if rect.height() < 120.0 {
            return;
        }
        egui::Area::new(egui::Id::unique("rail")).order(egui::Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
            widgets::glass(metrics::PANEL_RADIUS).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                let inner = rect.size() - vec2(20.0, 20.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                self.rail_header(ui, inner.x, acts);
                ui.add_space(6.0);
                self.rail_list(ui, acts);
            });
        });
    }

    fn rail_header(&mut self, ui: &mut egui::Ui, width: f32, acts: &mut Vec<Act>) {
        let p = colors();
        let resp = widgets::search_field(ui, &mut self.search, width);
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if resp.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(&first) = self.list.first()
        {
            acts.push(self.play_act(first));
        }
        if !matches!(self.tab, Tab::Live | Tab::Movies) {
            ui.add_space(2.0);
            let count = self.list.len();
            ui.label(egui::RichText::new(format!("{} · {count}", self.tab.label().to_uppercase())).font(typography::meta()).color(p.faint));
            return;
        }
        ui.add_space(4.0);
        let group = if self.tab == Tab::Movies { self.movie_group.clone() } else { self.group.clone() };
        let label = group.unwrap_or_else(|| if self.tab == Tab::Movies { "All movies".into() } else { "All channels".into() });
        let (r, click) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
        if click.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(9), p.hl);
        }
        let text = widgets::ellipsize(ui, &label, typography::small_strong(), p.text, width - 80.0);
        ui.painter().galley(pos2(r.left() + 8.0, r.center().y - text.size().y / 2.0), text, p.text);
        ui.painter().text(
            pos2(r.right() - 26.0, r.center().y),
            Align2::RIGHT_CENTER,
            self.list.len().to_string(),
            typography::meta(),
            p.faint,
        );
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 12.0, r.center().y), vec2(11.0, 11.0)), Icon::Chevrons, p.dim);
        self.anchor(Popup::Groups, r);
        if click.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            acts.push(Act::Popup(Some(Popup::Groups)));
        }
    }

    pub fn play_act(&self, index: usize) -> Act {
        let Some(c) = self.catalog.ready() else { return Act::Toast(String::new()) };
        if self.tab == Tab::Movies { Act::PlayMovie(c.movies[index].key.clone()) } else { Act::PlayChannel(c.channels[index].key.clone()) }
    }

    fn rail_list(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        match &self.catalog {
            Load::Loading(_) | Load::Idle => {
                let c = ui.available_rect_before_wrap().center();
                widgets::spinner(ui, c, 12.0, p.accent);
                return;
            }
            Load::Failed(e) => {
                ui.add_space(12.0);
                ui.label(egui::RichText::new(format!("The channels couldn't be loaded: {e}")).font(typography::body()).color(p.danger));
                return;
            }
            Load::Ready(_) => {}
        }
        if self.list.is_empty() {
            ui.add_space(16.0);
            let text = match self.tab {
                Tab::Favorites => "No favourites yet. Right-click a channel, or press the star while it plays.",
                Tab::Recent => "Channels you watch show up here.",
                Tab::Movies if self.catalog.ready().is_some_and(|c| c.movies.is_empty()) => "This source has no movies.",
                _ => "Nothing matches.",
            };
            ui.label(egui::RichText::new(text).font(typography::body()).color(p.dim));
            return;
        }
        let playing = self.playing.as_ref().map(|pl| pl.key().to_string());
        let list = std::mem::take(&mut self.list);
        let movies = self.tab == Tab::Movies;
        let area = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("rail", self.tab, self.group.clone()));
        area.show_rows(ui, metrics::CHANNEL_ROW, list.len(), |ui, range| {
            for &index in &list[range] {
                if movies {
                    self.movie_row(ui, index, playing.as_deref(), acts);
                } else {
                    self.channel_row(ui, index, playing.as_deref(), acts);
                }
            }
        });
        self.list = list;
    }

    fn channel_row(&mut self, ui: &mut egui::Ui, index: usize, playing: Option<&str>, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(catalog) = self.catalog.ready() else { return };
        let channel = &catalog.channels[index];
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), metrics::CHANNEL_ROW), Sense::click());
        let on = playing == Some(channel.key.as_str());
        if on || resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(11), if on { p.hl_strong } else { p.hl });
        }
        let mut x = rect.left() + 6.0;
        let number = channel.number.map(|n| n.to_string()).unwrap_or_default();
        ui.painter().text(pos2(x + 24.0, rect.center().y), Align2::RIGHT_CENTER, number, typography::meta(), p.faint);
        x += 32.0;
        let logo_rect = Rect::from_min_size(pos2(x, rect.center().y - metrics::LOGO / 2.0), vec2(metrics::LOGO, metrics::LOGO));
        let logo = channel.logo.clone();
        let name = channel.name.clone();
        let key = channel.key.clone();
        let favorite = self.marks.is_favorite(&self.source_id(), &key);
        let now = self.now_on(channel).cloned();
        let tex = logo.as_deref().and_then(|u| self.logos.get(u));
        widgets::logo_tile(ui.painter(), logo_rect, &name, tex.map(|t| t.0), tex.map(|t| t.1));
        x += metrics::LOGO + 11.0;
        let text_w = rect.right() - x - 34.0;
        let title = widgets::ellipsize(ui, &name, typography::item(), p.text, text_w);
        let has_sub = now.is_some();
        let ty = if has_sub { rect.center().y - title.size().y + 1.0 } else { rect.center().y - title.size().y / 2.0 };
        ui.painter().galley(pos2(x, ty), title, p.text);
        if let Some(prog) = &now {
            let sub = widgets::ellipsize(ui, &prog.title, typography::secondary(), p.dim, text_w);
            ui.painter().galley(pos2(x, rect.center().y + 2.0), sub, p.dim);
        }
        let right = pos2(rect.right() - 16.0, rect.center().y);
        if favorite {
            icons::paint(ui.painter(), Rect::from_center_size(right, vec2(13.0, 13.0)), Icon::Star, p.accent);
        } else if let Some(prog) = &now {
            // How far the programme is: a ring.
            let frac = prog.progress(time::now());
            ui.painter().circle_stroke(right, 8.0, Stroke::new(2.5, p.hl_strong));
            let pts: Vec<_> = (0..=24)
                .map(|i| -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU * i as f32 / 24.0)
                .map(|a| right + vec2(a.cos(), a.sin()) * 8.0)
                .collect();
            ui.painter().add(egui::Shape::line(pts, Stroke::new(2.5, p.accent)));
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            acts.push(Act::PlayChannel(key.clone()));
        }
        resp.context_menu(|ui| {
            if ui.button(if favorite { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                acts.push(Act::ToggleFavorite(key.clone()));
                ui.close();
            }
        });
    }

    fn movie_row(&mut self, ui: &mut egui::Ui, index: usize, playing: Option<&str>, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(catalog) = self.catalog.ready() else { return };
        let movie = catalog.movies[index].clone();
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), metrics::CHANNEL_ROW), Sense::click());
        let on = playing == Some(movie.key.as_str());
        if on || resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(11), if on { p.hl_strong } else { p.hl });
        }
        let poster = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 5.0), vec2(30.0, metrics::CHANNEL_ROW - 10.0));
        let tex = movie.poster.as_deref().and_then(|u| self.logos.get(u));
        match tex {
            Some((t, _)) => {
                ui.painter().image(t, poster, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None => widgets::logo_tile(ui.painter(), poster, &movie.name, None, None),
        }
        let x = poster.right() + 12.0;
        let w = rect.right() - x - 8.0;
        let title = widgets::ellipsize(ui, &movie.name, typography::item(), p.text, w);
        ui.painter().galley(pos2(x, rect.center().y - title.size().y + 1.0), title, p.text);
        let meta: Vec<String> = [movie.year.map(|y| y.to_string()), movie.rating.map(|r| format!("★ {r:.1}")), Some(movie.group.clone())]
            .into_iter()
            .flatten()
            .collect();
        let sub = widgets::ellipsize(ui, &meta.join(" · "), typography::secondary(), p.dim, w);
        ui.painter().galley(pos2(x, rect.center().y + 2.0), sub, p.dim);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            acts.push(Act::PlayMovie(movie.key.clone()));
        }
    }
}
