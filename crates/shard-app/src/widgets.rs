//! Widgets of the UI kit: icon buttons (accent + glow when active, inverted on hover), block-bar sliders,
//! segmented controls with a sliding block, ON | OFF switches, tabs, section headers.

use std::ops::RangeInclusive;

use egui::{pos2, vec2, Align, Align2, Color32, FontId, Id, Layout, Rect, Response, Sense, Stroke, Ui, UiBuilder};

use crate::icons::{self, Icon};
use crate::theme;

pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, selected: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    paint_icon_button(ui, rect, &resp, icon, selected, enabled);
    resp
}

pub fn paint_icon_button(ui: &Ui, rect: Rect, resp: &Response, icon: Icon, selected: bool, enabled: bool) {
    let p = ui.painter();
    let ctx = ui.ctx();
    let on = ctx.animate_bool_with_time(resp.id.with("on"), selected, theme::anim(0.16));
    let hov = ctx.animate_bool_with_time(resp.id.with("hov"), resp.hovered() && enabled && !selected, theme::anim(0.16));
    // active: accent block with the kit's glow; hover: inverted light block
    theme::glow(p, rect, theme::accent(), 0.5 * on, 6);
    p.rect_filled(rect, 0.0, mix(Color32::TRANSPARENT, theme::accent(), on));
    p.rect_filled(rect, 0.0, mix(Color32::TRANSPARENT, theme::TEXT, hov));
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = if !enabled { theme::TEXT_DISABLED } else { mix(mix(theme::TEXT, theme::BG_VIEWPORT, hov), Color32::WHITE, on) };
    icons::draw(p, rect.shrink(rect.width() * 0.2), icon, col);
}

/// Render mode of the viewport, drawn as a small shaded sphere (Blender's viewport shading buttons).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SphereKind {
    /// The real-time rasterised material (what the viewport shows).
    Raster,
    /// Path-traced: glossy, with a sharp highlight and an environment reflection.
    PathTrace,
}

