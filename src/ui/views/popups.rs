//! Menus that open from a button: where the sound plays, the sources, the channel
//! groups, the audio track, and settings.

use crate::output::Output;
use crate::settings::Appearance;
use crate::ui::app::{Act, App, Popup};
use crate::ui::icons::{self, Icon};
use crate::ui::style::{colors, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, pos2, vec2};

const MENU_WIDTH: f32 = 300.0;

impl App {
    /// Remember where the button that opens `popup` is.
    pub fn anchor(&self, popup: Popup, rect: Rect) {
        self.anchors.borrow_mut().insert(popup, rect);
    }

    pub fn popups(&mut self, ctx: &egui::Context, screen: Rect, acts: &mut Vec<Act>) {
        let Some(popup) = self.popup else { return };
        let Some(anchor) = self.anchors.borrow().get(&popup).copied() else { return };
        // Below the button, kept on screen; above it when the button is low down.
        let below = anchor.center().y < screen.center().y;
        let x = (anchor.right() - MENU_WIDTH).clamp(screen.left() + 8.0, screen.right() - MENU_WIDTH - 8.0);
        let (pivot, pos) =
            if below { (Align2::LEFT_TOP, pos2(x, anchor.bottom() + 8.0)) } else { (Align2::LEFT_BOTTOM, pos2(x, anchor.top() - 8.0)) };
        let resp = egui::Area::new(egui::Id::unique(("popup", popup as u8)))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                widgets::popover().show(ui, |ui| {
                    ui.set_width(MENU_WIDTH - 16.0);
                    match popup {
                        Popup::Outputs => self.outputs_menu(ui, acts),
                        Popup::Sources => self.sources_menu(ui, acts),
                        Popup::AudioTracks => self.tracks_menu(ui, acts),
                        Popup::Settings => self.settings_menu(ui, acts),
                        Popup::GuideChannels => self.guide_channels_menu(ui, acts),
                    }
                });
            });
        // A click elsewhere closes it (the button itself toggles).
        let clicked_outside = ctx.input(|i| i.pointer.any_click())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !resp.response.rect.contains(p) && !anchor.contains(p));
        if clicked_outside {
            acts.push(Act::Popup(None));
        }
    }

    fn outputs_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        heading(ui, "Play sound on");
        let mut outputs = vec![Output::Local];
        if let Some(d) = &self.discovery {
            outputs.extend(d.devices().into_iter().filter(|d| d.supported).map(Output::AirPlay));
        }
        for o in outputs {
            let (icon, sub) = match &o {
                Output::Local => (Icon::Speaker, "This computer's speakers".to_string()),
                Output::AirPlay(d) => (Icon::AirPlay, d.kind()),
            };
            let on = o == self.output || (matches!((&o, &self.output), (Output::AirPlay(a), Output::AirPlay(b)) if a.name == b.name));
            if menu_row(ui, icon, o.name(), Some(&sub), on).clicked() {
                acts.push(Act::Output(o));
            }
        }
        if self.discovery.as_ref().is_some_and(|d| d.devices().is_empty()) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Looking for AirPlay speakers…").font(typography::secondary()).color(colors().faint));
        }
        ui.add_space(8.0);
        self.picture_delay_row(ui, acts);
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "The picture waits for the speaker. Lips move before the sound? Delay the picture more ( ] ); after it, less ( [ ).",
            )
            .font(typography::secondary())
            .color(colors().faint),
        );
    }

    /// Picture delay for the output in use: − value +, the value resets it.
    fn picture_delay_row(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        let ms = self.picture_delay();
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Picture delay").font(typography::small_strong()).color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Plus, 28.0, p.text, "Later (])").clicked() {
                    acts.push(Act::PictureDelay(Some(50)));
                }
                let value = egui::RichText::new(crate::ui::app::delay_text(ms)).font(typography::meta()).color(if ms == 0 {
                    p.faint
                } else {
                    p.accent
                });
                let resp = ui.add_sized([64.0, 28.0], egui::Button::new(value).frame(false));
                if ms != 0 && resp.on_hover_text("Back to none").clicked() {
                    acts.push(Act::PictureDelay(None));
                }
                if widgets::icon_button(ui, Icon::Minus, 28.0, p.text, "Earlier ([)").clicked() {
                    acts.push(Act::PictureDelay(Some(-50)));
                }
            });
        });
    }

    fn sources_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = colors();
        heading(ui, "Sources");
        for (i, s) in self.sources.iter().enumerate() {
            let on = self.active == Some(i);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
            if on || resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(10), if on { p.hl_strong } else { p.hl });
            }
            let tag = widgets::tag(
                ui.painter(),
                pos2(r.left() + 8.0, r.center().y),
                s.tag(),
                if on { p.accent } else { p.hl_strong },
                if on { p.on_accent } else { p.dim },
            );
            let x = tag.right() + 10.0;
            let w = r.right() - x - 60.0;
            let name = widgets::ellipsize(ui, &s.name, typography::small_strong(), p.text, w);
            ui.painter().galley(pos2(x, r.top() + 7.0), name, p.text);
            let detail = widgets::ellipsize(ui, &s.detail(), typography::meta(), p.faint, w);
            ui.painter().galley(pos2(x, r.top() + 25.0), detail, p.faint);
            // Edit and remove on hover.
            let edit = Rect::from_center_size(pos2(r.right() - 44.0, r.center().y), vec2(24.0, 24.0));
            let remove = Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(24.0, 24.0));
            let edit_resp = ui.interact(edit, ui.scope_id().with(("edit", i)), Sense::click()).on_hover_text("Edit");
            let remove_resp = ui.interact(remove, ui.scope_id().with(("remove", i)), Sense::click()).on_hover_text("Remove");
            if resp.hovered() || edit_resp.hovered() || remove_resp.hovered() {
                icons::paint(ui.painter(), edit.shrink(5.0), Icon::Edit, if edit_resp.hovered() { p.text } else { p.dim });
                icons::paint(ui.painter(), remove.shrink(5.0), Icon::Trash, if remove_resp.hovered() { p.danger } else { p.dim });
            }
            if edit_resp.clicked() {
                acts.push(Act::EditSource(i));
            } else if remove_resp.clicked() {
                acts.push(Act::RemoveSource(i));
            } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                acts.push(Act::SwitchSource(i));
            }
        }
        if let Some(account) = self.catalog.ready().and_then(|c| c.account.clone()) {
            ui.add_space(4.0);
            let mut parts = vec![account.status.clone()];
            if let Some(e) = account.expires {
                let c = crate::iptv::time::civil(e);
                parts.push(format!("until {:02}/{:02}/{}", c.day, c.month, c.year));
            }
            if let (Some(a), Some(m)) = (account.active_connections, account.max_connections) {
                parts.push(format!("{a}/{m} connections"));
            }
            ui.label(egui::RichText::new(parts.join(" · ")).font(typography::meta()).color(p.faint));
        }
        ui.add_space(6.0);
        if menu_row(ui, Icon::Plus, "Add source…", None, false).clicked() {
            acts.push(Act::AddSource);
        }
    }

    fn tracks_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let Some(player) = &self.session.player else { return };
        let info = player.info();
        if info.audio_tracks.len() > 1 {
            heading(ui, "Audio");
            for (i, t) in info.audio_tracks.iter().enumerate() {
                if menu_row(ui, Icon::Audio, &t.label, None, info.audio_track == Some(i)).clicked() {
                    acts.push(Act::AudioTrack(t.stream));
                }
            }
        }
        if !info.subtitle_tracks.is_empty() {
            if info.audio_tracks.len() > 1 {
                ui.add_space(6.0);
            }
            heading(ui, "Subtitles");
            if menu_row(ui, Icon::Close, "Off", None, info.subtitle_track.is_none()).clicked() {
                acts.push(Act::SubtitleTrack(None));
            }
            for (i, t) in info.subtitle_tracks.iter().enumerate() {
                if menu_row(ui, Icon::List, &t.label, None, info.subtitle_track == Some(i)).clicked() {
                    acts.push(Act::SubtitleTrack(Some(t.stream)));
                }
            }
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Drop an .srt file on the window for subtitles of your own").font(typography::meta()).color(colors().faint),
        );
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        heading(ui, "Appearance");
        for a in Appearance::ALL {
            let icon = match a {
                Appearance::System => Icon::Settings,
                Appearance::Dark => Icon::Moon,
                Appearance::Light => Icon::Sun,
            };
            if menu_row(ui, icon, a.label(), None, self.prefs.theme == a).clicked() {
                acts.push(Act::Theme(a));
            }
        }
        let hidden = self.source().map_or(0, |s| s.hidden.len());
        if hidden > 0 {
            ui.add_space(6.0);
            heading(ui, "Groups");
            let sub =
                if hidden == 1 { "1 group hidden in this source".to_string() } else { format!("{hidden} groups hidden in this source") };
            if menu_row(ui, Icon::List, "Show hidden groups", Some(&sub), self.browse.show_hidden).clicked() {
                acts.push(Act::ShowHidden(!self.browse.show_hidden));
            }
            let locked = self.prefs.pin.is_some();
            let (title, sub) = if locked {
                ("Remove the PIN", "Hidden groups show without one")
            } else {
                ("Lock with a PIN", "Asked before hidden groups show")
            };
            if menu_row(ui, Icon::Key, title, Some(sub), false).clicked() {
                acts.push(Act::Pin(!locked));
            }
        }
        ui.add_space(6.0);
        heading(ui, "Keys");
        for (k, what) in [
            ("↑ ↓", "previous / next channel"),
            ("← →", "back / ahead 10 seconds"),
            ("Space", "pause"),
            ("F", "full screen"),
            ("M", "mute"),
            ("L", "channel list"),
            ("G", "guide"),
            ("[ ]", "picture earlier / later"),
            ("⌘F", "search"),
            ("⌘1–5", "tabs"),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized([56.0, 18.0], egui::Label::new(egui::RichText::new(k).font(typography::meta()).color(colors().text)));
                ui.label(egui::RichText::new(what).font(typography::secondary()).color(colors().dim));
            });
        }
        ui.add_space(6.0);
        heading(ui, "Updates");
        let status = self.updates.status();
        if menu_row(ui, Icon::Update, "Check automatically", Some(&status), self.updates.auto).clicked() {
            acts.push(Act::AutoUpdate(!self.updates.auto));
        }
        if menu_row(ui, Icon::FromStart, "Check now", None, false).clicked() {
            acts.push(Act::CheckForUpdates);
        }
    }
}

