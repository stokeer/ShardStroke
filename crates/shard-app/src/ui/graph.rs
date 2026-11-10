//! MATERIAL mode, as validated in design/mockup (board 03): the node graph as the background, graph tools on the
//! left, a graph bar centred on top, the add-node search at the cursor and the graph windows. The graph engine
//! is not built yet (PLS-10): the nodes are a sample layout that can be moved, picked and added, nothing is
//! computed, and the graph is not saved with the project.

use super::*;
use egui::Pos2;

/// Graph tools: (name, how it is used, icon, built)
const GTOOLS: [(&str, &str, Icon, bool); 8] = [
    ("Select / move", "Click, drag", Icon::Select, true),
    ("Pan", "Middle mouse", Icon::Swap, false),
    ("Add node", "Double-click the graph", Icon::Plus, true),
    ("Connect pins", "Drag from a pin", Icon::Anchor, false),
    ("Cut links", "Drag across links", Icon::Close, false),
    ("Frame selection", "", Icon::Maximize, false),
    ("Comment", "", Icon::Menu, false),
    ("Sample output color", "", Icon::Dropper, false),
];

/// (name, category) of the add-node search.
pub(super) const LIB: [(&str, &str); 12] = [
    ("Perlin noise", "noises"),
    ("Cell noise", "noises"),
    ("Fractal sum noise", "noises"),
    ("Anisotropic noise", "noises"),
    ("Grunge map noise", "noises"),
    ("Bricks", "patterns"),
    ("Tiles", "patterns"),
    ("Checker", "patterns"),
    ("Blur", "filters"),
    ("Levels", "filters"),
    ("Warp", "filters"),
    ("Height to normal", "filters"),
];
const CATS: [&str; 4] = ["all", "noises", "patterns", "filters"];
const OUTPUTS: [&str; 5] = ["Base color", "Roughness", "Metallic", "Normal", "Height"];
const NODE: egui::Vec2 = egui::Vec2::new(150.0, 104.0);

pub struct GNode {
    pub name: String,
    pub pos: egui::Vec2,
    pub ins: u8,
    pub outs: u8,
    pub params: [f32; 4],
}

pub struct Graph {
    nodes: Vec<GNode>,
    wires: Vec<(usize, usize, u8)>,
    sel: usize,
    tool: usize,
    size: usize,
    tile: bool,
    seed: f32,
    view: usize,
    dim: usize,
    snap: bool,
    origin: Pos2,
    add: Option<Pos2>,
    add_k: f32,
    add_find: String,
    add_cat: usize,
    add_sel: usize,
}

impl Graph {
    /// "GRAPH · 7 NODES · 2048" for the status bar.
    pub fn summary(&self) -> String {
        format!("GRAPH · {} NODES · {}", self.nodes.len(), [512, 1024, 2048, 4096][self.size.min(3)])
    }
}

impl Default for Graph {
    fn default() -> Self {
        let n = |name: &str, x: f32, y: f32, ins: u8, outs: u8| GNode { name: name.into(), pos: vec2(x, y), ins, outs, params: [0.62, 0.35, 0.45, 4.0] };
        Graph {
            nodes: vec![
                n("Noise", 40.0, 60.0, 0, 1),
                n("Tiles", 40.0, 230.0, 0, 1),
                n("Blend", 260.0, 140.0, 2, 1),
                n("Grunge", 40.0, 400.0, 0, 1),
                n("Levels", 260.0, 380.0, 1, 1),
                n("Height", 480.0, 230.0, 2, 1),
                n("Output", 700.0, 230.0, 1, 0),
            ],
            wires: vec![(0, 2, 0), (1, 2, 1), (2, 5, 0), (3, 4, 0), (4, 5, 1), (5, 6, 0)],
            sel: 2,
            tool: 0,
            size: 2,
            tile: true,
            seed: 417.0,
            view: 0,
            dim: 0,
            snap: true,
            origin: Pos2::ZERO,
            add: None,
            add_k: 0.0,
            add_find: String::new(),
            add_cat: 0,
            add_sel: 0,
        }
    }
}

