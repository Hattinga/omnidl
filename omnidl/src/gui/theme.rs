//! Apple-style look: system colors, a SF-like type scale (San Francisco on
//! macOS, Segoe UI on Windows, Inter or the desktop's font on Linux) and
//! light/dark appearance that follows the operating system.

use egui::epaint::text::{FontData, FontTweak};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme, Ui, vec2};
use std::path::Path;

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

/// The system UI font in two weights (Segoe UI, San Francisco, Inter …), with
/// a symbol font as fallback for titles in other scripts. Falls back to
/// egui's own fonts where they are missing.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut add = |key: &str, data: FontData| -> String {
        fonts.font_data.insert(key.to_owned(), std::sync::Arc::new(data));
        key.to_owned()
    };
    let (regular, semibold) = match system_font() {
        Some((r, s)) => (Some(add("ui", r)), Some(add("ui_semibold", s))),
        None => (None, None),
    };
    let symbols = SYMBOLS.iter().find_map(|p| std::fs::read(p).ok()).map(|b| add("symbols", FontData::from_owned(b)));

    let family = |main: Option<String>| -> Vec<String> {
        main.into_iter().chain(symbols.clone()).chain(fallback.iter().cloned()).collect()
    };
    let proportional = family(regular);
    let bold = family(semibold);
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), bold);
    ctx.set_fonts(fonts);
}

/// A candidate UI font: the folders it may live in (they differ between
/// distributions) and its files (static weights, a variable font or a
/// collection).
struct Family {
    dirs: &'static [&'static str],
    files: &'static [&'static str],
}

#[cfg(windows)]
const FAMILIES: &[Family] = &[Family { dirs: &["C:/Windows/Fonts"], files: &["segoeui.ttf", "seguisb.ttf"] }];

#[cfg(target_os = "macos")]
const FAMILIES: &[Family] = &[
    // San Francisco, a variable font since macOS 10.15.
    Family { dirs: &["/System/Library/Fonts"], files: &["SFNS.ttf"] },
    Family { dirs: &["/System/Library/Fonts"], files: &["HelveticaNeue.ttc"] },
];

/// Best first: Inter is closest to San Francisco, then the GNOME and Ubuntu
/// fonts, then what nearly every system has.
#[cfg(all(unix, not(target_os = "macos")))]
const FAMILIES: &[Family] = &[
    Family {
        dirs: &[
            "/usr/share/fonts/opentype/inter",
            "/usr/share/fonts/truetype/inter-vf",
            "/usr/share/fonts/rsms-inter-fonts",
            "/usr/share/fonts/rsms-inter-vf-fonts",
            "/usr/share/fonts/inter",
        ],
        files: &[
            "Inter-Regular.otf",
            "Inter-SemiBold.otf",
            "Inter-Regular.ttf",
            "Inter-SemiBold.ttf",
            "Inter.ttc",
            "InterVariable.ttf",
        ],
    },
    Family {
        dirs: &[
            "/usr/share/fonts/opentype/cantarell",
            "/usr/share/fonts/abattis-cantarell-vf-fonts",
            "/usr/share/fonts/abattis-cantarell-fonts",
            "/usr/share/fonts/cantarell",
        ],
        files: &["Cantarell-VF.otf", "Cantarell-Regular.otf", "Cantarell-Bold.otf"],
    },
    Family {
        dirs: &["/usr/share/fonts/truetype/ubuntu", "/usr/share/fonts/ubuntu"],
        files: &["UbuntuSans[wdth,wght].ttf", "Ubuntu[wdth,wght].ttf", "Ubuntu-R.ttf", "Ubuntu-M.ttf"],
    },
    Family {
        dirs: &["/usr/share/fonts/truetype/noto", "/usr/share/fonts/google-noto-vf", "/usr/share/fonts/google-noto", "/usr/share/fonts/noto"],
        files: &["NotoSans[wght].ttf", "NotoSans-Regular.ttf", "NotoSans-SemiBold.ttf", "NotoSans-Bold.ttf"],
    },
    Family {
        dirs: &["/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/dejavu-sans-fonts", "/usr/share/fonts/TTF"],
        files: &["DejaVuSans.ttf", "DejaVuSans-Bold.ttf"],
    },
];

