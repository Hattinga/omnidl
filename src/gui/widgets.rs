//! Controls and symbols in the style of Apple's Human Interface Guidelines.
//! Symbols are painted, not taken from a font, so they stay crisp at any scale.

use super::theme::{self, mix, regular, semibold};
use egui::{
    Align2, Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};
use std::f32::consts::{FRAC_PI_2, TAU};

// ---------------------------------------------------------------- controls

/// Filled accent button: the one default action of a view.
pub fn primary_button(ui: &mut Ui, text: &str, size: Vec2, enabled: bool) -> Response {
    let p = theme::palette(ui);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let fill = if !enabled {
            mix(p.blue, p.background, 0.55)
        } else if resp.is_pointer_button_down_on() {
            mix(p.blue, Color32::BLACK, 0.18)
        } else if resp.hovered() {
            mix(p.blue, Color32::BLACK, 0.08)
        } else {
            p.blue
        };
        ui.painter().rect_filled(rect, 8, fill);
        let text_color = if enabled { Color32::WHITE } else { Color32::from_white_alpha(210) };
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, text, semibold(14.0), text_color);
    }
    resp
}

/// Borderless text button in a tint color, like a toolbar or list action.
pub fn text_button(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), regular(13.5), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(8.0, 6.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, text));
    let tint = if resp.is_pointer_button_down_on() {
        color.gamma_multiply(0.45)
    } else if resp.hovered() {
        color.gamma_multiply(0.72)
    } else {
        color
    };
    ui.painter()
        .galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, tint);
    resp
}

/// Square button that shows only a symbol; a soft plate appears on hover.
pub fn symbol_button(ui: &mut Ui, size: f32, id_salt: &str, paint: impl FnOnce(&Painter, Rect, bool)) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id_salt), Sense::click());
    symbol_plate(ui, rect, &resp, paint)
}

/// Same as [`symbol_button`] but at a fixed spot, for rows that are painted by hand.
pub fn symbol_button_at(ui: &Ui, rect: Rect, id: egui::Id, paint: impl FnOnce(&Painter, Rect, bool)) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    symbol_plate(ui, rect, &resp, paint)
}

fn symbol_plate(ui: &Ui, rect: Rect, resp: &Response, paint: impl FnOnce(&Painter, Rect, bool)) -> Response {
    let p = theme::palette(ui);
    if resp.hovered() {
        let plate = if resp.is_pointer_button_down_on() { p.hover.gamma_multiply(2.2) } else { p.hover };
        ui.painter().rect_filled(rect, 6, plate);
    }
    paint(ui.painter(), rect, resp.hovered());
    resp.clone()
}

/// Segmented control. Returns `true` when the selection changed.
pub fn segmented(ui: &mut Ui, id_salt: &str, selected: &mut usize, labels: &[&str], width: f32) -> bool {
    let p = theme::palette(ui);
    let height = 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let id = ui.id().with(id_salt);
    let seg_w = width / labels.len() as f32;
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 8, p.track);

    let mut changed = false;
    for (i, label) in labels.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(rect.left() + i as f32 * seg_w, rect.top()), vec2(seg_w, height));
        let resp = ui.interact(seg, id.with(i), Sense::click());
        resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, *selected == i, *label));
        if resp.clicked() && *selected != i {
            *selected = i;
            changed = true;
        }
    }

    let pos = ui.ctx().animate_value_with_time(id, *selected as f32, 0.16);
    let knob = Rect::from_min_size(pos2(rect.left() + pos * seg_w, rect.top()), vec2(seg_w, height)).shrink(2.0);
    if !p.dark {
        painter.add(
            egui::Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(28) }
                .as_shape(knob, 6),
        );
    }
    painter.rect_filled(knob, 6, p.knob);

    for (i, label) in labels.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(rect.left() + i as f32 * seg_w, rect.top()), vec2(seg_w, height));
        if i > 0 && i != *selected && i - 1 != *selected {
            painter.line_segment(
                [pos2(seg.left(), seg.top() + 7.0), pos2(seg.left(), seg.bottom() - 7.0)],
                Stroke::new(1.0, p.border),
            );
        }
        let font = if i == *selected { semibold(13.0) } else { regular(13.0) };
        painter.text(seg.center(), Align2::CENTER_CENTER, *label, font, p.label);
    }
    changed
}

