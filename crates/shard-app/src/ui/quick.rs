//! Quick panel: a right click (no modifier, no drag) in the viewport opens colour, brush sliders and the
//! recently used colours / brushes at the cursor, ZBrush-style. Beyond Painter, whose right click only
//! shows a copy of the properties window.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, UiBuilder};
use shard_model::{Effect, EffectKind, Layer, Source};

use super::AppState;
use crate::icons::Icon;
use crate::paint::Tool;
use crate::theme;
use crate::widgets as w;

const RECENT_COLORS: usize = 16;
const RECENT_BRUSHES: usize = 8;
const PALETTE_MAX: usize = 32;
const WIDTH: f32 = 480.0;
/// A transient panel closes when the pointer goes this far from it (points).
const LEAVE_DIST: f32 = 160.0;

/// The blocks of the panel; the artist chooses which show and in what order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Color,
    Brush,
    RecentColors,
    ProjectColors,
    RecentBrushes,
}

impl Section {
    const ALL: [Section; 5] =
        [Section::Color, Section::Brush, Section::RecentColors, Section::ProjectColors, Section::RecentBrushes];

    fn key(self) -> &'static str {
        match self {
            Section::Color => "color",
            Section::Brush => "brush",
            Section::RecentColors => "recent_colors",
            Section::ProjectColors => "project_colors",
            Section::RecentBrushes => "recent_brushes",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Section::Color => "Colour picker",
            Section::Brush => "Brush and tool",
            Section::RecentColors => "Recent colours",
            Section::ProjectColors => "Project colours",
            Section::RecentBrushes => "Recent brushes",
        }
    }
}

/// Sections in display order with their visibility, from the config text (`color,-brush,...`); sections the
/// text does not name (older configs) are added at the end, shown.
fn sections(text: &str) -> Vec<(Section, bool)> {
    let mut out: Vec<(Section, bool)> = Vec::new();
    for tok in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let (key, on) = match tok.strip_prefix('-') {
            Some(k) => (k, false),
            None => (tok, true),
        };
        if let Some(s) = Section::ALL.into_iter().find(|s| s.key() == key) {
            if !out.iter().any(|(x, _)| *x == s) {
                out.push((s, on));
            }
        }
    }
    for s in Section::ALL {
        if !out.iter().any(|(x, _)| *x == s) {
            out.push((s, true));
        }
    }
    out
}

fn sections_text(secs: &[(Section, bool)]) -> String {
    secs.iter().map(|(s, on)| format!("{}{}", if *on { "" } else { "-" }, s.key())).collect::<Vec<_>>().join(",")
}

pub struct QuickPanel {
    pub open: bool,
    /// The next left click in the viewport samples a colour instead of painting.
    pub picking: bool,
    /// Top-left corner (points) and the size drawn last frame.
    pos: Pos2,
    size: egui::Vec2,
    /// Second colour, exchanged with the brush colour (ZBrush's SwitchColor).
    alt_color: [f32; 3],
    /// HSV of the brush colour (sRGB space); kept so the hue survives a grey or black colour.
    hsv: [f32; 3],
    hsv_of: [f32; 3],
    /// Base colours of the active texture set's uniform fills (sRGB), gathered when the panel opens.
    palette: Vec<[u8; 3]>,
    /// The body lists the sections to show, hide and reorder instead of the tools.
    arranging: bool,
}

impl Default for QuickPanel {
    fn default() -> Self {
        QuickPanel {
            open: false,
            picking: false,
            pos: Pos2::ZERO,
            size: egui::Vec2::ZERO,
            alt_color: [1.0, 1.0, 1.0],
            hsv: [0.0, 0.0, 0.0],
            hsv_of: [-1.0; 3],
            palette: Vec::new(),
            arranging: false,
        }
    }
}

fn to8(c: [f32; 3]) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