impl App {
    /// The channels the guide can show: the favourites, all of them, or a group.
    fn guide_channels_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        heading(ui, "Show in the guide");
        let counts = self.browse.channels.counts.clone();
        if counts.len() > 12 {
            widgets::filter_field(ui, &mut self.browse.group_filter, "Filter groups", ui.available_width());
            ui.add_space(4.0);
        }
        let needle = self.browse.group_filter.trim().to_lowercase();
        let favorites = self.marks.favorites(&self.source_id()).count();
        let picked = (!self.guide_favorites).then(|| self.browse.channels.picked.clone());
        let mut rows: Vec<(Option<Option<String>>, String, usize)> = Vec::new();
        if needle.is_empty() {
            if favorites > 0 {
                rows.push((None, "Favorites".into(), favorites));
            }
            rows.push((Some(None), "All channels".into(), counts.iter().map(|(_, n)| n).sum()));
        }
        rows.extend(counts.into_iter().filter(|(g, _)| g.to_lowercase().contains(&needle)).map(|(g, n)| (Some(Some(g.clone())), g, n)));
        egui::ScrollArea::vertical().id_salt("guide-channels").max_height(420.0).show_rows(
            ui,
            crate::ui::style::metrics::GROUP_ROW,
            rows.len(),
            |ui, range| {
                for (pick, name, count) in &rows[range] {
                    let hidden = matches!(pick, Some(Some(g)) if self.browse.hidden.contains(g));
                    if super::rail::group_row(ui, name, *count, *pick == picked, hidden).clicked() {
                        acts.push(Act::GuideChannels(pick.clone()));
                    }
                }
            },
        );
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(typography::meta()).color(colors().faint));
    ui.add_space(2.0);
}