/// iOS switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = theme::palette(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, *on, ""));
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let painter = ui.painter();
    painter.rect_filled(rect, 12, mix(p.track, p.green, t));
    let r = 10.0;
    let x = egui::lerp((rect.left() + r + 2.0)..=(rect.right() - r - 2.0), t);
    let c = pos2(x, rect.center().y);
    painter.circle_filled(c + vec2(0.0, 0.8), r + 0.6, Color32::from_black_alpha(35));
    painter.circle_filled(c, r, Color32::WHITE);
    resp
}

/// Stepper: `[ − | + ]`. Returns `true` when the value changed.
pub fn stepper(ui: &mut Ui, id_salt: &str, value: &mut usize, range: std::ops::RangeInclusive<usize>) -> bool {
    let p = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(78.0, 28.0), Sense::hover());
    let id = ui.id().with(id_salt);
    let painter = ui.painter().clone();
    painter.rect(rect, 7, p.control, Stroke::new(1.0, if p.dark { Color32::TRANSPARENT } else { p.border }), StrokeKind::Inside);
    let mid = rect.center().x;
    let halves = [
        (rect.with_max_x(mid), CornerRadius { nw: 7, sw: 7, ne: 0, se: 0 }, *value > *range.start(), false),
        (rect.with_min_x(mid), CornerRadius { nw: 0, sw: 0, ne: 7, se: 7 }, *value < *range.end(), true),
    ];
    let mut changed = false;
    for (half, radius, enabled, plus) in halves {
        let resp = ui.interact(half, id.with(plus), if enabled { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, if plus { "+" } else { "−" }));
        if enabled && resp.hovered() {
            let fill = if resp.is_pointer_button_down_on() { p.control_press } else { p.control_hover };
            painter.rect_filled(half.shrink(1.0), radius, fill);
        }
        let color = if enabled { p.label } else { p.tertiary };
        let c = half.center();
        let stroke = Stroke::new(1.6, color);
        painter.line_segment([c - vec2(5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
        if plus {
            painter.line_segment([c - vec2(0.0, 5.0), c + vec2(0.0, 5.0)], stroke);
        }
        if resp.clicked() {
            *value = if plus { *value + 1 } else { *value - 1 };
            changed = true;
        }
    }
    painter.line_segment(
        [pos2(mid, rect.top() + 6.0), pos2(mid, rect.bottom() - 6.0)],
        Stroke::new(1.0, p.border),
    );
    changed
}

/// Pop-up button: a menu of choices behind a button showing the current one.
pub fn popup<R>(
    ui: &mut Ui,
    id_salt: &str,
    text: &str,
    width: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let color = theme::palette(ui).secondary;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(text)
        .width(width)
        .icon(move |ui, rect, _, _| updown_chevrons(ui.painter(), rect.center(), color))
        .show_ui(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            add_contents(ui)
        })
        .inner
}

/// Menu entry with a checkmark for the current choice and the accent-colored
/// highlight Apple menus use.
pub fn menu_item(ui: &mut Ui, selected: bool, text: &str) -> Response {
    let p = theme::palette(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), regular(13.5), p.label);
    let width = ui.available_width().max(galley.size().x + 44.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, text));
    let fg = if !ui.is_enabled() {
        p.tertiary
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5, p.blue);
        Color32::WHITE
    } else {
        p.label
    };
    if selected {
        checkmark(ui.painter(), pos2(rect.left() + 13.0, rect.center().y), 4.5, Stroke::new(1.7, fg));
    }
    ui.painter().galley_with_override_text_color(
        pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    resp
}

/// Single line of text, cut with an ellipsis at `max_width`.
pub fn one_line(ui: &Ui, text: &str, font: egui::FontId, color: Color32, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.replace(['\n', '\r'], " "),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(10.0));
    ui.painter().layout_job(job)
}

