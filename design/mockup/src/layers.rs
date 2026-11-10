//! Layers panel as on design board 11: icon toolbar + channel picker, groups and layers that fold open,
//! colour + mask thumbnails, mask effects as small rows, and the right-click context menu.

use super::*;

pub struct Layer {
    pub name: &'static str,
    pub blend: &'static str,
    pub op: Option<f32>,
    pub color: Color32,
    /// Mask thumbnail colour, if the layer has a mask.
    pub mask: Option<Color32>,
    pub kind: Kind,
    pub depth: u8,
    pub visible: bool,
    pub parent: Option<usize>,
    pub open: bool,
    /// Effect rows (inside a mask) carry a small tag such as GEN or FILTER.
    pub tag: Option<&'static str>,
    pub deleted: bool,
}

impl Layer {
    fn effect(&self) -> bool {
        self.tag.is_some()
    }
}

pub fn stack() -> Vec<Layer> {
    let c = Color32::from_rgb;
    let l = |name, blend, op, color, mask, kind, depth, parent, tag| Layer {
        name, blend, op, color, mask, kind, depth, visible: true, parent, open: true, tag, deleted: false,
    };
    let mut v = vec![
        l("Weathering", "PTHR", Some(100.0), c(0x2A, 0x2A, 0x2A), None, Kind::Group, 0, None, None),
        l("Rust grunge", "MUL", Some(72.0), c(0x5E, 0x6B, 0x36), Some(c(0x22, 0x22, 0x22)), Kind::Fill, 1, Some(0), None),
        l("Metal edge wear", "NRM", None, c(0x22, 0x22, 0x22), None, Kind::Generator, 2, Some(1), Some("gen")),
        l("Blur slope", "NRM", None, c(0x3C, 0x3C, 0x3C), None, Kind::Filter, 2, Some(1), Some("filter")),
        l("Paint", "ADD", None, c(0xB4, 0xB4, 0xB4), None, Kind::Paint, 2, Some(1), Some("strokes · replayed")),
        l("Edge wear", "NRM", Some(100.0), c(0x1E, 0x1E, 0x1E), Some(c(0xE2, 0xE2, 0xE2)), Kind::Fill, 1, Some(0), None),
        l("Dust", "SCR", Some(30.0), c(0x26, 0x26, 0x26), None, Kind::Fill, 0, None, None),
        l("Base paint", "NRM", Some(100.0), c(0x3E, 0x4A, 0x3C), None, Kind::Paint, 0, None, None),
    ];
    v[6].visible = false;
    v
}

const CHANNELS: [&str; 5] = ["base color", "roughness", "metallic", "normal", "height"];

/// Context menu rows: (label, key, kind) with kind 0 = normal, 1 = missing (disabled), 2 = danger; "" = separator.
const MENU: [(&str, &str, u8); 15] = [
    ("duplicate", "ctrl+d", 0),
    ("group", "ctrl+g", 0),
    ("ungroup", "", 0),
    ("rename", "", 0),
    ("", "", 0),
    ("disable mask", "", 0),
    ("remove mask", "", 0),
    ("invert mask", "missing", 1),
    ("clear mask", "missing", 1),
    ("add anchor point", "missing", 1),
    ("", "", 0),
    ("copy", "ctrl+c", 0),
    ("cut", "ctrl+x", 0),
    ("paste", "ctrl+v", 0),
    ("delete", "del", 2),
];

fn mask_icon(p: &Painter, c: Pos2, col: Color32) {
    let r = Rect::from_center_size(c, vec2(11.0, 11.0));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, col), StrokeKind::Inside);
    p.rect_filled(r.shrink(3.0), 0.0, col);
}

fn toolbar_icon(p: &Painter, i: usize, c: Pos2, col: Color32) {
    use icons::Icon::*;
    // add effect / mask / paint layer / fill layer / smart material / group, delete
    let icon = [Wand, Mask, Brush, Bucket, Sphere, Folder, Trash][i.min(6)];
    icons::draw(p, Rect::from_center_size(c, vec2(16.0, 16.0)), icon, col);
}

