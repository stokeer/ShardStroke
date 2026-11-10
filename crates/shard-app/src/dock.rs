//! The windows laid over the 3D viewport (Painter's "views"). The viewport is *not* one of them: it is the
//! background of the program, fixed, drawn under everything; the windows sit on its left, right and bottom
//! sides or float over it, and the area they leave open is where the scene is centred and the brush works.
//!
//! The user arranges them (the behaviour validated in design/mockup):
//! - hold a window's title (or one of its tabs) still for half a second, then drag it; moving before that
//!   cancels, a bar fills while you hold, a quick click never moves anything. A floating window moves at once.
//! - drop it on another window's title or middle = a tab of that window; on its top / bottom half (left / right
//!   half on the bottom side) = that slot splits in two; near the left / right edge or the bottom = a new slot on
//!   that side; anywhere else = floating. The target lights up while you hover it.
//! - the gaps between windows and the inner edge of each side resize; a floating window resizes by its corner.
//! - X closes; the window buttons on the right edge and Window > Views bring windows back.
//! - "Lock layout" hides the X and stops all of it.
//! - with the advanced animation level, ASCII characters rain from under a window being moved and a floating
//!   window dissolves into them when it is closed.
//!
//! The arrangement is saved per user (`windows.json`) and restored at the next start.

use crate::theme;
use crate::widgets::mix;
use egui::{pos2, vec2, Align2, Color32, FontId, Id, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    Properties,
    Assets,
    TextureSetList,
    Layers,
    TextureSetSettings,
    DisplaySettings,
    ShaderSettings,
    History,
    Log,
    // MATERIAL mode (the node graph): structure only until the graph engine exists
    Nodes,
    NodeProperties,
    Preview3d,
    GraphParameters,
    Outputs,
    View2d,
}

impl Pane {
    /// Every window, in the order of the Window > Views menu.
    pub const ALL: [Pane; 9] = [
        Pane::Properties,
        Pane::Assets,
        Pane::TextureSetList,
        Pane::Layers,
        Pane::TextureSetSettings,
        Pane::DisplaySettings,
        Pane::ShaderSettings,
        Pane::History,
        Pane::Log,
    ];

    /// The windows of MATERIAL mode, in the order of its Window > Views menu.
    pub const MATERIAL: [Pane; 9] =
        [Pane::Nodes, Pane::Preview3d, Pane::NodeProperties, Pane::GraphParameters, Pane::Outputs, Pane::View2d, Pane::History, Pane::Assets, Pane::Log];

    /// The windows of the default layout (the others open from the right strip or Window > Views).
    pub const DEFAULT: [Pane; 9] = [
        Pane::TextureSetList,
        Pane::History,
        Pane::Layers,
        Pane::Properties,
        Pane::DisplaySettings,
        Pane::ShaderSettings,
        Pane::TextureSetSettings,
        Pane::Assets,
        Pane::Log,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Properties => "Properties",
            Pane::Assets => "Assets",
            Pane::TextureSetList => "Texture Set List",
            Pane::Layers => "Layers",
            Pane::TextureSetSettings => "Texture Set Settings",
            Pane::DisplaySettings => "Display Settings",
            Pane::ShaderSettings => "Shader Settings",
            Pane::History => "History",
            Pane::Log => "Log",
            Pane::Nodes => "Nodes",
            Pane::NodeProperties => "Node Properties",
            Pane::Preview3d => "3D Preview",
            Pane::GraphParameters => "Graph Parameters",
            Pane::Outputs => "Outputs",
            Pane::View2d => "2D View",
        }
    }

    /// Where a window comes back when it was closed: the side it starts on in the default layout.
    fn home(self) -> usize {
        match self {
            Pane::TextureSetList | Pane::TextureSetSettings | Pane::History | Pane::Nodes => LEFT,
            Pane::Assets | Pane::Log => BOTTOM,
            _ => RIGHT,
        }
    }
}

const LEFT: usize = 0;
const RIGHT: usize = 1;
const BOTTOM: usize = 2;
const HEADER: f32 = 30.0;
const HOLD_SECS: f64 = 0.5;

/// A place on a side: one window, or several as tabs.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Slot {
    pub tabs: Vec<Pane>,
    pub active: usize,
    /// Share of its side.
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Floating {
    pub pane: Pane,
    pub rect: [f32; 4],
}

impl Floating {
    fn rect(&self) -> Rect {
        Rect::from_min_size(pos2(self.rect[0], self.rect[1]), vec2(self.rect[2], self.rect[3]))
    }

    fn set(&mut self, r: Rect) {
        self.rect = [r.min.x, r.min.y, r.width(), r.height()];
    }
}

/// The arrangement: slots of the left, right and bottom sides, the size of each side, floating windows.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Dock {
    pub sides: [Vec<Slot>; 3],
    pub size: [f32; 3],
    pub floats: Vec<Floating>,
    #[serde(skip)]
    st: Interaction,
}

