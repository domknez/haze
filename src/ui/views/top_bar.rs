//! The bar along the top: wordmark, tabs, the source, settings and where the sound
//! plays.

use crate::output::Output;
use crate::ui::app::{Act, App, Popup, Tab};
use crate::ui::icons::{self, Icon};
use crate::ui::style::{colors, metrics, typography};
use crate::ui::widgets;
use eframe::egui::{self, CornerRadius, Rect, Sense, pos2, vec2};

impl App {
    pub fn top_bar(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let left = screen.left() + metrics::GAP + if full { 0.0 } else { metrics::LIGHTS };
        let rect = Rect::from_min_max(
            pos2(left, screen.top() + metrics::GAP - 4.0),
            pos2(screen.right() - metrics::GAP, screen.top() + metrics::GAP - 4.0 + metrics::TOP_BAR),
        );
        let p = colors();
        egui::Area::new(egui::Id::unique("top-bar")).order(egui::Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
            widgets::glass(14).inner_margin(egui::Margin::symmetric(8, 0)).show(ui, |ui| {
                ui.set_min_size(rect.size() - vec2(16.0, 0.0));
                ui.set_max_size(rect.size() - vec2(16.0, 0.0));
                ui.horizontal_centered(|ui| {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(vec2(58.0, 24.0), Sense::hover());
                    let g = ui.painter().layout_no_wrap("haze".into(), typography::wordmark(), p.text);
                    let w = g.size().x;
                    ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text);
                    ui.painter().text(pos2(r.left() + w, r.center().y), egui::Align2::LEFT_CENTER, "_", typography::wordmark(), p.accent);
                    let list = widgets::icon_button(ui, Icon::List, 32.0, if self.rail_open { p.text } else { p.dim }, "Channel list (L)");
                    if list.clicked() {
                        acts.push(Act::ToggleRail);
                    }
                    let guide = widgets::icon_button(ui, Icon::Guide, 32.0, if self.guide_open { p.text } else { p.dim }, "Guide (G)");
                    if guide.clicked() {
                        acts.push(Act::ToggleGuide);
                    }
                    ui.add_space(4.0);
                    for tab in Tab::ALL {
                        if self.tab_button(ui, tab).clicked() {
                            acts.push(Act::Tab(tab));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let speaker = match &self.output {
                            Output::Local => "This computer".to_string(),
                            Output::AirPlay(d) => d.name.clone(),
                        };
                        let airplay = widgets::pill(ui, Some(Icon::AirPlay), &speaker, matches!(self.output, Output::AirPlay(_)));
                        self.anchor(Popup::Outputs, airplay.rect);
                        if airplay.clicked() {
                            acts.push(Act::Popup(Some(Popup::Outputs)));
                        }
                        let settings = widgets::icon_button(ui, Icon::Settings, 32.0, p.dim, "Settings");
                        self.anchor(Popup::Settings, settings.rect);
                        if settings.clicked() {
                            acts.push(Act::Popup(Some(Popup::Settings)));
                        }
                        let src = self.source_pill(ui);
                        self.anchor(Popup::Sources, src.rect);
                        if src.clicked() {
                            acts.push(Act::Popup(Some(Popup::Sources)));
                        }
                    });
                });
            });
        });
    }

    fn tab_button(&self, ui: &mut egui::Ui, tab: Tab) -> egui::Response {
        let p = colors();
        let on = self.browse.tab == tab;
        let g = ui.painter().layout_no_wrap(tab.label().into(), typography::control(), p.text);
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 24.0, 30.0), Sense::click());
        if on || resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(9), if on { p.hl_strong } else { p.hl });
        }
        ui.painter().galley(r.center() - g.size() / 2.0, g, if on { p.text } else { p.dim });
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn source_pill(&self, ui: &mut egui::Ui) -> egui::Response {
        let p = colors();
        let (name, tag) = self.source().map_or(("No source".to_string(), ""), |s| (s.name.clone(), s.tag()));
        let g = ui.painter().layout_no_wrap(name, typography::control(), p.text);
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 74.0, 32.0), Sense::click());
        ui.painter().rect_filled(r, CornerRadius::same(metrics::CONTROL_RADIUS), if resp.hovered() { p.hl_strong } else { p.hl });
        let tag_rect = widgets::tag(ui.painter(), pos2(r.left() + 7.0, r.center().y), tag, p.accent, p.on_accent);
        ui.painter().galley(pos2(tag_rect.right() + 8.0, r.center().y - g.size().y / 2.0), g, p.text);
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(12.0, 12.0)), Icon::Chevrons, p.dim);
        if self.catalog.is_loading() {
            widgets::spinner(ui, pos2(r.right() - 30.0, r.center().y), 5.0, p.accent);
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}