impl App {
    /// Draws the panel in `r`; returns nothing, interaction updates the stack and selection.
    pub(crate) fn layers_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "layers", None, "");
        let now = ui.input(|i| i.time);

        // toolbar: add effect / mask / paint / fill / smart material / group, delete; channel picker
        let tb = Rect::from_min_size(inner.min, vec2(inner.width(), 40.0));
        p.rect_filled(tb, 0.0, CARD);
        for i in 0..7 {
            let b = Rect::from_center_size(pos2(tb.left() + 18.0 + i as f32 * 32.0, tb.center().y), vec2(26.0, 26.0));
            let resp = ui.interact(b, Id::new(("ltb", i)), Sense::click());
            let h = an(ui, ("ltb", i), resp.hovered());
            p.rect_filled(b, 0.0, alpha(TEXT, h));
            toolbar_icon(p, i, b.center(), mix(DIM, BG, h));
            if resp.clicked() && i == 6 {
                self.delete_layer(self.sel_layer, now);
            }
        }
        let dd = Rect::from_min_size(pos2(tb.right() - 150.0, tb.top() + 7.0), vec2(142.0, 26.0));
        let resp = ui.interact(dd, Id::new("channel"), Sense::click());
        let open = matches!(self.drop, Some(Drop { kind: DropKind::Channel, .. }));
        let h = an(ui, "channel", resp.hovered() || open);
        frame(p, dd, mix(BG, TEXT, h), mix(BORDER2, TEXT, h));
        lbl(p, pos2(dd.left() + 10.0, dd.center().y), Align2::LEFT_CENTER, CHANNELS[self.channel], 8.0, mix(TEXT, BG, h));
        caret(p, pos2(dd.right() - 12.0, dd.center().y), mix(TEXT, BG, h), an(ui, "channel_open", open));
        if resp.clicked() {
            self.drop = if open { None } else { Some(Drop { kind: DropKind::Channel, anchor: dd, k: 0.0 }) };
        }

        // rows; children fold with their parent
        let list = Rect::from_min_max(pos2(inner.left(), tb.bottom() + 12.0), inner.max);
        let lp = p.with_clip_rect(list.intersect(p.clip_rect()));
        let mut y = list.top();
        for i in 0..self.layers.len() {
            if self.layers[i].deleted {
                continue;
            }
            // shown fraction: product of every ancestor's open animation
            let mut f = 1.0;
            let mut a = self.layers[i].parent;
            while let Some(pi) = a {
                f *= an(ui, ("lopen", pi), self.layers[pi].open);
                a = self.layers[pi].parent;
            }
            let effect = self.layers[i].effect();
            let full_h = if effect { 34.0 } else { 56.0 };
            let rh = full_h * f;
            if rh < 0.5 {
                continue;
            }
            let row = Rect::from_min_size(pos2(list.left(), y), vec2(list.width(), rh));
            y += rh;
            let mut rp = lp.with_clip_rect(row.intersect(lp.clip_rect()));
            rp.multiply_opacity(f);
            let live = f > 0.9;
            let content = Rect::from_min_size(row.min, vec2(row.width(), full_h));
            let cy = content.center().y;
            let indent = self.layers[i].depth as f32 * 20.0;
            let eye = Rect::from_center_size(pos2(content.left() + 18.0 + indent * 0.5, cy), vec2(22.0, 22.0));
            let has_kids = self.layers.iter().any(|c| c.parent == Some(i) && !c.deleted);
            let arrow = Rect::from_center_size(pos2(eye.right() + 14.0, cy), vec2(16.0, 16.0));
            let x0 = arrow.right() + 8.0;
            let th = if effect {
                Rect::from_center_size(pos2(x0 + 22.0, cy), vec2(18.0, 18.0))
            } else {
                Rect::from_center_size(pos2(x0 + 21.0, cy), vec2(42.0, 42.0))
            };
            let mt = self.layers[i].mask.map(|_| Rect::from_min_size(pos2(th.right() + 8.0, th.top()), vec2(42.0, 42.0)));
            // right side: blend mode (drop list) over opacity (drag sideways)
            let rx = content.right() - 8.0;
            let blend_r = if effect {
                Rect::from_min_max(pos2(rx - 52.0, cy - 9.0), pos2(rx, cy + 9.0))
            } else {
                Rect::from_min_max(pos2(rx - 52.0, cy - 17.0), pos2(rx, cy - 1.0))
            };
            let op_r = Rect::from_min_max(pos2(rx - 52.0, cy + 1.0), pos2(rx, cy + 17.0));

            let resp = live.then(|| ui.interact(row, Id::new(("layer", i)), Sense::click()));
            let eye_resp = live.then(|| ui.interact(eye, Id::new(("eye", i)), Sense::click()));
            let arrow_resp = (live && has_kids).then(|| ui.interact(arrow, Id::new(("arrow", i)), Sense::click()));
            let mask_resp = mt.filter(|_| live).map(|m| ui.interact(m, Id::new(("mask", i)), Sense::click()));
            let blend_resp = live.then(|| ui.interact(blend_r, Id::new(("blend", i)), Sense::click()));
            let op_resp = (live && self.layers[i].op.is_some()).then(|| ui.interact(op_r, Id::new(("op", i)), Sense::click_and_drag()));
            let hov = |r: &Option<egui::Response>| r.as_ref().is_some_and(|r| r.hovered());
            let clicked = |r: &Option<egui::Response>| r.as_ref().is_some_and(|r| r.clicked());
            if clicked(&eye_resp) {
                self.layers[i].visible = !self.layers[i].visible;
            }
            if clicked(&arrow_resp) {
                self.layers[i].open = !self.layers[i].open;
            }
            if let Some(o) = &op_resp {
                if o.dragged() {
                    let v = self.layers[i].op.unwrap_or(100.0) + o.drag_delta().x * 0.5;
                    self.layers[i].op = Some(v.clamp(0.0, 100.0));
                }
                if o.hovered() || o.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
            }
            if clicked(&blend_resp) {
                let open = matches!(self.drop, Some(Drop { kind: DropKind::Blend(j), .. }) if j == i);
                self.drop = if open { None } else { Some(Drop { kind: DropKind::Blend(i), anchor: blend_r, k: 0.0 }) };
            }
            if clicked(&mask_resp) && !(self.sel_layer == i && self.sel_mask) {
                self.sel_layer = i;
                self.sel_mask = true;
                self.sel_t = now;
            }
            if let Some(resp) = &resp {
                let on_ctrl = hov(&eye_resp) || hov(&arrow_resp) || hov(&mask_resp) || hov(&blend_resp) || hov(&op_resp);
                if (resp.clicked() || resp.secondary_clicked()) && !on_ctrl && (self.sel_layer != i || self.sel_mask) {
                    self.sel_layer = i;
                    self.sel_mask = false;
                    self.sel_t = now;
                }
                if resp.secondary_clicked() {
                    self.ctx_menu = resp.interact_pointer_pos().map(|q| (q, i));
                    self.ctx_k = 0.0;
                }
            }

            let l = &self.layers[i];
            let s = an(ui, ("lsel", i), self.sel_layer == i);
            let ms = an(ui, ("msel", i), self.sel_layer == i && self.sel_mask);
            let h = an(ui, ("lhov", i), hov(&resp));
            let vis = an(ui, ("lvis", i), l.visible);
            rp.rect_filled(content, 0.0, mix(alpha(HOVER, h * 0.7), Color32::from_gray(0x24), s));
            // visibility
            let eh = an(ui, ("eyeh", i), hov(&eye_resp));
            rp.circle_filled(eye.center(), 9.0, alpha(TEXT, 0.08 * eh));
            rp.circle_stroke(eye.center(), 6.0, Stroke::new(1.3, mix(mix(FAINT, DIM, vis), TEXT, eh)));
            rp.circle_filled(eye.center(), 3.0 * vis, mix(DIM, TEXT, eh.max(s)));
            // fold arrow, turns as it opens
            if has_kids {
                let o = an(ui, ("lopen", i), l.open);
                let ah = an(ui, ("arrowh", i), hov(&arrow_resp));
                let ang = (1.0 - o) * -std::f32::consts::FRAC_PI_2;
                let rot = |v: Vec2| arrow.center() + vec2(v.x * ang.cos() - v.y * ang.sin(), v.x * ang.sin() + v.y * ang.cos());
                rp.add(Shape::convex_polygon(vec![rot(vec2(-4.0, -2.0)), rot(vec2(4.0, -2.0)), rot(vec2(0.0, 3.0))], mix(FAINT, TEXT, ah), Stroke::NONE));
            }
            let tc = mix(Color32::from_gray(0x4A), mix(SOFT, Color32::WHITE, s), vis);
            // the picked thumbnail (layer or its mask) gets the white outline
            let sel_line = |p: &Painter, r: Rect, t: f32| {
                if t > 0.01 {
                    p.rect_stroke(r.expand(1.0), 0.0, Stroke::new(2.0, alpha(Color32::WHITE, t)), StrokeKind::Inside);
                }
            };
            if effect {
                frame(&rp, th, l.color, BORDER2);
                sel_line(&rp, th, s);
                let nr = lbl(&rp, pos2(th.right() + 12.0, cy), Align2::LEFT_CENTER, l.name, 8.0, tc);
                lbl(&rp, pos2(nr.right() + 6.0, cy), Align2::LEFT_CENTER, l.tag.unwrap_or(""), 6.0, FAINT);
            } else {
                if l.kind == Kind::Group {
                    frame(&rp, th, CARD, BORDER2);
                    mask_icon(&rp, th.center(), DIM);
                } else {
                    frame(&rp, th, l.color, BORDER2);
                }
                sel_line(&rp, th, s * (1.0 - ms));
                let mut nx = th.right() + 14.0;
                if let (Some(m), Some(mr)) = (l.mask, mt) {
                    let mh = an(ui, ("maskh", i), hov(&mask_resp));
                    frame(&rp, mr, m, mix(BORDER2, DIM, mh));
                    mask_icon(&rp, mr.center(), if m.r() > 0x80 { BG } else { DIM });
                    sel_line(&rp, mr, ms);
                    nx = mr.right() + 12.0;
                }
                lbl(&rp, pos2(nx, cy), Align2::LEFT_CENTER, l.name, 9.0, tc);
            }
            // blend: a drop list
            let bh = an(ui, ("blendh", i), hov(&blend_resp));
            rp.rect_filled(blend_r, 0.0, alpha(HOVER, bh));
            txt(&rp, pos2(blend_r.right() - 4.0, blend_r.center().y), Align2::RIGHT_CENTER, l.blend, F::Mono, 10.0, mix(FAINT, TEXT, bh), 0.0);
            if bh > 0.01 {
                caret(&rp, pos2(blend_r.left() + 8.0, blend_r.center().y), alpha(TEXT, bh), 0.0);
            }
            // opacity: drag sideways; a thin bar shows the value while you touch it
            if let Some(op) = l.op {
                let oh = an(ui, ("oph", i), hov(&op_resp) || op_resp.as_ref().is_some_and(|o| o.dragged()));
                rp.rect_filled(op_r, 0.0, alpha(HOVER, oh));
                let bar = Rect::from_min_size(pos2(op_r.left(), op_r.bottom() - 2.0), vec2(op_r.width() * op / 100.0, 2.0));
                rp.rect_filled(bar, 0.0, alpha(acc(), oh));
                txt(&rp, pos2(op_r.right() - 4.0, op_r.center().y), Align2::RIGHT_CENTER, &format!("{op:.0}"), F::Mono, 10.0, mix(FAINT, TEXT, oh), 0.0);
                if oh > 0.01 {
                    txt(&rp, pos2(op_r.left() + 3.0, op_r.center().y), Align2::LEFT_CENTER, "<>", F::Mono, 10.0, alpha(DIM, oh), 0.0);
                }
            }
        }
        self.context_menu(ui);
        self.drop_list(ui);
    }

    fn delete_layer(&mut self, i: usize, now: f64) {
        if self.layers.iter().filter(|l| !l.deleted && l.parent.is_none()).count() <= 1 && self.layers[i].parent.is_none() {
            return;
        }
        let mut gone = vec![i];
        let mut k = 0;
        while k < gone.len() {
            let g = gone[k];
            gone.extend(self.layers.iter().enumerate().filter(|(_, l)| l.parent == Some(g)).map(|(j, _)| j));
            k += 1;
        }
        for g in gone {
            self.layers[g].deleted = true;
        }
        if let Some(n) = (0..self.layers.len()).find(|&j| !self.layers[j].deleted) {
            self.sel_layer = n;
            self.sel_t = now;
        }
    }

    fn context_menu(&mut self, ui: &mut Ui) {
        let Some((at, target)) = self.ctx_menu else { return };
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        approach(&mut self.ctx_k, true, dt, 0.12);
        let k = smooth(self.ctx_k);
        let w = 300.0;
        let rows: f32 = MENU.iter().map(|m| if m.0.is_empty() { 10.0 } else { 34.0 }).sum();
        let screen = ui.ctx().screen_rect();
        let mut r = Rect::from_min_size(at, vec2(w, rows + 8.0));
        r = r.translate(vec2((screen.right() - 8.0 - r.right()).min(0.0), (screen.bottom() - 8.0 - r.bottom()).min(0.0)));
        let r = r.translate(vec2(0.0, -6.0 * (1.0 - k)));
        let mut picked = None;
        egui::Area::new(Id::new("layer_ctx")).fixed_pos(r.min).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            ui.multiply_opacity(k);
            ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 16);
            win_frame(&p, r, CARD, BORDER2);
            let mut y = r.top() + 4.0;
            for (n, (s, key, kind)) in MENU.iter().enumerate() {
                if s.is_empty() {
                    p.hline(r.x_range(), y + 5.0, Stroke::new(1.0, BORDER2));
                    y += 10.0;
                    continue;
                }
                let ir = Rect::from_min_size(pos2(r.left() + 2.0, y), vec2(w - 4.0, 34.0));
                y += 34.0;
                if *kind == 1 {
                    // not built yet: disabled, in its own box
                    p.rect_stroke(ir.shrink2(vec2(0.0, 1.0)), 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Inside);
                    lbl(&p, pos2(ir.left() + 12.0, ir.center().y), Align2::LEFT_CENTER, s, 9.0, Color32::from_gray(0x55));
                    lbl(&p, pos2(ir.right() - 12.0, ir.center().y), Align2::RIGHT_CENTER, key, 7.0, Color32::from_gray(0x55));
                    continue;
                }
                let resp = ui.interact(ir, Id::new(("lctx", n)), Sense::click());
                let h = an(ui, ("lctx", n), resp.hovered());
                if *kind == 2 {
                    p.rect_filled(ir, 0.0, alpha(ERR, 0.12 * h));
                    p.rect_stroke(ir, 0.0, Stroke::new(1.0 + h, alpha(ERR, 0.35 + 0.65 * h)), StrokeKind::Inside);
                    lbl(&p, pos2(ir.left() + 12.0 + 3.0 * h, ir.center().y), Align2::LEFT_CENTER, s, 9.0, ERR);
                    lbl(&p, pos2(ir.right() - 12.0, ir.center().y), Align2::RIGHT_CENTER, key, 7.0, alpha(ERR, 0.7));
                } else {
                    p.rect_filled(ir, 0.0, alpha(TEXT, h));
                    lbl(&p, pos2(ir.left() + 12.0 + 3.0 * h, ir.center().y), Align2::LEFT_CENTER, s, 9.0, mix(TEXT, BG, h));
                    lbl(&p, pos2(ir.right() - 12.0, ir.center().y), Align2::RIGHT_CENTER, key, 7.0, mix(FAINT, BG, h));
                }
                if resp.clicked() {
                    picked = Some(*s);
                }
            }
        });
        let click_out = ui.input(|i| i.pointer.any_click()) && ui.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
        let opened_now = self.ctx_k < 0.2;
        if picked.is_some() || ui.input(|i| i.key_pressed(egui::Key::Escape)) || (click_out && !opened_now) {
            self.ctx_menu = None;
        }
        let now = ui.input(|i| i.time);
        match picked {
            Some("delete") => self.delete_layer(target, now),
            Some("disable mask") | Some("remove mask") => self.layers[target].mask = None,
            _ => {}
        }
    }
}

