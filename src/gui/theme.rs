//! Apple-style look: system colors, a SF-like type scale (Segoe UI on Windows)
//! and light/dark appearance that follows the operating system.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme, Ui, vec2};

#[derive(Clone, Copy)]
pub struct Palette {
    /// Window ground (grouped background).
    pub background: Color32,
    /// Lists, fields and grouped sections sitting on the background.
    pub card: Color32,
    /// Pop-up buttons and steppers.
    pub control: Color32,
    pub control_hover: Color32,
    pub control_press: Color32,
    /// Segmented-control track and switch track when off.
    pub track: Color32,
    /// Selected segment.
    pub knob: Color32,
    pub label: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub separator: Color32,
    pub border: Color32,
    /// Row highlight under the pointer.
    pub hover: Color32,
    pub blue: Color32,
    pub green: Color32,
    pub red: Color32,
    pub orange: Color32,
    pub gray: Color32,
    pub dark: bool,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const LIGHT: Palette = Palette {
    background: hex(0xF2F2F7),
    card: hex(0xFFFFFF),
    control: hex(0xFFFFFF),
    control_hover: hex(0xF5F5F7),
    control_press: hex(0xE8E8ED),
    track: hex(0xE3E3E8),
    knob: hex(0xFFFFFF),
    label: hex(0x1D1D1F),
    secondary: hex(0x6E6E73),
    tertiary: hex(0xAEAEB2),
    separator: hex(0xE5E5EA),
    border: hex(0xD1D1D6),
    hover: Color32::from_black_alpha(10),
    blue: hex(0x007AFF),
    green: hex(0x34C759),
    red: hex(0xFF3B30),
    orange: hex(0xFF9500),
    gray: hex(0x8E8E93),
    dark: false,
};

pub const DARK: Palette = Palette {
    background: hex(0x1C1C1E),
    card: hex(0x2C2C2E),
    control: hex(0x3A3A3C),
    control_hover: hex(0x444446),
    control_press: hex(0x505052),
    track: hex(0x3A3A3C),
    knob: hex(0x636366),
    label: hex(0xF5F5F7),
    secondary: hex(0x98989D),
    tertiary: hex(0x636366),
    separator: hex(0x3D3D41),
    border: hex(0x48484A),
    hover: Color32::from_rgba_premultiplied(10, 10, 10, 10),
    blue: hex(0x0A84FF),
    green: hex(0x30D158),
    red: hex(0xFF453A),
    orange: hex(0xFF9F0A),
    gray: hex(0x8E8E93),
    dark: true,
};

pub fn palette(ui: &Ui) -> &'static Palette {
    of(ui.visuals().dark_mode)
}

pub fn of(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

const SEMIBOLD: &str = "semibold";

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Linear blend in sRGB, good enough for hover and press tints.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::ThemePreference::System);
    for theme in [Theme::Light, Theme::Dark] {
        let p = of(theme == Theme::Dark);
        ctx.style_mut_of(theme, |s| {
            s.text_styles = [
                (TextStyle::Small, regular(12.0)),
                (TextStyle::Body, regular(14.0)),
                (TextStyle::Button, regular(14.0)),
                (TextStyle::Heading, semibold(17.0)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();
            s.animation_time = 0.18;
            s.interaction.tooltip_delay = 0.4;

            let sp = &mut s.spacing;
            sp.item_spacing = vec2(8.0, 8.0);
            sp.button_padding = vec2(12.0, 5.0);
            sp.interact_size = vec2(40.0, 28.0);
            sp.menu_margin = Margin::same(5);
            sp.window_margin = Margin::same(20);
            sp.icon_width = 12.0;
            sp.icon_width_inner = 8.0;
            sp.icon_spacing = 8.0;
            sp.combo_height = 320.0;

            let v = &mut s.visuals;
            v.panel_fill = p.background;
            v.window_fill = p.card;
            v.window_stroke = Stroke::new(1.0, p.separator);
            v.window_corner_radius = CornerRadius::same(12);
            v.menu_corner_radius = CornerRadius::same(8);
            v.window_shadow = Shadow {
                offset: [0, 10],
                blur: 36,
                spread: 0,
                color: Color32::from_black_alpha(if p.dark { 130 } else { 45 }),
            };
            v.popup_shadow = Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: Color32::from_black_alpha(if p.dark { 100 } else { 35 }),
            };
            v.extreme_bg_color = p.card;
            v.text_edit_bg_color = Some(p.card);
            v.faint_bg_color = p.hover;
            v.hyperlink_color = p.blue;
            v.warn_fg_color = p.orange;
            v.error_fg_color = p.red;
            v.weak_text_color = Some(p.secondary);
            v.selection.bg_fill = p.blue.gamma_multiply(0.3);
            v.selection.stroke = Stroke::new(1.5, p.blue);
            v.text_cursor.stroke = Stroke::new(2.0, p.blue);
            v.indent_has_left_vline = false;
            v.striped = false;

            let radius = CornerRadius::same(7);
            let w = &mut v.widgets;
            w.noninteractive.bg_fill = p.card;
            w.noninteractive.weak_bg_fill = p.card;
            w.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
            w.noninteractive.fg_stroke = Stroke::new(1.0, p.label);
            w.noninteractive.corner_radius = radius;
            for (state, fill) in [
                (&mut w.inactive, p.control),
                (&mut w.hovered, p.control_hover),
                (&mut w.active, p.control_press),
                (&mut w.open, p.control_hover),
            ] {
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.bg_stroke = Stroke::new(1.0, if p.dark { Color32::TRANSPARENT } else { p.border });
                state.fg_stroke = Stroke::new(1.5, p.label);
                state.corner_radius = radius;
                state.expansion = 0.0;
            }
        });
    }
}

/// Segoe UI in two weights, with the symbol font as fallback for titles in
/// other scripts. Falls back to egui's own fonts where they are missing.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut load = |key: &str, path: &str| -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        fonts
            .font_data
            .insert(key.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        Some(key.to_owned())
    };
    let regular = load("segoe", "C:/Windows/Fonts/segoeui.ttf");
    let semibold = load("segoe_sb", "C:/Windows/Fonts/seguisb.ttf").or_else(|| regular.clone());
    let symbols = load("segoe_sym", "C:/Windows/Fonts/seguisym.ttf");

    let family = |main: Option<String>| -> Vec<String> {
        main.into_iter().chain(symbols.clone()).chain(fallback.iter().cloned()).collect()
    };
    let proportional = family(regular);
    let bold = family(semibold);
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), bold);
    ctx.set_fonts(fonts);
}