#[cfg(not(any(windows, unix)))]
const FAMILIES: &[Family] = &[];

/// Fallback for symbols the UI font lacks.
#[cfg(windows)]
const SYMBOLS: &[&str] = &["C:/Windows/Fonts/seguisym.ttf"];
#[cfg(target_os = "macos")]
const SYMBOLS: &[&str] = &["/System/Library/Fonts/Apple Symbols.ttf"];
#[cfg(all(unix, not(target_os = "macos")))]
const SYMBOLS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];
#[cfg(not(any(windows, unix)))]
const SYMBOLS: &[&str] = &[];

/// Regular and semibold face of the first family present.
fn system_font() -> Option<(FontData, FontData)> {
    FAMILIES.iter().find_map(|family| {
        // Fonts live as long as the app; leaking spares a copy when both
        // weights come from the same variable font.
        let files: Vec<&'static [u8]> = family
            .dirs
            .iter()
            .flat_map(|dir| family.files.iter().map(move |f| Path::new(dir).join(f)))
            .filter_map(|path| std::fs::read(path).ok())
            .map(|bytes| &*Box::leak(bytes.into_boxed_slice()))
            .collect();
        Some((pick(&files, 400.0)?, pick(&files, 600.0)?))
    })
}

/// The upright, normal-width face closest to `weight`. A variable font is set
/// to it exactly; otherwise static files win, and on a tie the lighter face.
fn pick(files: &[&'static [u8]], weight: f32) -> Option<FontData> {
    let (bytes, face) = files
        .iter()
        .flat_map(|&bytes| faces(bytes).into_iter().map(move |face| (bytes, face)))
        .filter(|(_, face)| face.upright)
        .min_by_key(|(_, face)| {
            let exact = face.wght.is_some_and(|(lo, hi)| (lo..=hi).contains(&weight));
            let distance = if exact { 0 } else { (f32::from(face.weight) - weight).abs() as u32 };
            (distance, f32::from(face.weight) > weight, face.wght.is_some())
        })?;
    let mut tweak = FontTweak::default();
    if let Some((lo, hi)) = face.wght {
        tweak.coords.push(b"wght", weight.clamp(lo, hi));
    }
    // Optical size for text rather than headlines.
    if let Some((lo, hi)) = face.opsz {
        tweak.coords.push(b"opsz", 14.0f32.clamp(lo, hi));
    }
    Some(FontData { font: std::borrow::Cow::Borrowed(bytes), index: face.index, tweak })
}

/// One font in a file (a .ttc holds several).
#[derive(Debug, PartialEq)]
struct Face {
    index: u32,
    /// `usWeightClass`: 400 regular, 600 semibold.
    weight: u16,
    /// Neither italic nor condensed/expanded.
    upright: bool,
    /// Ranges of the `wght` and `opsz` axes of a variable font.
    wght: Option<(f32, f32)>,
    opsz: Option<(f32, f32)>,
}

/// Reads the faces of a TrueType/OpenType file or collection.
fn faces(b: &[u8]) -> Vec<Face> {
    let offsets: Vec<usize> = if b.get(..4) == Some(b"ttcf") {
        let count = be32(b, 8).unwrap_or(0);
        (0..count.min(64)).filter_map(|i| be32(b, 12 + 4 * i)).collect()
    } else {
        vec![0]
    };
    offsets
        .into_iter()
        .zip(0..)
        .filter_map(|(offset, index)| {
            let os2 = table(b, offset, b"OS/2")?;
            let fvar = table(b, offset, b"fvar");
            let selection = be16(os2, 62)?;
            Some(Face {
                index,
                weight: be16(os2, 4)?,
                // fsSelection bits: 0 italic, 9 oblique. Width class 5 is normal.
                upright: be16(os2, 6)? == 5 && selection & 0x201 == 0,
                wght: fvar.and_then(|f| axis(f, b"wght")),
                opsz: fvar.and_then(|f| axis(f, b"opsz")),
            })
        })
        .collect()
}

/// A table of the font whose table directory starts at `font`.
fn table<'a>(b: &'a [u8], font: usize, tag: &[u8; 4]) -> Option<&'a [u8]> {
    let count = be16(b, font + 4)? as usize;
    (0..count).find_map(|i| {
        let record = font + 12 + 16 * i;
        if b.get(record..record + 4)? != tag {
            return None;
        }
        let start = be32(b, record + 8)?;
        b.get(start..start + be32(b, record + 12)?)
    })
}

