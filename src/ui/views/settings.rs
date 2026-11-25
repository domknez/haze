//! The Settings window: its sections down the left (Appearance, Picture, Sources,
//! Hidden groups, Keyboard, Updates), the one picked on the right. ⌘, (Ctrl+,
//! elsewhere) or the gear opens it; Esc, ×, or a click beside it closes it.

use crate::settings::Appearance;
use crate::ui::app::{Act, App};
use crate::ui::icons::{self, Icon};
use crate::ui::style::{colors, metrics, typography};
use crate::ui::video::Shape;
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};

const WIDTH: f32 = 680.0;
/// The column of sections.
const NAV: f32 = 168.0;
const HEIGHT: f32 = 460.0;

/// A section of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Appearance,
    Picture,
    Sources,
    Groups,
    Keys,
    Updates,
}

impl Page {
    const ALL: [Page; 6] = [Page::Appearance, Page::Picture, Page::Sources, Page::Groups, Page::Keys, Page::Updates];

    fn label(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Picture => "Picture",
            Page::Sources => "Sources",
            Page::Groups => "Hidden groups",
            Page::Keys => "Keyboard",
            Page::Updates => "Updates",
        }
    }
}

impl App {
    /// The Settings window, centred over the dimmed app. Hidden while a dialog it
    /// opened (a source, the PIN) is up, and back after.
    pub fn settings_view(&mut self, ctx: &egui::Context, screen: Rect, page: Page, acts: &mut Vec<Act>) {
        if self.form.is_some() || self.pin.is_some() {
            return;
        }
        let p = colors();
        let width = WIDTH.min(screen.width() - 60.0);
        let height = HEIGHT.min(screen.height() - 140.0).max(240.0);
        let frame = widgets::popover().fill(p.glass_strong.to_opaque()).inner_margin(egui::Margin::same(14));
        let modal = egui::Modal::new(egui::Id::unique("settings")).frame(frame).backdrop_color(p.scrim).show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Settings").font(typography::heading()).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, 28.0, p.faint, "Close (Esc)").clicked() {
                        acts.push(Act::Settings(None));
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.vertical(|ui| {
                    ui.set_width(NAV);
                    nav(ui, page, acts);
                });
                let (line, _) = ui.allocate_exact_size(vec2(21.0, height), Sense::hover());
                ui.painter().vline(line.center().x, line.y_range(), Stroke::new(1.0, p.line));
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("settings-page", page as u8))
                        .max_height(height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                            ui.set_width(ui.available_width() - 10.0);
                            match page {
                                Page::Appearance => self.appearance_page(ui, acts),
                                Page::Picture => self.picture_page(ui, acts),
                                Page::Sources => self.sources_page(ui, acts),
                                Page::Groups => self.groups_page(ui, acts),
                                Page::Keys => keys_page(ui),
                                Page::Updates => self.updates_page(ui, acts),
                            }
                            ui.add_space(8.0);
                        });
                });
            });
        });
        if modal.should_close() {
            acts.push(Act::Settings(None));
        }
    }

    fn appearance_page(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        section(ui, "Theme");
        let options: Vec<(Appearance, Option<Icon>, &str)> =
            [(Appearance::System, Icon::Settings), (Appearance::Light, Icon::Sun), (Appearance::Dark, Icon::Moon)]
                .map(|(a, icon)| (a, Some(icon), a.label()))
                .into();
        if let Some(a) = widgets::segments(ui, "theme", &options, self.prefs.theme, ui.available_width()) {
            acts.push(Act::Theme(a));
        }
        note(ui, "System follows your computer's light or dark mode.");
    }

    fn picture_page(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        section(ui, "Size");
        let fill = self.prefs.fill;
        if switch_row(ui, "Fill the window", "Cuts the edges of the picture instead of black bars (Z)", fill) {
            acts.push(Act::Fill(!fill));
        }
        section(ui, "Shape");
        let options: Vec<(Shape, Option<Icon>, &str)> = Shape::ALL.map(|s| (s, None, s.label())).into();
        if let Some(s) = widgets::segments(ui, "shape", &options, self.session.shape, ui.available_width()) {
            acts.push(Act::Shape(s));
        }
        note(ui, "For a stream that's stretched or squeezed: what plays now, until the next channel (A).");
        section(ui, "Sound and picture");
        self.picture_delay_row(ui, acts);
        note(
            ui,
            &format!(
                "For {}, kept for it. The picture waits for the speaker: lips move before the sound? Delay the picture more ( ] ); after it, less ( [ ).",
                self.output.name()
            ),
        );
    }

    fn sources_page(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        section(ui, "Your sources");
        for (i, s) in self.sources.iter().enumerate() {
            let on = self.active == Some(i);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::click());
            if on || resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(10), if on { p.hl_strong } else { p.hl });
            }
            let tag = widgets::tag(
                ui.painter(),
                pos2(r.left() + 10.0, r.center().y),
                s.tag(),
                if on { p.accent } else { p.hl_strong },
                if on { p.on_accent } else { p.dim },
            );
            let x = tag.right() + 12.0;
            let w = r.right() - x - 90.0;
            let name = widgets::ellipsize(ui, &s.name, typography::small_strong(), p.text, w);
            ui.painter().galley(pos2(x, r.top() + 9.0), name, p.text);
            let detail = if on { format!("In use · {}", s.detail()) } else { s.detail() };
            let detail = widgets::ellipsize(ui, &detail, typography::meta(), p.faint, w);
            ui.painter().galley(pos2(x, r.top() + 30.0), detail, p.faint);
            let edit = Rect::from_center_size(pos2(r.right() - 56.0, r.center().y), vec2(30.0, 30.0));
            let remove = Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(30.0, 30.0));
            let edit_resp = small_button(ui, edit, ("edit", i), Icon::Edit, "Edit", false);
            let remove_resp = small_button(ui, remove, ("remove", i), Icon::Trash, "Remove", true);
            if edit_resp.clicked() {
                acts.push(Act::EditSource(i));
            } else if remove_resp.clicked() {
                acts.push(Act::RemoveSource(i));
            } else if !on && resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Use this source").clicked() {
                acts.push(Act::SwitchSource(i));
            }
            ui.add_space(4.0);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if widgets::pill(ui, Some(Icon::Plus), "Add source…", false).clicked() {
                acts.push(Act::AddSource);
            }
            if widgets::pill(ui, Some(Icon::Globe), "Free channels…", false).clicked() {
                acts.push(Act::AddFree);
            }
        });
        if self.source().is_none() {
            return;
        }
        section(ui, "Channels and guide");
        let status = if self.refresh.running() || self.catalog.is_loading() {
            "Loading…".to_string()
        } else {
            match self.refresh.loaded_at.and_then(|t| t.elapsed().ok()) {
                Some(age) => format!("Updated {}; again every six hours", crate::ui::app::ago(age)),
                None => "Loaded again every six hours".into(),
            }
        };
        if button_row(ui, "Refresh now", &status, "Refresh", false) {
            acts.push(Act::Refresh);
        }
        if let Some(account) = self.catalog.ready().and_then(|c| c.account.clone()) {
            section(ui, "Account");
            let mut parts = vec![account.status.clone()];
            if let Some(e) = account.expires {
                let c = crate::iptv::time::civil(e);
                parts.push(format!("until {:02}/{:02}/{}", c.day, c.month, c.year));
            }
            if let (Some(a), Some(m)) = (account.active_connections, account.max_connections) {
                parts.push(format!("{a} of {m} connections in use"));
            }
            text_row(ui, "Provider account", &parts.join(" · "));
        }
    }

    fn groups_page(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let hidden = self.source().map_or(0, |s| s.hidden.len());
        section(ui, "This source");
        if hidden == 0 {
            text_row(ui, "No hidden groups", "Right-click a group in the list to hide it from lists, search and zapping");
        } else {
            let sub = if hidden == 1 { "1 group hidden".to_string() } else { format!("{hidden} groups hidden") };
            if switch_row(ui, "Show hidden groups", &sub, self.browse.show_hidden) {
                acts.push(Act::ShowHidden(!self.browse.show_hidden));
            }
        }
        section(ui, "PIN");
        let locked = self.prefs.pin.is_some();
        let (title, sub, button) = if locked {
            ("Locked with a PIN", "Asked for before hidden groups show", "Remove")
        } else {
            ("No PIN", "Hidden groups show for anyone who asks", "Set a PIN")
        };
        if button_row(ui, title, sub, button, false) {
            acts.push(Act::Pin(!locked));
        }
    }

    fn updates_page(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        section(ui, "Version");
        let title = format!("haze {}", crate::update::CURRENT);
        let (label, strong) = match self.updates.offer() {
            Some((_, true)) => ("Restart", true),
            Some(_) => ("Download", true),
            None => ("Check now", false),
        };
        if button_row(ui, &title, &self.updates.status(), label, strong) {
            acts.push(if strong { Act::InstallUpdate } else { Act::CheckForUpdates });
        }
        if switch_row(ui, "Update automatically", "New versions download in the background and install on restart", self.updates.auto) {
            acts.push(Act::AutoUpdate(!self.updates.auto));
        }
    }
}