/// What the user is doing with the windows right now (never saved).
#[derive(Clone, Debug, Default)]
struct Interaction {
    /// A title pressed: (window, since, where).
    hold: Option<(Pane, f64, Pos2)>,
    /// A window being moved: (window, grab offset, size).
    drag: Option<(Pane, Vec2, Vec2)>,
    resizing: bool,
    /// Where every window was drawn last frame.
    rects: Vec<(Pane, Rect)>,
    rain: Vec<RainChar>,
    seed: u32,
}

#[derive(Clone, Debug)]
struct RainChar {
    pos: Pos2,
    vel: Vec2,
    ch: char,
    life: f32,
    /// Seconds it stays put before falling (the dissolve sweeps top to bottom).
    wait: f32,
}

const RAIN: [char; 10] = ['.', ':', '`', '|', '/', '+', '*', '0', '1', '#'];

/// Where a moved window lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// Nothing changes (dropped on its own window).
    Stay,
    /// A tab of the window holding this pane.
    Tab(Pane),
    /// Splits the slot holding this pane; true = the new one goes first (above / left).
    Split(Pane, bool),
    /// A new slot on a side (0 left, 1 right, 2 bottom); true = first.
    Side(usize, bool),
    Float(Rect),
}

/// Painter's default arrangement: properties + assets on the left, the free viewport area in the middle,
/// texture set list over layers + texture set settings on the right.
pub fn default_layout() -> Dock {
    let slot = |tabs: Vec<Pane>, weight: f32| Slot { tabs, active: 0, weight };
    Dock {
        sides: [
            // the arrangement of the agreed mockup: sets and history on the left, assets and log under the
            // viewport, layers over the properties on the right
            vec![slot(vec![Pane::TextureSetList, Pane::TextureSetSettings], 1.0), slot(vec![Pane::History], 0.66)],
            vec![
                slot(vec![Pane::Layers], 1.0),
                slot(vec![Pane::Properties, Pane::DisplaySettings, Pane::ShaderSettings], 0.75),
            ],
            vec![slot(vec![Pane::Assets], 1.0), slot(vec![Pane::Log], 0.55)],
        ],
        size: [340.0, 420.0, 230.0],
        floats: Vec::new(),
        st: Interaction::default(),
    }
}

/// MATERIAL mode's arrangement: nodes on the left, the 3D preview over the node's properties on the right.
pub fn material_layout() -> Dock {
    let slot = |tabs: Vec<Pane>, weight: f32| Slot { tabs, active: 0, weight };
    Dock {
        sides: [
            vec![slot(vec![Pane::Nodes], 1.0)],
            vec![slot(vec![Pane::Preview3d], 0.9), slot(vec![Pane::NodeProperties, Pane::GraphParameters], 1.0)],
            vec![slot(vec![Pane::Log], 1.0)],
        ],
        size: [220.0, 340.0, 180.0],
        floats: Vec::new(),
        st: Interaction::default(),
    }
}

fn layout_path(material: bool) -> Option<std::path::PathBuf> {
    // a new name: the arrangement of the previous window system (layout.json) does not apply any more
    Some(crate::config::config_dir()?.join(if material { "windows_material.json" } else { "windows.json" }))
}

/// The saved arrangement, if there is one.
pub fn load_layout() -> Option<Dock> {
    parse_layout(&std::fs::read_to_string(layout_path(false)?).ok()?)
}

/// MATERIAL mode's saved arrangement, if there is one.
pub fn load_material_layout() -> Option<Dock> {
    parse_layout(&std::fs::read_to_string(layout_path(true)?).ok()?)
}

pub fn parse_layout(json: &str) -> Option<Dock> {
    let mut d: Dock = serde_json::from_str(json).ok()?;
    // each window once; empty slots dropped
    let mut seen = Vec::new();
    for side in d.sides.iter_mut() {
        for s in side.iter_mut() {
            s.tabs.retain(|p| {
                let first = !seen.contains(p);
                seen.push(*p);
                first
            });
            s.active = s.active.min(s.tabs.len().saturating_sub(1));
            s.weight = s.weight.max(0.1);
        }
        side.retain(|s| !s.tabs.is_empty());
    }
    d.floats.retain(|f| {
        let first = !seen.contains(&f.pane);
        seen.push(f.pane);
        first && f.rect.iter().all(|v| v.is_finite())
    });
    Some(d)
}

pub fn layout_json(d: &Dock) -> Option<String> {
    serde_json::to_string_pretty(d).ok()
}

pub fn save_layout_json(json: &str) {
    save_layout_to(false, json);
}

pub fn save_material_layout_json(json: &str) {
    save_layout_to(true, json);
}

fn save_layout_to(material: bool, json: &str) {
    let Some(p) = layout_path(material) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, json);
}

impl Dock {
    /// Is the window in the layout (docked, as any tab, or floating)?
    pub fn contains(&self, pane: Pane) -> bool {
        self.sides.iter().flatten().any(|s| s.tabs.contains(&pane)) || self.floats.iter().any(|f| f.pane == pane)
    }

    /// Is the window showing: the active tab of its slot, or floating?
    pub fn shown(&self, pane: Pane) -> bool {
        self.sides.iter().flatten().any(|s| s.tabs.get(s.active) == Some(&pane)) || self.floats.iter().any(|f| f.pane == pane)
    }

