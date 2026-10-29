//! The PIN that keeps hidden groups hidden: asked before they show, and set or
//! removed from Settings.

use crate::ui::style::{colors, typography};
use crate::ui::widgets;
use eframe::egui::{self, Align2, Rect, Sense, vec2};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Show the hidden groups.
    Unlock,
    /// Choose a PIN.
    Set,
    /// Stop asking for one.
    Remove,
}

pub struct PinDialog {
    pub purpose: Purpose,
    pin: String,
    wrong: bool,
}

/// What came of the dialog this frame.
pub enum Outcome {
    /// Still open.
    Open,
    Cancelled,
    /// The right PIN (Unlock, Remove), or the new one's hash (Set).
    Done(Option<String>),
}

const WIDTH: f32 = 320.0;

/// How a PIN is kept in the settings: hashed, never as typed.
pub fn hash(pin: &str) -> String {
    Sha256::digest(format!("haze-pin:{pin}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

impl PinDialog {
    pub fn new(purpose: Purpose) -> PinDialog {
        PinDialog { purpose, pin: String::new(), wrong: false }
    }

    /// Draws the dialog; `saved` is the hash of the PIN set, if any.
    pub fn show(&mut self, ctx: &egui::Context, screen: Rect, saved: Option<&str>) -> Outcome {
        let p = colors();
        let mut outcome = Outcome::Open;
        egui::Area::new(egui::Id::unique("pin-scrim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0.0, p.scrim);
            if resp.clicked() {
                outcome = Outcome::Cancelled;
            }
        });
        egui::Area::new(egui::Id::unique("pin")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(
            ctx,
            |ui| {
                widgets::popover().inner_margin(egui::Margin::same(22)).show(ui, |ui| {
                    ui.set_width(WIDTH);
                    let (title, line) = match self.purpose {
                        Purpose::Unlock => ("Show hidden groups", "Enter the PIN."),
                        Purpose::Set => {
                            ("Lock hidden groups", "Choose a PIN of at least four digits; it's asked before hidden groups show.")
                        }
                        Purpose::Remove => ("Remove the PIN", "Enter the PIN to stop asking for it."),
                    };
                    ui.label(egui::RichText::new(title).font(typography::heading()).color(p.text));
                    ui.label(egui::RichText::new(line).font(typography::body()).color(p.dim));
                    ui.add_space(12.0);
                    let before = self.pin.clone();
                    let field = widgets::field(ui, &mut self.pin, "PIN", true, WIDTH);
                    field.request_focus();
                    self.pin.retain(|c| c.is_ascii_digit());
                    if self.pin != before {
                        self.wrong = false;
                    }
                    if self.wrong {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("That's not the PIN").font(typography::secondary()).color(p.danger));
                    }
                    ui.add_space(12.0);
                    let ready = self.pin.len() >= 4;
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if self.purpose == Purpose::Set { "Set PIN" } else { "OK" };
                            let ok = ui.add_enabled_ui(ready, |ui| widgets::pill(ui, None, label, true)).inner;
                            if ok.clicked() || (ready && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                outcome = self.check(saved);
                            }
                            if widgets::pill(ui, None, "Cancel", false).clicked() {
                                outcome = Outcome::Cancelled;
                            }
                        });
                    });
                });
            },
        );
        outcome
    }

    fn check(&mut self, saved: Option<&str>) -> Outcome {
        let typed = hash(&self.pin);
        match self.purpose {
            Purpose::Set => Outcome::Done(Some(typed)),
            Purpose::Unlock | Purpose::Remove if saved.is_none_or(|s| s == typed) => Outcome::Done(None),
            Purpose::Unlock | Purpose::Remove => {
                self.wrong = true;
                self.pin.clear();
                Outcome::Open
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_pin_against_its_hash() {
        let saved = hash("2468");
        assert_ne!(saved, "2468");
        let mut d = PinDialog::new(Purpose::Unlock);
        d.pin = "1111".into();
        assert!(matches!(d.check(Some(&saved)), Outcome::Open));
        assert!(d.wrong && d.pin.is_empty());
        d.pin = "2468".into();
        assert!(matches!(d.check(Some(&saved)), Outcome::Done(None)));
        let mut set = PinDialog::new(Purpose::Set);
        set.pin = "2468".into();
        assert!(matches!(set.check(None), Outcome::Done(Some(h)) if h == saved));
    }
}
