//! ShardStroke UI mockup: splash -> main window (TEXTURE / MATERIAL) with the start screen as a popup over it,
//! drawn with the design kit (greys, purple accent, Silkscreen labels, block bars). Nothing here touches real
//! projects.
//!
//! MOCKUP_SCREEN=splash|start|texture|material picks the first screen; MOCKUP_SHOT=<file.png> saves a
//! screenshot after a few frames and quits.

use eframe::egui::{
    self, epaint::TextShape, pos2, text::LayoutJob, vec2, Align2, Color32, FontData, FontDefinitions, FontFamily,
    FontId, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextFormat, Ui, Vec2,
};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

mod elements;
mod graph;
mod icons;
mod layers;
mod panels;
mod settings;
mod viewport;

use layers::Layer;

const BG: Color32 = Color32::from_rgb(0x0E, 0x0E, 0x0E);
const VIEW: Color32 = Color32::from_rgb(0x0B, 0x0B, 0x0B);
const PANEL: Color32 = Color32::from_rgb(0x15, 0x15, 0x15);
const CARD: Color32 = Color32::from_rgb(0x1B, 0x1B, 0x1B);
const HOVER: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
const SEL: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
const BORDER: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x2E);
const BORDER2: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
const TEXT: Color32 = Color32::from_rgb(0xE2, 0xE2, 0xE2);
const SOFT: Color32 = Color32::from_rgb(0xB4, 0xB4, 0xB4);
const DIM: Color32 = Color32::from_rgb(0x96, 0x96, 0x96);
const FAINT: Color32 = Color32::from_rgb(0x5C, 0x5C, 0x5C);
/// The highlight colour is a setting (Settings > Appearance), so it is read at draw time.
static ACCENT: AtomicU32 = AtomicU32::new(0x6C23DC);