// ---------------------------------------------------------------- symbols

/// `checkmark.circle.fill`
pub fn check_circle(painter: &Painter, c: Pos2, r: f32, fill: Color32) {
    painter.circle_filled(c, r, fill);
    let s = Stroke::new((r * 0.17).max(1.6), Color32::WHITE);
    painter.add(Shape::line(
        vec![c + vec2(-0.42, 0.02) * r, c + vec2(-0.12, 0.32) * r, c + vec2(0.44, -0.3) * r],
        s,
    ));
}

/// `xmark.circle.fill`
pub fn xmark_circle(painter: &Painter, c: Pos2, r: f32, fill: Color32, glyph: Color32) {
    painter.circle_filled(c, r, fill);
    let s = Stroke::new((r * 0.16).max(1.4), glyph);
    let d = r * 0.32;
    painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
    painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], s);
}

/// `exclamationmark.circle.fill`
pub fn exclamation_circle(painter: &Painter, c: Pos2, r: f32, fill: Color32) {
    painter.circle_filled(c, r, fill);
    let w = (r * 0.18).max(1.6);
    painter.line_segment([c + vec2(0.0, -0.46 * r), c + vec2(0.0, 0.1 * r)], Stroke::new(w, Color32::WHITE));
    painter.circle_filled(c + vec2(0.0, 0.4 * r), w * 0.62, Color32::WHITE);
}

/// `minus.circle.fill`
pub fn minus_circle(painter: &Painter, c: Pos2, r: f32, fill: Color32) {
    painter.circle_filled(c, r, fill);
    let s = Stroke::new((r * 0.17).max(1.6), Color32::WHITE);
    painter.line_segment([c + vec2(-0.42 * r, 0.0), c + vec2(0.42 * r, 0.0)], s);
}

/// `clock`
pub fn clock(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    let s = Stroke::new(1.5, color);
    painter.circle_stroke(c, r - 0.75, s);
    painter.line_segment([c, c + vec2(0.0, -0.52 * r)], s);
    painter.line_segment([c, c + vec2(0.38 * r, 0.0)], s);
}

/// Circular progress like the App Store: `frac` fills clockwise from the top,
/// `None` spins an indeterminate arc.
pub fn ring(painter: &Painter, c: Pos2, r: f32, frac: Option<f32>, time: f64, track: Color32, color: Color32) {
    let width = 2.6;
    let r = r - width / 2.0;
    painter.circle_stroke(c, r, Stroke::new(width, track));
    let (start, sweep) = match frac {
        Some(f) => (-FRAC_PI_2, f.clamp(0.0, 1.0) * TAU),
        None => (((time * 1.3) % 1.0) as f32 * TAU - FRAC_PI_2, 0.28 * TAU),
    };
    arc(painter, c, r, start, sweep, Stroke::new(width, color));
}

fn arc(painter: &Painter, c: Pos2, r: f32, start: f32, sweep: f32, stroke: Stroke) {
    if sweep <= 0.01 {
        return;
    }
    let n = ((sweep / TAU) * 48.0).ceil().max(3.0) as usize;
    let points: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + sweep * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    // Round caps.
    painter.circle_filled(points[0], stroke.width / 2.0, stroke.color);
    painter.circle_filled(points[n], stroke.width / 2.0, stroke.color);
    painter.add(Shape::line(points, stroke));
}

/// `magnifyingglass`
pub fn magnifier(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    let lens = c + vec2(-0.14, -0.14) * r;
    let lr = 0.52 * r;
    painter.circle_stroke(lens, lr, Stroke::new(1.6, color));
    let d = std::f32::consts::FRAC_1_SQRT_2;
    painter.line_segment([lens + vec2(d, d) * lr, c + vec2(0.62, 0.62) * r], Stroke::new(2.0, color));
}