    /// Bring `pane` to front, or re-open it on its home side when it was closed.
    pub fn show(&mut self, pane: Pane) {
        for s in self.sides.iter_mut().flatten() {
            if let Some(i) = s.tabs.iter().position(|p| *p == pane) {
                s.active = i;
                return;
            }
        }
        if self.floats.iter().any(|f| f.pane == pane) {
            return;
        }
        // beside the layers when they are docked, else a new slot on its home side
        if let Some(s) = self.sides.iter_mut().flatten().find(|s| s.tabs.contains(&Pane::Layers)) {
            s.tabs.push(pane);
            s.active = s.tabs.len() - 1;
            return;
        }
        self.sides[pane.home()].push(Slot { tabs: vec![pane], active: 0, weight: 1.0 });
    }

    /// Takes a window out of wherever it is; empty slots go away.
    pub fn remove(&mut self, pane: Pane) {
        for side in self.sides.iter_mut() {
            for s in side.iter_mut() {
                if let Some(i) = s.tabs.iter().position(|p| *p == pane) {
                    s.tabs.remove(i);
                    s.active = s.active.min(s.tabs.len().saturating_sub(1));
                }
            }
            side.retain(|s| !s.tabs.is_empty());
        }
        self.floats.retain(|f| f.pane != pane);
    }

    /// Moves `pane` to `target` (what a drop does).
    pub fn apply(&mut self, pane: Pane, target: Target) {
        if target == Target::Stay {
            return;
        }
        self.remove(pane);
        match target {
            Target::Tab(other) => {
                if let Some(s) = self.sides.iter_mut().flatten().find(|s| s.tabs.contains(&other)) {
                    s.tabs.push(pane);
                    s.active = s.tabs.len() - 1;
                } else {
                    self.show(pane);
                }
            }
            Target::Split(other, before) => {
                for side in self.sides.iter_mut() {
                    if let Some(i) = side.iter().position(|s| s.tabs.contains(&other)) {
                        side[i].weight /= 2.0;
                        let w = side[i].weight;
                        side.insert(if before { i } else { i + 1 }, Slot { tabs: vec![pane], active: 0, weight: w });
                        return;
                    }
                }
                self.show(pane);
            }
            Target::Side(side, first) => {
                let list = &mut self.sides[side];
                let w = if list.is_empty() { 1.0 } else { list.iter().map(|s| s.weight).sum::<f32>() / list.len() as f32 };
                let slot = Slot { tabs: vec![pane], active: 0, weight: w };
                if first {
                    list.insert(0, slot);
                } else {
                    list.push(slot);
                }
            }
            Target::Float(r) => {
                let mut f = Floating { pane, rect: [0.0; 4] };
                f.set(r);
                self.floats.push(f);
            }
            Target::Stay => {}
        }
    }