fn acc() -> Color32 {
    let v = ACCENT.load(Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

const LOGO: Color32 = Color32::from_rgb(0x4C, 0x22, 0x8C);
const WARN: Color32 = Color32::from_rgb(0xFF, 0xBD, 0x2E);
const ERR: Color32 = Color32::from_rgb(0xF4, 0x58, 0x56);

/// Tools of the strip and their quickbar sliders: (name, max, unit).
const TOOLS: [(&str, &[(&str, f32, &str)]); 10] = [
    ("paint", &[("size", 100.0, ""), ("flow", 100.0, ""), ("opacity", 100.0, ""), ("spacing", 100.0, "")]),
    ("eraser", &[("size", 100.0, ""), ("flow", 100.0, ""), ("opacity", 100.0, ""), ("spacing", 100.0, "")]),
    ("projection", &[("size", 100.0, ""), ("opacity", 100.0, "")]),
    ("polygon fill", &[("opacity", 100.0, "")]),
    ("smudge", &[("size", 100.0, ""), ("strength", 1.0, "")]),
    ("clone", &[("size", 100.0, ""), ("flow", 100.0, ""), ("opacity", 100.0, "")]),
    ("picker", &[]),
    ("select", &[]),
    ("bake", &[]),
    ("camera", &[]),
];

/// What a window button on the right edge does.
#[derive(Clone, Copy)]
enum Win {
    Panel(&'static str),
    Export,
    Env,
}

/// Right edge of TEXTURE mode: every window has a button (board 02 left layers / properties / log without one).
const TEXTURE_WINDOWS: [(graph::Ic, &str, Win); 10] = [
    (graph::Ic::Svg("stack"), "layers", Win::Panel("layers")),
    (graph::Ic::Svg("sliders"), "properties", Win::Panel("properties")),
    (graph::Ic::Svg("grid-four"), "texture sets", Win::Panel("texture sets")),
    (graph::Ic::App(icons::Icon::Display), "display settings", Win::Panel("display settings")),
    (graph::Ic::App(icons::Icon::Shader), "shader settings", Win::Panel("shader settings")),
    (graph::Ic::App(icons::Icon::Environment), "environment", Win::Env),
    (graph::Ic::App(icons::Icon::History), "history", Win::Panel("history")),
    (graph::Ic::App(icons::Icon::Assets), "assets", Win::Panel("assets")),
    (graph::Ic::Svg("terminal-window"), "log", Win::Panel("log")),
    (graph::Ic::App(icons::Icon::Export), "export textures", Win::Export),
];

/// Right edge of MATERIAL mode (board 03).
const MATERIAL_WINDOWS: [(graph::Ic, &str, Win); 10] = [
    (graph::Ic::Svg("grid-four"), "nodes", Win::Panel("nodes")),
    (graph::Ic::App(icons::Icon::Sphere), "3d preview", Win::Panel("3d preview")),
    (graph::Ic::Svg("sliders"), "node properties", Win::Panel("node properties")),
    (graph::Ic::Svg("faders"), "graph parameters", Win::Panel("graph parameters")),
    (graph::Ic::App(icons::Icon::Material), "outputs", Win::Panel("outputs")),
    (graph::Ic::Svg("image-square"), "2d view", Win::Panel("2d view")),
    (graph::Ic::App(icons::Icon::History), "history", Win::Panel("history")),
    (graph::Ic::App(icons::Icon::Assets), "assets", Win::Panel("assets")),
    (graph::Ic::Svg("terminal-window"), "log", Win::Panel("log")),
    (graph::Ic::Svg("sign-out"), "export material", Win::Export),
];

impl App {
    /// Window buttons down the right edge; lit while that window is showing.
    fn dock_strip(&mut self, ui: &mut Ui, p: &Painter, strip: Rect, wins: &[(graph::Ic, &str, Win)], specs: &[panels::Spec]) {
        let h = wins.len() as f32 * 38.0 + 4.0;
        let strip = Rect::from_min_size(strip.min, vec2(strip.width(), h));
        frame(p, strip, PANEL, BORDER);
        for (i, (ic, name, win)) in wins.iter().enumerate() {
            let r = Rect::from_min_size(strip.min + vec2(3.0, 3.0 + i as f32 * 38.0), vec2(34.0, 34.0));
            let resp = ui.interact(r, Id::new(("win", i, *name)), Sense::click());
            let shown = match win {
                Win::Panel(k) => self.panel_shown(k),
                Win::Export => self.export.open,
                Win::Env => self.view.pop,
            };
            let on = an(ui, ("win_on", *name), shown);
            let hv = an(ui, ("win_h", *name), resp.hovered());
            p.rect_filled(r, 0.0, mix(mix(Color32::TRANSPARENT, alpha(acc(), 0.22), on), TEXT, hv));
            p.rect_filled(Rect::from_min_size(pos2(r.right() + 1.0, r.top() + 6.0), vec2(2.0, (r.height() - 12.0) * on)), 0.0, acc());
            graph::draw_ic(ui, p, Rect::from_center_size(r.center(), vec2(18.0, 18.0)), *ic, mix(mix(DIM, TEXT, on), BG, hv));
            if resp.hovered() {
                graph::tip(ui, pos2(r.left() - 8.0 - 150.0, r.center().y), name, "");
            }
            if resp.clicked() {
                match win {
                    Win::Panel(k) => {
                        if let Some(spec) = specs.iter().find(|s| s.0 == *k) {
                            self.show_or_toggle(spec);
                        }
                    }
                    Win::Export => self.export.open = true,
                    Win::Env => {
                        self.view_open_shading(ui.ctx().screen_rect());
                        self.view_open_env();
                    }
                }
            }
        }
    }
}

/// Tool strip slot position: the last three (select, bake, camera) sit after a gap.
fn tool_y(i: f32) -> f32 {
    3.0 + i * 38.0 + (i - 6.0).clamp(0.0, 1.0) * 10.0
}

/// Animation level (Settings > Appearance): 0 off, 1 basic (fades, glows, sliding highlights),
/// 2 advanced (also the rolling text, ASCII rain / dissolve and moving stripes).
static ANIM: AtomicU8 = AtomicU8::new(2);

fn anim_off() -> bool {
    ANIM.load(Ordering::Relaxed) == 0
}

fn heavy() -> bool {
    ANIM.load(Ordering::Relaxed) == 2
}

/// `animate_value_with_time` that jumps straight to the value when animations are off.
fn anim_value(ctx: &egui::Context, id: Id, v: f32, t: f32) -> f32 {
    ctx.animate_value_with_time(id, v, if anim_off() { 0.0 } else { t })
}

/// Eased 0..1 since `t0` over `dur` seconds; 1 at once when animations are off.
fn fade_in(now: f64, t0: f64, dur: f64) -> f32 {
    if anim_off() { 1.0 } else { smooth(((now - t0) / dur) as f32) }
}

/// Hover / selection fade time.
const FADE: f32 = 0.16;

#[derive(Clone, Copy)]
enum F {
    Px,
    Mono,
    Ui,
    Title,
}

fn fid(f: F, size: f32) -> FontId {
    match f {
        F::Px => FontId::new(size, FontFamily::Name("pixel".into())),
        F::Mono => FontId::monospace(size),
        F::Ui => FontId::proportional(size),
        F::Title => FontId::new(size, FontFamily::Name("title".into())),
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Eased 0..1 that follows `on`.
fn an(ui: &Ui, id: impl std::hash::Hash, on: bool) -> f32 {
    smooth(ui.ctx().animate_bool_with_time(Id::new(id), on, if anim_off() { 0.0 } else { FADE }))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Steps `k` toward `on` over `dur` seconds (for things that open: menus, the start popup, fades).
fn approach(k: &mut f32, on: bool, dt: f32, dur: f32) {
    let step = if anim_off() { 1.0 } else { dt / dur };
    *k = if on { (*k + step).min(1.0) } else { (*k - step).max(0.0) };
}

/// Text with letter spacing, anchored at `pos`.
fn txt(p: &Painter, pos: Pos2, align: Align2, s: &str, f: F, size: f32, color: Color32, spacing: f32) -> Rect {
    let mut job = LayoutJob::default();
    job.append(s, 0.0, TextFormat { font_id: fid(f, size), color, extra_letter_spacing: spacing, ..Default::default() });
    let g = p.layout_job(job);
    let r = align.anchor_size(pos, g.size());
    p.galley(r.min, g, color);
    r
}

/// Pixel label: Silkscreen, uppercase, tracked.
fn lbl(p: &Painter, pos: Pos2, align: Align2, s: &str, size: f32, color: Color32) -> Rect {
    txt(p, pos, align, &s.to_uppercase(), F::Px, size * 1.25, color, size * 0.12)
}

/// Windows (panels, popups, menus, dialogs) have a 2 px border.
fn win_frame(p: &Painter, r: Rect, fill: Color32, stroke: Color32) {
    p.rect_filled(r, 0.0, fill);
    p.rect_stroke(r, 0.0, Stroke::new(2.0, stroke), StrokeKind::Inside);
}

fn frame(p: &Painter, r: Rect, fill: Color32, stroke: Color32) {
    p.rect_filled(r, 0.0, fill);
    p.rect_stroke(r, 0.0, Stroke::new(1.0, stroke), StrokeKind::Inside);
}

/// Blurred drop shadow with no offset: the kit's glow (accent, `a` opacity, `blur` px).
fn glow(p: &Painter, r: Rect, c: Color32, a: f32, blur: u8) {
    if a > 0.004 {
        // egui centres the blur on the edge; doubling it and spreading by half matches a design-tool drop shadow
        p.add(egui::epaint::Shadow { offset: [0, 0], blur: blur.saturating_mul(2), spread: blur / 2, color: alpha(c, a) }.as_shape(r, 0.0));
    }
}

fn vgrad(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

/// Grows a clockwise convex polygon by `d` px (mitred corners).
fn grow(pts: &[Pos2], d: f32) -> Vec<Pos2> {
    let n = pts.len();
    let normal = |a: Pos2, b: Pos2| {
        let e = (b - a).normalized();
        vec2(e.y, -e.x)
    };
    (0..n)
        .map(|i| {
            let n1 = normal(pts[(i + n - 1) % n], pts[i]);
            let n2 = normal(pts[i], pts[(i + 1) % n]);
            let m = (n1 + n2).normalized();
            pts[i] + m * (d / m.dot(n1).max(0.3))
        })
        .collect()
}

/// The shard: hexagon with a triangular notch, 140 x 201.68 design units. `halo` = the start screen's effects:
/// a solid 15 px ring that cuts what is behind, then a 4 px glow of the fill colour.
fn logo(p: &Painter, center: Pos2, h: f32, fill: Color32, notch: Color32, halo: Option<Color32>) {
    let s = h / 201.68;
    let o = center - vec2(70.0, 100.84) * s;
    let pt = |x: f32, y: f32| o + vec2(x, y) * s;
    let hex = vec![pt(70.0, 0.0), pt(140.0, 70.0), pt(111.0, 178.23), pt(70.0, 201.68), pt(29.0, 178.23), pt(0.0, 70.0)];
    if let Some(ring) = halo {
        p.add(Shape::convex_polygon(grow(&hex, 15.0), ring, Stroke::NONE));
        for k in (1..=5).rev() {
            let a = 0.22 * (1.0 - k as f32 / 6.0);
            p.add(Shape::convex_polygon(grow(&hex, k as f32), alpha(fill, a), Stroke::NONE));
        }
    }
    p.add(Shape::convex_polygon(hex, fill, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![pt(75.0, 25.0), pt(115.0, 58.0), pt(86.0, 69.5)], notch, Stroke::NONE));
}

/// Block bar: solid fill, a dithered leading edge, a faint dotted remainder.
fn blocks(p: &Painter, r: Rect, frac: f32, fill: Color32) {
    let r = Rect::from_min_max(r.min.round(), r.max.round());
    let fx = (r.left() + r.width() * frac.clamp(0.0, 1.0)).round();
    p.rect_filled(Rect::from_min_max(pos2(fx, r.top()), r.max), 0.0, Color32::from_gray(0x1E));
    p.rect_filled(Rect::from_min_max(r.min, pos2(fx, r.bottom())), 0.0, fill);
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = fx + (row % 2) as f32 * 2.0;
        while x < r.right() {
            let edge = x < fx + 8.0;
            let c = if edge { fill.gamma_multiply(0.7) } else { Color32::from_gray(0x3C) };
            p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(1.0, 1.0)).intersect(r), 0.0, c);
            x += if edge { 2.0 } else { 4.0 };
        }
        y += 2.0;
        row += 1;
    }
}

/// Labelled block slider, drag or click to set. The bar glides to the new value; accent while dragging.
fn slider(ui: &mut Ui, p: &Painter, r: Rect, id: &str, label: &str, v: &mut f32, max: f32, unit: &str) {
    let resp = ui.interact(r, Id::new(id), Sense::click_and_drag());
    let h = an(ui, (id, "h"), resp.hovered() || resp.dragged());
    let d = an(ui, (id, "d"), resp.dragged());
    p.rect_filled(r, 0.0, alpha(HOVER, h));
    // the label keeps its full width; the bar takes what is left
    let lw = (label.len() as f32 * 9.0 + 16.0).max((r.width() * 0.34).min(96.0));
    let vw = 42.0;
    let bar = Rect::from_min_max(pos2(r.left() + lw, r.center().y - 5.0), pos2(r.right() - vw, r.center().y + 5.0));
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            *v = ((pos.x - bar.left()) / bar.width()).clamp(0.0, 1.0) * max;
        }
    }
    let shown = anim_value(ui.ctx(), Id::new((id, "v")), *v / max, if resp.dragged() { 0.0 } else { 0.12 });
    lbl(p, pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, label, 8.0, mix(DIM, TEXT, h));
    blocks(p, bar, shown, mix(TEXT, acc(), d));
    let s = if max <= 1.0 { format!("{:.2}", v) } else { format!("{:.0}{unit}", v) };
    txt(p, pos2(r.right() - 6.0, r.center().y), Align2::RIGHT_CENTER, &s, F::Mono, 11.0, TEXT, 0.0);
}

/// Square button; hover fades to the inverted look. `primary` is accent filled with the kit's 6 px glow.
fn button(ui: &mut Ui, p: &Painter, r: Rect, id: &str, s: &str, primary: bool) -> bool {
    let resp = ui.interact(r, Id::new(id), Sense::click());
    let t = an(ui, (id, "h"), resp.hovered());
    let (fill, stroke, col) =
        if primary { (acc(), acc(), Color32::WHITE) } else { (Color32::from_gray(0x28), BORDER2, Color32::from_gray(0xD0)) };
    if primary {
        glow(p, r, acc(), 0.5 * (1.0 - t), 6);
    }
    frame(p, r, mix(fill, TEXT, t), mix(stroke, TEXT, t));
    lbl(p, r.center(), Align2::CENTER_CENTER, s, 8.0, mix(col, BG, t));
    resp.clicked()
}

/// Docked card with a 30 px header.
fn card(p: &Painter, r: Rect, title: &str, count: Option<usize>, right: &str) -> Rect {
    win_frame(p, r, PANEL, BORDER);
    let h = Rect::from_min_size(r.min, vec2(r.width(), 30.0));
    p.rect_filled(h.shrink(2.0), 0.0, CARD);
    p.hline(h.x_range().shrink(2.0), h.bottom(), Stroke::new(1.0, BORDER));
    let t = lbl(p, pos2(h.left() + 10.0, h.center().y), Align2::LEFT_CENTER, title, 8.0, TEXT);
    if let Some(n) = count {
        lbl(p, pos2(t.right() + 6.0, h.center().y), Align2::LEFT_CENTER, &format!("[{n:02}]"), 8.0, FAINT);
    }
    // the X only while the layout is unlocked
    let x = Rect::from_center_size(pos2(h.right() - 15.0, h.center().y), vec2(14.0, 14.0));
    if !panels::locked() {
        p.rect_stroke(x, 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Inside);
        let s = Stroke::new(1.0, DIM);
        p.line_segment([x.left_top() + vec2(4.0, 4.0), x.right_bottom() - vec2(4.0, 4.0)], s);
        p.line_segment([x.right_top() + vec2(-4.0, 4.0), x.left_bottom() + vec2(4.0, -4.0)], s);
    }
    let x = if panels::locked() { x.translate(vec2(22.0, 0.0)) } else { x };
    if !right.is_empty() {
        lbl(p, pos2(x.left() - 8.0, h.center().y), Align2::RIGHT_CENTER, right, 8.0, DIM);
    }
    Rect::from_min_max(pos2(r.left() + 1.0, h.bottom() + 1.0), r.max - vec2(1.0, 1.0))
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Splash,
    Main,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Material,
    Texture,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Paint,
    Fill,
    Group,
    Generator,
    Filter,
}

struct Node {
    name: &'static str,
    pos: Vec2,
    ins: u8,
    outs: u8,
}

struct Recent {
    name: &'static str,
    tag: &'static str,
    when: &'static str,
    thumb: Color32,
    note: Option<(&'static str, Color32)>,
}

struct App {
    screen: Screen,
    mode: Mode,
    /// Time of the last mode switch: the work area fades in from it.
    mode_t: f64,
    splash_t0: f64,
    /// 0..1 fade of the main window after the splash.
    main_k: f32,
    start_open: bool,
    start_k: f32,
    filter: usize,
    find: String,
    tool: usize,
    tool_t: f64,
    /// Quickbar values per tool, in the order of TOOLS.
    tool_v: [[f32; 4]; 10],
    brush: [f32; 4],
    props: [f32; 4],
    fill: [f32; 3],
    layers: Vec<Layer>,
    sel_layer: usize,
    nodes: Vec<Node>,
    wires: Vec<(usize, usize, u8)>,
    sel_node: usize,
    node_props: [f32; 4],
    menu: Option<usize>,
    menu_k: f32,
    /// Save in progress since (simulated 1.2 s), last result (1 saved, 2 failed), and whether this one fails.
    save_t: Option<f64>,
    save_state: u8,
    save_fail: bool,
    sel_t: f64,
    channel: usize,
    ctx_menu: Option<(Pos2, usize)>,
    ctx_k: f32,
    gen: [f32; 3],
    mask_p: [f32; 3],
    sel_mask: bool,
    drop: Option<layers::Drop>,
    dock: panels::Docks,
    view: viewport::View,
    gr: graph::Graph,
    cfg: settings::Cfg,
    active_set: usize,
    history_pos: usize,
    log: Vec<(f32, String)>,
    asset_find: String,
    asset_kind: usize,
    export: elements::Export,
    frames: u32,
    shot: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        install_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let first = std::env::var("MOCKUP_SCREEN").unwrap_or_default();
        let c = Color32::from_rgb;
        App {
            screen: if first.is_empty() || first == "splash" { Screen::Splash } else { Screen::Main },
            mode: if first == "material" { Mode::Material } else { Mode::Texture },
            mode_t: -10.0,
            splash_t0: -1.0,
            main_k: if first.is_empty() || first == "splash" { 0.0 } else { 1.0 },
            start_open: first == "start",
            start_k: if first == "start" { 1.0 } else { 0.0 },
            filter: 0,
            find: String::new(),
            tool: 0,
            tool_t: -10.0,
            tool_v: [
                [14.0, 100.0, 100.0, 20.0],
                [30.0, 100.0, 100.0, 20.0],
                [40.0, 100.0, 0.0, 0.0],
                [100.0, 0.0, 0.0, 0.0],
                [20.0, 0.6, 0.0, 0.0],
                [24.0, 100.0, 100.0, 0.0],
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
            ],
            brush: [14.0, 100.0, 100.0, 20.0],
            props: [32.0, 80.0, 0.38, 10.0],
            fill: [0.55, 0.0, 1.0],
            layers: layers::stack(),
            sel_layer: 1,
            nodes: vec![
                Node { name: "Noise", pos: vec2(40.0, 60.0), ins: 0, outs: 1 },
                Node { name: "Tiles", pos: vec2(40.0, 230.0), ins: 0, outs: 1 },
                Node { name: "Blend", pos: vec2(260.0, 140.0), ins: 2, outs: 1 },
                Node { name: "Grunge", pos: vec2(40.0, 400.0), ins: 0, outs: 1 },
                Node { name: "Levels", pos: vec2(260.0, 380.0), ins: 1, outs: 1 },
                Node { name: "Height", pos: vec2(480.0, 230.0), ins: 2, outs: 1 },
                Node { name: "Out", pos: vec2(700.0, 230.0), ins: 1, outs: 0 },
            ],
            wires: vec![(0, 2, 0), (1, 2, 1), (2, 5, 0), (3, 4, 0), (4, 5, 1), (5, 6, 0)],
            sel_node: 2,
            node_props: [0.62, 0.35, 0.45, 4.0],
            menu: None,
            menu_k: 0.0,
            save_t: None,
            save_state: 1,
            save_fail: false,
            sel_t: -10.0,
            channel: 0,
            ctx_menu: None,
            ctx_k: 0.0,
            gen: [0.62, 0.35, 0.8],
            mask_p: [1.0, 0.5, 0.0],
            sel_mask: false,
            drop: None,
            dock: Default::default(),
            view: Default::default(),
            gr: Default::default(),
            active_set: 0,
            history_pos: 4,
            log: elements::log_lines(),
            asset_find: String::new(),
            asset_kind: 0,
            export: Default::default(),
            cfg: settings::Cfg {
                open: first == "settings",
                k: if first == "settings" { 1.0 } else { 0.0 },
                page: std::env::var("MOCKUP_PAGE").ok().and_then(|v| v.parse().ok()).unwrap_or(2),
                ..Default::default()
            },
            frames: 0,
            shot: std::env::var("MOCKUP_SHOT").ok(),
        }
    }

    /// Properties content fades in after a new layer / node is picked.
    fn sel_fade(&self, ui: &Ui) -> f32 {
        fade_in(ui.input(|i| i.time), self.sel_t, 0.2)
    }

    fn start_save(&mut self, now: f64, fail: bool) {
        self.save_t = Some(now);
        self.save_fail = fail;
    }

    fn set_mode(&mut self, m: Mode, now: f64) {
        if self.mode != m {
            self.mode = m;
            self.mode_t = now;
        }
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let la = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut add = |key: &str, path: String| -> bool {
        match std::fs::read(&path) {
            Ok(b) => {
                fonts.font_data.insert(key.into(), Arc::new(FontData::from_owned(b)));
                true
            }
            Err(_) => false,
        }
    };
    let silk = add("silk", format!("{la}\\Microsoft\\Windows\\Fonts\\Silkscreen-Regular.ttf"));
    let jb = add("jb", format!("{la}\\Craft Fonts\\jetbrainsmono\\JetBrainsMono[wght].ttf"));
    let inter = add("inter", format!("{la}\\Craft Fonts\\inter\\Inter[opsz,wght].ttf"));
    let segoe = add("segoe", "C:\\Windows\\Fonts\\segoeui.ttf".to_string());
    // the app's name on the start screen is the only text not in the pixel font
    let mut title: Vec<String> = Vec::new();
    if jb {
        title.push("jb".into());
    }
    title.extend(fonts.families[&FontFamily::Monospace].iter().cloned());
    fonts.families.insert(FontFamily::Name("title".into()), title);
    // everything else: Silkscreen first, the others only as glyph fallback
    let mut fallback: Vec<String> = Vec::new();
    for (ok, k) in [(inter, "inter"), (segoe, "segoe"), (jb, "jb")] {
        if ok {
            fallback.push(k.into());
        }
    }
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        let list = fonts.families.get_mut(&fam).unwrap();
        for (i, k) in fallback.iter().enumerate() {
            list.insert(i, k.clone());
        }
        if silk {
            list.insert(0, "silk".into());
        }
    }
    let pixel = fonts.families[&FontFamily::Monospace].clone();
    fonts.families.insert(FontFamily::Name("pixel".into()), pixel);
    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        self.frames += 1;
        self.capture(ctx);
        if self.screen == Screen::Splash {
            if self.splash_t0 < 0.0 {
                self.splash_t0 = now;
            }
            let t = (now - self.splash_t0) as f32;
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ctx, |ui| self.splash(ui, t));
        } else {
            approach(&mut self.main_k, true, dt, 0.45);
            approach(&mut self.start_k, self.start_open, dt, 0.22);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ctx, |ui| {
                ui.multiply_opacity(smooth(self.main_k));
                self.main(ui, now, dt);
            });
            panels::LOCKED.store(self.cfg.lock == 0, Ordering::Relaxed);
            ANIM.store(self.cfg.anim as u8, Ordering::Relaxed);
            approach(&mut self.cfg.k, self.cfg.open, dt, 0.22);
            approach(&mut self.export.k, self.export.open, dt, 0.22);
            if self.start_k > 0.0 {
                self.start_modal(ctx, now);
            }
            if self.cfg.k > 0.0 {
                self.settings_modal(ctx, now);
            }
            if self.export.k > 0.0 {
                self.export_modal(ctx, now);
            }
            self.viewport_overlays(ctx);
            if self.mode == Mode::Material {
                self.add_node_popup(ctx);
            }
            if !self.start_open && !self.cfg.open && !self.export.open {
                self.viewport_keys(ctx, now);
            }
        }
        ctx.request_repaint();
    }
}

