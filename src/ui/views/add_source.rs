//! Adding or editing a source: a playlist link, a playlist file, an Xtream Codes
//! login, or one of the public lists of free channels. The source is checked (its
//! playlist loaded) before it is saved.

use crate::library::{self, Checked, Free, Kind, Source};
use crate::ui::icons::Icon;
use crate::ui::load::Load;
use crate::ui::style::{colors, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Link,
    File,
    Xtream,
    Free,
}

/// Picking a list of free channels.
#[derive(Default)]
struct FreePick {
    by_topic: bool,
    filter: String,
    picked: Option<Free>,
    /// The computer's country, once looked up.
    home: Option<Option<Free>>,
}

pub struct SourceForm {
    /// The source being edited (its id), or none for a new one.
    editing: Option<String>,
    /// The edited source's groups put elsewhere or hidden, kept as they were.
    shelves: std::collections::BTreeMap<String, crate::iptv::model::Shelf>,
    hidden: std::collections::BTreeSet<String>,
    mode: Mode,
    name: String,
    url: String,
    path: String,
    epg: String,
    server: String,
    user: String,
    password: String,
    check: Load<Checked>,
    free: FreePick,
}

const WIDTH: f32 = 520.0;

impl SourceForm {
    pub fn new() -> SourceForm {
        SourceForm {
            editing: None,
            shelves: Default::default(),
            hidden: Default::default(),
            mode: Mode::Link,
            name: String::new(),
            url: String::new(),
            path: String::new(),
            epg: String::new(),
            server: String::new(),
            user: String::new(),
            password: String::new(),
            check: Load::Idle,
            free: FreePick::default(),
        }
    }

    /// A new source from the free lists, the computer's country picked if it has one.
    pub fn free() -> SourceForm {
        let mut f = SourceForm { mode: Mode::Free, ..SourceForm::new() };
        if let Some(home) = f.free.home() {
            f.pick_free(home);
        }
        f
    }

    /// Pick `list`, naming the source after it.
    fn pick_free(&mut self, list: Free) {
        // The name follows the pick unless one was typed.
        if self.name.is_empty() || self.free.picked.is_some_and(|p| self.name == free_name(p)) {
            self.name = free_name(list);
        }
        self.free.picked = Some(list);
    }

    pub fn for_file(path: &str) -> SourceForm {
        let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        SourceForm { mode: Mode::File, path: path.to_string(), name, ..SourceForm::new() }
    }

    pub fn edit(source: &Source) -> SourceForm {
        let mut f = SourceForm {
            editing: Some(source.id.clone()),
            name: source.name.clone(),
            shelves: source.shelves.clone(),
            hidden: source.hidden.clone(),
            ..SourceForm::new()
        };
        match &source.kind {
            Kind::Playlist { location, epg } => {
                if crate::iptv::http::is_web(location) {
                    f.mode = Mode::Link;
                    f.url = location.clone();
                } else {
                    f.mode = Mode::File;
                    f.path = location.clone();
                }
                f.epg = epg.clone().unwrap_or_default();
            }
            Kind::Xtream { server, user } => {
                f.mode = Mode::Xtream;
                f.server = server.clone();
                f.user = user.clone();
                f.password = crate::credentials::load(&source.id).unwrap_or_default();
            }
        }
        f
    }

    /// The checked source and its catalog, once the check succeeded.
    pub fn poll(&mut self) -> Option<Checked> {
        self.check.poll();
        if !matches!(self.check, Load::Ready(_)) {
            return None;
        }
        let Load::Ready(checked) = std::mem::take(&mut self.check) else { return None };
        Some(checked)
    }

    fn ready(&self) -> bool {
        match self.mode {
            Mode::Link => self.url.trim().contains("://"),
            Mode::File => !self.path.trim().is_empty(),
            Mode::Xtream => !self.server.trim().is_empty() && !self.user.trim().is_empty() && !self.password.is_empty(),
            Mode::Free => self.free.picked.is_some(),
        }
    }