fn menu_row(ui: &mut egui::Ui, icon: Icon, title: &str, sub: Option<&str>, on: bool) -> egui::Response {
    let p = colors();
    let h = if sub.is_some() { 44.0 } else { 34.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    if on || resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(10), if on { p.hl_strong } else { p.hl });
    }
    let ic = Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(26.0, 26.0));
    ui.painter().rect_filled(ic, CornerRadius::same(8), if on { p.accent } else { p.hl });
    icons::paint(ui.painter(), ic.shrink(6.0), icon, if on { p.on_accent } else { p.dim });
    let x = ic.right() + 10.0;
    let w = r.right() - x - 30.0;
    match sub {
        Some(sub) => {
            let t = widgets::ellipsize(ui, title, typography::small_strong(), p.text, w);
            ui.painter().galley(pos2(x, r.top() + 6.0), t, p.text);
            let s = widgets::ellipsize(ui, sub, typography::secondary(), p.dim, w);
            ui.painter().galley(pos2(x, r.top() + 24.0), s, p.dim);
        }
        None => {
            let t = widgets::ellipsize(ui, title, typography::small_strong(), p.text, w);
            ui.painter().galley(pos2(x, r.center().y - t.size().y / 2.0), t, p.text);
        }
    }
    if on {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(12.0, 12.0)), Icon::Check, p.accent);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