impl App {
    fn capture(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.shot else { return };
        if self.frames == 40 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let _ = image::save_buffer(path, &raw, img.size[0] as u32, img.size[1] as u32, image::ExtendedColorType::Rgba8);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn splash(&mut self, ui: &mut Ui, t: f32) {
        // fades in, and out into the main window
        let out = smooth((t - 2.9) / 0.35);
        ui.multiply_opacity(smooth(t / 0.35) * (1.0 - out));
        let p = ui.painter().clone();
        let full = ui.max_rect();
        let r = Rect::from_center_size(full.center(), vec2(960.0, 600.0).min(full.size() - vec2(40.0, 40.0)));
        frame(&p, r, Color32::from_gray(0x16), BORDER2);
        let lc = pos2(r.left() + r.width() * 0.27, r.center().y);
        logo(&p, lc, r.height() * 0.48, Color32::from_gray(0x38), Color32::from_gray(0x16), None);
        let x = r.left() + r.width() * 0.5;
        let mut y = r.top() + r.height() * 0.17;
        lbl(&p, pos2(x, y), Align2::LEFT_TOP, "[01] // boot", 10.0, DIM);
        y += r.height() * 0.17;
        txt(&p, pos2(x, y), Align2::LEFT_TOP, "SHARDSTROKE", F::Px, 32.0, TEXT, 8.0);
        y += 50.0;
        lbl(&p, pos2(x, y), Align2::LEFT_TOP, "material authoring · 3d texturing", 8.0, DIM);
        lbl(&p, pos2(x, y + 14.0), Align2::LEFT_TOP, "v0.1.0 [codinome build]", 8.0, DIM);
        y += 58.0;
        let steps = [("GPU · VULKAN · RTX 4070", 0.3), ("UMBRA · BVH 1.2M TRIS", 0.8), ("DENOISER · OIDN 2.5.1", 1.3), ("LIBRARY · 2,184 ASSETS", 1.8)];
        for (s, at) in steps {
            if t < at {
                break;
            }
            // each line slides in and fades
            let k = smooth((t - at) / 0.25);
            let lp = p.clone().with_clip_rect(r);
            let mut lp = lp;
            lp.multiply_opacity(k);
            let ok = smooth((t - at - 0.45) / 0.15);
            let xo = x - 8.0 * (1.0 - k);
            let tr = lbl(&lp, pos2(xo, y), Align2::LEFT_TOP, if ok > 0.5 { "OK" } else { ".." }, 8.0, mix(FAINT, TEXT, ok));
            lbl(&lp, pos2(tr.right() + 8.0, y), Align2::LEFT_TOP, s, 8.0, SOFT);
            y += 22.0;
        }
        let prog = smooth(t / 2.6);
        let bar = Rect::from_min_size(pos2(x, r.top() + r.height() * 0.7), vec2(r.width() * 0.37, 14.0));
        blocks(&p, bar, prog, acc());
        lbl(&p, pos2(x, bar.bottom() + 10.0), Align2::LEFT_TOP, "loading library", 8.0, FAINT);
        lbl(&p, pos2(bar.right(), bar.bottom() + 10.0), Align2::RIGHT_TOP, &format!("{:.0}%", prog * 100.0), 8.0, TEXT);
        let click = ui.interact(full, Id::new("splash"), Sense::click()).clicked();
        if (t > 3.25 && self.shot.is_none()) || click {
            self.screen = Screen::Main;
            self.start_open = true;
        }
    }

    /// The start screen, as a popup over the main window. Closes on a click outside or Esc.
    fn start_modal(&mut self, ctx: &egui::Context, now: f64) {
        let k = smooth(self.start_k);
        let full = ctx.screen_rect();
        let t = now as f32;
        egui::Area::new(Id::new("start_modal")).fixed_pos(full.min).order(egui::Order::Foreground).show(ctx, |ui| {
            let (_, back) = ui.allocate_exact_size(full.size(), Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha((170.0 * k) as u8));
            ui.multiply_opacity(k);
            let size = vec2(1280.0, 780.0).min(full.size() - vec2(64.0, 64.0));
            let r = Rect::from_center_size(full.center() + vec2(0.0, 18.0 * (1.0 - k)), size);
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 40);
            win_frame(&p, r, BG, BORDER2);
            self.start(ui, r.shrink(20.0), t, now);
            let outside = back.clicked() && ui.ctx().input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
            if !self.cfg.open && self.cfg.k == 0.0 && (outside || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))) {
                self.start_open = false;
            }
        });
    }

    fn start(&mut self, ui: &mut Ui, full: Rect, t: f32, now: f64) {
        let p = ui.painter().clone();
        let left = Rect::from_min_size(full.min, vec2((full.width() * 0.34).max(340.0), full.height()));
        frame(&p, left, PANEL, BORDER);
        lbl(&p, left.left_top() + vec2(22.0, 22.0), Align2::LEFT_TOP, "[00] // start", 8.0, SOFT);
        lbl(&p, pos2(left.right() - 22.0, left.top() + 22.0), Align2::RIGHT_TOP, "v0.1.0 preview", 8.0, FAINT);
        let art = Rect::from_min_max(pos2(left.left() + 1.0, left.top() + 38.0), pos2(left.right() - 1.0, left.bottom() - 112.0));
        p.rect_filled(art, 0.0, Color32::from_rgb(0x12, 0x12, 0x14));
        {
            // every line rolls the same way along the diagonal, slow but visibly moving
            let cp = p.with_clip_rect(art.intersect(p.clip_rect()));
            let a = -30f32.to_radians();
            let d = vec2(a.cos(), a.sin());
            let n = vec2(-a.sin(), a.cos());
            let unit = "SHARDSTROKE - ";
            let seg = cp.layout_no_wrap(unit.to_string(), fid(F::Px, 22.0), Color32::WHITE).size().x;
            let line: String = unit.repeat(10);
            let reach = art.size().length();
            let spacing = 34.0;
            let rows = (reach / spacing) as i32 + 2;
            let col = Color32::from_rgb(0x30, 0x2B, 0x40);
            for k in -rows / 2..=rows / 2 {
                let roll = if heavy() { t * 22.0 } else { 0.0 };
                let phase = (roll + k as f32 * seg * 0.37).rem_euclid(seg);
                let start = art.center() + n * (k as f32 * spacing) - d * (reach * 0.5 + seg) + d * phase;
                let g = cp.layout_no_wrap(line.clone(), fid(F::Px, 22.0), col);
                cp.add(TextShape::new(start, g, col).with_angle(a));
            }
        }
        logo(&p, art.center(), art.height().min(art.width()) * 0.5, LOGO, Color32::from_rgb(0x12, 0x12, 0x14), Some(Color32::from_rgb(0x12, 0x12, 0x14)));
        // the gradient lies over the text and the logo
        vgrad(&p, art, Color32::from_rgba_unmultiplied(0x2A, 0x20, 0x44, 90), Color32::from_rgba_unmultiplied(0x15, 0x15, 0x17, 170));
        txt(&p, pos2(left.left() + 22.0, left.bottom() - 82.0), Align2::LEFT_TOP, "SHARDSTROKE", F::Title, 26.0, TEXT, 2.0);
        lbl(&p, pos2(left.left() + 22.0, left.bottom() - 38.0), Align2::LEFT_CENTER, "material authoring + 3d texturing", 8.0, DIM);
        let pn = Rect::from_min_size(pos2(left.right() - 108.0, left.bottom() - 50.0), vec2(96.0, 24.0));
        button(ui, &p, pn, "patch", "patch notes", false);

        // right column
        let rx = left.right() + 18.0;
        let right = Rect::from_min_max(pos2(rx, full.top()), full.max);
        let cw = (right.width() - 18.0) / 2.0;
        let ch = (right.height() * 0.33).clamp(200.0, 260.0);
        let cards = [
            ("[01] // material authoring", "new material", "build a tileable procedural material\nfrom a node graph. no mesh needed.", "graph · 2048 · tileable", "ctrl+shift+n",
             "[NOISE]--+\n         +--[BLEND]--[HEIGHT]--+\n[TILES]---+                     +--[ OUT ]\n         [GRUNGE]--[LEVELS]----+", Mode::Material),
            ("[02] // 3d texturing", "new 3d texturing", "import a mesh, bake its maps and\npaint layered materials on it.", "fbx · obj · gltf · usd", "ctrl+n",
             "    .+------+      LAYERS [00]\n  .' |    .'|    > PAINT\n +---+--+'  |     > FILL\n |   +--+---+     > MASK\n | .'   | .'\n +------+'", Mode::Texture),
        ];
        for (i, (head, title, body, foot, key, ascii, mode)) in cards.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(rx + i as f32 * (cw + 18.0), right.top()), vec2(cw, ch));
            let resp = ui.interact(r, Id::new(("card", i)), Sense::click());
            let h = an(ui, ("card", i), resp.hovered());
            // hover: 2 px accent border and the kit's soft 12 px glow at 26%
            glow(&p, r, acc(), 0.26 * h, 12);
            p.rect_filled(r, 0.0, CARD);
            vgrad(&p, r, alpha(Color32::from_rgb(0x22, 0x1C, 0x30), h), alpha(CARD, h));
            p.rect_stroke(r, 0.0, Stroke::new(1.0 + h, mix(BORDER, acc(), h)), StrokeKind::Inside);
            let sq = Rect::from_min_size(r.left_top() + vec2(18.0, 20.0), vec2(9.0, 9.0));
            glow(&p, sq, acc(), h, 4);
            p.rect_filled(sq, 0.0, mix(BORDER2, acc(), h));
            lbl(&p, pos2(sq.right() + 12.0, sq.center().y), Align2::LEFT_CENTER, head, 8.0, DIM);
            txt(&p, r.left_top() + vec2(18.0, 50.0), Align2::LEFT_TOP, ascii, F::Mono, 10.0, mix(FAINT, SOFT, h), 0.0);
            lbl(&p, pos2(r.left() + 18.0, r.bottom() - 92.0), Align2::LEFT_BOTTOM, title, 16.0, mix(TEXT, Color32::WHITE, h));
            lbl(&p, pos2(r.left() + 18.0, r.bottom() - 76.0), Align2::LEFT_TOP, body, 8.0, DIM);
            lbl(&p, pos2(r.left() + 18.0, r.bottom() - 18.0), Align2::LEFT_BOTTOM, foot, 8.0, FAINT);
            lbl(&p, pos2(r.right() - 18.0, r.bottom() - 18.0), Align2::RIGHT_BOTTOM, key, 8.0, TEXT);
            if resp.clicked() {
                self.set_mode(mode, now);
                self.start_open = false;
            }
        }
        let by = right.top() + ch + 16.0;
        let bw = (right.width() - 3.0 * 12.0) / 4.0;
        for (i, (s, k)) in [("open", "ctrl+o"), ("import mesh", ""), ("from template", ""), ("settings", "")].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(rx + i as f32 * (bw + 12.0), by), vec2(bw, 38.0));
            let resp = ui.interact(r, Id::new(("act", i)), Sense::click());
            if resp.clicked() && s == "settings" {
                self.cfg.open = true;
            }
            let h = an(ui, ("act", i), resp.hovered());
            let col = mix(SOFT, BG, h);
            frame(&p, r, mix(CARD, TEXT, h), mix(BORDER, TEXT, h));
            let ic = Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(16.0, 16.0));
            svg_icon(ui, ic, ["folder-open", "file-arrow-down", "squares-four", "gear-six"][i], col);
            lbl(&p, pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, s, 8.0, col);
            if !k.is_empty() {
                lbl(&p, pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, k, 8.0, mix(FAINT, BG, h));
            }
        }

        // recent projects
        let lr = Rect::from_min_max(pos2(rx, by + 38.0 + 16.0), right.max);
        frame(&p, lr, CARD, BORDER);
        let t1 = lbl(&p, lr.left_top() + vec2(16.0, 20.0), Align2::LEFT_CENTER, "recent", 8.0, TEXT);
        let recents = recents();
        lbl(&p, pos2(t1.right() + 4.0, t1.center().y), Align2::LEFT_CENTER, &format!("[{:02}]", recents.len()), 8.0, FAINT);
        let fr = Rect::from_min_size(pos2(lr.right() - 186.0, lr.top() + 10.0), vec2(170.0, 26.0));
        frame(&p, fr, BG, BORDER2);
        ui.put(fr.shrink2(vec2(8.0, 4.0)), egui::TextEdit::singleline(&mut self.find).frame(false).font(fid(F::Px, 10.0)).text_color(TEXT).hint_text("FIND..."));
        let mut fx = fr.left() - 10.0;
        for (i, s) in ["MAT", "3D", "ALL"].into_iter().enumerate() {
            let w = if s == "ALL" { 44.0 } else { 36.0 };
            let r = Rect::from_min_size(pos2(fx - w, fr.top()), vec2(w, 26.0));
            fx -= w + 8.0;
            let idx = 2 - i;
            let resp = ui.interact(r, Id::new(("flt", idx)), Sense::click());
            if resp.clicked() {
                self.filter = idx;
            }
            let on = an(ui, ("flt_on", idx), self.filter == idx || resp.hovered());
            frame(&p, r, mix(BG, TEXT, on), mix(BORDER2, TEXT, on));
            lbl(&p, r.center(), Align2::CENTER_CENTER, s, 8.0, mix(DIM, BG, on));
        }
        let hy = lr.top() + 58.0;
        let cols = [lr.left() + 66.0, lr.right() - 228.0, lr.right() - 124.0];
        for (x, s) in cols.iter().zip(["name", "has", "opened"]) {
            lbl(&p, pos2(*x, hy), Align2::LEFT_CENTER, s, 7.0, FAINT);
        }
        let q = self.find.to_lowercase();
        let mut y = hy + 16.0;
        for (i, rc) in recents.iter().enumerate() {
            let keep = match self.filter {
                1 => rc.tag.contains("3D"),
                2 => rc.tag.contains("MAT"),
                _ => true,
            } && rc.name.to_lowercase().contains(&q);
            if !keep || y + 46.0 > lr.bottom() {
                continue;
            }
            let r = Rect::from_min_size(pos2(lr.left() + 1.0, y), vec2(lr.width() - 2.0, 46.0));
            let resp = ui.interact(r, Id::new(("rec", i)), Sense::click());
            let h = an(ui, ("rec", i), resp.hovered());
            let first = i == 0;
            let bg = if first { SEL } else { CARD };
            p.rect_filled(r, 0.0, mix(bg, HOVER, h));
            let th = Rect::from_center_size(pos2(lr.left() + 31.0, r.center().y), vec2(32.0, 32.0));
            frame(&p, th, rc.thumb, if first { TEXT } else { mix(BORDER2, DIM, h) });
            let nr = lbl(&p, pos2(cols[0] + 4.0 * h, r.center().y), Align2::LEFT_CENTER, rc.name, 9.0, if first { TEXT } else { mix(DIM, TEXT, h) });
            if let Some((note, c)) = rc.note {
                lbl(&p, pos2(nr.right() + 8.0, r.center().y), Align2::LEFT_CENTER, note, 7.0, c);
            }
            let tr = lbl(&p, pos2(cols[1] + 5.0, r.center().y), Align2::LEFT_CENTER, rc.tag, 7.0, SOFT);
            p.rect_stroke(tr.expand2(vec2(5.0, 4.0)), 0.0, Stroke::new(1.0, if first { TEXT } else { BORDER2 }), StrokeKind::Outside);
            lbl(&p, pos2(cols[2], r.center().y), Align2::LEFT_CENTER, rc.when, 8.0, DIM);
            if resp.clicked() {
                self.set_mode(if rc.tag == "MAT" { Mode::Material } else { Mode::Texture }, now);
                self.start_open = false;
            }
            y += 47.0;
        }
    }

    fn main(&mut self, ui: &mut Ui, now: f64, dt: f32) {
        let p = ui.painter().clone();
        let full = ui.max_rect();
        p.rect_filled(full, 0.0, VIEW);
        let top = Rect::from_min_size(full.min, vec2(full.width(), 30.0));
        let status = Rect::from_min_max(pos2(full.left(), full.bottom() - 26.0), full.max);
        let work = Rect::from_min_max(pos2(full.left(), top.bottom()), pos2(full.right(), status.top()));
        // a mode switch cross-fades the work area
        let mut wp = p.clone();
        wp.multiply_opacity(fade_in(now, self.mode_t, 0.25));
        match self.mode {
            Mode::Texture => self.texture(ui, &wp, work),
            Mode::Material => self.material(ui, &wp, work),
        }
        self.top_bar(ui, &p, top, now, dt);
        self.status_bar(&p, status);
    }

    fn top_bar(&mut self, ui: &mut Ui, p: &Painter, top: Rect, now: f64, dt: f32) {
        p.rect_filled(top, 0.0, BG);
        p.hline(top.x_range(), top.bottom() - 0.5, Stroke::new(1.0, BORDER));
        // the logo, shared by both modes, opens the start screen
        let lr = Rect::from_min_size(top.min + vec2(6.0, 3.0), vec2(32.0, 24.0));
        let resp = ui.interact(lr, Id::new("logo_btn"), Sense::click());
        let h = an(ui, "logo_btn", resp.hovered() || self.start_open);
        glow(p, lr.shrink(6.0), acc(), 0.5 * h, 8);
        logo(p, lr.center(), 17.0, mix(TEXT, Color32::from_rgb(0x9A, 0x6A, 0xF0), h), BG, None);
        if resp.clicked() {
            self.start_open = true;
            self.menu = None;
        }
        let menus = ["file", "edit", "mode", "window", "viewport", "help"];
        let mut x = top.left() + 50.0;
        let mut popup = None;
        for (i, m) in menus.iter().enumerate() {
            let g = p.layout_no_wrap(m.to_uppercase(), fid(F::Px, 10.0), TEXT);
            let r = Rect::from_min_size(pos2(x - 7.0, top.top() + 4.0), vec2(g.size().x + 14.0 + 0.96 * m.len() as f32, 22.0));
            let resp = ui.interact(r, Id::new(("menu", i)), Sense::click());
            let open = self.menu == Some(i);
            let h = an(ui, ("menu", i), resp.hovered() || open);
            p.rect_filled(r, 0.0, alpha(TEXT, h));
            lbl(p, pos2(x, top.center().y), Align2::LEFT_CENTER, m, 8.0, mix(TEXT, BG, h));
            if resp.clicked() {
                self.menu = if open { None } else { Some(i) };
                self.menu_k = 0.0;
            } else if self.menu.is_some() && !open && resp.hovered() {
                self.menu = Some(i);
            }
            if self.menu == Some(i) {
                popup = Some((r.left_bottom(), i));
            }
            x = r.right() + 7.0;
        }
        if let Some((at, i)) = popup {
            approach(&mut self.menu_k, true, dt, 0.12);
            self.menu_popup(ui, at, i, now);
        }
        // mode tabs: the active highlight slides between them
        let tw = 86.0;
        let tabs = Rect::from_center_size(top.center(), vec2(tw * 2.0, 22.0));
        frame(p, tabs, PANEL, BORDER2);
        let at = anim_value(ui.ctx(), Id::new("tab_x"), if self.mode == Mode::Material { 0.0 } else { 1.0 }, 0.2);
        let hl = Rect::from_min_size(tabs.min + vec2(at * tw, 0.0), vec2(tw, 22.0)).shrink(1.0);
        p.rect_filled(hl, 0.0, SEL);
        for (i, (s, m)) in [("material", Mode::Material), ("texture", Mode::Texture)].into_iter().enumerate() {
            let r = Rect::from_min_size(tabs.min + vec2(i as f32 * tw, 0.0), vec2(tw, 22.0));
            let resp = ui.interact(r, Id::new(("tab", i)), Sense::click());
            let on = an(ui, ("tab_on", i), self.mode == m);
            let hv = an(ui, ("tab_h", i), resp.hovered() && self.mode != m);
            p.rect_filled(r.shrink(1.0), 0.0, alpha(HOVER, hv * (1.0 - on)));
            let dot = Rect::from_center_size(pos2(r.left() + 12.0, r.center().y), vec2(5.0, 5.0));
            glow(p, dot, acc(), 0.4 * on, 4);
            p.rect_filled(dot, 0.0, alpha(acc(), on));
            lbl(p, r.center() + vec2(7.0 * on, 0.0), Align2::CENTER_CENTER, s, 8.0, mix(DIM, TEXT, on.max(hv)));
            if resp.clicked() {
                self.set_mode(m, now);
            }
        }
        // project name + save status: yellow saving, green saved, red failed
        let pr = lbl(p, pos2(top.right() - 22.0, top.center().y), Align2::RIGHT_CENTER, "turret_v2.shard", 8.0, DIM);
        let dot = pos2(pr.right() + 10.0, top.center().y);
        let saving = self.save_t.is_some_and(|t| now - t < 1.2);
        if self.save_t.is_some_and(|t| now - t >= 1.2) {
            self.save_t = None;
            self.save_state = if self.save_fail { 2 } else { 1 };
            let t = now as f32;
            self.log.push((t, if self.save_fail { "Save failed: could not write turret_v2.shard".into() } else { "Saved turret_v2.shard".into() }));
        }
        let (col, what) = if saving {
            (WARN, "saving...")
        } else if self.save_state == 2 {
            (ERR, "save failed: could not write the file")
        } else {
            (Color32::from_rgb(0x4C, 0xC8, 0x6A), "saved")
        };
        // a soft pulse while it saves
        let pulse = if saving && heavy() { 0.5 + 0.5 * (now as f32 * 8.0).sin() } else { 0.0 };
        glow(p, Rect::from_center_size(dot, vec2(6.0, 6.0)), col, 0.35 + 0.35 * pulse, 4);
        p.circle_filled(dot, 3.0, col);
        let hover = Rect::from_min_max(pr.min - vec2(4.0, 4.0), dot + vec2(8.0, 8.0));
        if ui.rect_contains_pointer(hover) {
            graph::tip(ui, pos2(hover.left() - 230.0, top.center().y + 26.0), "turret_v2.shard", what);
        }
        if ui.input(|i| i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::S)) {
            self.start_save(now, false);
        }
    }

    fn menu_popup(&mut self, ui: &mut Ui, at: Pos2, i: usize, now: f64) {
        let window_items: Vec<(&str, &str)>;
        let items: &[(&str, &str)] = match i {
            0 => &[
                ("new project...", "ctrl+n"),
                ("new material...", "missing"),
                ("open...", "ctrl+o"),
                ("recent projects", ">"),
                ("open sample project...", ""),
                ("close", "ctrl+f4"),
                ("save", "ctrl+s"),
                ("save as...", "ctrl+shift+s"),
                ("import mesh (obj)...", ""),
                ("export textures...", "ctrl+shift+e"),
                ("start screen", ""),
                ("quit", ""),
            ],
            1 => &[("undo", "ctrl+z"), ("redo", "ctrl+y"), ("clear paint", ""), ("project configuration...", ""), ("settings...", "")],
            2 => &[("bake mesh maps", "f8"), ("painting", "f9"), ("rendering", "f10"), ("material (graph)", "")],
            4 => &[("show entire mesh", "f"), ("display next channel", "c"), ("display material", "m"), ("viewport shading...", "")],
            5 => &[("shardstroke (early build)", "")],
            3 => {
                let keys: Vec<&'static str> =
                    if self.mode == Mode::Texture { Self::texture_specs().iter().map(|s| s.0).collect() } else { Self::material_specs().iter().map(|s| s.0).collect() };
                let mut v: Vec<(&str, &str)> = keys.iter().map(|k| (*k, if self.panel_open(k) { "shown" } else { "hidden" })).collect();
                v.push(("hide ui", "tab"));
                v.push(("lock layout", if panels::locked() { "on" } else { "off" }));
                v.push(("reset layout", ""));
                window_items = v;
                &window_items
            }
            _ => &[("(mockup)", "")],
        };
        let k = smooth(self.menu_k);
        let r = Rect::from_min_size(at + vec2(0.0, 2.0 - 6.0 * (1.0 - k)), vec2(270.0, 8.0 + items.len() as f32 * 26.0));
        let mut close = false;
        let mut action = None;
        egui::Area::new(Id::new("menu_popup")).fixed_pos(r.min).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            ui.multiply_opacity(k);
            let (_, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 16);
            win_frame(&p, r, CARD, BORDER2);
            for (n, (s, key)) in items.iter().enumerate() {
                let ir = Rect::from_min_size(r.min + vec2(1.0, 4.0 + n as f32 * 26.0), vec2(r.width() - 2.0, 26.0));
                if *key == "missing" {
                    lbl(&p, pos2(ir.left() + 12.0, ir.center().y), Align2::LEFT_CENTER, s, 8.0, Color32::from_gray(0x50));
                    lbl(&p, pos2(ir.right() - 12.0, ir.center().y), Align2::RIGHT_CENTER, key, 6.0, Color32::from_gray(0x50));
                    continue;
                }
                let resp = ui.interact(ir, Id::new(("mi", i, n)), Sense::click());
                let h = an(ui, ("mi", i, n), resp.hovered());
                p.rect_filled(ir, 0.0, alpha(TEXT, h));
                lbl(&p, pos2(ir.left() + 12.0 + 3.0 * h, ir.center().y), Align2::LEFT_CENTER, s, 8.0, mix(TEXT, BG, h));
                lbl(&p, pos2(ir.right() - 12.0, ir.center().y), Align2::RIGHT_CENTER, key, 7.0, mix(FAINT, BG, h));
                if resp.clicked() {
                    action = Some(*s);
                    close = true;
                }
            }
        });
        if ui.ctx().input(|i| i.pointer.any_click()) && !close {
            let pos = ui.ctx().input(|i| i.pointer.interact_pos());
            if pos.is_some_and(|q| !r.contains(q) && q.y > 30.0) {
                close = true;
            }
        }
        if i == 3 {
            match action {
                Some("reset layout") => self.reset_layout(),
                Some("lock layout") => self.cfg.lock = 1 - self.cfg.lock,
                Some("hide ui") => self.view.hide_ui = !self.view.hide_ui,
                Some(k) => {
                    let specs: Vec<panels::Spec> =
                        if self.mode == Mode::Texture { Self::texture_specs().to_vec() } else { Self::material_specs().to_vec() };
                    if let Some(spec) = specs.iter().find(|s| s.0 == k) {
                        self.toggle_panel(spec);
                    }
                }
                None => {}
            }
        }
        // in the mockup "save as..." stands in for a save that fails
        if action == Some("save") {
            self.start_save(now, false);
        }
        if action == Some("save as...") {
            self.start_save(now, true);
        }
        if action == Some("export textures...") {
            self.export.open = true;
        }
        if action == Some("settings...") {
            self.cfg.open = true;
        }
        match action {
            Some("painting") | Some("rendering") => self.set_mode(Mode::Texture, now),
            Some("material (graph)") => self.set_mode(Mode::Material, now),
            Some("quit") => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            _ => {}
        }
        if action == Some("rendering") {
            self.view.pt = 1;
        }
        if action == Some("display next channel") {
            self.view.show = self.view.show % 5 + 1;
        }
        if action == Some("display material") {
            self.view.show = 0;
        }
        if action == Some("viewport shading...") {
            self.view_open_shading(ui.ctx().screen_rect());
        }
        if matches!(action, Some("start screen") | Some("new project...") | Some("open...")) {
            self.start_open = true;
        }
        let _ = now;
        if close {
            self.menu = None;
        }
    }

    fn status_bar(&self, p: &Painter, r: Rect) {
        p.rect_filled(r, 0.0, BG);
        p.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, BORDER));
        let y = r.center().y;
        let a = lbl(p, pos2(r.left() + 8.0, y), Align2::LEFT_CENTER, "> ready_", 8.0, TEXT);
        let (name, size) = elements::SETS[self.active_set];
        let set = if self.mode == Mode::Texture { format!("set {}/8 · {name} · {size}", self.active_set + 1) } else { "graph · 7 nodes · 2048".into() };
        let set = set.as_str();
        let b = lbl(p, pos2(a.right() + 20.0, y), Align2::LEFT_CENTER, set, 8.0, DIM);
        lbl(p, pos2(b.right() + 20.0, y), Align2::LEFT_CENTER, "autosave 2 min ago", 8.0, DIM);
        let g = lbl(p, pos2(r.right() - 8.0, y), Align2::RIGHT_CENTER, "rtx 4070 · vulkan", 8.0, DIM);
        lbl(p, pos2(g.left() - 20.0, y), Align2::RIGHT_CENTER, "vram 3.1 / 12 gb", 8.0, DIM);
    }

    fn texture_specs() -> [panels::Spec; 8] {
        use panels::Place::*;
        [
            ("layers", Right, 1.4, App::layers_panel),
            ("properties", Right, 1.0, App::props_panel),
            ("display settings", Right, 0.0, App::display_panel),
            ("shader settings", Right, 0.0, App::shader_panel),
            ("texture sets", Left, 1.2, App::sets_panel),
            ("history", Left, 0.8, App::history_panel),
            ("assets", Bottom, 1.7, App::assets_panel),
            ("log", Bottom, 1.0, App::log_panel),
        ]
    }

    /// A negative share means the window starts closed (its button on the right edge brings it in).
    fn material_specs() -> [panels::Spec; 9] {
        use panels::Place::*;
        [
            ("nodes", Left, 1.0, App::nodes_panel),
            ("3d preview", Right, 1.0, App::preview_panel),
            ("node properties", Right, 1.1, App::node_props_panel),
            ("log", Bottom, 1.0, App::log_panel),
            ("graph parameters", Right, -1.0, App::graph_params_panel),
            ("outputs", Right, -1.0, App::outputs_panel),
            ("2d view", Right, -1.0, App::view2d_panel),
            ("history", Left, -0.8, App::history_panel),
            ("assets", Bottom, -1.6, App::assets_panel),
        ]
    }

    fn texture(&mut self, ui: &mut Ui, p: &Painter, work: Rect) {
        // viewport is the background; panels dock on the sides / bottom or float over it
        let strip = Rect::from_min_size(work.min + vec2(4.0, 4.0), vec2(40.0, tool_y(10.0) + 1.0));
        let wins = Rect::from_min_max(pos2(work.right() - 40.0, work.top() + 4.0), work.max - vec2(4.0, 4.0));
        let area = Rect::from_min_max(pos2(strip.right() + 4.0, work.top() + 4.0), pos2(wins.left() - 4.0, work.bottom() - 4.0));
        let specs = Self::texture_specs();
        let z = self.dock_zones(ui, area, &specs);
        let view = if self.view.hide_ui { work } else { z.free };
        self.viewport_bg(ui, p, work);
        logo(p, view.center() + vec2(0.0, 10.0), view.height().min(view.width()) * 0.5, alpha(Color32::BLACK, 0.35), Color32::TRANSPARENT, None);
        self.viewport_input(ui, view);
        self.viewport_stats(ui, p, view);
        if self.view.hide_ui {
            return;
        }
        self.dock_strip(ui, p, wins, &TEXTURE_WINDOWS, &specs);

        // tool strip: the active highlight slides to the picked tool
        frame(p, strip, PANEL, BORDER);
        let ay = anim_value(ui.ctx(), Id::new("tool_y"), tool_y(self.tool as f32), 0.18);
        let act = Rect::from_min_size(strip.min + vec2(3.0, ay), vec2(34.0, 34.0));
        p.hline(strip.x_range().shrink(6.0), strip.top() + tool_y(7.0) - 6.0, Stroke::new(1.0, BORDER));
        glow(p, act, acc(), 0.5, 6);
        p.rect_filled(act, 0.0, acc());
        for i in 0..10 {
            let r = Rect::from_min_size(strip.min + vec2(3.0, tool_y(i as f32)), vec2(34.0, 34.0));
            let resp = ui.interact(r, Id::new(("tool", i)), Sense::click());
            if resp.hovered() {
                let keys = ["1", "2", "3", "4", "5", "6", "p", "", "", ""];
                graph::tip(ui, pos2(r.right() + 8.0, r.center().y), TOOLS[i].0, keys[i]);
            }
            if resp.clicked() && self.tool != i {
                self.tool = i;
                self.tool_t = ui.input(|i| i.time);
            }
            let on = an(ui, ("tool_on", i), self.tool == i);
            let h = an(ui, ("tool_h", i), resp.hovered() && self.tool != i);
            p.rect_filled(r, 0.0, alpha(TEXT, h * (1.0 - on)));
            tool_icon(p, i, r.center(), mix(mix(DIM, BG, h), Color32::WHITE, on));
        }

        // viewport controls: their own small group in the top-right corner
        let vc = Rect::from_min_size(pos2(view.right() - 236.0, view.top()), vec2(236.0, 34.0));
        frame(p, vc, PANEL, BORDER);
        self.viewport_controls(ui, p, vc);
        self.quickbar(ui, p, Rect::from_min_max(view.min, pos2(vc.left() - 8.0, view.bottom())), view);
        self.dock_draw(ui, p, &z, &specs);
    }

    /// Quickbar of the selected tool: fixed width, centred on the viewport (pushed left only if it would
    /// run under the viewport controls); a narrow viewport drops controls from the right.
    fn quickbar(&mut self, ui: &mut Ui, p: &Painter, room: Rect, view: Rect) {
        const SW: f32 = 190.0;
        let tool = TOOLS[self.tool];
        let n_all = tool.1.len();
        let head = 120.0;
        let fits = (((room.width() - head - 8.0) / SW) as usize).clamp(1, n_all.max(1));
        let n = if n_all == 0 { 0 } else { fits };
        let w = head + n as f32 * SW + 8.0 + if n_all == 0 { 220.0 } else { 0.0 };
        let cx = view.center().x.max(room.left() + w / 2.0).min(room.right() - w / 2.0);
        let target = Rect::from_center_size(pos2(cx, view.top() + 17.0), vec2(w, 34.0));
        // glides to its new width when the tool changes
        let aw = anim_value(ui.ctx(), Id::new("qb_w"), w, 0.2);
        let ax = anim_value(ui.ctx(), Id::new("qb_x"), target.center().x, 0.2);
        let bar = Rect::from_center_size(pos2(ax, target.center().y), vec2(aw, 34.0));
        frame(p, bar, PANEL, BORDER);
        let cp = p.with_clip_rect(bar.intersect(p.clip_rect()));
        let k = fade_in(ui.input(|i| i.time), self.tool_t, 0.2);
        let mut cp = cp;
        cp.multiply_opacity(k);
        // tool name
        let hr = Rect::from_min_size(bar.min, vec2(head, 34.0));
        cp.rect_filled(hr.shrink(1.0), 0.0, CARD);
        cp.vline(hr.right(), hr.y_range(), Stroke::new(1.0, BORDER));
        tool_icon(&cp, self.tool, pos2(hr.left() + 18.0, hr.center().y), acc());
        lbl(&cp, pos2(hr.left() + 34.0, hr.center().y), Align2::LEFT_CENTER, tool.0, 8.0, TEXT);
        if n_all == 0 {
            let hint = if self.tool == 6 { "click the mesh to pick" } else { "not built yet" };
            lbl(&cp, pos2(hr.right() + 12.0, hr.center().y), Align2::LEFT_CENTER, hint, 7.0, DIM);
        }
        for i in 0..n {
            let (name, max, unit) = tool.1[i];
            let r = Rect::from_min_size(pos2(hr.right() + 4.0 + i as f32 * SW, bar.top() + 4.0), vec2(SW - 4.0, 26.0));
            let mut v = self.tool_v[self.tool][i];
            slider(ui, &cp, r, &format!("qb{}_{i}", self.tool), name, &mut v, max, unit);
            self.tool_v[self.tool][i] = v;
        }

    }

    /// Properties follows the selected layer, effect or mask.
    fn props_panel(&mut self, ui: &mut Ui, p: &Painter, pr: Rect) {
        let kind = self.layers[self.sel_layer].kind;
        let label = if self.sel_mask {
            "properties // mask"
        } else {
            match kind {
                Kind::Paint => "properties // paint",
                Kind::Fill => "properties // fill",
                Kind::Group => "properties // group",
                Kind::Generator => "properties // generator",
                Kind::Filter => "properties // filter",
            }
        };
        let inner = card(p, pr, label, None, "");
        // content fades in when the selection changes
        let mut pp = p.clone();
        pp.multiply_opacity(self.sel_fade(ui));
        let p = &pp;
        let mut y = inner.top() + 10.0;
        let title = if self.sel_mask { format!("mask of {}", self.layers[self.sel_layer].name) } else { self.layers[self.sel_layer].name.to_string() };
        lbl(p, pos2(inner.left() + 8.0, y + 6.0), Align2::LEFT_CENTER, &title, 8.0, SOFT);
        y += 22.0;
        if self.sel_mask {
            for (i, s) in ["opacity", "contrast", "blur"].into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 26.0));
                let mut v = self.mask_p[i];
                slider(ui, p, r, &format!("mask{i}"), s, &mut v, 1.0, "");
                self.mask_p[i] = v;
                y += 28.0;
            }
            return;
        }
        match kind {
            Kind::Paint => {
                let names = ["size", "flow", "hardness", "spacing"];
                let maxes = [100.0, 100.0, 1.0, 100.0];
                let units = ["", "%", "", "%"];
                for i in 0..4 {
                    let r = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 26.0));
                    let mut v = self.props[i];
                    slider(ui, p, r, &format!("prop{i}"), names[i], &mut v, maxes[i], units[i]);
                    self.props[i] = v;
                    y += 28.0;
                }
                y += 8.0;
                swatches(p, pos2(inner.left() + 8.0, y), &[Color32::from_rgb(0x8C, 0x5A, 0x3C), Color32::from_rgb(0x2E, 0x3B, 0x2F)], "#8C5A3C / #2E3B2F");
            }
            Kind::Fill => {
                let names = ["roughness", "metallic", "opacity"];
                for i in 0..3 {
                    let r = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 26.0));
                    let mut v = self.fill[i];
                    slider(ui, p, r, &format!("fill{i}"), names[i], &mut v, 1.0, "");
                    self.fill[i] = v;
                    y += 28.0;
                }
                y += 8.0;
                let c = self.layers[self.sel_layer].color;
                swatches(p, pos2(inner.left() + 8.0, y), &[c], "uniform color · basecolor");
                y += 40.0;
                // a source slot: opens the resource picker
                let sr = Rect::from_min_size(pos2(inner.left() + 8.0, y), vec2(inner.width() - 16.0, 30.0));
                let resp = ui.interact(sr, Id::new("rough_src"), Sense::click());
                let h = an(ui, "rough_src", resp.hovered());
                frame(p, sr, mix(BG, HOVER, h), mix(BORDER2, acc(), h));
                lbl(p, pos2(sr.left() + 10.0, sr.center().y), Align2::LEFT_CENTER, "roughness source", 7.0, DIM);
                txt(p, pos2(sr.right() - 10.0, sr.center().y), Align2::RIGHT_CENTER, &format!("{} ▸", self.view.rough_src), F::Mono, 10.0, TEXT, 0.0);
                if resp.clicked() {
                    self.open_picker();
                }
            }
            Kind::Generator | Kind::Filter => {
                let names: &[&str] = if kind == Kind::Generator { &["wear level", "wear contrast", "amount"] } else { &["intensity", "angle", "amount"] };
                for i in 0..names.len() {
                    let r = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 26.0));
                    let mut v = self.gen[i];
                    slider(ui, p, r, &format!("gen{i}"), names[i], &mut v, 1.0, "");
                    self.gen[i] = v;
                    y += 28.0;
                }
            }
            Kind::Group => {
                lbl(p, pos2(inner.left() + 8.0, y + 6.0), Align2::LEFT_CENTER, "blend · opacity · mask of the group", 8.0, FAINT);
            }
        }
    }

    fn material(&mut self, ui: &mut Ui, p: &Painter, work: Rect) {
        let strip = Rect::from_min_size(work.min + vec2(4.0, 4.0), vec2(40.0, graph::GTOOLS.len() as f32 * 38.0 + 4.0));
        let wins = Rect::from_min_max(pos2(work.right() - 40.0, work.top() + 4.0), work.max - vec2(4.0, 4.0));
        let area = Rect::from_min_max(pos2(strip.right() + 4.0, work.top() + 4.0), pos2(wins.left() - 4.0, work.bottom() - 4.0));
        let specs = Self::material_specs();
        let z = self.dock_zones(ui, area, &specs);
        let graph = z.free;
        // dotted grid
        let mut gy = graph.top() + 12.0;
        while gy < graph.bottom() {
            let mut gx = graph.left() + 12.0;
            while gx < graph.right() {
                p.rect_filled(Rect::from_min_size(pos2(gx, gy), vec2(1.0, 1.0)), 0.0, Color32::from_gray(0x24));
                gx += 24.0;
            }
            gy += 24.0;
        }
        let gp = p.with_clip_rect(graph.intersect(p.clip_rect()));
        self.graph_input(ui, graph);
        self.graph(ui, &gp, graph);
        self.graph_bar(ui, p, graph);
        self.graph_strip(ui, p, strip);
        self.dock_strip(ui, p, wins, &MATERIAL_WINDOWS, &specs);
        self.dock_draw(ui, p, &z, &specs);
    }

    fn nodes_panel(&mut self, ui: &mut Ui, p: &Painter, tb: Rect) {
        let inner = card(p, tb, "nodes", Some(8), "");
        for (i, s) in ["noise", "pattern", "grunge", "blend", "levels", "blur", "height", "output"].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(inner.left(), inner.top() + 2.0 + i as f32 * 26.0), vec2(inner.width(), 26.0));
            let resp = ui.interact(r, Id::new(("ntool", i)), Sense::click());
            let h = an(ui, ("ntool", i), resp.hovered());
            p.rect_filled(r, 0.0, alpha(TEXT, h));
            lbl(p, pos2(r.left() + 10.0 + 3.0 * h, r.center().y), Align2::LEFT_CENTER, s, 8.0, mix(SOFT, BG, h));
            lbl(p, pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, "+", 8.0, mix(FAINT, BG, h));
        }

    }

    fn graph(&mut self, ui: &mut Ui, p: &Painter, graph: Rect) {
        let origin = graph.min + vec2(30.0, 60.0);
        self.gr.origin = origin;
        let nsize = vec2(150.0, 104.0);
        let port = |n: &Node, out: bool, k: u8| -> Pos2 {
            let r = Rect::from_min_size(origin + n.pos, nsize);
            let count = if out { n.outs } else { n.ins }.max(1) as f32;
            let y = r.top() + 30.0 + (r.height() - 30.0) * (k as f32 + 1.0) / (count + 1.0);
            pos2(if out { r.right() } else { r.left() }, y)
        };
        let sel_k: Vec<f32> = (0..self.nodes.len()).map(|i| an(ui, ("nsel", i), self.sel_node == i)).collect();
        for &(a, b, k) in &self.wires {
            let s = port(&self.nodes[a], true, 0);
            let e = port(&self.nodes[b], false, k);
            let dx = ((e.x - s.x) * 0.5).abs().max(40.0);
            let hot = sel_k[a].max(sel_k[b]);
            p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [s, s + vec2(dx, 0.0), e - vec2(dx, 0.0), e],
                false,
                Color32::TRANSPARENT,
                Stroke::new(1.5, mix(Color32::from_gray(0x55), acc(), hot)),
            ));
        }
        for i in 0..self.nodes.len() {
            let r = Rect::from_min_size(origin + self.nodes[i].pos, nsize);
            let resp = ui.interact(r, Id::new(("node", i)), Sense::click_and_drag());
            if resp.dragged() {
                self.nodes[i].pos += resp.drag_delta();
                self.sel_node = i;
            }
            if (resp.clicked() || resp.drag_started()) && self.sel_node != i {
                self.sel_node = i;
                self.sel_t = ui.input(|i| i.time);
            }
            let n = &self.nodes[i];
            let s = sel_k[i];
            let h = an(ui, ("nhov", i), resp.hovered());
            glow(p, r, acc(), 0.5 * s, 8);
            frame(p, r, CARD, mix(mix(BORDER2, DIM, h), acc(), s));
            let hd = Rect::from_min_size(r.min, vec2(r.width(), 26.0));
            p.rect_filled(hd.shrink(1.0), 0.0, mix(PANEL, SEL, s));
            lbl(p, pos2(hd.left() + 8.0, hd.center().y), Align2::LEFT_CENTER, n.name, 8.0, TEXT);
            let th = Rect::from_min_size(pos2(r.left() + 30.0, hd.bottom() + 8.0), vec2(r.width() - 60.0, r.height() - 42.0));
            node_thumb(p, th, i);
            for k in 0..n.ins {
                p.rect_filled(Rect::from_center_size(port(n, false, k), vec2(7.0, 7.0)), 0.0, DIM);
            }
            for k in 0..n.outs {
                p.rect_filled(Rect::from_center_size(port(n, true, k), vec2(7.0, 7.0)), 0.0, mix(TEXT, acc(), s));
            }
        }

    }

    fn preview_panel(&mut self, _ui: &mut Ui, p: &Painter, prev: Rect) {
        let inner = card(p, prev, "3d preview", None, "sphere");
        sphere(p, inner.center(), inner.height().min(inner.width()) * 0.38);
    }

    fn node_props_panel(&mut self, ui: &mut Ui, p: &Painter, pr: Rect) {
        let inner = card(p, pr, "properties // node", None, "");
        let mut pp = p.clone();
        pp.multiply_opacity(self.sel_fade(ui));
        let p = &pp;
        let mut y = inner.top() + 10.0;
        lbl(p, pos2(inner.left() + 8.0, y + 6.0), Align2::LEFT_CENTER, self.nodes[self.sel_node].name, 10.0, TEXT);
        y += 26.0;
        for (i, s) in ["amount", "contrast", "scale", "octaves"].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 26.0));
            let mut v = self.node_props[i];
            slider(ui, p, r, &format!("np{i}"), s, &mut v, if i == 3 { 8.0 } else { 1.0 }, "");
            if i == 3 {
                v = v.round();
            }
            self.node_props[i] = v;
            y += 28.0;
        }
    }
}

