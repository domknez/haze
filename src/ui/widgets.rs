//! Small painted pieces shared by the views.

use super::icons::{self, Icon};
use super::style::{colors, metrics, typography};
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

/// A floating panel over the picture.
pub fn glass(radius: u8) -> egui::Frame {
    let p = colors();
    egui::Frame::new().fill(p.glass).stroke(Stroke::new(1.0, p.line)).corner_radius(CornerRadius::same(radius))
}

/// A menu or dialog: nearly opaque.
pub fn popover() -> egui::Frame {
    let p = colors();
    egui::Frame::new()
        .fill(p.glass_strong)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(8))
        .shadow(egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(90) })
}

/// A square icon button with a hover fill.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, color: Color32, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = colors();
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(9), p.hl);
    }
    let inner = size * 0.52;
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(inner, inner)), icon, color);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

/// A round, filled button (play/pause).
pub fn round_button(ui: &mut Ui, icon: Icon, size: f32, fill: Color32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let fill = if resp.hovered() { fill.gamma_multiply(0.9) } else { fill };
    ui.painter().circle_filled(rect.center(), size / 2.0, fill);
    let inner = size * 0.38;
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(inner, inner)), icon, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rounded button with an optional icon; `filled` uses the accent.
pub fn pill(ui: &mut Ui, icon: Option<Icon>, text: &str, filled: bool) -> Response {
    let p = colors();
    let font = typography::control();
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 24.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let (fill, fg) = if filled { (p.accent, p.on_accent) } else { (p.hl, p.text) };
    let fill = if resp.hovered() { if filled { fill.gamma_multiply(0.9) } else { p.hl_strong } } else { fill };
    ui.painter().rect_filled(rect, CornerRadius::same(metrics::CONTROL_RADIUS), fill);
    let mut x = rect.left() + 12.0;
    if let Some(icon) = icon {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 8.0, rect.center().y), vec2(16.0, 16.0)), icon, fg);
        x += icon_w;
    }
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small monospaced tag (`LIVE`, `XC`, `HD`).
pub fn tag(painter: &egui::Painter, left_center: egui::Pos2, text: &str, fill: Color32, color: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_string(), typography::tag(), color);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 9.0), vec2(galley.size().x + 10.0, 18.0));
    painter.rect_filled(rect, CornerRadius::same(5), fill);
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    rect
}

/// A channel's logo on a tile, or its initials on a colour of its own.
pub fn logo_tile(painter: &egui::Painter, rect: Rect, name: &str, logo: Option<egui::TextureId>, logo_size: Option<egui::Vec2>) {
    let radius = CornerRadius::same((rect.width() * 0.26) as u8);
    match logo {
        Some(tex) => {
            // Logos come in every colour, many white on transparent: a dark tile suits most.
            painter.rect_filled(rect, radius, Color32::from_rgb(0x26, 0x26, 0x2c));
            let inner = rect.shrink(rect.width() * 0.12);
            let fitted = match logo_size {
                Some(s) if s.x > 0.0 && s.y > 0.0 => super::video::fit(inner, s.x / s.y),
                _ => inner,
            };
            painter.image(tex, fitted, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, radius, tile_color(name));
            let font = egui::FontId::new(rect.width() * 0.3, typography::extrabold());
            painter.text(rect.center(), Align2::CENTER_CENTER, initials(name), font, Color32::WHITE);
        }
    }
}

/// A steady colour per name, picked from a set that suits white text.
fn tile_color(name: &str) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(0xc2, 0x41, 0x2a),
        Color32::from_rgb(0x1d, 0x4e, 0xd8),
        Color32::from_rgb(0x0f, 0x76, 0x6e),
        Color32::from_rgb(0x7c, 0x3a, 0xed),
        Color32::from_rgb(0xdb, 0x27, 0x77),
        Color32::from_rgb(0xb4, 0x53, 0x09),
        Color32::from_rgb(0x15, 0x80, 0x3d),
        Color32::from_rgb(0x33, 0x41, 0x55),
    ];
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    COLORS[h as usize % COLORS.len()]
}