/// A small procedural picture per node (only a look: nothing is computed).
fn node_thumb(p: &egui::Painter, r: Rect, seed: usize) {
    let cells = 12;
    let (cw, ch) = (r.width() / cells as f32, r.height() / cells as f32);
    for j in 0..cells {
        for i in 0..cells {
            let (x, y) = (i as f32 / cells as f32, j as f32 / cells as f32);
            let v = match seed % 7 {
                0 => (x * 7.0).sin() * (y * 5.0).cos() * 0.5 + 0.5,
                1 => {
                    if ((i / 3) + (j / 3)) % 2 == 0 { 0.8 } else { 0.25 }
                }
                3 => ((i * 31 + j * 17) % 11) as f32 / 11.0,
                4 => ((x * 9.0).sin() * 0.5 + 0.5) * 0.6 + 0.2,
                5 => (x + y) * 0.5,
                6 => 0.55 + 0.25 * ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt(),
                _ => ((x * 6.0).sin() + (y * 6.0).cos()) * 0.25 + 0.5,
            };
            let g = (v.clamp(0.0, 1.0) * 180.0 + 20.0) as u8;
            p.rect_filled(Rect::from_min_size(pos2(r.min.x + i as f32 * cw, r.min.y + j as f32 * ch), vec2(cw + 0.5, ch + 0.5)), 0.0, Color32::from_gray(g));
        }
    }
}

impl AppState {
    fn port(&self, n: &GNode, out: bool, k: u8) -> Pos2 {
        let r = Rect::from_min_size(self.graph.origin + n.pos, NODE);
        let count = if out { n.outs } else { n.ins }.max(1) as f32;
        let y = r.min.y + 30.0 + (r.height() - 30.0) * (k as f32 + 1.0) / (count + 1.0);
        pos2(if out { r.max.x } else { r.min.x }, y)
    }

