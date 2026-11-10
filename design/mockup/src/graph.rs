//! MATERIAL mode, proposed on board 03: graph tools on the left, a graph bar centred on top, the add-node
//! search at the cursor, and the graph windows (parameters, outputs, 2D view). The board leaves most keys
//! "to define", so tools here are picked by clicking only.

use super::*;

/// Graph tools: (name, how it is used, icon)
pub const GTOOLS: [(&str, &str, Ic); 8] = [
    ("select / move", "click · drag", Ic::App(icons::Icon::Select)),
    ("pan", "mmb", Ic::Svg("hand")),
    ("add node", "double-click the graph", Ic::App(icons::Icon::Plus)),
    ("connect pins", "drag", Ic::Svg("plugs-connected")),
    ("cut links", "drag", Ic::Svg("scissors")),
    ("frame selection", "", Ic::App(icons::Icon::Maximize)),
    ("comment", "", Ic::Svg("chat-text")),
    ("sample output color", "", Ic::App(icons::Icon::Dropper)),
];

/// (name, category) of the add-node search.
const LIB: [(&str, &str); 12] = [
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
const OUTPUTS: [&str; 5] = ["base color", "roughness", "metallic", "normal", "height"];

pub struct Graph {
    pub tool: usize,
    size: usize,
    tile: usize,
    seed: f32,
    view: usize,
    dim: usize,
    snap: bool,
    pub origin: Pos2,
    add: Option<Pos2>,
    add_k: f32,
    add_find: String,
    add_cat: usize,
    add_sel: usize,
}

impl Default for Graph {
    fn default() -> Self {
        Graph { tool: 0, size: 2, tile: 0, seed: 417.0, view: 0, dim: 0, snap: true, origin: Pos2::ZERO, add: None, add_k: 0.0, add_find: String::new(), add_cat: 0, add_sel: 0 }
    }
}

/// An icon from the app's set or an Iconify SVG.
#[derive(Clone, Copy)]
pub enum Ic {
    App(icons::Icon),
    Svg(&'static str),
}

pub fn draw_ic(ui: &Ui, p: &Painter, r: Rect, ic: Ic, c: Color32) {
    match ic {
        Ic::App(i) => icons::draw(p, r, i, c),
        Ic::Svg(n) => svg_icon(ui, r, n, c),
    }
}

impl App {
    /// Tool strip of the graph (left), same place as the painting tools.
    pub(crate) fn graph_strip(&mut self, ui: &mut Ui, p: &Painter, strip: Rect) {
        frame(p, strip, PANEL, BORDER);
        let ay = anim_value(ui.ctx(), Id::new("gtool_y"), self.gr.tool as f32, 0.18);
        let act = Rect::from_min_size(strip.min + vec2(3.0, 3.0 + ay * 38.0), vec2(34.0, 34.0));
        glow(p, act, acc(), 0.5, 6);
        p.rect_filled(act, 0.0, acc());
        for (i, (name, how, ic)) in GTOOLS.iter().enumerate() {
            let r = Rect::from_min_size(strip.min + vec2(3.0, 3.0 + i as f32 * 38.0), vec2(34.0, 34.0));
            let resp = ui.interact(r, Id::new(("gtool", i)), Sense::click());
            if resp.clicked() {
                self.gr.tool = i;
            }
            let on = an(ui, ("gtool_on", i), self.gr.tool == i);
            let h = an(ui, ("gtool_h", i), resp.hovered() && self.gr.tool != i);
            p.rect_filled(r, 0.0, alpha(TEXT, h * (1.0 - on)));
            draw_ic(ui, p, Rect::from_center_size(r.center(), vec2(18.0, 18.0)), *ic, mix(mix(DIM, BG, h), Color32::WHITE, on));
            if resp.hovered() {
                tip(ui, pos2(r.right() + 8.0, r.center().y), name, how);
            }
        }
    }

    /// The graph bar: fixed width, centred on top of the graph.
    pub(crate) fn graph_bar(&mut self, ui: &mut Ui, p: &Painter, graph: Rect) {
        let w = 760.0_f32.min(graph.width() - 16.0);
        let bar = Rect::from_center_size(pos2(graph.center().x, graph.top() + 17.0), vec2(w, 34.0));
        frame(p, bar, PANEL, BORDER);
        let bp = p.with_clip_rect(bar.intersect(p.clip_rect()));
        let hr = Rect::from_min_size(bar.min, vec2(80.0, 34.0));
        bp.rect_filled(hr.shrink(1.0), 0.0, CARD);
        bp.vline(hr.right(), hr.y_range(), Stroke::new(1.0, BORDER));
        lbl(&bp, hr.center(), Align2::CENTER_CENTER, "graph", 8.0, TEXT);
        let mut x = hr.right() + 8.0;
        let cy = bar.center().y;
        let mut field = |label: &str, value: &str, ww: f32, id: &str, ui: &mut Ui| -> bool {
            let lr = lbl(&bp, pos2(x, cy), Align2::LEFT_CENTER, label, 7.0, DIM);
            let r = Rect::from_min_size(pos2(lr.right() + 6.0, cy - 12.0), vec2(ww, 24.0));
            let resp = ui.interact(r, Id::new(id), Sense::click());
            let h = an(ui, id, resp.hovered());
            frame(&bp, r, mix(BG, TEXT, h), mix(BORDER2, TEXT, h));
            lbl(&bp, r.center(), Align2::CENTER_CENTER, value, 7.0, mix(TEXT, BG, h));
            x = r.right() + 12.0;
            resp.clicked()
        };
        let sizes = ["512", "1024", "2048", "4096"];
        if field("size", &format!("{} ▾", sizes[self.gr.size]), 64.0, "g_size", ui) {
            self.gr.size = (self.gr.size + 1) % 4;
        }
        if field("tile", if self.gr.tile == 0 { "on" } else { "off" }, 36.0, "g_tile", ui) {
            self.gr.tile = 1 - self.gr.tile;
        }
        if field("seed", &format!("{:.0}", self.gr.seed), 44.0, "g_seed", ui) {
            self.gr.seed = ((self.gr.seed * 7.0 + 13.0) % 997.0).round();
        }
        if field("view", &format!("{} ▾", OUTPUTS[self.gr.view]), 100.0, "g_view", ui) {
            self.gr.view = (self.gr.view + 1) % OUTPUTS.len();
        }
        let dr = Rect::from_min_size(pos2(x, cy - 12.0), vec2(64.0, 24.0));
        settings::seg(ui, &bp, dr, "g_dim", &["2d", "3d"], &mut self.gr.dim, None);
        x = dr.right() + 8.0;
        for (i, s) in ["align", "snap", "fit"].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, cy - 12.0), vec2(46.0, 24.0));
            x += 50.0;
            let resp = ui.interact(r, Id::new(("g_tool", i)), Sense::click());
            let on = an(ui, ("g_tool_on", i), (i == 1 && self.gr.snap) || resp.hovered());
            frame(&bp, r, mix(BG, TEXT, on), mix(BORDER2, TEXT, on));
            lbl(&bp, r.center(), Align2::CENTER_CENTER, s, 6.5, mix(DIM, BG, on));
            if resp.clicked() && i == 1 {
                self.gr.snap = !self.gr.snap;
            }
        }
        if x + 110.0 < bar.right() {
            lbl(&bp, pos2(bar.right() - 10.0, cy), Align2::RIGHT_CENTER, &format!("{} nodes · 3.2 ms", self.nodes.len()), 6.5, FAINT);
        }
    }

    /// Double-click on the empty graph (or a click with the add-node tool) opens the search at the cursor.
    pub(crate) fn graph_input(&mut self, ui: &mut Ui, graph: Rect) {
        let resp = ui.interact(graph, Id::new("graph_bg"), Sense::click());
        if resp.double_clicked() || (resp.clicked() && self.gr.tool == 2) {
            self.gr.add = resp.interact_pointer_pos();
            self.gr.add_k = 0.0;
            self.gr.add_find.clear();
            self.gr.add_sel = 0;
        }
    }

    pub(crate) fn add_node_popup(&mut self, ctx: &egui::Context) {
        let Some(at) = self.gr.add else { return };
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        approach(&mut self.gr.add_k, true, dt, 0.12);
        let k = smooth(self.gr.add_k);
        let r = Rect::from_min_size(at + vec2(0.0, -6.0 * (1.0 - k)), vec2(320.0, 330.0));
        let q = self.gr.add_find.to_lowercase();
        let cat = CATS[self.gr.add_cat];
        let items: Vec<(&str, &str)> = LIB.iter().copied().filter(|(n, c)| (cat == "all" || *c == cat) && n.to_lowercase().contains(&q)).collect();
        let (up, down, enter, esc) = ctx.input(|i| {
            (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape))
        });
        if up {
            self.gr.add_sel = self.gr.add_sel.saturating_sub(1);
        }
        if down {
            self.gr.add_sel = (self.gr.add_sel + 1).min(items.len().saturating_sub(1));
        }
        let mut pick = enter.then_some(self.gr.add_sel);
        egui::Area::new(Id::new("add_node")).fixed_pos(r.min).order(egui::Order::Foreground).show(ctx, |ui| {
            ui.multiply_opacity(k);
            ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 20);
            win_frame(&p, r, CARD, BORDER2);
            lbl(&p, pos2(r.left() + 12.0, r.top() + 16.0), Align2::LEFT_CENTER, "add node >", 8.0, TEXT);
            let fr = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + 30.0), vec2(r.width() - 20.0, 24.0));
            frame(&p, fr, BG, acc());
            let te = ui.put(fr.shrink2(vec2(8.0, 3.0)), egui::TextEdit::singleline(&mut self.gr.add_find).frame(false).font(fid(F::Px, 10.0)).text_color(TEXT).hint_text("SEARCH"));
            if self.gr.add_k < 0.3 {
                te.request_focus();
            }
            let mut x = r.left() + 10.0;
            for (i, c) in CATS.iter().enumerate() {
                let w = c.len() as f32 * 9.0 + 16.0;
                let cr = Rect::from_min_size(pos2(x, fr.bottom() + 6.0), vec2(w, 22.0));
                x += w + 4.0;
                let resp = ui.interact(cr, Id::new(("add_cat", i)), Sense::click());
                if resp.clicked() {
                    self.gr.add_cat = i;
                    self.gr.add_sel = 0;
                }
                let on = an(ui, ("add_cat", i), self.gr.add_cat == i || resp.hovered());
                frame(&p, cr, mix(BG, TEXT, on), mix(BORDER2, TEXT, on));
                lbl(&p, cr.center(), Align2::CENTER_CENTER, c, 6.5, mix(DIM, BG, on));
            }
            let mut y = fr.bottom() + 36.0;
            for (n, (name, c)) in items.iter().enumerate() {
                if y + 26.0 > r.bottom() - 18.0 {
                    break;
                }
                let ir = Rect::from_min_size(pos2(r.left() + 1.0, y), vec2(r.width() - 2.0, 26.0));
                y += 26.0;
                let resp = ui.interact(ir, Id::new(("add_it", *name)), Sense::click());
                if resp.hovered() {
                    self.gr.add_sel = n;
                }
                let on = an(ui, ("add_it_on", *name), self.gr.add_sel == n);
                p.rect_filled(ir, 0.0, alpha(TEXT, on));
                txt(&p, pos2(ir.left() + 12.0, ir.center().y), Align2::LEFT_CENTER, name, F::Ui, 12.0, mix(SOFT, BG, on), 0.0);
                lbl(&p, pos2(ir.right() - 12.0, ir.center().y), Align2::RIGHT_CENTER, &if n == self.gr.add_sel { format!("{c} · enter") } else { c.to_string() }, 6.0, mix(FAINT, BG, on));
                if resp.clicked() {
                    pick = Some(n);
                }
            }
            lbl(&p, pos2(r.left() + 12.0, r.bottom() - 10.0), Align2::LEFT_CENTER, &format!("enter adds · esc closes · {} / {}", items.len(), LIB.len()), 6.0, FAINT);
        });
        if let Some(n) = pick {
            if let Some((name, _)) = items.get(n) {
                let pos = at - self.gr.origin - vec2(75.0, 15.0);
                self.nodes.push(Node { name, pos, ins: 1, outs: 1 });
                self.sel_node = self.nodes.len() - 1;
                self.sel_t = ctx.input(|i| i.time);
            }
            self.gr.add = None;
        }
        let out = ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
        if esc || (out && self.gr.add_k >= 1.0) {
            self.gr.add = None;
        }
    }

    pub(crate) fn graph_params_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "graph parameters", None, "");
        let row = |y: f32| Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 26.0));
        let mut y = inner.top() + 8.0;
        lbl(p, pos2(inner.left() + 10.0, y + 13.0), Align2::LEFT_CENTER, "output size", 7.0, DIM);
        settings::seg(ui, p, Rect::from_min_size(pos2(inner.left() + inner.width() * 0.4, y), vec2(inner.width() * 0.6 - 10.0, 26.0)), "gp_size", &["512", "1k", "2k", "4k"], &mut self.gr.size, None);
        y += 34.0;
        lbl(p, pos2(inner.left() + 10.0, y + 13.0), Align2::LEFT_CENTER, "tileable", 7.0, DIM);
        settings::seg(ui, p, Rect::from_min_size(pos2(inner.left() + inner.width() * 0.4, y), vec2(inner.width() * 0.6 - 10.0, 26.0)), "gp_tile", &["on", "off"], &mut self.gr.tile, None);
        y += 34.0;
        slider(ui, p, row(y), "gp_seed", "seed", &mut self.gr.seed, 999.0, "");
    }

    pub(crate) fn outputs_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "outputs", Some(OUTPUTS.len()), "");
        for (i, o) in OUTPUTS.iter().enumerate() {
            let row = Rect::from_min_size(pos2(inner.left(), inner.top() + 4.0 + i as f32 * 40.0), vec2(inner.width(), 38.0));
            if row.bottom() > inner.bottom() {
                break;
            }
            let resp = ui.interact(row, Id::new(("out", i)), Sense::click());
            if resp.clicked() {
                self.gr.view = i;
            }
            let on = an(ui, ("out_on", i), self.gr.view == i);
            let h = an(ui, ("out_h", i), resp.hovered());
            p.rect_filled(row, 0.0, mix(alpha(HOVER, h), SEL, on));
            p.rect_filled(Rect::from_min_size(row.min, vec2(2.0, row.height() * on)), 0.0, acc());
            node_thumb(p, Rect::from_center_size(pos2(row.left() + 26.0, row.center().y), vec2(30.0, 30.0)), i + 2);
            lbl(p, pos2(row.left() + 52.0, row.center().y), Align2::LEFT_CENTER, o, 8.0, mix(SOFT, Color32::WHITE, on));
        }
    }

    pub(crate) fn view2d_panel(&mut self, _ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "2d view", None, OUTPUTS[self.gr.view]);
        let s = inner.width().min(inner.height()) - 24.0;
        let t = Rect::from_center_size(inner.center(), vec2(s, s));
        node_thumb(p, t, self.sel_node);
        p.rect_stroke(t, 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Inside);
    }
}

/// Small label box beside a hovered button.
pub fn tip(ui: &Ui, at: Pos2, name: &str, how: &str) {
    let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("tip")));
    let w = (name.len().max(how.len()) as f32 * 9.0 + 24.0).max(120.0);
    let r = Rect::from_min_size(pos2(at.x, at.y - 18.0), vec2(w, if how.is_empty() { 22.0 } else { 36.0 }));
    let r = if r.right() > ui.ctx().screen_rect().right() { r.translate(vec2(-(w + 60.0), 0.0)) } else { r };
    lp.rect_filled(r, 0.0, CARD);
    lp.rect_stroke(r, 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Inside);
    lbl(&lp, pos2(r.left() + 8.0, r.top() + 11.0), Align2::LEFT_CENTER, name, 7.0, TEXT);
    if !how.is_empty() {
        lbl(&lp, pos2(r.left() + 8.0, r.top() + 26.0), Align2::LEFT_CENTER, how, 6.0, FAINT);
    }
}
