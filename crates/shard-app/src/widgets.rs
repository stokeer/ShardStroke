//! Painter-style widgets: tabs, thin sliders, section headers, icon buttons.

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
    let r = theme::RADIUS as f32;
    if selected {
        p.rect_filled(rect, r, theme::accent_soft());
    } else if resp.hovered() && enabled {
        p.rect_filled(rect, r, theme::WIDGET_HOVER);
    }
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = if !enabled {
        theme::TEXT_DISABLED
    } else if selected {
        theme::accent_hover()
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        theme::TEXT
    };
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

/// Segmented control: equal segments on a well, the selected one in the accent. Disabled segments show their
/// reason on hover. Returns the segment clicked this frame.
pub fn segmented(ui: &mut Ui, items: &[(&str, bool, &str)], selected: usize) -> Option<usize> {
    let h = 26.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::RADIUS as f32 + 1.0, theme::WELL);
    let n = items.len().max(1) as f32;
    let w = (rect.width() - 4.0) / n;
    let mut picked = None;
    for (k, (label, enabled, why)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.min.x + 2.0 + w * k as f32, rect.min.y + 2.0), vec2(w, h - 4.0));
        let resp = ui.interact(r, ui.id().with(("segmented", k, *label)), if *enabled { Sense::click() } else { Sense::hover() });
        let sel = k == selected;
        if sel {
            p.rect_filled(r, theme::RADIUS as f32, theme::accent());
        } else if resp.hovered() && *enabled {
            p.rect_filled(r, theme::RADIUS as f32, theme::WIDGET_HOVER);
        }
        let col = if sel {
            theme::on_accent()
        } else if !*enabled {
            theme::TEXT_DISABLED
        } else {
            theme::TEXT
        };
        p.text(r.center(), Align2::CENTER_CENTER, *label, theme::semibold(11.5), col);
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
    ui.label(egui::RichText::new(text).font(theme::semibold(10.5)).color(theme::TEXT_DIM));
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
        let font = FontId::proportional(11.0);
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
            FontId::proportional(11.0),
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

/// Painter slider: label left, value right (editable), thin track with a round handle below.
/// Returns true when the value changed.
pub fn slider(ui: &mut Ui, label: &str, v: &mut f32, range: RangeInclusive<f32>, decimals: usize, enabled: bool) -> bool {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    let p = ui.painter().clone();
    let (lo, hi) = (*range.start(), *range.end());
    let dim = if enabled { theme::TEXT } else { theme::TEXT_DISABLED };
    p.text(pos2(rect.min.x + 4.0, rect.min.y + 9.0), Align2::LEFT_CENTER, label, FontId::proportional(11.5), dim);
    let mut changed = false;
    // value (DragValue without chrome)
    let vrect = Rect::from_min_size(pos2(rect.max.x - 70.0, rect.min.y), vec2(66.0, 18.0));
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
        let r = child.add(
            egui::DragValue::new(v)
                .range(lo..=hi)
                .speed(span / 300.0)
                .max_decimals(decimals)
                .min_decimals(decimals),
        );
        changed |= r.changed();
    }
    // track
    let id = ui.id().with(("slider", label));
    changed |= slider_track(ui, id, rect.min.x + 8.0, rect.max.x - 8.0, rect.max.y - 9.0, v, lo, hi, enabled);
    changed
}

/// Thin track with a round handle on the line `y` between `x0` and `x1`; drag or click sets `v`.
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
    let t = ((*v - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    let hx = x0 + t * (x1 - x0);
    let track = if enabled { theme::SLIDER_TRACK } else { theme::SLIDER_TRACK_OFF };
    let fill = if enabled { theme::accent() } else { theme::SLIDER_FILL_OFF };
    let groove = Rect::from_min_max(pos2(x0, y - 2.0), pos2(x1, y + 2.0));
    p.rect_filled(groove, 2.0, track);
    if hx > x0 {
        p.rect_filled(Rect::from_min_max(pos2(x0, y - 2.0), pos2(hx, y + 2.0)), 2.0, fill);
    }
    let hot = resp.hovered() || resp.dragged();
    let rad = if hot && enabled { 7.0 } else { 6.0 };
    if enabled {
        // soft halo under the handle
        p.circle_filled(pos2(hx, y), rad + 2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 60));
    }
    p.circle_filled(pos2(hx, y), rad, if enabled { Color32::WHITE } else { theme::SLIDER_FILL_OFF });
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    changed
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
pub const FORM_LABEL_W: f32 = 92.0;
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
        theme::semibold(12.5),
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
        theme::semibold(12.0),
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
            FontId::proportional(11.5),
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
            egui::ComboBox::from_id_salt(id).width(width - 4.0).selected_text(current).show_ui(ui, |ui| {
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

/// Pill switch (accent track when on).
pub fn switch(ui: &mut Ui, on: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let (track, knob_x) = if on { (theme::accent(), rect.max.x - 10.0) } else { (theme::SLIDER_TRACK, rect.min.x + 10.0) };
    let track = if !enabled {
        theme::SLIDER_TRACK_OFF
    } else if resp.hovered() && !on {
        theme::WIDGET_HOVER
    } else {
        track
    };
    p.rect_filled(rect, 10.0, track);
    p.circle_filled(pos2(knob_x, rect.center().y), 7.0, if enabled { Color32::WHITE } else { theme::TEXT_DISABLED });
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
        FontId::proportional(11.5),
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
        theme::semibold(12.0),
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
        theme::semibold(11.5),
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
