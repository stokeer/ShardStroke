//! Docked panels (every card with an X), arranged by the user:
//! - hold a header (or a tab) still for half a second, then drag it; moving early cancels, a bar fills as you hold;
//!   while it moves, ASCII characters rain from under it
//! - drop on another panel's header or middle = tabs; on its top / bottom half (left / right half at the bottom
//!   side) = split that slot in two; near the left / right edge or the bottom = new slot on that side; anywhere
//!   else = floating
//! - drag the gaps between slots, or the inner edge of a side, to resize; floating windows resize by the corner
//! - X closes, the Window menu brings it back; "lock layout" hides the X and stops all of it.

use super::*;
use std::sync::atomic::AtomicBool;

pub static LOCKED: AtomicBool = AtomicBool::new(false);

pub fn locked() -> bool {
    LOCKED.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Place {
    Left,
    Right,
    Bottom,
    Float,
}

pub type Draw = fn(&mut App, &mut Ui, &Painter, Rect);

/// (key, default side, share of its side, draw)
pub type Spec = (&'static str, Place, f32, Draw);

pub struct Slot {
    tabs: Vec<&'static str>,
    active: usize,
    weight: f32,
}

struct Floating {
    key: &'static str,
    rect: Rect,
}

/// One mode's arrangement: slots of the left, right and bottom sides, their sizes, floating windows.
pub struct Dock {
    sides: [Vec<Slot>; 3],
    size: [f32; 3],
    floats: Vec<Floating>,
}

pub struct Docks {
    modes: [Option<Dock>; 2],
    /// Header pressed: (panel, since, where).
    hold: Option<(&'static str, f64, Pos2)>,
    /// Panel being moved: (panel, grab offset, size).
    drag: Option<(&'static str, Vec2, Vec2)>,
    resizing: bool,
    hold_secs: f32,
    rain: Vec<RainChar>,
    seed: u32,
}

/// One falling character of the drag rain.
struct RainChar {
    pos: Pos2,
    vel: Vec2,
    ch: char,
    life: f32,
    /// Seconds it stays put at full strength before falling (the dissolve sweeps top to bottom).
    wait: f32,
}

const RAIN: [char; 10] = ['.', ':', '`', '|', '/', '+', '*', '0', '1', '#'];

impl Default for Docks {
    fn default() -> Self {
        Docks { modes: [None, None], hold: None, drag: None, resizing: false, hold_secs: 0.5, rain: Vec::new(), seed: 7 }
    }
}

pub struct Zones {
    pub left: Option<Rect>,
    pub right: Option<Rect>,
    pub bottom: Option<Rect>,
    /// What is left for the viewport / graph.
    pub free: Rect,
    area: Rect,
    /// (side, slot index, rect)
    slots: Vec<(usize, usize, Rect)>,
}

enum Target {
    Stay,
    Tab(&'static str),
    Split(&'static str, bool),
    Side(usize, bool),
    Float,
}

const GAP: f32 = 4.0;
const HEADER: f32 = 30.0;

fn side_of(p: Place) -> usize {
    match p {
        Place::Left => 0,
        Place::Right => 1,
        _ => 2,
    }
}

impl App {
    fn mode_ix(&self) -> usize {
        if self.mode == Mode::Texture { 1 } else { 0 }
    }

    fn dk(&mut self) -> &mut Dock {
        let m = self.mode_ix();
        self.dock.modes[m].as_mut().unwrap()
    }

    /// Works out the three sides and every slot's rect in `area`.
    pub(crate) fn dock_zones(&mut self, ui: &Ui, area: Rect, specs: &[Spec]) -> Zones {
        let m = self.mode_ix();
        if self.dock.modes[m].is_none() {
            let mut sides: [Vec<Slot>; 3] = Default::default();
            // a weight of 0 means "a tab of the slot before it on that side"
            for (key, place, weight, _) in specs {
                if *weight < 0.0 {
                    continue;
                }
                let side = &mut sides[side_of(*place)];
                match side.last_mut() {
                    Some(last) if *weight == 0.0 => last.tabs.push(key),
                    _ => side.push(Slot { tabs: vec![key], active: 0, weight: weight.max(0.5) }),
                }
            }
            self.dock.modes[m] = Some(Dock { sides, size: [340.0, 420.0, 230.0], floats: Vec::new() });
        }
        let t = if self.dock.resizing { 0.0 } else { 0.22 };
        let ctx = ui.ctx();
        let d = self.dock.modes[m].as_ref().unwrap();
        let sz = |i: usize| anim_value(&ctx, Id::new(("zone", m, i)), if d.sides[i].is_empty() { 0.0 } else { d.size[i] }, t);
        let (lw, rw, bh) = (sz(0), sz(1), sz(2));
        let left = (lw > 1.0).then(|| Rect::from_min_size(area.min, vec2(lw, area.height())));
        let right = (rw > 1.0).then(|| Rect::from_min_max(pos2(area.right() - rw, area.top()), area.max));
        let bl = left.map_or(area.left(), |z| z.right() + GAP);
        let br = right.map_or(area.right(), |z| z.left() - GAP);
        let bottom = (bh > 1.0).then(|| Rect::from_min_max(pos2(bl, area.bottom() - bh), pos2(br, area.bottom())));
        let free = Rect::from_min_max(pos2(bl, area.top()), pos2(br, bottom.map_or(area.bottom(), |z| z.top() - GAP)));
        let mut slots = Vec::new();
        for (side, zone) in [left, right, bottom].into_iter().enumerate() {
            let Some(z) = zone else { continue };
            let list = &d.sides[side];
            let vertical = side < 2;
            let total: f32 = list.iter().map(|s| s.weight).sum();
            let len = if vertical { z.height() } else { z.width() } - GAP * list.len().saturating_sub(1) as f32;
            let mut at = if vertical { z.top() } else { z.left() };
            for (i, s) in list.iter().enumerate() {
                let n = len * s.weight / total;
                let r = if vertical {
                    Rect::from_min_size(pos2(z.left(), at), vec2(z.width(), n))
                } else {
                    Rect::from_min_size(pos2(at, z.top()), vec2(n, z.height()))
                };
                slots.push((side, i, r));
                at += n + GAP;
            }
        }
        Zones { left, right, bottom, free, area, slots }
    }

    /// Draws the slots, the resize handles, the floating windows and the panel being moved.
    pub(crate) fn dock_draw(&mut self, ui: &mut Ui, p: &Painter, z: &Zones, specs: &[Spec]) {
        let ctx = ui.ctx().clone();
        let m = self.mode_ix();
        let draw_of = |key: &str| specs.iter().find(|s| s.0 == key).map(|s| s.3);
        let t = if self.dock.resizing { 0.0 } else { 0.22 };
        for &(side, idx, target) in &z.slots {
            let first = self.dk().sides[side][idx].tabs[0];
            let a = |n: &str, v: f32| anim_value(&ctx, Id::new((m, first, n)), v, t);
            let r = Rect::from_min_size(pos2(a("x", target.left()), a("y", target.top())), vec2(a("w", target.width()), a("h", target.height())));
            self.draw_slot(ui, p, r, side, idx, &draw_of);
        }
        if !locked() && self.dock.drag.is_none() {
            self.resize_handles(ui, p, z);
        }
        // floating windows
        let floats: Vec<(usize, &'static str, Rect)> = self.dk().floats.iter().enumerate().map(|(i, f)| (i, f.key, f.rect)).collect();
        for (i, key, r) in floats {
            let Some(draw) = draw_of(key) else { continue };
            // while it moves, only the copy under the pointer is drawn
            if self.dock.drag.is_some_and(|d| d.0 == key) {
                continue;
            }
            egui::Area::new(Id::new((m, key, "float"))).fixed_pos(r.min).order(egui::Order::Middle).show(&ctx, |ui| {
                ui.allocate_exact_size(r.size(), Sense::hover());
                let pp = ui.painter().clone();
                let mut fp = pp.clone();
                if self.dock.drag.is_some_and(|d| d.0 == key) {
                    fp.multiply_opacity(0.3);
                }
                glow(&fp, r, Color32::BLACK, 0.55, 20);
                draw(self, ui, &fp, r);
                let hdr = Rect::from_min_size(r.min, vec2(r.width() - 30.0, HEADER));
                self.handle(ui, &pp, hdr, key, r);
                self.close_x(ui, &pp, r, key);
                if !locked() {
                    // corner resize
                    let c = Rect::from_min_max(r.max - vec2(14.0, 14.0), r.max);
                    let cr = ui.interact(c, Id::new((key, "corner")), Sense::drag());
                    let h = an(ui, (key, "corner"), cr.hovered() || cr.dragged());
                    for k in 0..3 {
                        let o = 4.0 + k as f32 * 3.5;
                        pp.line_segment([c.max - vec2(o, 2.0), c.max - vec2(2.0, o)], Stroke::new(1.0, mix(FAINT, acc(), h)));
                    }
                    if cr.hovered() || cr.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                    }
                    if cr.dragged() {
                        let d = cr.drag_delta();
                        let f = &mut self.dk().floats[i];
                        f.rect.max = (f.rect.max + d).max(f.rect.min + vec2(220.0, 160.0));
                    }
                }
            });
        }
        self.drag_and_drop(ui, z, &draw_of);
        self.rain(ui);
    }

    fn rnd(&mut self) -> f32 {
        self.dock.seed = self.dock.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.dock.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Spawns characters along the bottom edge of the moving window.
    fn spawn_rain(&mut self, r: Rect, dt: f32) {
        if !heavy() {
            return;
        }
        let n = (r.width() * dt * 0.9).ceil() as usize;
        for _ in 0..n {
            let x = r.left() + self.rnd() * r.width();
            let ch = RAIN[(self.rnd() * RAIN.len() as f32) as usize % RAIN.len()];
            let vel = vec2((self.rnd() - 0.5) * 14.0, 30.0 + self.rnd() * 60.0);
            let life = 0.25 + self.rnd() * 0.3;
            self.dock.rain.push(RainChar { pos: pos2(x, r.bottom() + 2.0), vel, ch, life, wait: 0.0 });
        }
    }

    /// A closing floating window turns into a grid of characters that falls apart from the top down.
    fn dissolve(&mut self, r: Rect) {
        if !heavy() {
            return;
        }
        let mut y = r.top() + 6.0;
        while y < r.bottom() {
            let mut x = r.left() + 5.0;
            while x < r.right() {
                if self.rnd() < 0.7 {
                    let ch = RAIN[(self.rnd() * RAIN.len() as f32) as usize % RAIN.len()];
                    let vel = vec2((self.rnd() - 0.5) * 40.0, -10.0 - self.rnd() * 30.0);
                    let wait = (y - r.top()) / r.height() * 0.3 + self.rnd() * 0.06;
                    let life = 0.3 + self.rnd() * 0.3;
                    self.dock.rain.push(RainChar { pos: pos2(x, y), vel, ch, life, wait });
                }
                x += 9.0;
            }
            y += 12.0;
        }
    }

    /// Moves and draws the rain; it keeps falling and fading after the drop.
    fn rain(&mut self, ui: &Ui) {
        if self.dock.rain.is_empty() {
            return;
        }
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("drag_rain")));
        for d in self.dock.rain.iter_mut() {
            if d.wait > 0.0 {
                d.wait -= dt;
                txt(&lp, d.pos, Align2::CENTER_CENTER, &d.ch.to_string(), F::Mono, 11.0, acc(), 0.0);
                continue;
            }
            d.vel.y += 260.0 * dt;
            d.pos += d.vel * dt;
            d.life -= dt;
            // accent only, fading out along the short fall
            let a = smooth(d.life / 0.55);
            txt(&lp, d.pos, Align2::CENTER_CENTER, &d.ch.to_string(), F::Mono, 11.0, alpha(acc(), a), 0.0);
        }
        self.dock.rain.retain(|d| d.life > 0.0);
    }

    fn draw_slot(&mut self, ui: &mut Ui, p: &Painter, r: Rect, side: usize, idx: usize, draw_of: &dyn Fn(&str) -> Option<Draw>) {
        let (tabs, active) = {
            let s = &self.dk().sides[side][idx];
            (s.tabs.clone(), s.active.min(s.tabs.len() - 1))
        };
        let key = tabs[active];
        let Some(draw) = draw_of(key) else { return };
        let mut sp = p.clone();
        if self.dock.drag.is_some_and(|d| d.0 == key) {
            sp.multiply_opacity(0.3);
        }
        draw(self, ui, &sp, r);
        if tabs.len() == 1 {
            let hdr = Rect::from_min_size(r.min, vec2(r.width() - 30.0, HEADER));
            self.handle(ui, &sp, hdr, key, r);
        } else {
            // tab strip over the card's title
            let hdr = Rect::from_min_size(r.min, vec2(r.width() - 30.0, HEADER));
            sp.rect_filled(hdr.shrink(1.0), 0.0, CARD);
            // tabs share the header; each keeps its natural width when there is room
            let natural: f32 = tabs.iter().map(|k| k.len() as f32 * 10.0 + 24.0).sum();
            let squeeze = (hdr.width() / natural).min(1.0);
            let mut x = hdr.left() + 1.0;
            for (i, k) in tabs.iter().enumerate() {
                let w = (k.len() as f32 * 10.0 + 24.0) * squeeze;
                let tr = Rect::from_min_size(pos2(x, hdr.top() + 1.0), vec2(w, HEADER - 2.0));
                x += w;
                let resp = self.handle(ui, &sp, tr, k, r);
                if resp.clicked() {
                    self.dk().sides[side][idx].active = i;
                }
                let on = an(ui, ("tab_on", *k), i == active);
                let h = an(ui, ("tab_h", *k), resp.hovered());
                sp.rect_filled(tr, 0.0, mix(alpha(HOVER, h), PANEL, on));
                sp.rect_filled(Rect::from_min_max(pos2(tr.left(), tr.bottom() - 2.0), tr.max), 0.0, alpha(acc(), on));
                if i > 0 {
                    sp.vline(tr.left(), tr.y_range(), Stroke::new(1.0, BORDER));
                }
                let tp = sp.with_clip_rect(tr.intersect(sp.clip_rect()));
                lbl(&tp, tr.center(), Align2::CENTER_CENTER, k, if squeeze < 0.8 { 6.5 } else { 8.0 }, mix(mix(DIM, SOFT, h), TEXT, on));
            }
        }
        self.close_x(ui, &sp, r, key);
    }

    /// Header / tab: click passes through (returned); held still for the hold time, the panel comes loose.
    fn handle(&mut self, ui: &mut Ui, p: &Painter, rect: Rect, key: &'static str, panel: Rect) -> egui::Response {
        let resp = ui.interact(rect, Id::new((key, "hdr")), Sense::click_and_drag());
        if locked() {
            return resp;
        }
        let now = ui.input(|i| i.time);
        let q = ui.input(|i| i.pointer.interact_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        // a floating window moves straight away, no hold
        let floating = self.dk().floats.iter().any(|f| f.key == key);
        if floating && resp.drag_started() && self.dock.drag.is_none() {
            if let Some(q) = q {
                self.dock.drag = Some((key, q - panel.min, panel.size()));
                self.dock.hold = None;
            }
            return resp;
        }
        if !floating && resp.is_pointer_button_down_on() && self.dock.hold.is_none() && self.dock.drag.is_none() {
            if let Some(q) = q {
                self.dock.hold = Some((key, now, q));
            }
        }
        if let Some((k, t0, q0)) = self.dock.hold {
            if k == key {
                let moved = q.is_some_and(|q| (q - q0).length() > 6.0);
                if !down || moved {
                    self.dock.hold = None;
                } else {
                    let f = ((now - t0) as f32 / self.dock.hold_secs).clamp(0.0, 1.0);
                    let bar = Rect::from_min_size(pos2(rect.left(), rect.bottom() - 2.0), vec2(rect.width() * f, 2.0));
                    p.rect_filled(bar, 0.0, acc());
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Wait);
                    if f >= 1.0 {
                        let size = vec2(panel.width().clamp(240.0, 440.0), panel.height().clamp(200.0, 520.0));
                        let grab = (q0 - panel.min).min(size - vec2(40.0, 10.0)).max(vec2(10.0, 10.0));
                        self.dock.drag = Some((key, grab, size));
                        self.dock.hold = None;
                    }
                }
            }
        }
        if resp.hovered() && self.dock.hold.is_none() && self.dock.drag.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        resp
    }

    fn close_x(&mut self, ui: &mut Ui, p: &Painter, r: Rect, key: &'static str) {
        if locked() {
            return;
        }
        let x = Rect::from_center_size(pos2(r.right() - 15.0, r.top() + 15.0), vec2(18.0, 18.0));
        let xr = ui.interact(x, Id::new((key, "x")), Sense::click());
        let xh = an(ui, (key, "x"), xr.hovered());
        if xh > 0.0 {
            let xb = x.shrink(2.0);
            p.rect_filled(xb, 0.0, alpha(TEXT, xh));
            let s = Stroke::new(1.0, alpha(BG, xh));
            p.line_segment([xb.left_top() + vec2(4.0, 4.0), xb.right_bottom() - vec2(4.0, 4.0)], s);
            p.line_segment([xb.right_top() + vec2(-4.0, 4.0), xb.left_bottom() + vec2(4.0, -4.0)], s);
        }
        if xr.clicked() {
            if self.dk().floats.iter().any(|f| f.key == key) {
                self.dissolve(r);
            }
            self.take(key);
        }
    }

    /// Gaps between slots and the inner edge of each side.
    fn resize_handles(&mut self, ui: &mut Ui, p: &Painter, z: &Zones) {
        let mut any = false;
        // between neighbours of one side
        for w in z.slots.windows(2) {
            let ((sa, ia, ra), (sb, _, rb)) = (w[0], w[1]);
            if sa != sb {
                continue;
            }
            let vertical = sa < 2;
            let band = if vertical {
                Rect::from_min_max(pos2(ra.left(), ra.bottom() - 2.0), pos2(ra.right(), rb.top() + 2.0))
            } else {
                Rect::from_min_max(pos2(ra.right() - 2.0, ra.top()), pos2(rb.left() + 2.0, ra.bottom()))
            };
            let resp = ui.interact(band, Id::new(("split", sa, ia)), Sense::drag());
            let h = an(ui, ("split", sa, ia), resp.hovered() || resp.dragged());
            p.rect_filled(band.shrink2(if vertical { vec2(0.0, 1.0) } else { vec2(1.0, 0.0) }), 0.0, alpha(acc(), h));
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
            }
            if resp.dragged() {
                any = true;
                let px = if vertical { resp.drag_delta().y } else { resp.drag_delta().x };
                let len: f32 = z.slots.iter().filter(|s| s.0 == sa).map(|s| if vertical { s.2.height() } else { s.2.width() }).sum();
                let list = &mut self.dk().sides[sa];
                let total: f32 = list.iter().map(|s| s.weight).sum();
                let dw = px * total / len.max(1.0);
                let min = total * 0.12;
                if list[ia].weight + dw >= min && list[ia + 1].weight - dw >= min {
                    list[ia].weight += dw;
                    list[ia + 1].weight -= dw;
                }
            }
        }
        // the inner edge of each side
        let edges = [
            (0, z.left.map(|r| Rect::from_min_max(pos2(r.right() - 1.0, r.top()), pos2(r.right() + GAP + 1.0, r.bottom())))),
            (1, z.right.map(|r| Rect::from_min_max(pos2(r.left() - GAP - 1.0, r.top()), pos2(r.left() + 1.0, r.bottom())))),
            (2, z.bottom.map(|r| Rect::from_min_max(pos2(r.left(), r.top() - GAP - 1.0), pos2(r.right(), r.top() + 1.0)))),
        ];
        for (side, band) in edges {
            let Some(band) = band else { continue };
            let resp = ui.interact(band, Id::new(("edge", side)), Sense::drag());
            let h = an(ui, ("edge", side), resp.hovered() || resp.dragged());
            p.rect_filled(band, 0.0, alpha(acc(), h));
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(if side == 2 { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
            }
            if resp.dragged() {
                any = true;
                let d = resp.drag_delta();
                let s = &mut self.dk().size[side];
                *s = match side {
                    0 => *s + d.x,
                    1 => *s - d.x,
                    _ => *s - d.y,
                }
                .clamp(if side == 2 { 120.0 } else { 220.0 }, if side == 2 { 600.0 } else { 720.0 });
            }
        }
        self.dock.resizing = any;
    }

    fn drag_and_drop(&mut self, ui: &mut Ui, z: &Zones, draw_of: &dyn Fn(&str) -> Option<Draw>) {
        let Some((key, grab, size)) = self.dock.drag else { return };
        let Some(q) = ui.input(|i| i.pointer.hover_pos()) else { return };
        let target = self.drop_target(z, q, key);
        // where it lands
        let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("dock_preview")));
        let (zone, label) = self.target_rect(z, &target, q);
        if let Some(zr) = zone {
            lp.rect_filled(zr, 0.0, alpha(acc(), 0.14));
            lp.rect_stroke(zr, 0.0, Stroke::new(2.0, acc()), StrokeKind::Inside);
            lbl(&lp, zr.center(), Align2::CENTER_CENTER, label, 9.0, Color32::WHITE);
        }
        // the panel follows the pointer, raining ASCII from under it
        let r = Rect::from_min_size(q - grab, size);
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        self.spawn_rain(r, dt);
        if let Some(draw) = draw_of(key) {
            egui::Area::new(Id::new("dragged_panel")).fixed_pos(r.min).order(egui::Order::Foreground).interactable(false).show(ui.ctx(), |ui| {
                ui.multiply_opacity(0.85);
                let pp = ui.painter().clone();
                glow(&pp, r, Color32::BLACK, 0.6, 24);
                draw(self, ui, &pp, r);
                pp.rect_stroke(r, 0.0, Stroke::new(1.0, acc()), StrokeKind::Inside);
            });
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if ui.input(|i| i.pointer.primary_down()) {
            return;
        }
        self.dock.drag = None;
        if matches!(target, Target::Stay) {
            return;
        }
        self.take(key);
        let d = self.dk();
        match target {
            Target::Tab(other) => {
                if let Some(s) = d.sides.iter_mut().flatten().find(|s| s.tabs.contains(&other)) {
                    s.tabs.push(key);
                    s.active = s.tabs.len() - 1;
                }
            }
            Target::Split(other, before) => {
                for side in d.sides.iter_mut() {
                    if let Some(i) = side.iter().position(|s| s.tabs.contains(&other)) {
                        side[i].weight /= 2.0;
                        let w = side[i].weight;
                        side.insert(if before { i } else { i + 1 }, Slot { tabs: vec![key], active: 0, weight: w });
                        break;
                    }
                }
            }
            Target::Side(side, first) => {
                let w = d.sides[side].iter().map(|s| s.weight).sum::<f32>().max(1.0) / d.sides[side].len().max(1) as f32;
                let slot = Slot { tabs: vec![key], active: 0, weight: w };
                if first {
                    d.sides[side].insert(0, slot);
                } else {
                    d.sides[side].push(slot);
                }
            }
            Target::Float => d.floats.push(Floating { key, rect: r }),
            Target::Stay => {}
        }
    }

    fn drop_target(&mut self, z: &Zones, q: Pos2, key: &'static str) -> Target {
        for &(side, idx, r) in &z.slots {
            if !r.contains(q) {
                continue;
            }
            let s = &self.dk().sides[side][idx];
            let Some(other) = s.tabs.iter().copied().find(|t| *t != key) else { return Target::Stay };
            let header = q.y < r.top() + HEADER;
            let middle = r.shrink2(r.size() * 0.3).contains(q);
            if header || middle {
                return Target::Tab(other);
            }
            let before = if side < 2 { q.y < r.center().y } else { q.x < r.center().x };
            return Target::Split(other, before);
        }
        let a = z.area;
        if q.x < a.left() + 120.0 {
            Target::Side(0, q.y < a.center().y)
        } else if q.x > a.right() - 120.0 {
            Target::Side(1, q.y < a.center().y)
        } else if q.y > a.bottom() - 110.0 {
            Target::Side(2, q.x < a.center().x)
        } else {
            Target::Float
        }
    }

    fn target_rect(&mut self, z: &Zones, t: &Target, q: Pos2) -> (Option<Rect>, &'static str) {
        let slot_of = |app: &mut App, k: &str| {
            z.slots.iter().find(|(s, i, _)| app.dk().sides[*s][*i].tabs.contains(&k)).map(|(s, _, r)| (*s, *r))
        };
        match t {
            Target::Tab(k) => (slot_of(self, k).map(|(_, r)| r), "tab"),
            Target::Split(k, before) => {
                let Some((side, r)) = slot_of(self, k) else { return (None, "") };
                let h = if side < 2 {
                    if *before { Rect::from_min_max(r.min, pos2(r.right(), r.center().y)) } else { Rect::from_min_max(pos2(r.left(), r.center().y), r.max) }
                } else if *before {
                    Rect::from_min_max(r.min, pos2(r.center().x, r.bottom()))
                } else {
                    Rect::from_min_max(pos2(r.center().x, r.top()), r.max)
                };
                (Some(h), "split")
            }
            Target::Side(side, _) => {
                let a = z.area;
                let r = match side {
                    0 => z.left.unwrap_or(Rect::from_min_size(a.min, vec2(300.0, a.height()))),
                    1 => z.right.unwrap_or(Rect::from_min_max(pos2(a.right() - 300.0, a.top()), a.max)),
                    _ => z.bottom.unwrap_or(Rect::from_min_max(pos2(a.left(), a.bottom() - 200.0), a.max)),
                };
                (Some(r), ["dock left", "dock right", "dock bottom"][*side])
            }
            Target::Float => (Some(Rect::from_min_size(q - vec2(4.0, 4.0), vec2(8.0, 8.0))), ""),
            Target::Stay => (None, ""),
        }
    }

    /// Takes a panel out of wherever it is (slot or floating); empty slots go away.
    fn take(&mut self, key: &'static str) {
        let d = self.dk();
        for side in d.sides.iter_mut() {
            for s in side.iter_mut() {
                if let Some(i) = s.tabs.iter().position(|t| *t == key) {
                    s.tabs.remove(i);
                    s.active = s.active.min(s.tabs.len().saturating_sub(1));
                }
            }
            side.retain(|s| !s.tabs.is_empty());
        }
        d.floats.retain(|f| f.key != key);
    }

    pub(crate) fn panel_open(&self, key: &str) -> bool {
        let m = self.mode_ix();
        self.dock.modes[m].as_ref().is_none_or(|d| d.sides.iter().flatten().any(|s| s.tabs.contains(&key)) || d.floats.iter().any(|f| f.key == key))
    }

    /// Window menu: closes it, or brings it back on its default side.
    pub(crate) fn toggle_panel(&mut self, spec: &Spec) {
        if self.dock.modes[self.mode_ix()].is_none() {
            return;
        }
        if self.panel_open(spec.0) {
            self.take(spec.0);
        } else {
            self.dk().sides[side_of(spec.1)].push(Slot { tabs: vec![spec.0], active: 0, weight: spec.2.abs().max(0.5) });
        }
    }

    /// Showing = the active tab of its slot, or floating.
    pub(crate) fn panel_shown(&self, key: &str) -> bool {
        let m = self.mode_ix();
        self.dock.modes[m].as_ref().is_some_and(|d| {
            d.sides.iter().flatten().any(|s| s.tabs.get(s.active.min(s.tabs.len() - 1)) == Some(&key)) || d.floats.iter().any(|f| f.key == key)
        })
    }

    /// Window button: a hidden tab comes to the front; otherwise the window opens or closes.
    pub(crate) fn show_or_toggle(&mut self, spec: &Spec) {
        if self.panel_open(spec.0) && !self.panel_shown(spec.0) {
            for s in self.dk().sides.iter_mut().flatten() {
                if let Some(i) = s.tabs.iter().position(|t| *t == spec.0) {
                    s.active = i;
                }
            }
        } else {
            self.toggle_panel(spec);
        }
    }

    pub(crate) fn reset_layout(&mut self) {
        let m = self.mode_ix();
        self.dock.modes[m] = None;
    }
}