    /// Where the window was drawn last frame.
    pub fn rect_of(&self, pane: Pane) -> Option<Rect> {
        self.st.rects.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r)
    }

    /// The three sides and the free area of `area` for this frame.
    fn zones(&self, ctx: &egui::Context, area: Rect) -> Zones {
        let t = if self.st.resizing { 0.0 } else { theme::anim(0.22) };
        let sz = |i: usize| ctx.animate_value_with_time(Id::new(("dock_zone", i)), if self.sides[i].is_empty() { 0.0 } else { self.size[i] }, t);
        let (lw, rw, bh) = (sz(LEFT), sz(RIGHT), sz(BOTTOM));
        let gap = theme::GAP;
        let left = (lw > 1.0).then(|| Rect::from_min_size(area.min, vec2(lw, area.height())));
        let right = (rw > 1.0).then(|| Rect::from_min_max(pos2(area.max.x - rw, area.min.y), area.max));
        let bl = left.map_or(area.min.x, |z| z.max.x + gap);
        let br = right.map_or(area.max.x, |z| z.min.x - gap);
        let bottom = (bh > 1.0).then(|| Rect::from_min_max(pos2(bl, area.max.y - bh), pos2(br, area.max.y)));
        let free = Rect::from_min_max(pos2(bl, area.min.y), pos2(br, bottom.map_or(area.max.y, |z| z.min.y - gap)));
        let mut slots = Vec::new();
        for (side, zone) in [left, right, bottom].into_iter().enumerate() {
            let Some(z) = zone else { continue };
            let list = &self.sides[side];
            let vertical = side != BOTTOM;
            let total: f32 = list.iter().map(|s| s.weight).sum::<f32>().max(1e-3);
            let len = if vertical { z.height() } else { z.width() } - gap * list.len().saturating_sub(1) as f32;
            let mut at = if vertical { z.min.y } else { z.min.x };
            for (i, s) in list.iter().enumerate() {
                let n = len * s.weight / total;
                let r = if vertical {
                    Rect::from_min_size(pos2(z.min.x, at), vec2(z.width(), n))
                } else {
                    Rect::from_min_size(pos2(at, z.min.y), vec2(n, z.height()))
                };
                slots.push((side, i, r));
                at += n + gap;
            }
        }
        Zones { left, right, bottom, free, area, slots }
    }

    /// Lays the windows out in `area` and draws them; `title` names a tab, `body` fills a window. Returns the
    /// free area (the viewport's).
    pub fn show_in(
        &mut self,
        ctx: &egui::Context,
        area: Rect,
        locked: bool,
        title: &dyn Fn(Pane) -> String,
        body: &mut dyn FnMut(&mut Ui, Pane),
    ) -> Rect {
        let z = self.zones(ctx, area);
        let mut drawn = Vec::new();
        // docked windows glide to their slot
        let t = if self.st.resizing { 0.0 } else { theme::anim(0.22) };
        for &(side, idx, target) in &z.slots {
            let first = self.sides[side][idx].tabs[0];
            let a = |n: &str, v: f32| ctx.animate_value_with_time(Id::new(("dock_slot", first, n)), v, t);
            let r = Rect::from_min_size(pos2(a("x", target.min.x), a("y", target.min.y)), vec2(a("w", target.width()), a("h", target.height())));
            let (tabs, active) = (self.sides[side][idx].tabs.clone(), self.sides[side][idx].active);
            let picked = self.window(ctx, Id::new(("dock_win", first)), Order::Background, r, &tabs, active, locked, false, title, body);
            if let Some(i) = picked {
                self.sides[side][idx].active = i;
            }
            drawn.extend(tabs.iter().map(|p| (*p, r)));
        }
        if !locked && self.st.drag.is_none() {
            self.resize_handles(ctx, &z);
        } else {
            self.st.resizing = false;
        }
        // floating windows, on top
        for i in 0..self.floats.len() {
            let f = self.floats[i].clone();
            if self.st.drag.is_some_and(|d| d.0 == f.pane) {
                continue;
            }
            let r = f.rect();
            self.window(ctx, Id::new(("dock_float", f.pane)), Order::Middle, r, &[f.pane], 0, locked, true, title, body);
            drawn.push((f.pane, r));
        }
        self.st.rects = drawn;
        self.drag_and_drop(ctx, &z, locked, title);
        self.rain(ctx);
        z.free
    }

    /// One window: frame, title or tabs, X, body. Returns a tab picked this frame.
    #[allow(clippy::too_many_arguments)]
    fn window(
        &mut self,
        ctx: &egui::Context,
        id: Id,
        order: Order,
        r: Rect,
        tabs: &[Pane],
        active: usize,
        locked: bool,
        floating: bool,
        title: &dyn Fn(Pane) -> String,
        body: &mut dyn FnMut(&mut Ui, Pane),
    ) -> Option<usize> {
        let mut picked = None;
        let pane = tabs[active.min(tabs.len() - 1)];
        let moving = self.st.drag.is_some_and(|d| d.0 == pane);
        egui::Area::new(id).order(order).fixed_pos(r.min).constrain(false).fade_in(false).show(ctx, |ui| {
            ui.set_clip_rect(r);
            let (_, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            if moving {
                ui.multiply_opacity(0.3);
            }
            let p = ui.painter().clone();
            if floating {
                theme::glow(&p, r, Color32::BLACK, 0.55, 20);
            }
            p.rect_filled(r, 0.0, theme::PANEL);
            let hdr = Rect::from_min_size(r.min, vec2(r.width(), HEADER));
            p.rect_filled(hdr.shrink(theme::WINDOW_STROKE), 0.0, theme::PANEL_ALT);
            p.hline(hdr.x_range(), hdr.max.y, Stroke::new(1.0, theme::BORDER));
            // title or tabs; each is a handle
            let x_room = if locked { 6.0 } else { 30.0 };
            let strip = Rect::from_min_max(hdr.min, pos2(hdr.max.x - x_room, hdr.max.y));
            let names: Vec<String> = tabs
                .iter()
                .map(|t| {
                    let n = title(*t).to_uppercase();
                    // tab widths ignore the count (it is not drawn on tabs)
                    if tabs.len() > 1 { n.rfind(" [").map(|k| n[..k].to_string()).unwrap_or(n) } else { n }
                })
                .collect();
            let natural: f32 = names.iter().map(|n| n.len() as f32 * 10.0 + 24.0).sum();
            let squeeze = (strip.width() / natural.max(1.0)).min(1.0);
            let mut x = strip.min.x + 2.0;
            for (i, (t, name)) in tabs.iter().zip(&names).enumerate() {
                let w = if tabs.len() == 1 { strip.width() - 4.0 } else { (name.len() as f32 * 10.0 + 24.0) * squeeze };
                let tr = Rect::from_min_size(pos2(x, hdr.min.y + 2.0), vec2(w, HEADER - 4.0));
                x += w;
                let resp = self.handle(ui, &p, tr, *t, r, floating, locked);
                if resp.clicked() && tabs.len() > 1 {
                    picked = Some(i);
                }
                let on = i == active;
                if tabs.len() > 1 {
                    let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim(0.16));
                    let o = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, theme::anim(0.16));
                    p.rect_filled(tr, 0.0, mix(mix(Color32::TRANSPARENT, theme::WIDGET_HOVER, h), theme::PANEL, o));
                    p.rect_filled(Rect::from_min_max(pos2(tr.min.x, tr.max.y - 2.0), tr.max), 0.0, mix(Color32::TRANSPARENT, theme::accent(), o));
                    if i > 0 {
                        p.vline(tr.min.x, tr.y_range(), Stroke::new(1.0, theme::BORDER));
                    }
                }
                let tp = p.with_clip_rect(tr.intersect(p.clip_rect()));
                let col = if on { theme::TEXT } else { theme::TEXT_DIM };
                let font = FontId::proportional(if squeeze < 0.8 { 8.0 } else { 10.0 });
                // a trailing " [n]" is the window's count: dimmer, and left out of tabs (no room there)
                let (head, count) = match name.rfind(" [") {
                    Some(k) => (&name[..k], Some(&name[k + 1..])),
                    None => (name.as_str(), None),
                };
                if tabs.len() == 1 {
                    let at = tp.text(pos2(tr.min.x + 8.0, tr.center().y), Align2::LEFT_CENTER, head, font.clone(), col);
                    if let Some(c) = count {
                        tp.text(pos2(at.max.x + 6.0, tr.center().y), Align2::LEFT_CENTER, c, FontId::proportional(8.0), theme::TEXT_DISABLED);
                    }
                } else {
                    tp.text(tr.center(), Align2::CENTER_CENTER, head, font, col);
                }
            }
            if !locked && self.close_x(ui, &p, r, pane) && floating {
                self.dissolve(r);
            }
            // body
            let inner = Rect::from_min_max(pos2(r.min.x + 8.0, hdr.max.y + 6.0), r.max - vec2(8.0, 6.0));
            if !moving || floating {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(("dock_body", pane)));
                child.set_clip_rect(inner.intersect(r));
                body(&mut child, pane);
            }
            p.rect_stroke(r, 0.0, Stroke::new(theme::WINDOW_STROKE, theme::BORDER), egui::StrokeKind::Inside);
            // corner resize of a floating window
            if floating && !locked {
                let c = Rect::from_min_max(r.max - vec2(14.0, 14.0), r.max);
                let cr = ui.interact(c, Id::new(("dock_corner", pane)), Sense::drag());
                let h = ui.ctx().animate_bool_with_time(cr.id, cr.hovered() || cr.dragged(), theme::anim(0.16));
                for k in 0..3 {
                    let o = 4.0 + k as f32 * 3.5;
                    p.line_segment([c.max - vec2(o, 2.0), c.max - vec2(2.0, o)], Stroke::new(1.0, mix(theme::TEXT_DISABLED, theme::accent(), h)));
                }
                if cr.hovered() || cr.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
                if cr.dragged() {
                    let d = cr.drag_delta();
                    if let Some(f) = self.floats.iter_mut().find(|f| f.pane == pane) {
                        let fr = f.rect();
                        f.set(Rect::from_min_max(fr.min, (fr.max + d).max(fr.min + vec2(220.0, 160.0))));
                    }
                }
            }
        });
        picked
    }

    /// A title or tab: a click passes through (returned); held still for half a second the window comes loose;
    /// a floating one moves at once.
    #[allow(clippy::too_many_arguments)]
    fn handle(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect, pane: Pane, window: Rect, floating: bool, locked: bool) -> egui::Response {
        let resp = ui.interact(rect, Id::new(("dock_hdr", pane)), Sense::click_and_drag());
        if locked {
            return resp;
        }
        let now = ui.input(|i| i.time);
        let q = ui.input(|i| i.pointer.interact_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        if floating && resp.drag_started() && self.st.drag.is_none() {
            if let Some(q) = q {
                self.st.drag = Some((pane, q - window.min, window.size()));
                self.st.hold = None;
            }
            return resp;
        }
        if !floating && resp.is_pointer_button_down_on() && self.st.hold.is_none() && self.st.drag.is_none() {
            if let Some(q) = q {
                self.st.hold = Some((pane, now, q));
            }
        }
        if let Some((k, t0, q0)) = self.st.hold {
            if k == pane {
                let moved = q.is_some_and(|q| (q - q0).length() > 6.0);
                if !down || moved {
                    self.st.hold = None;
                } else {
                    let f = ((now - t0) / HOLD_SECS).clamp(0.0, 1.0) as f32;
                    p.rect_filled(Rect::from_min_size(pos2(rect.min.x, rect.max.y - 2.0), vec2(rect.width() * f, 2.0)), 0.0, theme::accent());
                    ui.ctx().request_repaint();
                    if f >= 1.0 {
                        let size = vec2(window.width().clamp(240.0, 440.0), window.height().clamp(200.0, 560.0));
                        let grab = (q0 - window.min).min(size - vec2(40.0, 10.0)).max(vec2(10.0, 10.0));
                        self.st.drag = Some((pane, grab, size));
                        self.st.hold = None;
                    }
                }
            }
        }
        if resp.hovered() && self.st.hold.is_none() && self.st.drag.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        resp
    }

    /// The X: true when it closed the window this frame.
    fn close_x(&mut self, ui: &mut Ui, p: &egui::Painter, r: Rect, pane: Pane) -> bool {
        let x = Rect::from_center_size(pos2(r.max.x - 15.0, r.min.y + 15.0), vec2(18.0, 18.0));
        let xr = ui.interact(x, Id::new(("dock_x", pane)), Sense::click());
        let h = ui.ctx().animate_bool_with_time(xr.id, xr.hovered(), theme::anim(0.16));
        p.rect_filled(x.shrink(2.0), 0.0, mix(Color32::TRANSPARENT, theme::TEXT, h));
        crate::icons::draw(p, x.shrink(5.0), crate::icons::Icon::Close, mix(theme::TEXT_DIM, theme::BG_VIEWPORT, h));
        if xr.clicked() {
            self.remove(pane);
            return true;
        }
        false
    }

    /// The gaps between windows of a side, and the inner edge of each side.
    fn resize_handles(&mut self, ctx: &egui::Context, z: &Zones) {
        let lp = ctx.layer_painter(egui::LayerId::new(Order::Middle, Id::new("dock_handles")));
        let mut any = false;
        egui::Area::new(Id::new("dock_handles_area")).order(Order::Middle).fixed_pos(z.area.min).constrain(false).fade_in(false).interactable(true).show(ctx, |ui| {
            ui.set_clip_rect(z.area.expand(4.0));
            for w in z.slots.windows(2) {
                let ((sa, ia, ra), (sb, _, rb)) = (w[0], w[1]);
                if sa != sb {
                    continue;
                }
                let vertical = sa != BOTTOM;
                let band = if vertical {
                    Rect::from_min_max(pos2(ra.min.x, ra.max.y - 1.0), pos2(ra.max.x, rb.min.y + 1.0))
                } else {
                    Rect::from_min_max(pos2(ra.max.x - 1.0, ra.min.y), pos2(rb.min.x + 1.0, ra.max.y))
                };
                let resp = ui.interact(band, Id::new(("dock_split", sa, ia)), Sense::drag());
                let h = ctx.animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), theme::anim(0.16));
                lp.rect_filled(band, 0.0, mix(Color32::TRANSPARENT, theme::accent(), h));
                if resp.hovered() || resp.dragged() {
                    ctx.set_cursor_icon(if vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
                }
                if resp.dragged() {
                    any = true;
                    let px = if vertical { resp.drag_delta().y } else { resp.drag_delta().x };
                    let len: f32 = z.slots.iter().filter(|s| s.0 == sa).map(|s| if vertical { s.2.height() } else { s.2.width() }).sum();
                    let list = &mut self.sides[sa];
                    let total: f32 = list.iter().map(|s| s.weight).sum();
                    let dw = px * total / len.max(1.0);
                    let min = total * 0.12;
                    if list[ia].weight + dw >= min && list[ia + 1].weight - dw >= min {
                        list[ia].weight += dw;
                        list[ia + 1].weight -= dw;
                    }
                }
            }
            let gap = theme::GAP;
            let edges = [
                (LEFT, z.left.map(|r| Rect::from_min_max(pos2(r.max.x - 1.0, r.min.y), pos2(r.max.x + gap + 1.0, r.max.y)))),
                (RIGHT, z.right.map(|r| Rect::from_min_max(pos2(r.min.x - gap - 1.0, r.min.y), pos2(r.min.x + 1.0, r.max.y)))),
                (BOTTOM, z.bottom.map(|r| Rect::from_min_max(pos2(r.min.x, r.min.y - gap - 1.0), pos2(r.max.x, r.min.y + 1.0)))),
            ];
            for (side, band) in edges {
                let Some(band) = band else { continue };
                let resp = ui.interact(band, Id::new(("dock_edge", side)), Sense::drag());
                let h = ctx.animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), theme::anim(0.16));
                lp.rect_filled(band, 0.0, mix(Color32::TRANSPARENT, theme::accent(), h));
                if resp.hovered() || resp.dragged() {
                    ctx.set_cursor_icon(if side == BOTTOM { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
                }
                if resp.dragged() {
                    any = true;
                    let d = resp.drag_delta();
                    let s = &mut self.size[side];
                    *s = match side {
                        LEFT => *s + d.x,
                        RIGHT => *s - d.x,
                        _ => *s - d.y,
                    }
                    .clamp(if side == BOTTOM { 120.0 } else { 220.0 }, if side == BOTTOM { 600.0 } else { 720.0 });
                }
            }
        });
        self.st.resizing = any;
    }

    /// The window being moved follows the pointer; its landing place lights up; releasing applies it.
    fn drag_and_drop(&mut self, ctx: &egui::Context, z: &Zones, locked: bool, title: &dyn Fn(Pane) -> String) {
        let Some((pane, grab, size)) = self.st.drag else { return };
        if locked {
            self.st.drag = None;
            return;
        }
        let Some(q) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let r = Rect::from_min_size(q - grab, size);
        let target = self.target(z, q, pane, r);
        let lp = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("dock_preview")));
        if let Some((zr, label)) = self.target_rect(z, &target) {
            lp.rect_filled(zr, 0.0, Color32::from_rgba_unmultiplied(theme::accent().r(), theme::accent().g(), theme::accent().b(), 36));
            lp.rect_stroke(zr, 0.0, Stroke::new(2.0, theme::accent()), egui::StrokeKind::Inside);
            lp.text(zr.center(), Align2::CENTER_CENTER, label, FontId::proportional(10.0), Color32::WHITE);
        }
        // the window itself (title only: its body stays where it is until it lands)
        egui::Area::new(Id::new("dock_dragged")).order(Order::Foreground).fixed_pos(r.min).constrain(false).interactable(false).show(ctx, |ui| {
            let (_, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter();
            theme::glow(p, r, Color32::BLACK, 0.6, 24);
            p.rect_filled(r, 0.0, theme::PANEL);
            p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), HEADER)).shrink(theme::WINDOW_STROKE), 0.0, theme::PANEL_ALT);
            p.text(pos2(r.min.x + 10.0, r.min.y + HEADER * 0.5), Align2::LEFT_CENTER, title(pane).to_uppercase(), FontId::proportional(10.0), theme::TEXT);
            p.rect_stroke(r, 0.0, Stroke::new(theme::WINDOW_STROKE, theme::accent()), egui::StrokeKind::Inside);
        });
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        self.spawn_rain(r, dt);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        ctx.request_repaint();
        if ctx.input(|i| i.pointer.primary_down()) {
            return;
        }
        self.st.drag = None;
        self.apply(pane, target);
    }

    fn target(&self, z: &Zones, q: Pos2, pane: Pane, moved: Rect) -> Target {
        for &(side, idx, r) in &z.slots {
            if !r.contains(q) {
                continue;
            }
            let s = &self.sides[side][idx];
            let Some(other) = s.tabs.iter().copied().find(|t| *t != pane) else { return Target::Stay };
            if q.y < r.min.y + HEADER || r.shrink2(r.size() * 0.3).contains(q) {
                return Target::Tab(other);
            }
            let before = if side != BOTTOM { q.y < r.center().y } else { q.x < r.center().x };
            return Target::Split(other, before);
        }
        let a = z.area;
        if q.x < a.min.x + 120.0 {
            Target::Side(LEFT, q.y < a.center().y)
        } else if q.x > a.max.x - 120.0 {
            Target::Side(RIGHT, q.y < a.center().y)
        } else if q.y > a.max.y - 110.0 {
            Target::Side(BOTTOM, q.x < a.center().x)
        } else {
            Target::Float(moved)
        }
    }

    fn target_rect(&self, z: &Zones, t: &Target) -> Option<(Rect, &'static str)> {
        let slot_of = |k: Pane| z.slots.iter().find(|(s, i, _)| self.sides[*s][*i].tabs.contains(&k)).map(|(s, _, r)| (*s, *r));
        match *t {
            Target::Tab(k) => slot_of(k).map(|(_, r)| (r, "TAB")),
            Target::Split(k, before) => {
                let (side, r) = slot_of(k)?;
                let half = if side != BOTTOM {
                    if before { Rect::from_min_max(r.min, pos2(r.max.x, r.center().y)) } else { Rect::from_min_max(pos2(r.min.x, r.center().y), r.max) }
                } else if before {
                    Rect::from_min_max(r.min, pos2(r.center().x, r.max.y))
                } else {
                    Rect::from_min_max(pos2(r.center().x, r.min.y), r.max)
                };
                Some((half, "SPLIT"))
            }
            Target::Side(side, _) => {
                let a = z.area;
                let r = match side {
                    LEFT => z.left.unwrap_or(Rect::from_min_size(a.min, vec2(300.0, a.height()))),
                    RIGHT => z.right.unwrap_or(Rect::from_min_max(pos2(a.max.x - 300.0, a.min.y), a.max)),
                    _ => z.bottom.unwrap_or(Rect::from_min_max(pos2(a.min.x, a.max.y - 200.0), a.max)),
                };
                Some((r, ["DOCK LEFT", "DOCK RIGHT", "DOCK BOTTOM"][side]))
            }
            Target::Float(_) | Target::Stay => None,
        }
    }

    // ------------------------------------------------------------ ASCII effects (advanced animation level)

    fn rnd(&mut self) -> f32 {
        let s = &mut self.st.seed;
        *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*s >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Characters falling from under the window being moved.
    fn spawn_rain(&mut self, r: Rect, dt: f32) {
        if !theme::heavy() {
            return;
        }
        let n = (r.width() * dt * 0.9).ceil() as usize;
        for _ in 0..n {
            let x = r.min.x + self.rnd() * r.width();
            let ch = RAIN[(self.rnd() * RAIN.len() as f32) as usize % RAIN.len()];
            let vel = vec2((self.rnd() - 0.5) * 14.0, 30.0 + self.rnd() * 60.0);
            let life = 0.25 + self.rnd() * 0.3;
            self.st.rain.push(RainChar { pos: pos2(x, r.max.y + 2.0), vel, ch, life, wait: 0.0 });
        }
    }

    /// A closing floating window turns into characters that fall apart from the top down.
    fn dissolve(&mut self, r: Rect) {
        if !theme::heavy() {
            return;
        }
        let mut y = r.min.y + 6.0;
        while y < r.max.y {
            let mut x = r.min.x + 5.0;
            while x < r.max.x {
                if self.rnd() < 0.7 {
                    let ch = RAIN[(self.rnd() * RAIN.len() as f32) as usize % RAIN.len()];
                    let vel = vec2((self.rnd() - 0.5) * 40.0, -10.0 - self.rnd() * 30.0);
                    let wait = (y - r.min.y) / r.height() * 0.3 + self.rnd() * 0.06;
                    let life = 0.3 + self.rnd() * 0.3;
                    self.st.rain.push(RainChar { pos: pos2(x, y), vel, ch, life, wait });
                }
                x += 9.0;
            }
            y += 12.0;
        }
    }

    fn rain(&mut self, ctx: &egui::Context) {
        if self.st.rain.is_empty() {
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        let lp = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("dock_rain")));
        let acc = theme::accent();
        for d in self.st.rain.iter_mut() {
            let a = if d.wait > 0.0 {
                d.wait -= dt;
                1.0
            } else {
                d.vel.y += 260.0 * dt;
                d.pos += d.vel * dt;
                d.life -= dt;
                let t = (d.life / 0.55).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            let col = Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), (a * 255.0) as u8);
            lp.text(d.pos, Align2::CENTER_CENTER, d.ch.to_string(), FontId::monospace(11.0), col);
        }
        self.st.rain.retain(|d| d.life > 0.0);
        ctx.request_repaint();
    }
}