    /// The graph, as the background of MATERIAL mode (under the windows).
    pub(super) fn graph_canvas(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG_VIEWPORT)).show(ctx, |ui| {
            let full = ui.max_rect();
            // the work area fades in after a mode switch
            let f = ((now - self.mode_t) / 0.25).clamp(0.0, 1.0) as f32;
            ui.multiply_opacity(if theme::animation() == 0 { 1.0 } else { f * f * (3.0 - 2.0 * f) });
            let p = ui.painter().clone();
            let mut gy = full.min.y + 12.0;
            while gy < full.max.y {
                let mut gx = full.min.x + 12.0;
                while gx < full.max.x {
                    p.rect_filled(Rect::from_min_size(pos2(gx, gy), vec2(1.0, 1.0)), 0.0, Color32::from_gray(0x24));
                    gx += 24.0;
                }
                gy += 24.0;
            }
            let free = if self.free_area.is_finite() { self.free_area } else { full };
            self.graph.origin = free.min + vec2(30.0, 60.0);
            // empty graph: double-click (or a click with the add-node tool) opens the search there
            let bg = ui.interact(full, ui.id().with("graph_bg"), Sense::click());
            if bg.double_clicked() || (bg.clicked() && self.graph.tool == 2) {
                self.graph.add = bg.interact_pointer_pos();
                self.graph.add_k = 0.0;
                self.graph.add_find.clear();
                self.graph.add_sel = 0;
            }
            let sel_k: Vec<f32> = (0..self.graph.nodes.len()).map(|i| ui.ctx().animate_bool_with_time(ui.id().with(("gsel", i)), self.graph.sel == i, theme::anim(0.16))).collect();
            for &(a, b, k) in &self.graph.wires {
                let s = self.port(&self.graph.nodes[a], true, 0);
                let e = self.port(&self.graph.nodes[b], false, k);
                let dx = ((e.x - s.x) * 0.5).abs().max(40.0);
                let hot = sel_k[a].max(sel_k[b]);
                p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [s, s + vec2(dx, 0.0), e - vec2(dx, 0.0), e],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(1.5, w::mix(Color32::from_gray(0x55), theme::accent(), hot)),
                ));
            }
            for i in 0..self.graph.nodes.len() {
                let r = Rect::from_min_size(self.graph.origin + self.graph.nodes[i].pos, NODE);
                let resp = ui.interact(r, ui.id().with(("gnode", i)), Sense::click_and_drag());
                if resp.dragged() {
                    self.graph.nodes[i].pos += resp.drag_delta();
                }
                if resp.drag_stopped() && self.graph.snap {
                    let q = &mut self.graph.nodes[i].pos;
                    *q = vec2((q.x / 24.0).round() * 24.0 - 6.0, (q.y / 24.0).round() * 24.0 - 6.0);
                }
                if resp.clicked() || resp.drag_started() {
                    self.graph.sel = i;
                }
                let n = &self.graph.nodes[i];
                let s = sel_k[i];
                let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim(0.16));
                theme::glow(&p, r, theme::accent(), 0.5 * s, 8);
                p.rect_filled(r, 0.0, theme::PANEL_ALT);
                p.rect_stroke(r, 0.0, Stroke::new(1.0, w::mix(w::mix(theme::BORDER_STRONG, theme::TEXT_DIM, h), theme::accent(), s)), egui::StrokeKind::Inside);
                let hd = Rect::from_min_size(r.min, vec2(r.width(), 26.0));
                p.rect_filled(hd.shrink(1.0), 0.0, w::mix(theme::PANEL, theme::SELECTED, s));
                p.text(pos2(hd.min.x + 8.0, hd.center().y), Align2::LEFT_CENTER, n.name.to_uppercase(), FontId::proportional(10.0), theme::TEXT);
                node_thumb(&p, Rect::from_min_size(pos2(r.min.x + 30.0, hd.max.y + 8.0), vec2(r.width() - 60.0, r.height() - 42.0)), i);
                for k in 0..n.ins {
                    p.rect_filled(Rect::from_center_size(self.port(n, false, k), vec2(7.0, 7.0)), 0.0, theme::TEXT_DIM);
                }
                for k in 0..n.outs {
                    p.rect_filled(Rect::from_center_size(self.port(n, true, k), vec2(7.0, 7.0)), 0.0, w::mix(theme::TEXT, theme::accent(), s));
                }
            }
        });
    }

    /// Over the graph: its bar, the add-node search; F9 goes back to painting.
    pub(super) fn graph_overlays(&mut self, ctx: &egui::Context) {
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            self.material = false;
            self.mode_t = ctx.input(|i| i.time);
            return;
        }
        self.graph_bar(ctx);
        self.add_node_popup(ctx);
    }

    /// The graph bar: fixed width, centred on top of the graph.
    fn graph_bar(&mut self, ctx: &egui::Context) {
        let free = self.free_area;
        if !free.is_finite() {
            return;
        }
        let w_ = 760.0_f32.min(free.width() - 16.0);
        let bar = Rect::from_center_size(pos2(free.center().x, free.min.y + 19.0), vec2(w_, 34.0));
        egui::Area::new(egui::Id::new("graph_bar")).order(egui::Order::Middle).fixed_pos(bar.min).fade_in(false).show(ctx, |ui| {
            let (_, _) = ui.allocate_exact_size(bar.size(), Sense::hover());
            let p = ui.painter().with_clip_rect(bar);
            p.rect_filled(bar, 0.0, theme::PANEL);
            p.rect_stroke(bar, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            let hr = Rect::from_min_size(bar.min, vec2(80.0, 34.0));
            p.rect_filled(hr.shrink(1.0), 0.0, theme::PANEL_ALT);
            p.vline(hr.max.x, hr.y_range(), Stroke::new(1.0, theme::BORDER));
            p.text(hr.center(), Align2::CENTER_CENTER, "GRAPH", FontId::proportional(10.0), theme::TEXT);
            let cy = bar.center().y;
            let mut x = hr.max.x + 8.0;
            let sizes = ["512", "1024", "2048", "4096"];
            let fields: [(&str, String, f32); 4] = [
                ("SIZE", sizes[self.graph.size].to_string(), 64.0),
                ("TILE", if self.graph.tile { "ON".into() } else { "OFF".into() }, 36.0),
                ("SEED", format!("{:.0}", self.graph.seed), 44.0),
                ("VIEW", OUTPUTS[self.graph.view].to_uppercase(), 100.0),
            ];
            for (k, (label, value, ww)) in fields.into_iter().enumerate() {
                let lr = p.text(pos2(x, cy), Align2::LEFT_CENTER, label, FontId::proportional(8.0), theme::TEXT_DIM);
                let r = Rect::from_min_size(pos2(lr.max.x + 6.0, cy - 12.0), vec2(ww, 24.0));
                let resp = ui.interact(r, ui.id().with(("gbar", k)), Sense::click());
                let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim(0.16));
                p.rect_filled(r, 0.0, w::mix(theme::WELL, theme::TEXT, h));
                p.rect_stroke(r, 0.0, Stroke::new(1.0, w::mix(theme::BORDER_STRONG, theme::TEXT, h)), egui::StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, value, FontId::proportional(8.0), w::mix(theme::TEXT, theme::BG_VIEWPORT, h));
                if resp.clicked() {
                    match k {
                        0 => self.graph.size = (self.graph.size + 1) % 4,
                        1 => self.graph.tile = !self.graph.tile,
                        2 => self.graph.seed = ((self.graph.seed * 7.0 + 13.0) % 997.0).round(),
                        _ => self.graph.view = (self.graph.view + 1) % OUTPUTS.len(),
                    }
                }
                x = r.max.x + 12.0;
            }
            let dr = Rect::from_min_size(pos2(x, cy - 12.0), vec2(64.0, 24.0));
            if let Some(k) = w::segmented_at(ui, dr, &[("2D", true, ""), ("3D", true, "")], self.graph.dim) {
                self.graph.dim = k;
            }
            x = dr.max.x + 8.0;
            for (k, s) in ["ALIGN", "SNAP", "FIT"].into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(x, cy - 12.0), vec2(46.0, 24.0));
                x += 50.0;
                let resp = ui.interact(r, ui.id().with(("gtool_btn", k)), Sense::click());
                let on = ui.ctx().animate_bool_with_time(resp.id, (k == 1 && self.graph.snap) || resp.hovered(), theme::anim(0.16));
                p.rect_filled(r, 0.0, w::mix(theme::WELL, theme::TEXT, on));
                p.rect_stroke(r, 0.0, Stroke::new(1.0, w::mix(theme::BORDER_STRONG, theme::TEXT, on)), egui::StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, s, FontId::proportional(8.0), w::mix(theme::TEXT_DIM, theme::BG_VIEWPORT, on));
                if resp.clicked() && k == 1 {
                    self.graph.snap = !self.graph.snap;
                }
            }
            if x + 110.0 < bar.max.x {
                p.text(pos2(bar.max.x - 10.0, cy), Align2::RIGHT_CENTER, format!("{} NODES", self.graph.nodes.len()), FontId::proportional(8.0), theme::TEXT_DISABLED);
            }
        });
    }

    fn add_node_popup(&mut self, ctx: &egui::Context) {
        let Some(at) = self.graph.add else { return };
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        self.graph.add_k = if theme::animation() == 0 { 1.0 } else { (self.graph.add_k + dt / 0.12).min(1.0) };
        let k = self.graph.add_k;
        let r = Rect::from_min_size(at + vec2(0.0, -6.0 * (1.0 - k)), vec2(320.0, 330.0));
        let q = self.graph.add_find.to_lowercase();
        let cat = CATS[self.graph.add_cat];
        let items: Vec<(&str, &str)> = LIB.iter().copied().filter(|(n, c)| (cat == "all" || *c == cat) && n.to_lowercase().contains(&q)).collect();
        let (up, down, enter, esc) =
            ctx.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if up {
            self.graph.add_sel = self.graph.add_sel.saturating_sub(1);
        }
        if down {
            self.graph.add_sel = (self.graph.add_sel + 1).min(items.len().saturating_sub(1));
        }
        let mut pick = enter.then_some(self.graph.add_sel);
        egui::Area::new(egui::Id::new("add_node")).order(egui::Order::Foreground).fixed_pos(r.min).fade_in(false).show(ctx, |ui| {
            ui.multiply_opacity(k);
            let (_, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter().clone();
            theme::glow(&p, r, Color32::BLACK, 0.6, 20);
            p.rect_filled(r, 0.0, theme::PANEL_ALT);
            p.rect_stroke(r, 0.0, Stroke::new(theme::WINDOW_STROKE, theme::BORDER_STRONG), egui::StrokeKind::Inside);
            p.text(pos2(r.min.x + 12.0, r.min.y + 16.0), Align2::LEFT_CENTER, "ADD NODE >", FontId::proportional(10.0), theme::TEXT);
            let fr = Rect::from_min_size(pos2(r.min.x + 10.0, r.min.y + 30.0), vec2(r.width() - 20.0, 24.0));
            p.rect_filled(fr, 0.0, theme::WELL);
            p.rect_stroke(fr, 0.0, Stroke::new(1.0, theme::accent()), egui::StrokeKind::Inside);
            let te = ui.put(fr.shrink2(vec2(8.0, 3.0)), egui::TextEdit::singleline(&mut self.graph.add_find).frame(false).hint_text("SEARCH"));
            if k < 0.3 {
                te.request_focus();
            }
            let mut x = r.min.x + 10.0;
            for (i, c) in CATS.iter().enumerate() {
                let ww = c.len() as f32 * 9.0 + 16.0;
                let cr = Rect::from_min_size(pos2(x, fr.max.y + 6.0), vec2(ww, 22.0));
                x += ww + 4.0;
                let resp = ui.interact(cr, ui.id().with(("add_cat", i)), Sense::click());
                if resp.clicked() {
                    self.graph.add_cat = i;
                    self.graph.add_sel = 0;
                }
                let on = ui.ctx().animate_bool_with_time(resp.id, self.graph.add_cat == i || resp.hovered(), theme::anim(0.16));
                p.rect_filled(cr, 0.0, w::mix(theme::WELL, theme::TEXT, on));
                p.rect_stroke(cr, 0.0, Stroke::new(1.0, w::mix(theme::BORDER_STRONG, theme::TEXT, on)), egui::StrokeKind::Inside);
                p.text(cr.center(), Align2::CENTER_CENTER, c.to_uppercase(), FontId::proportional(8.0), w::mix(theme::TEXT_DIM, theme::BG_VIEWPORT, on));
            }
            let mut y = fr.max.y + 36.0;
            for (n, (name, c)) in items.iter().enumerate() {
                if y + 26.0 > r.max.y - 18.0 {
                    break;
                }
                let ir = Rect::from_min_size(pos2(r.min.x + 2.0, y), vec2(r.width() - 4.0, 26.0));
                y += 26.0;
                let resp = ui.interact(ir, ui.id().with(("add_it", *name)), Sense::click());
                if resp.hovered() {
                    self.graph.add_sel = n;
                }
                let on = ui.ctx().animate_bool_with_time(resp.id.with("on"), self.graph.add_sel == n, theme::anim(0.12));
                p.rect_filled(ir, 0.0, w::mix(Color32::TRANSPARENT, theme::TEXT, on));
                p.text(pos2(ir.min.x + 12.0, ir.center().y), Align2::LEFT_CENTER, name.to_uppercase(), FontId::proportional(10.0), w::mix(theme::TEXT, theme::BG_VIEWPORT, on));
                let tag = if n == self.graph.add_sel { format!("{} · ENTER", c.to_uppercase()) } else { c.to_uppercase() };
                p.text(pos2(ir.max.x - 12.0, ir.center().y), Align2::RIGHT_CENTER, tag, FontId::proportional(8.0), w::mix(theme::TEXT_DISABLED, theme::BG_VIEWPORT, on));
                if resp.clicked() {
                    pick = Some(n);
                }
            }
            p.text(pos2(r.min.x + 12.0, r.max.y - 10.0), Align2::LEFT_CENTER, format!("ENTER ADDS · ESC CLOSES · {} / {}", items.len(), LIB.len()), FontId::proportional(8.0), theme::TEXT_DISABLED);
        });
        if let Some(n) = pick {
            if let Some((name, _)) = items.get(n) {
                let pos = at - self.graph.origin - vec2(75.0, 15.0);
                self.graph.nodes.push(GNode { name: name.to_string(), pos, ins: 1, outs: 1, params: [0.5, 0.5, 0.5, 4.0] });
                self.graph.sel = self.graph.nodes.len() - 1;
                self.dock_mat.show(Pane::NodeProperties);
            }
            self.graph.add = None;
        }
        let out = ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
        if esc || (out && self.graph.add_k >= 1.0) {
            self.graph.add = None;
        }
        ctx.request_repaint();
    }

    /// Graph tools (left strip of MATERIAL mode); the ones without an engine behind them are greyed.
    pub(super) fn graph_strip(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            for (i, (name, how, icon, built)) in GTOOLS.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), if *built { Sense::click() } else { Sense::hover() });
                w::paint_icon_button(ui, rect, &resp, *icon, self.graph.tool == i, *built);
                let tip = if *built { format!("{name}  ·  {how}") } else { format!("{name} - not built yet") };
                if resp.on_hover_text(tip).clicked() && *built {
                    self.graph.tool = i;
                }
            }
        });
    }

    /// Window buttons of MATERIAL mode down the right edge.
    pub(super) fn graph_dock_strip(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            let buttons: [(Icon, Pane); 9] = [
                (Icon::Grid, Pane::Nodes),
                (Icon::Sphere, Pane::Preview3d),
                (Icon::Sliders, Pane::NodeProperties),
                (Icon::Shader, Pane::GraphParameters),
                (Icon::Material, Pane::Outputs),
                (Icon::Display, Pane::View2d),
                (Icon::History, Pane::History),
                (Icon::Assets, Pane::Assets),
                (Icon::Terminal, Pane::Log),
            ];
            for (icon, pane) in buttons {
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                let shown = self.dock_mat.contains(pane);
                let on = ui.ctx().animate_bool_with_time(resp.id.with("on"), shown, theme::anim(0.16));
                let hov = ui.ctx().animate_bool_with_time(resp.id.with("hov"), resp.hovered(), theme::anim(0.16));
                let p = ui.painter();
                let [r, g, b] = theme::accent_rgb();
                p.rect_filled(rect, 0.0, w::mix(Color32::TRANSPARENT, Color32::from_rgba_unmultiplied(r, g, b, 56), on));
                p.rect_filled(rect, 0.0, w::mix(Color32::TRANSPARENT, theme::TEXT, hov));
                p.rect_filled(Rect::from_min_size(pos2(rect.max.x + 1.0, rect.min.y + 6.0), vec2(2.0, (rect.height() - 12.0) * on)), 0.0, theme::accent());
                crate::icons::draw(p, rect.shrink(8.0), icon, w::mix(w::mix(theme::TEXT_DIM, theme::TEXT, on), theme::BG_VIEWPORT, hov));
                if resp.on_hover_text(pane.title()).clicked() {
                    if shown && self.dock_mat.shown(pane) {
                        self.dock_mat.remove(pane);
                    } else {
                        self.dock_mat.show(pane);
                    }
                }
            }
            ui.add_space(4.0);
            w::separator(ui);
            ui.add_space(4.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            w::paint_icon_button(ui, rect, &resp, Icon::Export, false, false);
            resp.on_hover_text("Export material - not built yet");
        });
    }

    // ------------------------------------------------------------ the windows of MATERIAL mode

    pub(super) fn nodes_pane(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("ADD A NODE TO THE GRAPH").size(8.0).color(theme::TEXT_DISABLED));
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (name, cat) in LIB {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim(0.16));
                let p = ui.painter();
                p.rect_filled(r, 0.0, w::mix(Color32::TRANSPARENT, theme::TEXT, h));
                p.text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, name.to_uppercase(), FontId::proportional(10.0), w::mix(theme::TEXT, theme::BG_VIEWPORT, h));
                p.text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, format!("{} +", cat.to_uppercase()), FontId::proportional(8.0), w::mix(theme::TEXT_DISABLED, theme::BG_VIEWPORT, h));
                if resp.clicked() {
                    let at = self.free_area.center();
                    self.graph.nodes.push(GNode { name: name.to_string(), pos: at - self.graph.origin - NODE * 0.5, ins: 1, outs: 1, params: [0.5, 0.5, 0.5, 4.0] });
                    self.graph.sel = self.graph.nodes.len() - 1;
                }
            }
        });
    }

    pub(super) fn node_props_pane(&mut self, ui: &mut egui::Ui) {
        let Some(n) = self.graph.nodes.get_mut(self.graph.sel) else { return };
        ui.label(RichText::new(n.name.to_uppercase()).size(10.0).color(theme::TEXT));
        ui.label(RichText::new("THE GRAPH ENGINE IS NOT BUILT YET: NOTHING IS COMPUTED").size(8.0).color(theme::TEXT_DISABLED));
        ui.add_space(6.0);
        for (k, label) in ["Amount", "Contrast", "Scale"].into_iter().enumerate() {
            w::form_slider(ui, label, &mut n.params[k], 0.0..=1.0, 2);
        }
        w::form_slider(ui, "Octaves", &mut n.params[3], 1.0..=8.0, 0);
    }

    pub(super) fn preview3d_pane(&mut self, ui: &mut egui::Ui) {
        let r = ui.available_rect_before_wrap();
        let rad = r.width().min(r.height()) * 0.38;
        w::paint_sphere(ui.painter(), r.center(), rad, w::SphereKind::Raster, false);
    }

    pub(super) fn graph_params_pane(&mut self, ui: &mut egui::Ui) {
        w::form_row(ui, "Output size", |ui| {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            if let Some(k) = w::segmented_at(ui, r, &[("512", true, ""), ("1K", true, ""), ("2K", true, ""), ("4K", true, "")], self.graph.size) {
                self.graph.size = k;
            }
        });
        if w::form_toggle(ui, "Tileable", self.graph.tile) {
            self.graph.tile = !self.graph.tile;
        }
        w::form_slider(ui, "Seed", &mut self.graph.seed, 0.0..=999.0, 0);
    }

    pub(super) fn outputs_pane(&mut self, ui: &mut egui::Ui) {
        for (i, o) in OUTPUTS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
            if resp.clicked() {
                self.graph.view = i;
            }
            let on = ui.ctx().animate_bool_with_time(resp.id.with("on"), self.graph.view == i, theme::anim(0.16));
            let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim(0.16));
            let p = ui.painter();
            p.rect_filled(r, 0.0, w::mix(w::mix(Color32::TRANSPARENT, theme::WIDGET_HOVER, h), theme::SELECTED, on));
            p.rect_filled(Rect::from_min_size(r.min, vec2(2.0, r.height() * on)), 0.0, theme::accent());
            node_thumb(p, Rect::from_center_size(pos2(r.min.x + 24.0, r.center().y), vec2(30.0, 30.0)), i + 2);
            p.text(pos2(r.min.x + 50.0, r.center().y), Align2::LEFT_CENTER, o.to_uppercase(), FontId::proportional(10.0), w::mix(theme::TEXT_DIM, theme::TEXT, on));
        }
    }

    pub(super) fn view2d_pane(&mut self, ui: &mut egui::Ui) {
        let r = ui.available_rect_before_wrap();
        let s = r.width().min(r.height()) - 12.0;
        if s > 8.0 {
            let t = Rect::from_center_size(r.center(), vec2(s, s));
            node_thumb(ui.painter(), t, self.graph.sel);
            ui.painter().rect_stroke(t, 0.0, Stroke::new(1.0, theme::BORDER_STRONG), egui::StrokeKind::Inside);
        }
    }
}
