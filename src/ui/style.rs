//! The look: colours for dark and light, type, dimensions, and the egui style
//! they produce. Panels float over the picture as tinted glass.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Palette {
    /// Behind everything when nothing plays.
    pub backdrop: Color32,
    /// Panels over the picture.
    pub glass: Color32,
    /// Menus and dialogs: nearly opaque.
    pub glass_strong: Color32,
    /// Hairlines and panel edges.
    pub line: Color32,
    /// Hover, selection and field fills.
    pub hl: Color32,
    pub hl_strong: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    /// Scrim behind dialogs.
    pub scrim: Color32,
    /// Text over the picture itself (always light).
    pub on_video: Color32,
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        (r as u16 * a as u16 / 255) as u8,
        (g as u16 * a as u16 / 255) as u8,
        (b as u16 * a as u16 / 255) as u8,
        a,
    )
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub static DARK: Palette = Palette {
    backdrop: rgb(0x0e, 0x0e, 0x11),
    glass: rgba(0x14, 0x14, 0x18, 196),
    glass_strong: rgba(0x1e, 0x1e, 0x24, 246),
    line: rgba(0xff, 0xff, 0xff, 24),
    hl: rgba(0xff, 0xff, 0xff, 22),
    hl_strong: rgba(0xff, 0xff, 0xff, 40),
    text: rgb(0xf4, 0xf2, 0xef),
    dim: rgb(0xa9, 0xa7, 0xa5),
    faint: rgb(0x76, 0x75, 0x74),
    accent: rgb(0xff, 0x6a, 0x3d),
    on_accent: rgb(0xff, 0xff, 0xff),
    danger: rgb(0xff, 0x5a, 0x5a),
    scrim: rgba(0, 0, 0, 120),
    on_video: rgb(0xf4, 0xf2, 0xef),
};

pub static LIGHT: Palette = Palette {
    backdrop: rgb(0xe9, 0xe7, 0xe3),
    glass: rgba(0xfa, 0xf8, 0xf5, 232),
    glass_strong: rgba(0xff, 0xff, 0xff, 248),
    line: rgba(0, 0, 0, 24),
    hl: rgba(0, 0, 0, 14),
    hl_strong: rgba(0, 0, 0, 28),
    text: rgb(0x18, 0x17, 0x1a),
    dim: rgb(0x5c, 0x5a, 0x5e),
    faint: rgb(0x8c, 0x8a, 0x8e),
    accent: rgb(0xe8, 0x50, 0x1f),
    on_accent: rgb(0xff, 0xff, 0xff),
    danger: rgb(0xd0, 0x30, 0x30),
    scrim: rgba(0, 0, 0, 70),
    on_video: rgb(0xf4, 0xf2, 0xef),
};

static IS_DARK: AtomicBool = AtomicBool::new(true);

pub fn colors() -> &'static Palette {
    if IS_DARK.load(Ordering::Relaxed) { &DARK } else { &LIGHT }
}

/// Dimensions.
pub mod metrics {
    pub const WINDOW_SIZE: [f32; 2] = [1280.0, 760.0];
    pub const MIN_WINDOW_SIZE: [f32; 2] = [720.0, 440.0];
    /// Space between panels and the window edge.
    pub const GAP: f32 = 12.0;
    pub const TOP_BAR: f32 = 46.0;
    /// Room for the macOS traffic lights left of the top bar.
    pub const LIGHTS: f32 = if cfg!(target_os = "macos") { 76.0 } else { 0.0 };
    pub const RAIL_WIDTH: f32 = 320.0;
    pub const BOTTOM: f32 = 164.0;
    pub const CHANNEL_ROW: f32 = 54.0;
    pub const LOGO: f32 = 38.0;
    pub const PANEL_RADIUS: u8 = 16;
    pub const CONTROL_RADIUS: u8 = 10;
    /// Controls hide after this long without the pointer moving.
    pub const IDLE_HIDE: f32 = 3.0;
}

/// Text roles.
pub mod typography {
    use eframe::egui::{FontFamily, FontId};

    pub fn semibold() -> FontFamily {
        FontFamily::Name("semibold".into())
    }
    pub fn bold() -> FontFamily {
        FontFamily::Name("bold".into())
    }
    pub fn extrabold() -> FontFamily {
        FontFamily::Name("extrabold".into())
    }