/// Range of a variation axis from the `fvar` table.
fn axis(fvar: &[u8], tag: &[u8; 4]) -> Option<(f32, f32)> {
    let (offset, count, size) = (be16(fvar, 4)? as usize, be16(fvar, 8)? as usize, be16(fvar, 10)? as usize);
    let fixed = |at: usize| be32(fvar, at).map(|v| v as u32 as i32 as f32 / 65536.0);
    (0..count).find_map(|i| {
        let record = offset + size * i;
        (fvar.get(record..record + 4)? == tag).then_some((fixed(record + 4)?, fixed(record + 12)?))
    })
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font file reduced to what `faces` reads: OS/2 and, for variable
    /// fonts, fvar. `base` is where it will sit inside a collection.
    fn font(base: usize, weight: u16, width: u16, italic: bool, axes: &[(&[u8; 4], f32, f32)]) -> Vec<u8> {
        let mut os2 = vec![0u8; 64];
        os2[4..6].copy_from_slice(&weight.to_be_bytes());
        os2[6..8].copy_from_slice(&width.to_be_bytes());
        os2[62..64].copy_from_slice(&(u16::from(italic)).to_be_bytes());
        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"OS/2", os2)];
        if !axes.is_empty() {
            let mut fvar = vec![0, 1, 0, 0, 0, 16, 0, 2];
            fvar.extend((axes.len() as u16).to_be_bytes());
            fvar.extend(20u16.to_be_bytes());
            fvar.extend([0; 4]);
            for (tag, lo, hi) in axes {
                fvar.extend(*tag);
                for v in [lo, lo, hi] {
                    fvar.extend(((v * 65536.0) as i32).to_be_bytes());
                }
                fvar.extend([0; 4]);
            }
            tables.push((b"fvar", fvar));
        }
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0; 6]);
        let mut at = base + 12 + 16 * tables.len();
        for (tag, data) in &tables {
            out.extend(*tag);
            out.extend([0; 4]);
            out.extend((at as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            at += data.len();
        }
        for (_, data) in tables {
            out.extend(data);
        }
        out
    }

    fn leak(b: Vec<u8>) -> &'static [u8] {
        Box::leak(b.into_boxed_slice())
    }

    #[test]
    fn reads_static_and_variable_faces() {
        let faces_of = |b: Vec<u8>| faces(&b);
        assert_eq!(
            faces_of(font(0, 600, 5, false, &[])),
            vec![Face { index: 0, weight: 600, upright: true, wght: None, opsz: None }]
        );
        let vf = faces_of(font(0, 400, 5, false, &[(b"wght", 100.0, 900.0), (b"opsz", 14.0, 32.0)]));
        assert_eq!(vf[0].wght, Some((100.0, 900.0)));
        assert_eq!(vf[0].opsz, Some((14.0, 32.0)));
        assert!(!faces_of(font(0, 400, 5, true, &[]))[0].upright, "kursiv");
        assert!(!faces_of(font(0, 700, 3, false, &[]))[0].upright, "schmal");
        assert!(faces(b"kaputt").is_empty());
        assert!(faces(&[]).is_empty());
    }

    /// Wie HelveticaNeue.ttc: Regular für Text, Medium als nächstes zu Semibold.
    #[test]
    fn picks_faces_from_a_collection() {
        let weights = [(700, 5, false), (400, 5, false), (400, 5, true), (500, 5, false), (700, 3, false)];
        let header = 12 + 4 * weights.len();
        let mut fonts = Vec::new();
        let mut at = header;
        for (w, width, italic) in weights {
            let f = font(at, w, width, italic, &[]);
            at += f.len();
            fonts.push(f);
        }
        let mut ttc = b"ttcf".to_vec();
        ttc.extend([0, 1, 0, 0]);
        ttc.extend((weights.len() as u32).to_be_bytes());
        let mut at = header as u32;
        for f in &fonts {
            ttc.extend(at.to_be_bytes());
            at += f.len() as u32;
        }
        ttc.extend(fonts.concat());
        let files = [leak(ttc)];
        assert_eq!(pick(&files, 400.0).unwrap().index, 1);
        assert_eq!(pick(&files, 600.0).unwrap().index, 3, "Medium vor Bold");
    }

    #[test]
    fn variable_fonts_take_the_exact_weight() {
        let files = [leak(font(0, 400, 5, false, &[(b"wght", 100.0, 900.0), (b"opsz", 14.0, 32.0)]))];
        let semibold = pick(&files, 600.0).unwrap();
        let coords: Vec<(egui::epaint::text::Tag, f32)> = semibold.tweak.coords.as_ref().to_vec();
        assert_eq!(coords, vec![(egui::epaint::text::Tag::new(b"wght"), 600.0), (egui::epaint::text::Tag::new(b"opsz"), 14.0)]);
        // Static files beat the variable font where they match.
        let files = [files[0], leak(font(0, 400, 5, false, &[])), leak(font(0, 700, 5, false, &[]))];
        let regular = pick(&files, 400.0).unwrap();
        assert!(regular.tweak.coords.as_ref().is_empty());
        assert!(std::ptr::eq(regular.font.as_ptr(), files[1].as_ptr()));
    }

    /// Findet die Systemschrift, wo eine der bekannten Dateien liegt; unter
    /// Windows unverändert Segoe UI und Segoe UI Semibold.
    #[test]
    fn finds_the_system_font() {
        let present: Vec<_> = FAMILIES
            .iter()
            .flat_map(|f| f.dirs.iter().flat_map(|d| f.files.iter().map(move |n| Path::new(d).join(n))))
            .filter(|p| p.is_file())
            .collect();
        let font = system_font();
        if let Some((r, s)) = &font {
            eprintln!("Schriften: {present:?}");
            eprintln!("regular: Index {} {:?}; semibold: Index {} {:?}", r.index, r.tweak.coords, s.index, s.tweak.coords);
        }
        assert_eq!(font.is_some(), !present.is_empty(), "{present:?}");
        #[cfg(target_os = "macos")]
        if let (Some((_, s)), true) = (&font, Path::new("/System/Library/Fonts/SFNS.ttf").is_file()) {
            assert!(!s.tweak.coords.as_ref().is_empty(), "San Francisco als variable Schrift: {:?}", faces(&s.font));
        }
        #[cfg(windows)]
        if let (Some((r, s)), Ok(segoe), Ok(semi)) =
            (&font, std::fs::read("C:/Windows/Fonts/segoeui.ttf"), std::fs::read("C:/Windows/Fonts/seguisb.ttf"))
        {
            assert!(*r.font == *segoe && *s.font == *semi, "Segoe UI wie bisher");
            assert!(r.tweak == FontTweak::default() && s.tweak == FontTweak::default());
        }
    }
}