    fn start_check(&mut self, ctx: &egui::Context) {
        let epg = Some(self.epg.trim().to_string()).filter(|e| !e.is_empty());
        let kind = match self.mode {
            Mode::Link => Kind::Playlist { location: self.url.trim().to_string(), epg },
            Mode::File => Kind::Playlist { location: self.path.trim().to_string(), epg },
            Mode::Xtream => {
                let login = crate::iptv::xtream::Login::new(&self.server, &self.user, "");
                Kind::Xtream { server: login.server, user: login.user }
            }
            Mode::Free => Kind::Playlist { location: self.free.picked.map(Free::url).unwrap_or_default(), epg: None },
        };
        let name = match self.name.trim() {
            "" => default_name(&kind),
            n => n.to_string(),
        };
        let id = self.editing.clone().unwrap_or_else(library::new_id);
        let source = Source { id, name, kind, shelves: self.shelves.clone(), hidden: self.hidden.clone() };
        let password = self.password.clone();
        self.check = Load::start(ctx, move || library::check(source, password));
    }

    /// Draws the dialog; Some(false) when it's dismissed.
    pub fn show(&mut self, ctx: &egui::Context, screen: Rect) -> Option<bool> {
        let p = colors();
        let mut close = false;
        // Scrim: a click outside the dialog closes it.
        egui::Area::new(egui::Id::unique("form-scrim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0.0, p.scrim);
            if resp.clicked() && !self.check.is_loading() {
                close = true;
            }
        });
        egui::Area::new(egui::Id::unique("form")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(
            ctx,
            |ui| {
                widgets::popover().inner_margin(egui::Margin::same(24)).show(ui, |ui| {
                    ui.set_width(WIDTH);
                    ui.label(
                        egui::RichText::new(if self.editing.is_some() { "Edit source" } else { "Add source" })
                            .font(typography::heading())
                            .color(p.text),
                    );
                    ui.label(
                        egui::RichText::new(
                            "A playlist by link or from a file, your provider's login (Xtream Codes), or free channels to start with.",
                        )
                        .font(typography::body())
                        .color(p.dim),
                    );
                    ui.add_space(12.0);
                    self.mode_tabs(ui);
                    ui.add_space(14.0);
                    let busy = self.check.is_loading();
                    ui.add_enabled_ui(!busy, |ui| self.fields(ui));
                    ui.add_space(10.0);
                    if let Load::Failed(e) = &self.check {
                        ui.label(egui::RichText::new(format!("Couldn't load it: {e}")).font(typography::secondary()).color(p.danger));
                        ui.add_space(6.0);
                    }
                    ui.horizontal(|ui| {
                        if self.mode == Mode::Xtream {
                            ui.label(
                                egui::RichText::new("Passwords are kept in the system keychain")
                                    .font(typography::secondary())
                                    .color(p.faint),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if busy {
                                let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                                widgets::spinner(ui, r.center(), 8.0, p.accent);
                                ui.label(egui::RichText::new("Loading the channels…").font(typography::secondary()).color(p.dim));
                            } else {
                                let ok = ui.add_enabled_ui(self.ready(), |ui| widgets::pill(ui, None, "Check and save", true)).inner;
                                if ok.clicked() || (self.ready() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                    self.start_check(ui.ctx());
                                }
                                if widgets::pill(ui, None, "Cancel", false).clicked() {
                                    close = true;
                                }
                            }
                        });
                    });
                });
            },
        );
        close.then_some(false)
    }

    fn mode_tabs(&mut self, ui: &mut egui::Ui) {
        let p = colors();
        let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, 36.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(12), p.hl);
        let modes = [
            (Mode::Link, Icon::Link, "M3U link"),
            (Mode::File, Icon::File, "M3U file"),
            (Mode::Xtream, Icon::Key, "Xtream Codes"),
            (Mode::Free, Icon::Globe, "Free TV"),
        ];
        let w = (rect.width() - 6.0) / modes.len() as f32;
        for (i, (mode, icon, label)) in modes.into_iter().enumerate() {
            let r = Rect::from_min_size(rect.min + vec2(3.0 + i as f32 * w, 3.0), vec2(w, 30.0));
            let resp = ui.interact(r, ui.scope_id().with(("mode", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let on = self.mode == mode;
            if on {
                ui.painter().rect_filled(r, CornerRadius::same(10), p.accent);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(10), p.hl);
            }
            let fg = if on { p.on_accent } else { p.dim };
            let galley = ui.painter().layout_no_wrap(label.to_string(), typography::control(), fg);
            let total = galley.size().x + 22.0;
            let x = r.center().x - total / 2.0;
            crate::ui::icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 7.0, r.center().y), vec2(14.0, 14.0)), icon, fg);
            ui.painter().galley(egui::pos2(x + 22.0, r.center().y - galley.size().y / 2.0), galley, fg);
            if resp.clicked() && self.mode != mode {
                self.mode = mode;
                self.check = Load::Idle;
                if mode == Mode::Free
                    && self.free.picked.is_none()
                    && let Some(home) = self.free.home()
                {
                    self.pick_free(home);
                }
            }
        }
    }

    fn fields(&mut self, ui: &mut egui::Ui) {
        let label = |ui: &mut egui::Ui, text: &str| {
            ui.label(egui::RichText::new(text).font(typography::small_strong()).color(colors().dim));
        };
        if self.mode == Mode::Free {
            self.free_fields(ui);
            ui.add_space(6.0);
        }
        label(ui, "Name");
        widgets::field(ui, &mut self.name, "e.g. Home list", false, WIDTH);
        ui.add_space(6.0);
        match self.mode {
            Mode::Link => {
                label(ui, "Playlist URL (.m3u / .m3u8)");
                widgets::field(ui, &mut self.url, "https://…", false, WIDTH);
                if crate::iptv::xtream::Login::from_playlist_link(&self.url).is_some() {
                    ui.label(
                        egui::RichText::new("An Xtream Codes link: haze signs in with it, for posters, plots and seasons too.")
                            .font(typography::secondary())
                            .color(colors().faint),
                    );
                }
                ui.add_space(6.0);
                label(ui, "Guide (XMLTV) — optional, otherwise the one the playlist names");
                widgets::field(ui, &mut self.epg, "https://…/epg.xml.gz", false, WIDTH);
            }
            Mode::File => {
                label(ui, "Playlist file — or drop it on the window");
                widgets::field(ui, &mut self.path, "/Users/…/playlist.m3u8", false, WIDTH);
                ui.add_space(6.0);
                label(ui, "Guide (XMLTV) — optional, a file or a link");
                widgets::field(ui, &mut self.epg, "https://…/epg.xml.gz", false, WIDTH);
            }
            Mode::Xtream => {
                label(ui, "Server — or paste the playlist link the provider gave you");
                widgets::field(ui, &mut self.server, "http://line.provider.tv:8080", false, WIDTH);
                if let Some(login) = crate::iptv::xtream::Login::from_playlist_link(&self.server) {
                    (self.server, self.user, self.password) = (login.server, login.user, login.password);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let half = (WIDTH - 10.0) / 2.0;
                    ui.vertical(|ui| {
                        label(ui, "User name");
                        widgets::field(ui, &mut self.user, "", false, half);
                    });
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        label(ui, "Password");
                        widgets::field(ui, &mut self.password, "", true, half);
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Live TV, the guide, the archive and movies all come from the login.")
                        .font(typography::secondary())
                        .color(colors().faint),
                );
            }
            Mode::Free => {
                ui.label(
                    egui::RichText::new(
                        "Channels their broadcasters offer free to everyone, as listed by the Free-TV project. \
                         Some only play in their own country, and most have no guide.",
                    )
                    .font(typography::secondary())
                    .color(colors().faint),
                );
            }
        }
    }

    /// The lists of free channels, by country or by topic, to pick one from.
    fn free_fields(&mut self, ui: &mut egui::Ui) {
        let p = colors();
        let options = [(false, Some(Icon::Globe), "By country"), (true, Some(Icon::List), "By topic")];
        if let Some(by) = widgets::segments(ui, "free-by", &options, self.free.by_topic, WIDTH) {
            self.free.by_topic = by;
            self.free.filter.clear();
        }
        ui.add_space(8.0);
        let hint = if self.free.by_topic { "Find a topic" } else { "Find a country" };
        widgets::filter_field(ui, &mut self.free.filter, hint, WIDTH);
        ui.add_space(6.0);
        let home = self.free.home();
        let filter = self.free.filter.trim().to_lowercase();
        let mut lists: Vec<Free> = if self.free.by_topic { Free::topics().collect() } else { Free::countries().collect() };
        if let Some(h) = home.filter(|_| !self.free.by_topic) {
            lists.retain(|l| *l != h);
            lists.insert(0, h);
        }
        lists.retain(|l| filter.is_empty() || l.name().to_lowercase().contains(&filter));
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt(("free-lists", self.free.by_topic)).max_height(212.0).auto_shrink([false, true]).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if lists.is_empty() {
                    ui.label(egui::RichText::new("None by that name").font(typography::secondary()).color(p.faint));
                }
                for list in lists {
                    let on = self.free.picked == Some(list);
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    if on || resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(8), if on { p.hl_strong } else { p.hl });
                    }
                    let font = if on { typography::small_strong() } else { typography::secondary() };
                    ui.painter().text(egui::pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, list.name(), font, p.text);
                    let mut right = r.right() - 10.0;
                    if on {
                        let check = Rect::from_center_size(egui::pos2(right - 7.0, r.center().y), vec2(14.0, 14.0));
                        crate::ui::icons::paint(ui.painter(), check, Icon::Check, p.accent);
                        right -= 24.0;
                    }
                    if Some(list) == home {
                        ui.painter().text(
                            egui::pos2(right, r.center().y),
                            Align2::RIGHT_CENTER,
                            "Your country",
                            typography::meta(),
                            p.faint,
                        );
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        pick = Some(list);
                    }
                }
            },
        );
        if let Some(list) = pick {
            self.pick_free(list);
            self.check = Load::Idle;
        }
    }
}