    pub fn wordmark() -> FontId {
        FontId::new(17.0, extrabold())
    }
    /// The programme on now, in the bottom panel.
    pub fn now_title() -> FontId {
        FontId::new(22.0, extrabold())
    }
    /// Dialog and empty-screen titles.
    pub fn heading() -> FontId {
        FontId::new(20.0, extrabold())
    }
    /// Channel names.
    pub fn item() -> FontId {
        FontId::new(14.0, bold())
    }
    /// Tabs, buttons.
    pub fn control() -> FontId {
        FontId::new(13.5, semibold())
    }
    pub fn body() -> FontId {
        FontId::new(13.0, FontFamily::Proportional)
    }
    /// What's on, under channel names.
    pub fn secondary() -> FontId {
        FontId::new(12.0, FontFamily::Proportional)
    }
    pub fn small_strong() -> FontId {
        FontId::new(12.0, semibold())
    }
    /// Times, numbers, tags.
    pub fn meta() -> FontId {
        FontId::new(11.0, FontFamily::Monospace)
    }
    pub fn tag() -> FontId {
        FontId::new(9.5, FontFamily::Name("mono-bold".into()))
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("Manrope", &include_bytes!("../../assets/fonts/Manrope-Regular.ttf")[..]),
        ("Manrope-SemiBold", include_bytes!("../../assets/fonts/Manrope-SemiBold.ttf")),
        ("Manrope-Bold", include_bytes!("../../assets/fonts/Manrope-Bold.ttf")),
        ("Manrope-ExtraBold", include_bytes!("../../assets/fonts/Manrope-ExtraBold.ttf")),
        ("Mono", include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf")),
        ("Mono-Bold", include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf")),
        ("Inter", include_bytes!("../../assets/fonts/Inter-Regular.ttf")),
        ("Keys", include_bytes!("../../assets/fonts/NotoSansSymbols2-Keys.ttf")),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let chain = |first: &str| [vec![first.to_string(), "Inter".to_string(), "Keys".to_string()], fallbacks.clone()].concat();
    fonts.families.insert(FontFamily::Proportional, chain("Manrope"));
    fonts.families.insert(typography::semibold(), chain("Manrope-SemiBold"));
    fonts.families.insert(typography::bold(), chain("Manrope-Bold"));
    fonts.families.insert(typography::extrabold(), chain("Manrope-ExtraBold"));
    fonts.families.insert(FontFamily::Monospace, chain("Mono"));
    fonts.families.insert(FontFamily::Name("mono-bold".into()), chain("Mono-Bold"));
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    IS_DARK.store(dark, Ordering::Relaxed);
    let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(theme);
    ctx.set_visuals_of(theme, visuals(dark));
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, typography::heading()),
            (TextStyle::Body, typography::body()),
            (TextStyle::Button, typography::control()),
            (TextStyle::Small, typography::secondary()),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.bar_width = 6.0;
        s.interaction.selectable_labels = false;
    });
}

fn visuals(dark: bool) -> Visuals {
    let p = colors();
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.backdrop;
    v.window_fill = p.glass_strong;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = CornerRadius::same(metrics::PANEL_RADIUS);
    v.menu_corner_radius = CornerRadius::same(12);
    v.extreme_bg_color = p.hl;
    v.faint_bg_color = p.hl;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.4);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.popup_shadow = egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 50 }) };
    v.window_shadow = v.popup_shadow;
    for (w, bg) in [
        (&mut v.widgets.noninteractive, Color32::TRANSPARENT),
        (&mut v.widgets.inactive, p.hl),
        (&mut v.widgets.hovered, p.hl_strong),
        (&mut v.widgets.active, p.hl_strong),
        (&mut v.widgets.open, p.hl),
    ] {
        w.bg_fill = bg;
        w.weak_bg_fill = bg;
        w.bg_stroke = Stroke::NONE;
        w.corner_radius = CornerRadius::same(metrics::CONTROL_RADIUS);
        w.fg_stroke.color = p.text;
    }
    v.widgets.noninteractive.fg_stroke.color = p.dim;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v
}
