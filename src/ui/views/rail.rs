//! The channel list down the left: search, the group, and what's on each channel.

use crate::iptv::time;
use crate::iptv::model::Shelf;
use crate::ui::app::{Act, App, Tab};
use crate::ui::icons::{self, Icon};
use crate::ui::load::Load;
use crate::ui::style::{colors, metrics, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};

impl App {
    /// Whether the column of groups shows beside the channels.
    pub fn shows_groups(&self) -> bool {
        self.groups_open && matches!(self.tab, Tab::Live | Tab::Movies | Tab::Series) && self.browse_counts().len() > 1
    }

    /// How wide the rail is now.
    pub fn rail_width(&self) -> f32 {
        metrics::RAIL_WIDTH + if self.shows_groups() { metrics::GROUPS_WIDTH } else { 0.0 }
    }

    pub fn rail(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let top = screen.top() + metrics::GAP - 4.0 + metrics::TOP_BAR + metrics::GAP;
        let bottom =
            if self.player.is_some() { screen.bottom() - metrics::BOTTOM - metrics::GAP * 2.0 } else { screen.bottom() - metrics::GAP };
        let width = self.rail_width().min(screen.width() - metrics::GAP * 2.0);
        let rect = Rect::from_min_max(pos2(screen.left() + metrics::GAP, top), pos2(screen.left() + metrics::GAP + width, bottom));
        if rect.height() < 120.0 {
            return;
        }
        egui::Area::new(egui::Id::unique("rail")).order(egui::Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
            widgets::glass(metrics::PANEL_RADIUS).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                let inner = rect.size() - vec2(20.0, 20.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if self.shows_groups() {
                        let w = metrics::GROUPS_WIDTH - 10.0;
                        ui.allocate_ui_with_layout(vec2(w, inner.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.set_min_size(vec2(w, inner.y));
                            self.groups_column(ui, w, acts);
                        });
                        // A hairline between the columns.
                        let (line, _) = ui.allocate_exact_size(vec2(10.0, inner.y), Sense::hover());
                        ui.painter().vline(line.center().x - 1.0, line.y_range(), Stroke::new(1.0, colors().line));
                    }
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(vec2(w, inner.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_min_size(vec2(w, inner.y));
                        self.rail_header(ui, w, acts);
                        ui.add_space(6.0);
                        self.rail_list(ui, acts);
                    });
                });
            });
        });
    }

    /// The groups: a filter when there are many, then every group with its count.
    fn groups_column(&mut self, ui: &mut egui::Ui, width: f32, acts: &mut Vec<Act>) {
        let p = colors();
        let library = matches!(self.tab, Tab::Movies | Tab::Series);
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.add_space(6.0);
            ui.label(egui::RichText::new(if library { "CATEGORIES" } else { "GROUPS" }).font(typography::meta()).color(p.faint));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Close, 26.0, p.faint, "Hide the groups").clicked() {
                    acts.push(Act::ToggleGroups);
                }
            });
        });
        if self.browse_counts().len() > 12 {
            widgets::filter_field(ui, &mut self.group_filter, "Filter groups", width);
            ui.add_space(4.0);
        }
        let current = self.browse_group().clone();
        let needle = self.group_filter.trim().to_lowercase();
        let counts = self.browse_counts();
        let total: usize = counts.iter().map(|(_, n)| n).sum();
        let shown: Vec<&(String, usize)> = counts.iter().filter(|(g, _)| needle.is_empty() || g.to_lowercase().contains(&needle)).collect();
        let all = all_label(self.tab);
        let mut chosen = None;
        // Playlists' groups can be moved between channels, movies and series.
        let movable = self.source().is_some_and(|s| s.login().is_none());
        let here = match self.tab {
            Tab::Movies => Shelf::Movies,
            Tab::Series => Shelf::Series,
            _ => Shelf::Channels,
        };
        let moved = |g: &str| self.source().is_some_and(|s| s.shelves.contains_key(g));
        egui::ScrollArea::vertical().id_salt(("groups", self.tab)).auto_shrink([false, false]).show_rows(
            ui,
            metrics::GROUP_ROW,
            shown.len() + 1,
            |ui, range| {
                for i in range {
                    let (name, count, on) = match i {
                        0 => (all, total, current.is_none()),
                        _ => (shown[i - 1].0.as_str(), shown[i - 1].1, current.as_deref() == Some(shown[i - 1].0.as_str())),
                    };
                    let row = group_row(ui, name, count, on);
                    if row.clicked() {
                        chosen = Some(if i == 0 { None } else { Some(name.to_string()) });
                    }
                    if i > 0 && movable {
                        row.context_menu(|ui| {
                            for (shelf, label) in
                                [(Shelf::Channels, "Show as channels"), (Shelf::Movies, "Show as movies"), (Shelf::Series, "Show as series")]
                            {
                                if shelf != here && ui.button(label).clicked() {
                                    acts.push(Act::Shelve(name.to_string(), Some(shelf)));
                                    ui.close();
                                }
                            }
                            if moved(name) && ui.button("Sort automatically").clicked() {
                                acts.push(Act::Shelve(name.to_string(), None));
                                ui.close();
                            }
                        });
                    }
                }
            },
        );
        if let Some(g) = chosen {
            acts.push(Act::Group(g));
        }
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
        ui.add_space(4.0);
        if let Some(i) = self.open_series {
            self.series_header(ui, width, i, acts);
            return;
        }
        // What's listed: the group (click to show the column of groups) and how many.
        let browsing = matches!(self.tab, Tab::Live | Tab::Movies | Tab::Series);
        let label = match (browsing, self.browse_group().clone()) {
            (true, Some(g)) => g,
            (true, None) => all_label(self.tab).into(),
            (false, _) => self.tab.label().into(),
        };
        let can_open = browsing && !self.shows_groups() && self.browse_counts().len() > 1;
        let (r, click) = ui.allocate_exact_size(vec2(width, 30.0), if can_open { Sense::click() } else { Sense::hover() });
        if can_open && click.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(9), p.hl);
        }
        let mut x = r.left() + 8.0;
        if can_open {
            icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 6.0, r.center().y), vec2(13.0, 13.0)), Icon::List, p.dim);
            x += 20.0;
        }
        let text = widgets::ellipsize(ui, &label, typography::small_strong(), p.text, r.right() - x - 60.0);
        ui.painter().galley(pos2(x, r.center().y - text.size().y / 2.0), text, p.text);
        ui.painter().text(
            pos2(r.right() - 8.0, r.center().y),
            Align2::RIGHT_CENTER,
            self.list.len().to_string(),
            typography::meta(),
            p.faint,
        );
        if can_open && click.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show the groups").clicked() {
            acts.push(Act::ToggleGroups);
        }
    }

    /// The way back to the series, then the open one's name and how many episodes.
    fn series_header(&self, ui: &mut egui::Ui, width: f32, index: usize, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(series) = self.catalog.ready().and_then(|c| c.series.get(index)) else { return };
        let (r, click) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
        if click.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(9), p.hl);
        }
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(12.0, 12.0)), Icon::Prev, p.dim);
        let x = r.left() + 28.0;
        let text = widgets::ellipsize(ui, &series.name, typography::small_strong(), p.text, r.right() - x - 60.0);
        ui.painter().galley(pos2(x, r.center().y - text.size().y / 2.0), text, p.text);
        if !self.episodes.is_loading() {
            let n = self.list.len();
            let count = if n == 1 { "1 episode".to_string() } else { format!("{n} episodes") };
            ui.painter().text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, count, typography::meta(), p.faint);
        }
        if click.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Back to the series").clicked() {
            acts.push(Act::OpenSeries(None));
        }
    }

    pub fn play_act(&self, index: usize) -> Act {
        let Some(c) = self.catalog.ready() else { return Act::Toast(String::new()) };
        match self.tab {
            Tab::Movies => Act::PlayMovie(c.movies[index].key.clone()),
            Tab::Series if self.open_series.is_some() => Act::PlayEpisode(index),
            Tab::Series => Act::OpenSeries(Some(index)),
            _ => Act::PlayChannel(c.channels[index].key.clone()),
        }
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
        if self.open_series.is_some() {
            match &self.episodes {
                Load::Loading(_) | Load::Idle => {
                    let c = ui.available_rect_before_wrap().center();
                    widgets::spinner(ui, c, 12.0, p.accent);
                    return;
                }
                Load::Failed(e) => {
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(format!("The episodes couldn't be loaded: {e}")).font(typography::body()).color(p.danger));
                    return;
                }
                Load::Ready(_) => {}
            }
        }
        if self.list.is_empty() {
            ui.add_space(16.0);
            let text = match self.tab {
                Tab::Favorites => "No favourites yet. Right-click a channel, or press the star while it plays.",
                Tab::Recent => "Channels you watch show up here.",
                Tab::Movies if self.catalog.ready().is_some_and(|c| c.movies.is_empty()) => "This source has no movies.",
                Tab::Series if self.catalog.ready().is_some_and(|c| c.series.is_empty()) => "This source has no series.",
                _ => "Nothing matches.",
            };
            ui.label(egui::RichText::new(text).font(typography::body()).color(p.dim));
            return;
        }
        let playing = self.playing.as_ref().map(|pl| pl.key().to_string());
        let list = std::mem::take(&mut self.list);
        let salt = ("rail", self.tab, self.browse_group().clone(), self.open_series);
        let area = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(salt);
        area.show_rows(ui, metrics::CHANNEL_ROW, list.len(), |ui, range| {
            for &index in &list[range] {
                match self.tab {
                    Tab::Movies => self.movie_row(ui, index, playing.as_deref(), acts),
                    Tab::Series if self.open_series.is_some() => self.episode_row(ui, index, playing.as_deref(), acts),
                    Tab::Series => self.series_row(ui, index, acts),
                    _ => self.channel_row(ui, index, playing.as_deref(), acts),
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
        let Some(catalog) = self.catalog.ready() else { return };
        let movie = catalog.movies[index].clone();
        let meta = [movie.year.map(|y| y.to_string()), movie.rating.map(|r| format!("★ {r:.1}")), Some(movie.group.clone())];
        let on = playing == Some(movie.key.as_str());
        // The year shows below; `Dune (2021)` reads as `Dune`.
        let name = movie.year.and_then(|y| movie.name.strip_suffix(&format!(" ({y})"))).unwrap_or(&movie.name);
        if self.poster_row(ui, name, movie.poster.as_deref(), &meta, on).clicked() {
            acts.push(Act::PlayMovie(movie.key.clone()));
        }
    }

    fn series_row(&mut self, ui: &mut egui::Ui, index: usize, acts: &mut Vec<Act>) {
        let Some(catalog) = self.catalog.ready() else { return };
        let series = catalog.series[index].clone();
        let episodes = match series.episodes.len() {
            0 => None,
            1 => Some("1 episode".to_string()),
            n => Some(format!("{n} episodes")),
        };
        let meta = [series.year.map(|y| y.to_string()), series.rating.map(|r| format!("★ {r:.1}")), episodes, Some(series.group.clone())];
        if self.poster_row(ui, &series.name, series.poster.as_deref(), &meta, false).clicked() {
            acts.push(Act::OpenSeries(Some(index)));
        }
    }

    /// A movie or series: its poster, name, and a line of what's known about it.
    fn poster_row(&mut self, ui: &mut egui::Ui, name: &str, poster: Option<&str>, meta: &[Option<String>], on: bool) -> egui::Response {
        let p = colors();
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), metrics::CHANNEL_ROW), Sense::click());
        if on || resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(11), if on { p.hl_strong } else { p.hl });
        }
        let frame = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 5.0), vec2(30.0, metrics::CHANNEL_ROW - 10.0));
        match poster.and_then(|u| self.logos.get(u)) {
            Some((t, _)) => {
                ui.painter().image(t, frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None => widgets::logo_tile(ui.painter(), frame, name, None, None),
        }
        let x = frame.right() + 12.0;
        let w = rect.right() - x - 8.0;
        let title = widgets::ellipsize(ui, name, typography::item(), p.text, w);
        ui.painter().galley(pos2(x, rect.center().y - title.size().y + 1.0), title, p.text);
        let meta: Vec<&str> = meta.iter().flatten().map(String::as_str).collect();
        let sub = widgets::ellipsize(ui, &meta.join(" · "), typography::secondary(), p.dim, w);
        ui.painter().galley(pos2(x, rect.center().y + 2.0), sub, p.dim);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn episode_row(&mut self, ui: &mut egui::Ui, index: usize, playing: Option<&str>, acts: &mut Vec<Act>) {
        let p = colors();
        let Some(episode) = self.episodes.ready().and_then(|l| l.get(index)) else { return };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), metrics::CHANNEL_ROW), Sense::click());
        let on = playing == Some(episode.key.as_str());
        if on || resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(11), if on { p.hl_strong } else { p.hl });
        }
        let x = rect.left() + 14.0;
        let w = rect.right() - x - 8.0;
        let title = if episode.title.is_empty() { format!("Episode {}", episode.number) } else { episode.title.clone() };
        let title = widgets::ellipsize(ui, &title, typography::item(), p.text, w);
        ui.painter().galley(pos2(x, rect.center().y - title.size().y + 1.0), title, p.text);
        let code = format!("Season {} · Episode {}", episode.season, episode.number);
        let sub = widgets::ellipsize(ui, &code, typography::secondary(), if on { p.accent } else { p.dim }, w);
        ui.painter().galley(pos2(x, rect.center().y + 2.0), sub, p.dim);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            acts.push(Act::PlayEpisode(index));
        }
    }
}

/// The first row of the groups: everything the tab has.
fn all_label(tab: Tab) -> &'static str {
    match tab {
        Tab::Movies => "All movies",
        Tab::Series => "All series",
        _ => "All channels",
    }
}

fn group_row(ui: &mut egui::Ui, title: &str, count: usize, on: bool) -> egui::Response {
    let p = colors();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), metrics::GROUP_ROW), Sense::click());
    if on || resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(8), if on { p.hl_strong } else { p.hl });
    }
    if on {
        let bar = Rect::from_min_size(pos2(r.left(), r.center().y - 8.0), vec2(3.0, 16.0));
        ui.painter().rect_filled(bar, CornerRadius::same(2), p.accent);
    }
    let t = widgets::ellipsize(ui, title, typography::small_strong(), if on { p.text } else { p.dim }, r.width() - 64.0);
    ui.painter().galley(pos2(r.left() + 12.0, r.center().y - t.size().y / 2.0), t, p.text);
    ui.painter().text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, count.to_string(), typography::meta(), p.faint);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