fn swatches(p: &Painter, at: Pos2, cols: &[Color32], label: &str) {
    let mut x = at.x;
    for c in cols {
        frame(p, Rect::from_min_size(pos2(x, at.y), vec2(28.0, 28.0)), *c, BORDER2);
        x += 34.0;
    }
    txt(p, pos2(x + 6.0, at.y + 14.0), Align2::LEFT_CENTER, label, F::Mono, 10.0, DIM, 0.0);
}

fn node_thumb(p: &Painter, r: Rect, seed: usize) {
    let cells = 12;
    let cw = r.width() / cells as f32;
    let ch = r.height() / cells as f32;
    for j in 0..cells {
        for i in 0..cells {
            let x = i as f32 / cells as f32;
            let y = j as f32 / cells as f32;
            let v = match seed {
                0 => (x * 7.0).sin() * (y * 5.0).cos() * 0.5 + 0.5,
                1 => if ((i / 3) + (j / 3)) % 2 == 0 { 0.8 } else { 0.25 },
                3 => ((i * 31 + j * 17) % 11) as f32 / 11.0,
                4 => ((x * 9.0).sin() * 0.5 + 0.5) * 0.6 + 0.2,
                5 => (x + y) * 0.5,
                6 => 0.55 + 0.25 * ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt(),
                _ => ((x * 6.0).sin() + (y * 6.0).cos()) * 0.25 + 0.5,
            };
            let g = (v.clamp(0.0, 1.0) * 180.0 + 20.0) as u8;
            p.rect_filled(Rect::from_min_size(pos2(r.left() + i as f32 * cw, r.top() + j as f32 * ch), vec2(cw + 0.5, ch + 0.5)), 0.0, Color32::from_gray(g));
        }
    }
}