/// Square button with a shaded sphere; `selected` paints the accent behind it.
pub fn sphere_button(ui: &mut Ui, kind: SphereKind, size: f32, selected: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let hovered = resp.hovered() && enabled;
    if selected {
        p.rect_filled(rect, theme::RADIUS as f32, theme::accent_soft());
        p.rect_stroke(rect.shrink(0.5), theme::RADIUS as f32, Stroke::new(1.0, theme::accent()), egui::StrokeKind::Inside);
    } else if hovered {
        p.rect_filled(rect, theme::RADIUS as f32, theme::WIDGET_HOVER);
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint_sphere(p, rect.center(), size * 0.32, kind, !enabled);
    resp
}

/// A lit sphere: concentric discs from the shadow side to the lit side (light from the upper left).
pub fn paint_sphere(p: &egui::Painter, c: egui::Pos2, r: f32, kind: SphereKind, dim: bool) {
    let (dark, lit) = match kind {
        SphereKind::Raster => (58.0, 205.0),
        SphereKind::PathTrace => (26.0, 150.0),
    };
    let fade = |v: f32| -> Color32 {
        let v = if dim { v * 0.45 + 18.0 } else { v };
        Color32::from_gray(v.clamp(0.0, 255.0) as u8)
    };
    const STEPS: usize = 14;
    for i in 0..STEPS {
        let t = i as f32 / STEPS as f32;
        let rad = r * (1.0 - t * 0.82);
        let off = vec2(-0.32, -0.34) * r * t;
        p.circle_filled(c + off, rad, fade(dark + (lit - dark) * t.powf(1.4)));
    }
    if kind == SphereKind::PathTrace {
        // environment reflected along the lower half, a rim light and a sharp highlight
        let band: Vec<egui::Pos2> =
            (0..=12).map(|k| -> egui::Pos2 { let a = std::f32::consts::PI * (0.12 + 0.76 * k as f32 / 12.0); c + vec2(-a.cos() * r * 0.86, a.sin() * r * 0.36) }).collect();
        p.add(egui::Shape::line(band, Stroke::new(r * 0.16, fade(118.0))));
        p.circle_stroke(c, r - 0.6, Stroke::new(1.0, fade(110.0)));
        p.circle_filled(c + vec2(-0.36, -0.38) * r, r * 0.17, fade(255.0));
    }
}

/// Linear mix of two colours (alpha too).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Segmented control: a dark well, the picked segment is a light block that slides to the new pick. Disabled
/// segments show their reason on hover. Returns the segment clicked this frame.
pub fn segmented(ui: &mut Ui, items: &[(&str, bool, &str)], selected: usize) -> Option<usize> {
    let h = 26.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    segmented_at(ui, rect, items, selected)
}

/// [`segmented`] drawn in a given rect.
pub fn segmented_at(ui: &mut Ui, rect: Rect, items: &[(&str, bool, &str)], selected: usize) -> Option<usize> {
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, theme::WELL);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, theme::BORDER_STRONG), egui::StrokeKind::Inside);
    let n = items.len().max(1) as f32;
    let w = rect.width() / n;
    let id = ui.id().with(("segmented", items.first().map(|i| i.0).unwrap_or("")));
    let x = ui.ctx().animate_value_with_time(id, selected as f32, theme::anim(0.18));
    p.rect_filled(Rect::from_min_size(pos2(rect.min.x + w * x, rect.min.y), vec2(w, rect.height())).shrink(1.0), 0.0, theme::TEXT);
    let mut picked = None;
    for (k, (label, enabled, why)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.min.x + w * k as f32, rect.min.y), vec2(w, rect.height()));
        if k > 0 {
            p.vline(r.min.x, r.y_range(), Stroke::new(1.0, theme::BORDER_STRONG));
        }
        let resp = ui.interact(r, ui.id().with(("segmented", k, *label)), if *enabled { Sense::click() } else { Sense::hover() });
        let sel = k == selected;
        if !sel && resp.hovered() && *enabled {
            p.rect_filled(r.shrink(1.0), 0.0, theme::WIDGET_HOVER);
        }
        // the light block under the label, as it slides
        let under = (1.0 - (x - k as f32).abs()).clamp(0.0, 1.0);
        let col = if !*enabled { theme::TEXT_DISABLED } else { mix(theme::TEXT_DIM, theme::BG_VIEWPORT, under) };
        p.text(r.center(), Align2::CENTER_CENTER, label.to_uppercase(), theme::semibold(8.0), col);
        let resp = if *enabled || why.is_empty() { resp } else { resp.on_hover_text(*why) };
        if resp.clicked() && *enabled {
            picked = Some(k);
        }
    }
    picked
}

/// Small uppercase group caption (popovers, settings pages).
pub fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).font(theme::semibold(10.0)).color(theme::TEXT_DIM));
    ui.add_space(2.0);
}

/// Small flyout triangle in the lower-right corner of a tool button.
pub fn flyout_mark(ui: &Ui, rect: Rect) {
    let p = ui.painter();
    let c = rect.right_bottom() - vec2(3.0, 3.0);
    p.add(egui::Shape::convex_polygon(
        vec![c, c - vec2(4.0, 0.0), c - vec2(0.0, 4.0)],
        theme::TEXT_DIM,
        Stroke::NONE,
    ));
}

pub struct TabResult {
    pub selected: usize,
    pub closed: Option<usize>,
}