struct Zones {
    left: Option<Rect>,
    right: Option<Rect>,
    bottom: Option<Rect>,
    free: Rect,
    area: Rect,
    /// (side, slot index, rect)
    slots: Vec<(usize, usize, Rect)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(d: &Dock, p: Pane) -> usize {
        d.sides.iter().flatten().filter(|s| s.tabs.contains(&p)).count() + d.floats.iter().filter(|f| f.pane == p).count()
    }

    #[test]
    fn default_layout_has_every_default_window_once() {
        let d = default_layout();
        for p in Pane::DEFAULT {
            assert_eq!(count(&d, p), 1, "{p:?}");
        }
    }

    /// A closed window opens again once, in front, however many times it is asked for.
    #[test]
    fn closed_windows_open_again_once() {
        for p in Pane::DEFAULT {
            let mut d = default_layout();
            d.remove(p);
            assert_eq!(count(&d, p), 0, "{p:?} closed");
            d.show(p);
            d.show(p);
            assert_eq!(count(&d, p), 1, "{p:?}");
            assert!(d.shown(p), "{p:?} brought to front");
        }
    }

    #[test]
    fn layout_round_trips_and_repairs_duplicates() {
        let d = default_layout();
        let back = parse_layout(&layout_json(&d).unwrap()).expect("round trip");
        assert_eq!(back.sides, d.sides);
        // a file listing a window twice keeps the first
        let mut twice = default_layout();
        twice.sides[BOTTOM].push(Slot { tabs: vec![Pane::Layers], active: 0, weight: 1.0 });
        let fixed = parse_layout(&layout_json(&twice).unwrap()).unwrap();
        assert_eq!(count(&fixed, Pane::Layers), 1);
        assert_eq!(fixed.sides[BOTTOM].len(), d.sides[BOTTOM].len(), "the emptied slot is dropped");
        assert!(parse_layout("not json").is_none());
    }