fn color32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, if max > 0.0 { d / max } else { 0.0 }, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = |n: f32| {
        let k = (n + h6).rem_euclid(6.0);
        v - v * s * (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// Base colours (sRGB) of the uniform fills of `layers`, in stack order, without repeats.
fn palette_of(layers: &[Layer], out: &mut Vec<[u8; 3]>) {
    fn effects(es: &[Effect], out: &mut Vec<[u8; 3]>) {
        for e in es {
            match &e.kind {
                EffectKind::Fill(f) => {
                    for s in &f.sources {
                        if let Source::Uniform { channels, color: Some(c), .. } = s {
                            let c8 = to8(c.map(shard_compose::linear_to_srgb));
                            if channels.contains(0) && !out.contains(&c8) && out.len() < PALETTE_MAX {
                                out.push(c8);
                            }
                        }
                    }
                }
                EffectKind::Group { children } => effects(children, out),
                _ => {}
            }
        }
    }
    for l in layers {
        effects(&l.content, out);
        palette_of(&l.children, out);
    }
}

/// A colour swatch; returns its response (click = use the colour).
fn swatch(ui: &mut egui::Ui, c: [u8; 3], size: f32, current: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    p.rect_filled(r, 4.0, color32(c));
    let edge = if current {
        Stroke::new(2.0, theme::accent())
    } else if resp.hovered() {
        Stroke::new(1.5, Color32::WHITE)
    } else {
        Stroke::new(1.0, theme::BORDER)
    };
    p.rect_stroke(r, 4.0, edge, egui::StrokeKind::Inside);
    resp.on_hover_text(hex(c))
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(10.5).color(theme::TEXT_DIM));
}

impl AppState {
    /// Right click (no modifier, no drag) over the viewport at `at` (points): open the panel there, or close
    /// it again (a pinned panel stays).
    pub fn quick_click(&mut self, at: Pos2) {
        if self.quick.open {
            if !self.config.quick_pinned {
                self.quick.open = false;
                self.quick.picking = false;
            }
            return;
        }
        self.quick.open = true;
        self.quick.picking = false;
        let size = if self.quick.size.x > 0.0 { self.quick.size } else { vec2(WIDTH, 360.0) };
        self.quick.pos = match self.config.quick_pos {
            Some([x, y]) if self.config.quick_pinned => pos2(x, y),
            _ => at - size * 0.5,
        };
        let mut pal = Vec::new();
        if let Some(st) = self.stacks.get(self.active()) {
            palette_of(&st.layers, &mut pal);
        }
        self.quick.palette = pal;
    }

    /// A left click over the viewport (Alt not held): true when the panel takes it, so it does not paint.
    /// The eyedropper samples the colour under it; a transient panel closes.
    pub fn quick_takes_click(&mut self, vp: Option<(glam::Vec2, f32)>) -> bool {
        if self.quick.picking {
            self.quick.picking = false;
            if let Some(c) = vp.and_then(|(ndc, aspect)| self.scene.sample_color(aspect, ndc)) {
                self.quick.alt_color = self.scene.brush.color;
                self.scene.brush.color = c;
                self.status = format!("Picked colour {}", hex(to8(c)));
            }
            return true;
        }
        if self.quick.open && !self.config.quick_pinned {
            self.quick.open = false;
            return true;
        }
        false
    }

    /// A stroke starts: the colour joins the recent ones (painting colour, not masks or the eraser).
    pub(super) fn quick_note_stroke(&mut self) {
        if self.mask_edit.is_some() || self.scene.tool != Tool::Paint {
            return;
        }
        let c = to8(self.scene.brush.color);
        let recent = &mut self.config.recent_colors;
        if recent.first() == Some(&c) {
            return;
        }
        recent.retain(|x| *x != c);
        recent.insert(0, c);
        recent.truncate(RECENT_COLORS);
        self.config.save();
    }

    /// A stroke ended: its brush settings join the recent ones.
    pub(super) fn quick_note_brush(&mut self) {
        let b = &self.scene.brush;
        let r2 = |v: f32| (v * 100.0).round() / 100.0;
        let snap = [r2(b.radius), r2(b.hardness), r2(b.flow), r2(b.opacity), r2(b.spacing)];
        let recent = &mut self.config.recent_brushes;
        if recent.first() == Some(&snap) {
            return;
        }
        recent.retain(|x| *x != snap);
        recent.insert(0, snap);
        recent.truncate(RECENT_BRUSHES);
        self.config.save();
    }

    pub(super) fn quick_panel(&mut self, ctx: &egui::Context) {
        if self.quick.picking {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if !self.quick.open {
            return;
        }
        let pinned = self.config.quick_pinned;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.quick.picking {
                self.quick.picking = false;
            } else if !pinned {
                self.quick.open = false;
                return;
            }
        }
        // keep the hue when the brush colour changed elsewhere
        let c = self.scene.brush.color;
        if c != self.quick.hsv_of {
            let hsv = rgb_to_hsv(c);
            let keep_hue = hsv[1] <= 0.0 || hsv[2] <= 0.0;
            self.quick.hsv = [if keep_hue { self.quick.hsv[0] } else { hsv[0] }, hsv[1], hsv[2]];
            self.quick.hsv_of = c;
        }
        let screen = ctx.screen_rect();
        let size = if self.quick.size.x > 0.0 { self.quick.size } else { vec2(WIDTH, 360.0) };
        let lo = screen.min + vec2(8.0, 8.0);
        let hi = (screen.max - size - vec2(8.0, 8.0)).max(lo);
        self.quick.pos = self.quick.pos.clamp(lo, hi);
        let frame = egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::same(10))
            .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 32, spread: 0, color: Color32::from_black_alpha(150) });
        let mut close = false;
        let shown = egui::Area::new(egui::Id::new("quick_panel"))
            // above the floating dock windows (Foreground); its own tooltips still draw over it
            .order(egui::Order::Tooltip)
            .fade_in(false)
            .fixed_pos(self.quick.pos)
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(WIDTH - 20.0);
                    close = self.quick_header(ui);
                    ui.add_space(6.0);
                    if self.quick.arranging {
                        self.quick_arrange(ui);
                    } else {
                        self.quick_body(ui);
                    }
                });
            });
        let rect = shown.response.rect;
        self.quick.size = rect.size();
        if close {
            self.quick.open = false;
            self.quick.picking = false;
            return;
        }
        if !pinned {
            // a click elsewhere in the program, or the pointer wandering off, dismisses it
            let (pressed, down, at) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.any_down(), i.pointer.latest_pos()));
            if let Some(at) = at {
                let outside = !rect.contains(at);
                let far = !rect.expand(LEAVE_DIST).contains(at);
                if (pressed && outside && !self.quick.picking) || (!down && far && !self.quick.picking) {
                    self.quick.open = false;
                }
            }
        }
    }

    /// Title, drag handle, pin and close. Returns true when closed.
    fn quick_header(&mut self, ui: &mut egui::Ui) -> bool {
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        // the handle leaves out the buttons: a press on a click-only button would otherwise drag the panel below it
        let handle = Rect::from_min_max(bar.min, pos2(bar.max.x - 80.0, bar.max.y));
        let drag = ui.interact(handle, ui.id().with("quick_handle"), Sense::drag());
        if drag.dragged() {
            self.quick.pos += drag.drag_delta();
        }
        if drag.drag_stopped() && self.config.quick_pinned {
            self.config.quick_pos = Some([self.quick.pos.x, self.quick.pos.y]);
            self.config.save();
        }
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        ui.painter().text(
            pos2(bar.min.x + 2.0, bar.center().y),
            Align2::LEFT_CENTER,
            "Quick panel",
            theme::semibold(12.0),
            theme::TEXT,
        );
        let mut close = false;
        let mut row = ui.new_child(UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
        row.spacing_mut().item_spacing.x = 4.0;
        if w::icon_button(&mut row, Icon::Close, 20.0, false, true).on_hover_text("Close (Esc)").clicked() {
            close = true;
        }
        let arranging = self.quick.arranging;
        let tip = "Choose and order the sections";
        if w::icon_button(&mut row, Icon::Menu, 20.0, arranging, true).on_hover_text(tip).clicked() {
            self.quick.arranging = !arranging;
        }
        let pinned = self.config.quick_pinned;
        let tip = if pinned { "Unpin: close after use" } else { "Pin: keep open as a floating window" };
        if w::icon_button(&mut row, Icon::Pin, 20.0, pinned, true).on_hover_text(tip).clicked() {
            self.config.quick_pinned = !pinned;
            self.config.quick_pos = (!pinned).then(|| [self.quick.pos.x, self.quick.pos.y]);
            self.config.save();
        }
        close
    }

    /// The visible sections in the artist's order; the colour picker and the brush sit side by side when
    /// they follow each other.
    fn quick_body(&mut self, ui: &mut egui::Ui) {
        let vis: Vec<Section> =
            sections(&self.config.quick_sections).into_iter().filter(|(_, on)| *on).map(|(s, _)| s).collect();
        if vis.is_empty() {
            caption(ui, "Every section is hidden: the menu button above shows them again.");
            return;
        }
        let mut i = 0;
        while i < vis.len() {
            if i > 0 {
                w::separator(ui);
            }
            let pair = matches!(
                (vis[i], vis.get(i + 1)),
                (Section::Color, Some(Section::Brush)) | (Section::Brush, Some(Section::Color))
            );
            if pair {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let other = ui.available_width() - 222.0 - 14.0;
                    for s in [vis[i], vis[i + 1]] {
                        ui.vertical(|ui| {
                            ui.set_width(if s == Section::Color { 222.0 } else { other });
                            self.quick_section(ui, s);
                        });
                    }
                });
                i += 2;
            } else {
                self.quick_section(ui, vis[i]);
                i += 1;
            }
        }
    }

    fn quick_section(&mut self, ui: &mut egui::Ui, s: Section) {
        match s {
            Section::Color => self.quick_color(ui),
            Section::Brush => self.quick_sliders(ui),
            Section::RecentColors => {
                let recent = self.config.recent_colors.clone();
                self.quick_swatch_row(ui, "Recent colours", &recent, "Colours you paint with appear here.");
            }
            Section::ProjectColors => {
                let pal = self.quick.palette.clone();
                self.quick_swatch_row(ui, "Project colours", &pal, "No uniform base colours in this texture set.");
            }
            Section::RecentBrushes => self.quick_brushes(ui),
        }
    }

    /// Show, hide and reorder the sections.
    fn quick_arrange(&mut self, ui: &mut egui::Ui) {
        let mut secs = sections(&self.config.quick_sections);
        let mut changed = false;
        caption(ui, "Sections of the quick panel, top to bottom");
        ui.add_space(4.0);
        let n = secs.len();
        let mut swap: Option<(usize, usize)> = None;
        for k in 0..n {
            let (s, on) = secs[k];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if w::switch(ui, on, true).clicked() {
                    secs[k].1 = !on;
                    changed = true;
                }
                ui.label(egui::RichText::new(s.label()).color(if on { theme::TEXT } else { theme::TEXT_DIM }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if w::icon_button(ui, Icon::ChevronDown, 22.0, false, k + 1 < n).on_hover_text("Move down").clicked()
                        && k + 1 < n
                    {
                        swap = Some((k, k + 1));
                    }
                    if w::icon_button(ui, Icon::ChevronUp, 22.0, false, k > 0).on_hover_text("Move up").clicked() && k > 0 {
                        swap = Some((k, k - 1));
                    }
                });
            });
            ui.add_space(2.0);
        }
        if let Some((a, b)) = swap {
            secs.swap(a, b);
            changed = true;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Reset").on_hover_text("Every section, in the default order").clicked() {
                secs = sections("");
                changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    self.quick.arranging = false;
                }
            });
        });
        if changed {
            let text = sections_text(&secs);
            self.config.quick_sections = if text == sections_text(&sections("")) { String::new() } else { text };
            self.config.save();
        }
    }

    /// Saturation / value square, hue strip, current and second colour, eyedropper.
    fn quick_color(&mut self, ui: &mut egui::Ui) {
        const N: usize = 16;
        let [h, s, v] = self.quick.hsv;
        let (area, _) = ui.allocate_exact_size(vec2(222.0, 150.0), Sense::hover());
        let sq = Rect::from_min_size(area.min, vec2(194.0, 150.0));
        let strip = Rect::from_min_max(pos2(sq.max.x + 8.0, sq.min.y), area.max);
        let p = ui.painter().clone();
        let mut mesh = egui::Mesh::default();
        for j in 0..=N {
            for i in 0..=N {
                let (si, vj) = (i as f32 / N as f32, 1.0 - j as f32 / N as f32);
                let at = pos2(sq.min.x + si * sq.width(), sq.min.y + (j as f32 / N as f32) * sq.height());
                mesh.colored_vertex(at, color32(to8(hsv_to_rgb([h, si, vj]))));
            }
        }
        let row = (N + 1) as u32;
        for j in 0..N as u32 {
            for i in 0..N as u32 {
                let a = j * row + i;
                mesh.add_triangle(a, a + 1, a + row);
                mesh.add_triangle(a + 1, a + row + 1, a + row);
            }
        }
        for k in 0..=36u32 {
            let t = k as f32 / 36.0;
            let y = strip.min.y + t * strip.height();
            let c = color32(to8(hsv_to_rgb([t, 1.0, 1.0])));
            mesh.colored_vertex(pos2(strip.min.x, y), c);
            mesh.colored_vertex(pos2(strip.max.x, y), c);
            if k > 0 {
                let b = (N as u32 + 1) * (N as u32 + 1) + 2 * k;
                mesh.add_triangle(b - 2, b - 1, b);
                mesh.add_triangle(b - 1, b + 1, b);
            }
        }
        p.add(egui::Shape::mesh(mesh));
        p.rect_stroke(sq, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
        p.rect_stroke(strip, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
        let mut hsv = self.quick.hsv;
        let r = ui.interact(sq, ui.id().with("quick_sv"), Sense::click_and_drag());
        if r.is_pointer_button_down_on() {
            if let Some(at) = r.interact_pointer_pos() {
                hsv[1] = ((at.x - sq.min.x) / sq.width()).clamp(0.0, 1.0);
                hsv[2] = 1.0 - ((at.y - sq.min.y) / sq.height()).clamp(0.0, 1.0);
            }
        }
        let r = ui.interact(strip, ui.id().with("quick_hue"), Sense::click_and_drag());
        if r.is_pointer_button_down_on() {
            if let Some(at) = r.interact_pointer_pos() {
                hsv[0] = ((at.y - strip.min.y) / strip.height()).clamp(0.0, 0.9999);
            }
        }
        if hsv != self.quick.hsv {
            self.quick.hsv = hsv;
            let c = hsv_to_rgb(hsv);
            self.scene.brush.color = c;
            self.quick.hsv_of = c;
        }
        let mark = pos2(sq.min.x + s * sq.width(), sq.min.y + (1.0 - v) * sq.height());
        p.circle_stroke(mark, 6.0, Stroke::new(2.0, Color32::BLACK));
        p.circle_stroke(mark, 5.0, Stroke::new(1.5, Color32::WHITE));
        let hy = strip.min.y + h * strip.height();
        p.rect_stroke(
            Rect::from_min_max(pos2(strip.min.x - 2.0, hy - 2.5), pos2(strip.max.x + 2.0, hy + 2.5)),
            2.0,
            Stroke::new(1.5, Color32::WHITE),
            egui::StrokeKind::Middle,
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let cur = to8(self.scene.brush.color);
            let (r, resp) = ui.allocate_exact_size(vec2(46.0, 26.0), Sense::hover());
            ui.painter().rect_filled(r, 5.0, color32(cur));
            ui.painter().rect_stroke(r, 5.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            resp.on_hover_text("Brush colour");
            let alt = to8(self.quick.alt_color);
            let swap = swatch(ui, alt, 26.0, false).on_hover_text("Second colour: click to swap");
            let swap = swap.clicked() | w::icon_button(ui, Icon::Swap, 22.0, false, true).on_hover_text("Swap colours").clicked();
            if swap {
                std::mem::swap(&mut self.scene.brush.color, &mut self.quick.alt_color);
            }
            let tip = "Eyedropper: click the model to pick its colour";
            if w::icon_button(ui, Icon::Dropper, 22.0, self.quick.picking, true).on_hover_text(tip).clicked() {
                self.quick.picking = !self.quick.picking;
                if self.quick.picking {
                    self.status = "Click the model to pick a colour (Esc cancels)".into();
                }
            }
            ui.label(egui::RichText::new(hex(cur)).monospace().size(11.0).color(theme::TEXT_DIM));
        });
    }

    fn quick_sliders(&mut self, ui: &mut egui::Ui) {
        let tool = self.scene.tool;
        let b = &mut self.scene.brush;
        w::slider(ui, "Size", &mut b.radius, 1.0..=200.0, 0, true);
        let mut flow = b.flow * 100.0;
        if w::slider(ui, "Flow", &mut flow, 1.0..=100.0, 0, true) {
            b.flow = flow / 100.0;
        }
        let mut op = b.opacity * 100.0;
        if w::slider(ui, "Stroke opacity", &mut op, 1.0..=100.0, 0, true) {
            b.opacity = op / 100.0;
        }
        w::slider(ui, "Hardness", &mut b.hardness, 0.0..=1.0, 2, true);
        w::slider(ui, "Spacing", &mut b.spacing, 1.0..=200.0, 0, true);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (icon, t, tip) in [(Icon::Brush, Tool::Paint, "Paint (1)"), (Icon::Eraser, Tool::Eraser, "Eraser (2)")] {
                if w::icon_button(ui, icon, 24.0, tool == t, true).on_hover_text(tip).clicked() {
                    self.scene.tool = t;
                }
            }
            let name = if tool == Tool::Eraser { "Eraser" } else { "Paint" };
            ui.label(egui::RichText::new(name).size(11.0).color(theme::TEXT_DIM));
        });
    }

    /// A captioned row of swatches; a click makes the colour the brush colour (the old one becomes the second).
    fn quick_swatch_row(&mut self, ui: &mut egui::Ui, title: &str, colors: &[[u8; 3]], empty: &str) {
        let cur = to8(self.scene.brush.color);
        let mut pick: Option<[u8; 3]> = None;
        caption(ui, title);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
            if colors.is_empty() {
                caption(ui, empty);
            }
            for &c in colors {
                if swatch(ui, c, 22.0, c == cur).clicked() {
                    pick = Some(c);
                }
            }
        });
        if let Some(c) = pick {
            self.quick.alt_color = self.scene.brush.color;
            self.scene.brush.color = c.map(|v| v as f32 / 255.0);
        }
    }

    fn quick_brushes(&mut self, ui: &mut egui::Ui) {
        caption(ui, "Recent brushes");
        let b = self.scene.brush;
        let cur = [b.radius, b.hardness, b.flow, b.opacity, b.spacing];
        let mut pick = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if self.config.recent_brushes.is_empty() {
                caption(ui, "Brush settings you paint with appear here.");
            }
            for &snap in &self.config.recent_brushes {
                let (r, resp) = ui.allocate_exact_size(vec2(50.0, 50.0), Sense::click());
                let p = ui.painter();
                let same = snap.iter().zip(cur).all(|(a, b)| (a - b).abs() < 0.006);
                p.rect_filled(r, 6.0, theme::WELL);
                let edge = if same {
                    Stroke::new(2.0, theme::accent())
                } else if resp.hovered() {
                    Stroke::new(1.5, theme::WIDGET_HOVER)
                } else {
                    Stroke::new(1.0, theme::BORDER)
                };
                p.rect_stroke(r, 6.0, edge, egui::StrokeKind::Inside);
                // the dab: rings composited outside in, so the total opacity follows the brush falloff
                let rad = 5.0 + 13.0 * (snap[0] / 200.0).clamp(0.0, 1.0).sqrt();
                let c = r.center() - vec2(0.0, 5.0);
                let mut cum = 0.0f32;
                for k in 0..=14 {
                    let d = 1.0 - k as f32 / 14.0;
                    let a = crate::paint::falloff(d * 0.999, snap[1]) * snap[3];
                    let step = if cum < 1.0 { ((a - cum) / (1.0 - cum)).clamp(0.0, 1.0) } else { 0.0 };
                    cum = a.max(cum);
                    if step > 0.0 && d > 0.0 {
                        p.circle_filled(c, rad * d, Color32::from_white_alpha((step * 230.0) as u8));
                    }
                }
                p.text(
                    pos2(r.center().x, r.max.y - 8.0),
                    Align2::CENTER_CENTER,
                    format!("{:.0}", snap[0]),
                    FontId::proportional(10.0),
                    theme::TEXT_DIM,
                );
                let tip = format!(
                    "Size {:.0}\nHardness {:.2}\nFlow {:.0}%\nOpacity {:.0}%\nSpacing {:.0}",
                    snap[0],
                    snap[1],
                    snap[2] * 100.0,
                    snap[3] * 100.0,
                    snap[4]
                );
                if resp.on_hover_text(tip).clicked() {
                    pick = Some(snap);
                }
            }
        });
        if let Some(s) = pick {
            let b = &mut self.scene.brush;
            (b.radius, b.hardness, b.flow, b.opacity, b.spacing) = (s[0], s[1], s[2], s[3], s[4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keep_order_hide_and_fill_in() {
        let s = sections("brush,-color,junk,brush");
        assert_eq!(s[0], (Section::Brush, true));
        assert_eq!(s[1], (Section::Color, false));
        // the ones not named come after, shown
        assert_eq!(s.len(), Section::ALL.len());
        assert!(s[2..].iter().all(|(_, on)| *on));
        assert_eq!(sections(&sections_text(&s)), s);
        assert_eq!(sections("").iter().map(|(x, _)| *x).collect::<Vec<_>>(), Section::ALL.to_vec());
    }

    #[test]
    fn hsv_round_trip() {
        for c in [[0.9, 0.15, 0.1], [0.2, 0.5, 0.85], [0.3, 0.3, 0.3], [0.0, 0.0, 0.0], [1.0, 1.0, 0.0]] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            assert!(c.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-5), "{c:?} -> {back:?}");
        }
    }
}