/// Uppercase tab strip with a blue underline under the active tab and optional × per tab.
pub fn tab_strip(ui: &mut Ui, labels: &[(&str, bool)], selected: usize) -> TabResult {
    let mut out = TabResult { selected, closed: None };
    let h = 24.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter().clone();
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, theme::BORDER));
    let mut x = rect.min.x + 4.0;
    for (i, (label, closable)) in labels.iter().enumerate() {
        let font = FontId::proportional(10.0);
        let galley = p.layout_no_wrap(label.to_string(), font, theme::TEXT);
        let w = galley.size().x + if *closable { 24.0 } else { 12.0 };
        let tab = Rect::from_min_size(pos2(x, rect.min.y), vec2(w, h));
        let id = ui.id().with(("tab", i, *label));
        let resp = ui.interact(tab, id, Sense::click());
        if resp.clicked() {
            out.selected = i;
        }
        let active = out.selected == i;
        let col = if active { Color32::WHITE } else { theme::TEXT_DIM };
        p.text(
            pos2(tab.min.x + 6.0, tab.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.0),
            col,
        );
        if active {
            p.line_segment(
                [pos2(tab.min.x, tab.max.y - 1.0), pos2(tab.max.x, tab.max.y - 1.0)],
                Stroke::new(2.0, theme::accent()),
            );
        }
        if *closable {
            let xr = Rect::from_center_size(pos2(tab.max.x - 9.0, tab.center().y), vec2(12.0, 12.0));
            let cid = id.with("close");
            let cr = ui.interact(xr, cid, Sense::click());
            icons::draw(&p, xr, Icon::Close, if cr.hovered() { Color32::WHITE } else { theme::TEXT_DIM });
            if cr.clicked() {
                out.closed = Some(i);
            }
        }
        x += w + 2.0;
    }
    out
}

/// Slider of the kit, on one row: the label, the block bar, the value (editable) at the right. Drag or click the
/// bar to set it. Returns true when the value changed.
pub fn slider(ui: &mut Ui, label: &str, v: &mut f32, range: RangeInclusive<f32>, decimals: usize, enabled: bool) -> bool {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let p = ui.painter().clone();
    let (lo, hi) = (*range.start(), *range.end());
    let dim = if enabled { theme::TEXT } else { theme::TEXT_DISABLED };
    let natural = p.layout_no_wrap(label.to_uppercase(), FontId::proportional(10.0), dim).size().x;
    // the bar starts after the label, at least at 42 % of the row so the bars of a form line up; a long name pushes
    // it right but always leaves it some room, and is cut with an ellipsis (full name on hover)
    let label_w = (natural + 14.0).max(rect.width() * 0.42).min(rect.width() - 52.0 - 60.0);
    let mut job = egui::text::LayoutJob::single_section(label.to_uppercase(), egui::TextFormat::simple(FontId::proportional(10.0), dim));
    job.wrap = egui::text::TextWrapping { max_width: label_w - 10.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let cut = galley.size().x + 1.0 < natural;
    p.galley(pos2(rect.min.x + 4.0, rect.center().y - galley.size().y * 0.5), galley, dim);
    if cut {
        let hr = Rect::from_min_size(rect.min, vec2(label_w, rect.height()));
        ui.interact(hr, ui.id().with(("slider_label", label)), Sense::hover()).on_hover_text(label);
    }
    let mut changed = false;
    let vrect = Rect::from_min_size(pos2(rect.max.x - 52.0, rect.min.y + 3.0), vec2(48.0, 20.0));
    {
        // child Ui that does not advance the parent's layout cursor
        let mut child = ui.new_child(UiBuilder::new().max_rect(vrect).layout(Layout::right_to_left(Align::Center)));
        if !enabled {
            child.disable();
        }
        let vis = child.visuals_mut();
        for ws in [&mut vis.widgets.inactive, &mut vis.widgets.hovered, &mut vis.widgets.active] {
            ws.weak_bg_fill = Color32::TRANSPARENT;
            ws.bg_fill = Color32::TRANSPARENT;
            ws.bg_stroke = Stroke::NONE;
            ws.fg_stroke = Stroke::new(1.0, theme::VALUE);
        }
        let span = (hi - lo).max(1e-6);
        let r = child.add(egui::DragValue::new(v).range(lo..=hi).speed(span / 300.0).max_decimals(decimals).min_decimals(decimals));
        changed |= r.changed();
    }
    let id = ui.id().with(("slider", label));
    changed |= slider_track(ui, id, rect.min.x + label_w, vrect.min.x - 8.0, rect.center().y, v, lo, hi, enabled);
    changed
}

/// The kit's block bar on the line `y` between `x0` and `x1`; drag or click sets `v`.
pub fn slider_track(ui: &mut Ui, id: Id, x0: f32, x1: f32, y: f32, v: &mut f32, lo: f32, hi: f32, enabled: bool) -> bool {
    let p = ui.painter().clone();
    let mut changed = false;
    let hit = Rect::from_min_max(pos2(x0 - 6.0, y - 8.0), pos2(x1 + 6.0, y + 8.0));
    let resp = ui.interact(hit, id, if enabled { Sense::click_and_drag() } else { Sense::hover() });
    if enabled && (resp.dragged() || resp.clicked()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0);
            let nv = lo + t * (hi - lo);
            if (nv - *v).abs() > f32::EPSILON {
                *v = nv;
                changed = true;
            }
        }
    }
    // block bar: light fill (accent while dragging) that glides to the value, a dithered leading edge, a dotted rest
    let t = ((*v - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    let shown = ui.ctx().animate_value_with_time(id.with("shown"), t, if resp.dragged() { 0.0 } else { theme::anim(0.12) });
    let drag = ui.ctx().animate_bool_with_time(id.with("drag"), resp.dragged(), theme::anim(0.16));
    let fill = if enabled { mix(theme::TEXT, theme::accent(), drag) } else { theme::SLIDER_FILL_OFF };
    let bar = Rect::from_min_max(pos2(x0, y - 4.0), pos2(x1, y + 4.0));
    blocks(&p, bar, shown, fill, if enabled { theme::SLIDER_TRACK } else { theme::SLIDER_TRACK_OFF });
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    changed
}

/// The kit's block bar (sliders, progress): solid fill up to `frac`, a dithered 8 px leading edge, faint dots
/// over the `track` for the rest.
pub fn blocks(p: &egui::Painter, r: Rect, frac: f32, fill: Color32, track: Color32) {
    let r = Rect::from_min_max(r.min.round(), r.max.round());
    let fx = (r.left() + r.width() * frac.clamp(0.0, 1.0)).round();
    p.rect_filled(Rect::from_min_max(pos2(fx, r.top()), r.max), 0.0, track);
    p.rect_filled(Rect::from_min_max(r.min, pos2(fx, r.bottom())), 0.0, fill);
    let dot = mix(track, theme::TEXT, 0.18);
    let edge = mix(fill, Color32::TRANSPARENT, 0.3);
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = fx + (row % 2) as f32 * 2.0;
        while x < r.right() {
            let on_edge = x < fx + 8.0;
            p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(1.0, 1.0)).intersect(r), 0.0, if on_edge { edge } else { dot });
            x += if on_edge { 2.0 } else { 4.0 };
        }
        y += 2.0;
        row += 1;
    }
}