/// The sections down the left, the one picked inverted, and the version under them.
fn nav(ui: &mut egui::Ui, page: Page, acts: &mut Vec<Act>) {
    let p = colors();
    for pg in Page::ALL {
        let on = pg == page;
        let (r, resp) = ui.allocate_exact_size(vec2(NAV, 34.0), Sense::click());
        let text = if on {
            ui.painter().rect_filled(r, CornerRadius::same(8), p.text);
            p.backdrop
        } else {
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(8), p.hl);
            }
            if resp.hovered() { p.text } else { p.dim }
        };
        let font = if on { typography::small_strong() } else { typography::secondary() };
        ui.painter().text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, pg.label(), font, text);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
            acts.push(Act::Settings(Some(pg)));
        }
        ui.add_space(1.0);
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new(format!("haze {}", crate::update::CURRENT)).font(typography::meta()).color(p.faint));
    });
}

fn keys_page(ui: &mut egui::Ui) {
    let p = colors();
    // ⌘ on macOS, Ctrl elsewhere.
    let cmd = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
    let groups: [(&str, Vec<(String, &str)>); 3] = [
        (
            "Watching",
            vec![
                ("↑ ↓".into(), "Previous / next channel"),
                ("← →".into(), "Back / ahead 10 seconds"),
                ("Space".into(), "Pause"),
                ("M".into(), "Mute"),
                ("[ ]".into(), "Picture earlier / later"),
            ],
        ),
        (
            "Picture",
            vec![("F".into(), "Full screen"), ("P".into(), "Mini player"), ("Z".into(), "Fill the window"), ("A".into(), "Picture shape")],
        ),
        (
            "Finding",
            vec![
                ("L".into(), "Channel list"),
                ("G".into(), "Guide"),
                (format!("{cmd}F"), "Search (the guide too)"),
                (format!("{cmd}1–5"), "Tabs"),
                (format!("{cmd}R"), "Refresh the channels"),
                (format!("{cmd},"), "Settings"),
                ("Esc".into(), "Close what's on top"),
            ],
        ),
    ];
    for (title, keys) in groups {
        section(ui, title);
        for (k, what) in keys {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            ui.painter().text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, what, typography::secondary(), p.text);
            let key = ui.painter().layout_no_wrap(k, typography::meta(), p.text);
            let cap =
                Rect::from_min_size(pos2(r.right() - 10.0 - key.size().x - 16.0, r.center().y - 12.0), vec2(key.size().x + 16.0, 24.0));
            ui.painter().rect_filled(cap, CornerRadius::same(6), p.hl);
            ui.painter().galley(cap.center() - key.size() / 2.0, key, p.text);
        }
    }
}