impl FreePick {
    /// The computer's country, looked up the first time it's asked for.
    fn home(&mut self) -> Option<Free> {
        *self.home.get_or_insert_with(Free::home)
    }
}

/// `Free TV · Croatia`
fn free_name(list: Free) -> String {
    format!("Free TV · {}", list.name())
}

fn default_name(kind: &Kind) -> String {
    match kind {
        Kind::Playlist { location, .. } if let Some(login) = crate::iptv::xtream::Login::from_playlist_link(location) => {
            default_name(&Kind::Xtream { server: login.server, user: login.user })
        }
        Kind::Playlist { location, .. } => {
            let tail = location.trim_end_matches('/').rsplit('/').next().unwrap_or(location);
            let stem = tail.split(['?', '.']).next().unwrap_or(tail);
            if stem.is_empty() || stem.len() > 32 { "Playlist".into() } else { stem.to_string() }
        }
        Kind::Xtream { server, .. } => {
            let host = server.split("://").nth(1).unwrap_or(server);
            host.split([':', '/']).next().unwrap_or(host).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sources_after_where_they_come_from() {
        assert_eq!(default_name(&Kind::Playlist { location: "https://x.tv/lists/hrvatska.m3u8?t=1".into(), epg: None }), "hrvatska");
        assert_eq!(default_name(&Kind::Playlist { location: "https://x.tv/get.php?username=a&password=b".into(), epg: None }), "x.tv");
        assert_eq!(default_name(&Kind::Xtream { server: "http://line.provider.tv:8080".into(), user: "me".into() }), "line.provider.tv");
    }
}