/// Up to three letters or digits that name a channel: `HRT 1` → `HRT1`, `Sport Klub 2` → `SK2`.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let s: String = match words.as_slice() {
        [] => "TV".into(),
        [one] => one.chars().take(4).collect(),
        [first, ..] if first.len() <= 4 && first.chars().all(|c| c.is_uppercase() || c.is_ascii_digit()) => {
            let digits: String = words[1..].iter().filter(|w| w.chars().all(|c| c.is_ascii_digit())).copied().collect();
            format!("{first}{digits}")
        }
        many => {
            many.iter()
                .map(|w| {
                    if w.chars().all(|c| c.is_ascii_digit()) {
                        w.to_string()
                    } else {
                        w.chars().next().map(String::from).unwrap_or_default()
                    }
                })
                .collect()
        }
    };
    s.to_uppercase().chars().take(4).collect()
}

/// A text field on a soft fill.
pub fn field(ui: &mut Ui, text: &mut String, hint: &str, password: bool, width: f32) -> Response {
    let p = colors();
    let frame = egui::Frame::new()
        .fill(p.hl)
        .corner_radius(CornerRadius::same(metrics::CONTROL_RADIUS))
        .inner_margin(egui::Margin::symmetric(11, 9));
    frame
        .show(ui, |ui| {
            ui.add_sized(
                [width - 22.0, 18.0],
                egui::TextEdit::singleline(text)
                    .hint_text(egui::RichText::new(hint).color(p.faint))
                    .password(password)
                    .frame(egui::Frame::NONE)
                    .font(typography::body())
                    .text_color(p.text),
            )
        })
        .inner
}

/// The search field: a magnifier, the text, and a cross to clear it.
pub fn search_field(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    filter_field(ui, text, "Search", width)
}

/// A search-style field with its own hint.
pub fn filter_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let p = colors();
    let frame = egui::Frame::new()
        .fill(p.hl)
        .corner_radius(CornerRadius::same(metrics::CONTROL_RADIUS))
        .inner_margin(egui::Margin::symmetric(10, 7));
    frame
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 22.0), Sense::hover());
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(14.0, 14.0)), Icon::Search, p.faint);
                let edit = ui.add_sized(
                    [width - if text.is_empty() { 48.0 } else { 76.0 }, 22.0],
                    egui::TextEdit::singleline(text)
                        .hint_text(egui::RichText::new(hint).color(p.faint))
                        .frame(egui::Frame::NONE)
                        .font(typography::body())
                        .text_color(p.text),
                );
                if !text.is_empty() && icon_button(ui, Icon::Close, 22.0, p.dim, "Clear").clicked() {
                    text.clear();
                }
                edit
            })
            .inner
        })
        .inner
}

/// A thin bar showing `fraction` done.
pub fn progress(painter: &egui::Painter, rect: Rect, fraction: f32, fill: Color32, track: Color32) {
    let r = CornerRadius::same((rect.height() / 2.0) as u8);
    painter.rect_filled(rect, r, track);
    let done = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    if done.width() > 0.5 {
        painter.rect_filled(done, r, fill);
    }
}

/// A turning arc while something loads.
pub fn spinner(ui: &Ui, center: egui::Pos2, radius: f32, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let start = t * 5.0;
    let pts: Vec<egui::Pos2> = (0..=24).map(|i| start + i as f32 / 24.0 * 4.2).map(|a| center + vec2(a.cos(), a.sin()) * radius).collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.5, color)));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
}

/// `text` cut to `width` with an ellipsis.
pub fn ellipsize(ui: &Ui, text: &str, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn makes_initials() {
        assert_eq!(initials("HRT 1"), "HRT1");
        assert_eq!(initials("Sport Klub 2"), "SK2");
        assert_eq!(initials("Arena Sport 1 HD"), "AS1H");
        assert_eq!(initials("CNN"), "CNN");
        assert_eq!(initials("Discovery"), "DISC");
        assert_eq!(initials("--"), "TV");
    }
}