/// Editable number without chrome, right aligned inside `rect`.
pub fn value_box(ui: &mut Ui, rect: Rect, v: &mut f32, lo: f32, hi: f32, decimals: usize) -> bool {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)));
    let vis = child.visuals_mut();
    for ws in [&mut vis.widgets.inactive, &mut vis.widgets.hovered, &mut vis.widgets.active] {
        ws.weak_bg_fill = Color32::TRANSPARENT;
        ws.bg_fill = Color32::TRANSPARENT;
        ws.bg_stroke = Stroke::NONE;
        ws.fg_stroke = Stroke::new(1.0, theme::VALUE);
    }
    let span = (hi - lo).max(1e-6);
    child
        .add(egui::DragValue::new(v).range(lo..=hi).speed(span / 300.0).max_decimals(decimals).min_decimals(decimals))
        .changed()
}

// ---------------------------------------------------------------- property form
// Layout of Painter's PROPERTIES tab: sections separated by a line, a fixed label column, controls that
// fill the rest of the row, sub-sections with a chevron and an indented body.

/// Left padding of titles, labels and controls inside the form.
pub const FORM_PAD: f32 = 6.0;
/// Width of the label column.
pub const FORM_LABEL_W: f32 = 128.0;
const FORM_ROW_H: f32 = 26.0;