pub enum DropKind {
    Channel,
    Blend(usize),
}

/// An open drop list under `anchor`.
pub struct Drop {
    pub kind: DropKind,
    pub anchor: Rect,
    pub k: f32,
}

const BLENDS: [(&str, &str); 11] = [
    ("NRM", "normal"),
    ("PTHR", "passthrough"),
    ("MUL", "multiply"),
    ("SCR", "screen"),
    ("OVR", "overlay"),
    ("ADD", "linear dodge (add)"),
    ("SUB", "subtract"),
    ("DIV", "divide"),
    ("DIF", "difference"),
    ("DRK", "darken"),
    ("LGT", "lighten"),
];

/// Small down caret; `open` turns it to point up.
fn caret(p: &Painter, c: Pos2, col: Color32, open: f32) {
    let f = 1.0 - 2.0 * open;
    p.add(Shape::convex_polygon(vec![c + vec2(-3.5, -1.5 * f), c + vec2(3.5, -1.5 * f), c + vec2(0.0, 2.5 * f)], col, Stroke::NONE));
}

impl App {
    fn drop_list(&mut self, ui: &mut Ui) {
        let Some(d) = &mut self.drop else { return };
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        approach(&mut d.k, true, dt, 0.12);
        let k = smooth(d.k);
        let anchor = d.anchor;
        let (items, current): (Vec<(&str, &str)>, usize) = match d.kind {
            DropKind::Channel => (CHANNELS.iter().map(|c| (*c, "")).collect(), self.channel),
            DropKind::Blend(i) => (
                BLENDS.iter().map(|(c, n)| (*n, *c)).collect(),
                BLENDS.iter().position(|b| b.0 == self.layers[i].blend).unwrap_or(0),
            ),
        };
        let w = anchor.width().max(210.0);
        let r = Rect::from_min_size(pos2(anchor.right() - w, anchor.bottom() + 2.0 - 6.0 * (1.0 - k)), vec2(w, items.len() as f32 * 26.0 + 8.0));
        let mut picked = None;
        egui::Area::new(Id::new("drop_list")).fixed_pos(r.min).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            ui.multiply_opacity(k);
            ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 16);
            win_frame(&p, r, CARD, BORDER2);
            for (n, (name, code)) in items.iter().enumerate() {
                let ir = Rect::from_min_size(pos2(r.left() + 1.0, r.top() + 4.0 + n as f32 * 26.0), vec2(w - 2.0, 26.0));
                let resp = ui.interact(ir, Id::new(("drop", n)), Sense::click());
                let h = an(ui, ("drop", n), resp.hovered());
                p.rect_filled(ir, 0.0, alpha(TEXT, h));
                let cur = n == current;
                if cur {
                    p.rect_filled(Rect::from_center_size(pos2(ir.left() + 10.0, ir.center().y), vec2(5.0, 5.0)), 0.0, acc());
                }
                lbl(&p, pos2(ir.left() + 22.0 + 3.0 * h, ir.center().y), Align2::LEFT_CENTER, name, 8.0, mix(if cur { Color32::WHITE } else { SOFT }, BG, h));
                txt(&p, pos2(ir.right() - 10.0, ir.center().y), Align2::RIGHT_CENTER, code, F::Mono, 10.0, mix(FAINT, BG, h), 0.0);
                if resp.clicked() {
                    picked = Some(n);
                }
            }
        });
        let click_out = ui.input(|i| i.pointer.any_click())
            && ui.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q) && !anchor.contains(q));
        if let Some(n) = picked {
            match self.drop.as_ref().map(|d| &d.kind) {
                Some(DropKind::Channel) => self.channel = n,
                Some(DropKind::Blend(i)) => {
                    let i = *i;
                    self.layers[i].blend = BLENDS[n].0;
                }
                None => {}
            }
        }
        if picked.is_some() || click_out || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.drop = None;
        }
    }
}