/// A small heading over a group of settings.
fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(text.to_uppercase()).font(typography::meta()).color(colors().faint));
    });
    ui.add_space(8.0);
}

/// A line of explanation under a setting.
fn note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(text).font(typography::meta()).color(colors().faint)).wrap());
    });
}

/// A setting's name and a line under it, in a row of `r`.
fn row_text(ui: &egui::Ui, r: Rect, title: &str, detail: &str, room: f32) {
    let p = colors();
    let w = r.width() - room - 20.0;
    let t = widgets::ellipsize(ui, title, typography::small_strong(), p.text, w);
    ui.painter().galley(pos2(r.left() + 10.0, r.top() + 9.0), t, p.text);
    let d = widgets::ellipsize(ui, detail, typography::meta(), p.faint, w);
    ui.painter().galley(pos2(r.left() + 10.0, r.top() + 29.0), d, p.faint);
}

/// A setting with nothing to change.
fn text_row(ui: &mut egui::Ui, title: &str, detail: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
    row_text(ui, r, title, detail, 0.0);
}

/// A setting with a switch; true when clicked.
fn switch_row(ui: &mut egui::Ui, title: &str, detail: &str, on: bool) -> bool {
    let p = colors();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(10), p.hl);
    }
    row_text(ui, r, title, detail, 56.0);
    widgets::switch(
        ui,
        resp.id.with("switch"),
        Rect::from_min_size(pos2(r.right() - 10.0 - 38.0, r.center().y - 11.0), vec2(38.0, 22.0)),
        on,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A setting with a button at its end; true when the button is clicked.
fn button_row(ui: &mut egui::Ui, title: &str, detail: &str, label: &str, strong: bool) -> bool {
    let p = colors();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
    let fg = if strong { p.on_accent } else { p.text };
    let galley = ui.painter().layout_no_wrap(label.to_string(), typography::small_strong(), fg);
    let button =
        Rect::from_min_size(pos2(r.right() - 10.0 - galley.size().x - 28.0, r.center().y - 15.0), vec2(galley.size().x + 28.0, 30.0));
    row_text(ui, r, title, detail, button.width() + 10.0);
    let resp = ui.interact(button, ui.scope_id().with(("button", title)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = match (strong, resp.hovered()) {
        (true, h) => {
            if h {
                p.accent.gamma_multiply(0.9)
            } else {
                p.accent
            }
        }
        (false, true) => p.hl_strong,
        (false, false) => p.hl,
    };
    ui.painter().rect_filled(button, CornerRadius::same(metrics::CONTROL_RADIUS), fill);
    ui.painter().galley(button.center() - galley.size() / 2.0, galley, fg);
    resp.clicked()
}

/// A small icon button at `r`; `danger` reddens it on hover.
fn small_button(ui: &egui::Ui, r: Rect, salt: (&str, usize), icon: Icon, tip: &str, danger: bool) -> egui::Response {
    let p = colors();
    let resp = ui.interact(r, ui.scope_id().with(salt), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(8), p.hl_strong);
    }
    let color = if resp.hovered() && danger {
        p.danger
    } else if resp.hovered() {
        p.text
    } else {
        p.dim
    };
    icons::paint(ui.painter(), r.shrink(8.0), icon, color);
    resp
}