/// Section title with a separator line above it (not for the first one).
pub fn form_title(ui: &mut Ui, title: &str, first: bool, menu: bool) {
    if !first {
        ui.add_space(8.0);
        let y = ui.cursor().min.y;
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], Stroke::new(1.0, theme::BORDER));
        ui.add_space(10.0);
    } else {
        ui.add_space(4.0);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    ui.painter().text(
        pos2(rect.min.x + FORM_PAD, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(10.0),
        Color32::WHITE,
    );
    if menu {
        let r = Rect::from_center_size(pos2(rect.max.x - 12.0, rect.center().y), vec2(16.0, 16.0));
        icons::draw(ui.painter(), r, Icon::Menu, theme::TEXT_DIM);
    }
}

/// Sub-section header (chevron + name); the open state persists. The body goes in [`form_indent`].
pub fn form_sub(ui: &mut Ui, id_salt: &str, title: &str, default_open: bool) -> bool {
    let id = ui.id().with(("form_sub", id_salt));
    let mut open = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    if resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    }
    icons::draw(
        ui.painter(),
        Rect::from_center_size(pos2(rect.min.x + FORM_PAD + 6.0, rect.center().y), vec2(14.0, 14.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        theme::TEXT_DIM,
    );
    ui.painter().text(
        pos2(rect.min.x + FORM_PAD + 20.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(10.0),
        theme::TEXT,
    );
    open
}

/// Indented body of a sub-section.
pub fn form_indent(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }).show(ui, add);
}

/// One labelled row: label column, then `add` in the remaining width.
pub fn form_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(FORM_PAD);
        let (r, _) = ui.allocate_exact_size(vec2(FORM_LABEL_W, FORM_ROW_H), Sense::hover());
        ui.painter().text(
            pos2(r.min.x, r.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.0),
            theme::TEXT,
        );
        let width = (ui.available_width() - 6.0).max(40.0);
        ui.allocate_ui_with_layout(vec2(width, FORM_ROW_H), Layout::left_to_right(Align::Center), |ui| {
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}

/// Combo row; returns the index picked this frame.
pub fn form_combo(ui: &mut Ui, id: &str, label: &str, current: &str, items: &[&str], enabled: bool) -> Option<usize> {
    let mut picked = None;
    form_row(ui, label, |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let width = ui.available_width();
            egui::ComboBox::from_id_salt(id).icon(combo_icon).width(width - 4.0).selected_text(current).show_ui(ui, |ui| {
                for (k, it) in items.iter().enumerate() {
                    if ui.selectable_label(*it == current, *it).clicked() {
                        picked = Some(k);
                    }
                }
            });
        });
    });
    picked
}

/// On/off switch at the right of a labelled row; true when it was clicked.
pub fn form_toggle(ui: &mut Ui, label: &str, on: bool) -> bool {
    form_row(ui, label, |ui| switch(ui, on, true).on_hover_text(if on { "On" } else { "Off" }).clicked())
}

/// ON | OFF switch: a small segmented control, the light block on the current state. Clicking anywhere on
/// it flips the state (the caller reads `clicked()`).
pub fn switch(ui: &mut Ui, on: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme::WELL);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, theme::BORDER_STRONG), egui::StrokeKind::Inside);
    let x = ui.ctx().animate_bool_with_time(resp.id.with("sw"), !on, theme::anim(0.16));
    let half = rect.width() / 2.0;
    let block = Rect::from_min_size(pos2(rect.min.x + x * half, rect.min.y), vec2(half, rect.height())).shrink(1.0);
    p.rect_filled(block, 0.0, if enabled { theme::TEXT } else { theme::SLIDER_FILL_OFF });
    p.vline(rect.center().x, rect.y_range(), Stroke::new(1.0, theme::BORDER_STRONG));
    for (k, s) in ["ON", "OFF"].into_iter().enumerate() {
        let c = pos2(rect.min.x + half * (k as f32 + 0.5), rect.center().y);
        let under = (1.0 - (x - k as f32).abs()).clamp(0.0, 1.0);
        let col = if enabled { mix(theme::TEXT_DIM, theme::BG_VIEWPORT, under) } else { theme::TEXT_DISABLED };
        p.text(c, Align2::CENTER_CENTER, s, theme::semibold(8.0), col);
    }
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Painter slider inset to the form's padding.
pub fn form_slider(ui: &mut Ui, label: &str, v: &mut f32, range: RangeInclusive<f32>, decimals: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.vertical(|ui| {
            let wd = ui.available_width() - 4.0;
            ui.set_width(wd);
            changed = slider(ui, label, v, range, decimals, true);
        });
    });
    changed
}