    /// Every drop keeps each window exactly once: as a tab, splitting a slot, on a new side, floating.
    #[test]
    fn every_drop_keeps_each_window_once() {
        for moved in Pane::DEFAULT {
            let other = if moved == Pane::Layers { Pane::TextureSetList } else { Pane::Layers };
            for target in [
                Target::Tab(other),
                Target::Split(other, true),
                Target::Split(other, false),
                Target::Side(BOTTOM, false),
                Target::Side(LEFT, true),
                Target::Float(Rect::from_min_size(pos2(400.0, 200.0), vec2(300.0, 300.0))),
            ] {
                let mut d = default_layout();
                d.apply(moved, target);
                for p in Pane::DEFAULT {
                    assert_eq!(count(&d, p), 1, "{moved:?} -> {target:?}: {p:?}");
                }
                match target {
                    Target::Tab(o) => assert!(d.sides.iter().flatten().any(|s| s.tabs.contains(&moved) && s.tabs.contains(&o))),
                    Target::Float(_) => assert!(d.floats.iter().any(|f| f.pane == moved)),
                    Target::Side(side, _) => assert!(d.sides[side].iter().any(|s| s.tabs == vec![moved])),
                    _ => {}
                }
                assert!(d.sides.iter().flatten().all(|s| !s.tabs.is_empty()), "no empty slot left behind");
            }
        }
    }

    #[test]
    fn a_closed_window_comes_back() {
        let mut d = default_layout();
        d.remove(Pane::Assets);
        assert!(!d.contains(Pane::Assets));
        d.show(Pane::Assets);
        assert!(d.contains(Pane::Assets) && d.shown(Pane::Assets));
    }
}