/// `gearshape`
pub fn gear(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    const TEETH: usize = 8;
    let step = TAU / TEETH as f32;
    let (outer, inner) = (r, r * 0.76);
    let (top, base) = (step * 0.2, step * 0.3);
    let mut points = Vec::with_capacity(TEETH * 4);
    for i in 0..TEETH {
        let a = i as f32 * step - FRAC_PI_2;
        for (angle, radius) in [(a - base, inner), (a - top, outer), (a + top, outer), (a + base, inner)] {
            points.push(c + vec2(angle.cos(), angle.sin()) * radius);
        }
    }
    let s = Stroke::new(1.5, color);
    painter.add(Shape::closed_line(points, s));
    painter.circle_stroke(c, r * 0.3, s);
}

/// Blue macOS folder.
pub fn folder(painter: &Painter, rect: Rect, color: Color32) {
    let (w, h) = (rect.width(), rect.height());
    let tab = Rect::from_min_size(rect.min, vec2(w * 0.45, h * 0.4));
    painter.rect_filled(tab, 2, mix(color, Color32::WHITE, 0.2));
    let body = Rect::from_min_max(rect.min + vec2(0.0, h * 0.2), rect.max);
    painter.rect_filled(body, 2, color);
}

pub enum Direction {
    Right,
    Down,
}

/// `chevron.right` / `chevron.down`
pub fn chevron(painter: &Painter, c: Pos2, size: f32, dir: Direction, stroke: Stroke) {
    let (a, b, d) = match dir {
        Direction::Right => (vec2(-0.5, -1.0), vec2(0.5, 0.0), vec2(-0.5, 1.0)),
        Direction::Down => (vec2(-1.0, -0.5), vec2(0.0, 0.5), vec2(1.0, -0.5)),
    };
    painter.add(Shape::line(vec![c + a * size, c + b * size, c + d * size], stroke));
}

/// `chevron.up.chevron.down`, the pop-up button indicator.
pub fn updown_chevrons(painter: &Painter, c: Pos2, color: Color32) {
    let s = Stroke::new(1.5, color);
    painter.add(Shape::line(vec![c + vec2(-3.2, -2.0), c + vec2(0.0, -5.0), c + vec2(3.2, -2.0)], s));
    painter.add(Shape::line(vec![c + vec2(-3.2, 2.0), c + vec2(0.0, 5.0), c + vec2(3.2, 2.0)], s));
}

/// `checkmark`
pub fn checkmark(painter: &Painter, c: Pos2, size: f32, stroke: Stroke) {
    painter.add(Shape::line(
        vec![c + vec2(-1.0, 0.05) * size, c + vec2(-0.3, 0.75) * size, c + vec2(1.0, -0.75) * size],
        stroke,
    ));
}

/// `play.fill`
pub fn play(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    let (w, h) = (0.62 * r, 0.72 * r);
    let points = vec![c + vec2(-0.38 * w, -h), c + vec2(w, 0.0), c + vec2(-0.38 * w, h)];
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// `arrow.clockwise`: a ring open at the top right, the head at the top.
pub fn arrow_clockwise(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    let rr = 0.58 * r;
    let gap = 0.95;
    arc(painter, c, rr, -FRAC_PI_2 + gap, TAU - gap, Stroke::new(1.7, color));
    // Pointing along the turn, which at the top runs to the right.
    let top = c + vec2(0.0, -rr);
    let h = 0.62 * rr;
    let head = vec![top + vec2(h, 0.0), top + vec2(-0.35 * h, -0.8 * h), top + vec2(-0.35 * h, 0.8 * h)];
    painter.add(Shape::convex_polygon(head, color, Stroke::NONE));
}

/// `arrow.down.circle`, used for the empty list.
pub fn arrow_down_circle(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    let s = Stroke::new(2.2, color);
    painter.circle_stroke(c, r, s);
    painter.line_segment([c + vec2(0.0, -0.46 * r), c + vec2(0.0, 0.42 * r)], s);
    painter.add(Shape::line(
        vec![c + vec2(-0.3 * r, 0.12 * r), c + vec2(0.0, 0.44 * r), c + vec2(0.3 * r, 0.12 * r)],
        s,
    ));
}