fn sphere(p: &Painter, c: Pos2, r: f32) {
    let steps = 28;
    for i in 0..steps {
        let f = i as f32 / steps as f32;
        let center = c + vec2(-r * 0.3, -r * 0.3) * f;
        let g = (40.0 + 150.0 * f.powf(1.6)) as u8;
        p.circle_filled(center, r * (1.0 - f), Color32::from_rgb(g, (g as f32 * 0.85) as u8, (g as f32 * 0.75) as u8));
    }
}

fn tool_icon(p: &Painter, i: usize, c: Pos2, col: Color32) {
    use icons::Icon::*;
    let icon = [Brush, Eraser, Cube, PolyFill, Smudge, Clone, Picker, Select, Bake, Camera][i.min(9)];
    icons::draw(p, Rect::from_center_size(c, vec2(18.0, 18.0)), icon, col);
}

/// Icons the app's set does not have, from Iconify (Phosphor, MIT), tinted like the others.
fn svg_icon(ui: &Ui, r: Rect, name: &'static str, c: Color32) {
    let bytes: &'static [u8] = match name {
        "folder-open" => include_bytes!("../assets/icons/folder-open.svg"),
        "file-arrow-down" => include_bytes!("../assets/icons/file-arrow-down.svg"),
        "squares-four" => include_bytes!("../assets/icons/squares-four.svg"),
        "gear-six" => include_bytes!("../assets/icons/gear-six.svg"),
        "sliders-horizontal" => include_bytes!("../assets/icons/sliders-horizontal.svg"),
        "palette" => include_bytes!("../assets/icons/palette.svg"),
        "monitor" => include_bytes!("../assets/icons/monitor.svg"),
        "books" => include_bytes!("../assets/icons/books.svg"),
        "stack" => include_bytes!("../assets/icons/stack.svg"),
        "sliders" => include_bytes!("../assets/icons/sliders.svg"),
        "grid-four" => include_bytes!("../assets/icons/grid-four.svg"),
        "terminal-window" => include_bytes!("../assets/icons/terminal-window.svg"),
        "hand" => include_bytes!("../assets/icons/hand.svg"),
        "scissors" => include_bytes!("../assets/icons/scissors.svg"),
        "chat-text" => include_bytes!("../assets/icons/chat-text.svg"),
        "plugs-connected" => include_bytes!("../assets/icons/plugs-connected.svg"),
        "faders" => include_bytes!("../assets/icons/faders.svg"),
        "image-square" => include_bytes!("../assets/icons/image-square.svg"),
        "sign-out" => include_bytes!("../assets/icons/sign-out.svg"),
        _ => include_bytes!("../assets/icons/keyboard.svg"),
    };
    egui::Image::new(egui::ImageSource::Bytes { uri: format!("bytes://{name}.svg").into(), bytes: egui::load::Bytes::Static(bytes) })
        .tint(c)
        .paint_at(ui, r);
}

fn recents() -> Vec<Recent> {
    let c = Color32::from_rgb;
    vec![
        Recent { name: "turret_v2", tag: "3D/MAT", when: "2 min ago", thumb: c(0x6A, 0x6A, 0x6A), note: None },
        Recent { name: "painted_metal_worn", tag: "MAT", when: "1 hour ago", thumb: c(0x1E, 0x1E, 0x1E), note: None },
        Recent { name: "crate_kit", tag: "3D", when: "yesterday", thumb: c(0x48, 0x48, 0x48), note: None },
        Recent { name: "concrete_slab_cracked", tag: "MAT", when: "3 days ago", thumb: c(0x1E, 0x1E, 0x1E), note: None },
        Recent { name: "radio_handset", tag: "3D", when: "last week", thumb: c(0x3E, 0x4A, 0x3C), note: Some(("[autosave]", WARN)) },
        Recent { name: "old_helmet", tag: "3D", when: "1 month ago", thumb: c(0x1E, 0x1E, 0x1E), note: Some(("[missing]", ERR)) },
    ]
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1440.0, 900.0]).with_title("ShardStroke — UI mockup"),
        ..Default::default()
    };
    eframe::run_native("shard-mockup", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