/// Two sliders on one row (X / Y of tiling or offset) after the label column, with an optional lock
/// button between them. `lock`: Some(state) shows the toggle and keeps both values equal while on.
pub fn form_pair(
    ui: &mut Ui,
    label: &str,
    a: &mut f32,
    b: &mut f32,
    range: RangeInclusive<f32>,
    decimals: usize,
    lock: Option<&mut bool>,
) -> bool {
    let (lo, hi) = (*range.start(), *range.end());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
    ui.painter().text(
        pos2(rect.min.x + FORM_PAD, rect.max.y - 10.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(10.0),
        theme::TEXT,
    );
    let x0 = rect.min.x + FORM_PAD + FORM_LABEL_W;
    let x1 = rect.max.x - 10.0;
    let lock_w = if lock.is_some() { 30.0 } else { 14.0 };
    let half = ((x1 - x0) - lock_w) * 0.5;
    let y = rect.max.y - 10.0;
    let (before_a, before_b) = (*a, *b);
    let mut changed = false;
    for (k, (v, sx)) in [(&mut *a, x0), (&mut *b, x0 + half + lock_w)].into_iter().enumerate() {
        let vr = Rect::from_min_size(pos2(sx + half - 60.0, rect.min.y), vec2(60.0, 18.0));
        changed |= value_box(ui, vr, v, lo, hi, decimals);
        let id = ui.id().with(("form_pair", label, k));
        changed |= slider_track(ui, id, sx + 6.0, sx + half - 6.0, y, v, lo, hi, true);
    }
    if let Some(l) = lock {
        let lr = Rect::from_center_size(pos2(x0 + half + lock_w * 0.5, y - 2.0), vec2(24.0, 22.0));
        let resp = ui.interact(lr, ui.id().with(("form_lock", label)), Sense::click());
        if resp.clicked() {
            *l = !*l;
        }
        let p = ui.painter();
        p.rect_filled(lr, 3.0, if *l { Color32::from_gray(70) } else { theme::WIDGET });
        let c = lr.center();
        let col = if *l { Color32::WHITE } else { theme::TEXT_DIM };
        // padlock: body + shackle
        p.rect_filled(Rect::from_center_size(pos2(c.x, c.y + 2.0), vec2(10.0, 7.0)), 1.5, col);
        p.line_segment([pos2(c.x - 3.0, c.y - 1.0), pos2(c.x - 3.0, c.y - 4.0)], Stroke::new(1.4, col));
        p.line_segment([pos2(c.x + 3.0, c.y - 1.0), pos2(c.x + 3.0, c.y - 4.0)], Stroke::new(1.4, col));
        p.line_segment([pos2(c.x - 3.0, c.y - 4.0), pos2(c.x + 3.0, c.y - 4.0)], Stroke::new(1.4, col));
        if *l {
            if (*a - before_a).abs() > 1e-6 {
                *b = *a;
            } else if (*b - before_b).abs() > 1e-6 {
                *a = *b;
            }
        }
    }
    changed || *a != before_a || *b != before_b
}

/// Collapsible section header: chevron + title + reset icon. Returns (open, reset_clicked).
pub fn section(ui: &mut Ui, id_salt: &str, title: &str, default_open: bool) -> (bool, bool, Rect) {
    let id = ui.id().with(("section", id_salt));
    let mut open = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    let p = ui.painter().clone();
    if resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    }
    icons::draw(
        &p,
        Rect::from_center_size(pos2(rect.min.x + 9.0, rect.center().y), vec2(14.0, 14.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        theme::TEXT_DIM,
    );
    p.text(
        pos2(rect.min.x + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(10.0),
        theme::TEXT,
    );
    let rr = Rect::from_center_size(pos2(rect.max.x - 12.0, rect.center().y), vec2(16.0, 16.0));
    let rresp = ui.interact(rr, id.with("reset"), Sense::click());
    icons::draw(&p, rr, Icon::Reset, if rresp.hovered() { Color32::WHITE } else { theme::TEXT_DIM });
    (open, rresp.clicked(), rect)
}

/// Uppercase title row with optional right-aligned menu icon.
pub fn title_row(ui: &mut Ui, title: &str, menu: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    ui.painter().text(
        pos2(rect.min.x + 4.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(10.0),
        Color32::WHITE,
    );
    if menu {
        let r = Rect::from_center_size(pos2(rect.max.x - 12.0, rect.center().y), vec2(16.0, 16.0));
        icons::draw(ui.painter(), r, Icon::Menu, theme::TEXT_DIM);
    }
    resp
}

pub fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 7.0), Sense::hover());
    ui.painter().line_segment(
        [pos2(rect.min.x, rect.center().y), pos2(rect.max.x, rect.center().y)],
        Stroke::new(1.0, theme::BORDER),
    );
}

#[allow(dead_code)]
pub fn unique(ui: &Ui, s: &str) -> Id {
    ui.id().with(s)
}

/// Checkbox in the kit: on hover the box keeps its fill and lights its border, the label stays light
/// (egui's inverted hover painted the label dark over the dark panel).
pub fn checkbox(ui: &mut Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.scope(|ui| {
        let v = &mut ui.visuals_mut().widgets;
        v.hovered.bg_fill = theme::WIDGET;
        v.hovered.weak_bg_fill = theme::WIDGET;
        v.hovered.bg_stroke = Stroke::new(1.0, theme::accent());
        v.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        ui.checkbox(on, text)
    })
    .inner
}

/// Toggle that repeats along a drag: pressing one item flips it, and every other item of the same `group`
/// the pointer passes over while the button is held takes that same state (hide / show many eyes in one
/// stroke). Returns the new state when this item must change.
pub fn drag_toggle(ui: &Ui, group: &str, resp: &Response, on: bool) -> Option<bool> {
    let key = Id::new(("drag_toggle", group));
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
    if !down {
        ui.ctx().data_mut(|d| d.remove::<bool>(key));
        return None;
    }
    let target = ui.ctx().data(|d| d.get_temp::<bool>(key));
    match target {
        None if resp.is_pointer_button_down_on() => {
            ui.ctx().data_mut(|d| d.insert_temp(key, !on));
            Some(!on)
        }
        Some(t) if t != on && pos.is_some_and(|p| resp.rect.contains(p)) => Some(t),
        _ => None,
    }
}

/// True while a `drag_toggle` stroke of `group` is going on (the button still held).
pub fn drag_toggle_active(ctx: &egui::Context, group: &str) -> bool {
    ctx.input(|i| i.pointer.primary_down()) && ctx.data(|d| d.get_temp::<bool>(Id::new(("drag_toggle", group))).is_some())
}

/// Drop arrow of every combo: always pointing down, like the rest of the kit (egui turns it up when the list
/// has to open above, which made the last rows of a list look different).
pub fn combo_icon(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, _open: bool, _side: egui::AboveOrBelow) {
    let r = Rect::from_center_size(rect.center(), vec2(rect.width() * 0.7, rect.height() * 0.45));
    ui.painter().add(egui::Shape::convex_polygon(
        vec![r.left_top(), r.right_top(), r.center_bottom()],
        visuals.fg_stroke.color,
        Stroke::NONE,
    ));
}
