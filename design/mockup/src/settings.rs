//! Settings: modal over the program (Esc or a click outside closes it), five pages like the design board 17.

use super::*;

/// Highlight presets; the first is the kit's purple.
const PRESETS: [u32; 5] = [0x6C23DC, 0x7CCC4A, 0x4D8CFF, 0x30BEAA, 0xFF9632];

const PAGES: [(&str, &str); 5] = [
    ("general", "procedurals · autosave"),
    ("appearance", "highlight · loading colors"),
    ("viewport", "render · antialiasing"),
    ("libraries", "asset folders"),
    ("shortcuts", "industry-standard keys"),
];

pub struct Cfg {
    pub open: bool,
    pub k: f32,
    pub page: usize,
    pub page_t: f64,
    pub ext_engine: usize,
    pub autosave: usize,
    pub aa: usize,
    pub render: usize,
    pub ai_denoise: usize,
    pub stats: usize,
    /// 0 = locked (no X, no dragging), 1 = free.
    pub lock: usize,
    /// 0 off, 1 basic, 2 advanced.
    pub anim: usize,
    pub folders: Vec<(&'static str, bool)>,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            open: false,
            k: 0.0,
            page: 2,
            page_t: -10.0,
            ext_engine: 0,
            autosave: 2,
            aa: 2,
            render: 0,
            ai_denoise: 0,
            stats: 1,
            lock: 1,
            anim: 2,
            folders: vec![("D:\\assets\\materials", true), ("D:\\assets\\brushes", false), ("C:\\Users\\me\\ShardStroke\\library", false)],
        }
    }
}

fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Pixel label wrapped to `w`.
fn wrapped(p: &Painter, at: Pos2, s: &str, w: f32, color: Color32) -> f32 {
    let mut job = LayoutJob::default();
    job.wrap.max_width = w;
    job.append(&s.to_uppercase(), 0.0, TextFormat { font_id: fid(F::Px, 8.0 * 1.25), color, extra_letter_spacing: 0.96, line_height: Some(15.0), ..Default::default() });
    let g = p.layout_job(job);
    let h = g.size().y;
    p.galley(at, g, color);
    h
}

/// Segmented control; the light block slides to the picked option. `off` greys an option out.
pub(crate) fn seg(ui: &mut Ui, p: &Painter, r: Rect, id: &str, opts: &[&str], v: &mut usize, off: Option<usize>) {
    frame(p, r, BG, BORDER2);
    let w = r.width() / opts.len() as f32;
    let x = anim_value(ui.ctx(), Id::new((id, "seg")), *v as f32, 0.18);
    p.rect_filled(Rect::from_min_size(r.min + vec2(x * w, 0.0), vec2(w, r.height())).shrink(1.0), 0.0, TEXT);
    for (i, s) in opts.iter().enumerate() {
        let c = Rect::from_min_size(r.min + vec2(i as f32 * w, 0.0), vec2(w, r.height()));
        if i > 0 {
            p.vline(c.left(), c.y_range(), Stroke::new(1.0, BORDER2));
        }
        let resp = ui.interact(c, Id::new((id, i)), Sense::click());
        let dead = off == Some(i);
        let h = an(ui, (id, i, "h"), resp.hovered() && *v != i && !dead);
        p.rect_filled(c.shrink(1.0), 0.0, alpha(HOVER, h));
        if resp.clicked() && !dead {
            *v = i;
        }
        let on = (1.0 - (x - i as f32).abs()).clamp(0.0, 1.0);
        let col = if dead { Color32::from_gray(0x4A) } else { mix(mix(DIM, TEXT, h), BG, on) };
        lbl(p, c.center(), Align2::CENTER_CENTER, s, 8.0, col);
    }
}

/// A titled box; returns the area below the title.
fn sbox(p: &Painter, r: Rect, title: &str) -> Rect {
    frame(p, r, PANEL, BORDER);
    let t = Rect::from_min_size(r.min, vec2(r.width(), 30.0));
    p.hline(t.x_range(), t.bottom(), Stroke::new(1.0, BORDER));
    lbl(p, pos2(t.left() + 14.0, t.center().y), Align2::LEFT_CENTER, title, 8.0, TEXT);
    Rect::from_min_max(pos2(r.left(), t.bottom()), r.max)
}

/// Row: name + wrapped description on the left; returns (row rect, control rect on the right).
fn row(p: &Painter, x: f32, y: f32, w: f32, name: &str, desc: &str, cw: f32) -> (Rect, Rect) {
    lbl(p, pos2(x + 14.0, y + 12.0), Align2::LEFT_TOP, name, 9.0, TEXT);
    let dh = if desc.is_empty() { 0.0 } else { wrapped(p, pos2(x + 14.0, y + 30.0), desc, w - cw - 48.0, Color32::from_gray(0x6A)) };
    let h = (30.0 + dh + 10.0).max(48.0);
    let r = Rect::from_min_size(pos2(x, y), vec2(w, h));
    let c = Rect::from_center_size(pos2(r.right() - 14.0 - cw / 2.0, r.center().y), vec2(cw, 28.0));
    (r, c)
}

impl App {
    pub(crate) fn settings_modal(&mut self, ctx: &egui::Context, now: f64) {
        let k = smooth(self.cfg.k);
        let full = ctx.screen_rect();
        egui::Area::new(Id::new("settings_modal")).fixed_pos(full.min).order(egui::Order::Tooltip).show(ctx, |ui| {
            let (_, back) = ui.allocate_exact_size(full.size(), Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha((150.0 * k) as u8));
            ui.multiply_opacity(k);
            let size = vec2(940.0, 640.0).min(full.size() - vec2(64.0, 64.0));
            let r = Rect::from_center_size(full.center() + vec2(0.0, 18.0 * (1.0 - k)), size);
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 40);
            win_frame(&p, r, CARD, BORDER2);
            self.settings(ui, &p, r, now);
            let outside = back.clicked() && ui.ctx().input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
            if outside || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
                self.cfg.open = false;
            }
        });
    }

    fn settings(&mut self, ui: &mut Ui, p: &Painter, r: Rect, now: f64) {
        // navigation
        let nav = Rect::from_min_size(r.min + vec2(1.0, 1.0), vec2(220.0, r.height() - 2.0));
        p.rect_filled(nav, 0.0, PANEL);
        p.vline(nav.right(), nav.y_range(), Stroke::new(1.0, BORDER));
        txt(p, nav.min + vec2(16.0, 20.0), Align2::LEFT_TOP, "SETTINGS", F::Px, 18.0, TEXT, 3.0);
        lbl(p, nav.min + vec2(16.0, 46.0), Align2::LEFT_TOP, "shardstroke preferences", 7.0, FAINT);
        let ny = anim_value(ui.ctx(), Id::new("nav_y"), self.cfg.page as f32, 0.18);
        let item_h = 50.0;
        let top = nav.top() + 76.0;
        let act = Rect::from_min_size(pos2(nav.left(), top + ny * item_h), vec2(nav.width(), item_h));
        p.rect_filled(act, 0.0, SEL);
        p.rect_filled(Rect::from_min_size(act.min, vec2(2.0, item_h)), 0.0, acc());
        for (i, (name, sub)) in PAGES.iter().enumerate() {
            let ir = Rect::from_min_size(pos2(nav.left(), top + i as f32 * item_h), vec2(nav.width(), item_h));
            let resp = ui.interact(ir, Id::new(("nav", i)), Sense::click());
            let on = self.cfg.page == i;
            let h = an(ui, ("nav", i), resp.hovered() && !on);
            p.rect_filled(ir, 0.0, alpha(TEXT, h));
            let o = an(ui, ("nav_on", i), on);
            let ic = Rect::from_min_size(pos2(ir.left() + 16.0 + 4.0 * o, ir.top() + 13.0), vec2(16.0, 16.0));
            svg_icon(ui, ic, ["sliders-horizontal", "palette", "monitor", "books", "keyboard"][i], mix(mix(DIM, acc(), o), BG, h));
            lbl(p, pos2(ic.right() + 10.0, ir.top() + 14.0), Align2::LEFT_TOP, name, 9.0, mix(mix(SOFT, Color32::WHITE, o), BG, h));
            lbl(p, pos2(ic.right() + 10.0, ir.top() + 31.0), Align2::LEFT_TOP, sub, 7.0, mix(FAINT, BG, h));
            if resp.clicked() && !on {
                self.cfg.page = i;
                self.cfg.page_t = now;
            }
        }

        // close
        let x = Rect::from_min_size(pos2(r.right() - 30.0, r.top() + 10.0), vec2(18.0, 18.0));
        let xr = ui.interact(x, Id::new("set_close"), Sense::click());
        let xh = an(ui, "set_close", xr.hovered());
        frame(p, x, alpha(TEXT, xh), BORDER2);
        let s = Stroke::new(1.0, mix(DIM, BG, xh));
        p.line_segment([x.left_top() + vec2(5.0, 5.0), x.right_bottom() - vec2(5.0, 5.0)], s);
        p.line_segment([x.right_top() + vec2(-5.0, 5.0), x.left_bottom() + vec2(5.0, -5.0)], s);
        if xr.clicked() {
            self.cfg.open = false;
        }

        // page content fades and slides in
        let f = fade_in(now, self.cfg.page_t, 0.22);
        let mut cp = p.clone();
        cp.multiply_opacity(f);
        let area = Rect::from_min_max(pos2(nav.right() + 18.0 + 10.0 * (1.0 - f), r.top() + 40.0), r.max - vec2(18.0, 18.0));
        match self.cfg.page {
            0 => self.page_general(ui, &cp, area),
            1 => self.page_appearance(ui, &cp, area),
            2 => self.page_viewport(ui, &cp, area),
            3 => self.page_libraries(ui, &cp, area),
            _ => page_shortcuts(&cp, area),
        }
    }

    fn page_general(&mut self, ui: &mut Ui, p: &Painter, a: Rect) {
        let w = a.width();
        let b = sbox(p, Rect::from_min_size(a.min, vec2(w, 160.0)), "procedurals");
        let (r1, c) = row(p, b.left(), b.top(), w, "external procedural engine", "use the external engine for procedural resources when it is installed. off: the built-in engine only.", 140.0);
        seg(ui, p, c, "ext", &["on", "off"], &mut self.cfg.ext_engine, None);
        p.hline(r1.x_range(), r1.bottom(), Stroke::new(1.0, Color32::from_gray(0x1E)));
        let (_, c) = row(p, b.left(), r1.bottom(), w, "render cache", "1.4 gb on disk. rebuilt on demand.", 140.0);
        button(ui, p, c, "clear_cache", "clear cache", false);
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), a.top() + 172.0), vec2(w, 100.0)), "autosave");
        let (_, c) = row(p, b.left(), b.top(), w, "save a copy every", "kept next to the project; offered back when it is newer.", 360.0);
        seg(ui, p, c, "autosave", &["off", "1", "2", "5", "10 min"], &mut self.cfg.autosave, None);
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), a.top() + 284.0), vec2(w, 100.0)), "layout");
        let (_, c) = row(p, b.left(), b.top(), w, "lock layout", "hides the x of every window and stops moving, tabbing, splitting and resizing them.", 140.0);
        seg(ui, p, c, "lock", &["on", "off"], &mut self.cfg.lock, None);

    }

    fn page_appearance(&mut self, ui: &mut Ui, p: &Painter, a: Rect) {
        let w = a.width();
        let b = sbox(p, Rect::from_min_size(a.min, vec2(w, 110.0)), "highlight color");
        let (_, c) = row(p, b.left(), b.top(), w, "preset", "the only color in the ui: active, selected, primary and in progress.", 230.0);
        let cur = ACCENT.load(Ordering::Relaxed);
        for (i, v) in PRESETS.iter().enumerate() {
            let sw = Rect::from_min_size(pos2(c.left() + i as f32 * 38.0, c.top() - 2.0), vec2(32.0, 32.0));
            let resp = ui.interact(sw, Id::new(("preset", i)), Sense::click());
            let on = an(ui, ("preset", i), cur == *v);
            let h = an(ui, ("preset_h", i), resp.hovered());
            glow(p, sw, hex(*v), 0.5 * on, 6);
            frame(p, sw.shrink(3.0 * (1.0 - h)), hex(*v), mix(BORDER2, TEXT, on));
            if resp.clicked() {
                ACCENT.store(*v, Ordering::Relaxed);
            }
        }
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), a.top() + 122.0), vec2(w, 160.0)), "loading");
        let (r1, c) = row(p, b.left(), b.top(), w, "loading stripes", "drawn on a layer while it computes.", 120.0);
        let st = Rect::from_min_size(c.min, vec2(120.0, 28.0));
        let t = if heavy() { ui.input(|i| i.time) as f32 } else { 0.0 };
        let sp = p.with_clip_rect(st.intersect(p.clip_rect()));
        sp.rect_filled(st, 0.0, BG);
        let mut sx = st.left() - 28.0 + (t * 20.0).rem_euclid(14.0);
        while sx < st.right() {
            sp.add(Shape::convex_polygon(vec![pos2(sx, st.bottom()), pos2(sx + 7.0, st.bottom()), pos2(sx + 7.0 + 28.0, st.top()), pos2(sx + 28.0, st.top())], alpha(acc(), 0.55), Stroke::NONE));
            sx += 14.0;
        }
        p.rect_stroke(st, 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Inside);
        p.hline(r1.x_range(), r1.bottom(), Stroke::new(1.0, Color32::from_gray(0x1E)));
        let (_, c) = row(p, b.left(), r1.bottom(), w, "selection outline", "around the selected layer thumbnail.", 120.0);
        let o = Rect::from_min_size(c.min, vec2(28.0, 28.0));
        frame(p, o, Color32::from_gray(0x3E), TEXT);
        lbl(p, pos2(o.right() + 10.0, o.center().y), Align2::LEFT_CENTER, "#e2e2e2", 8.0, DIM);
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), a.top() + 294.0), vec2(w, 124.0)), "animation");
        let (_, c) = row(p, b.left(), b.top(), w, "animation level", "basic: fades, glows and sliding highlights. advanced: also the rolling start text, ascii rain and dissolve, moving stripes.", 300.0);
        seg(ui, p, c, "anim", &["off", "basic", "advanced"], &mut self.cfg.anim, None);
        let rr = Rect::from_min_size(pos2(a.right() - 90.0, a.top() + 432.0), vec2(90.0, 28.0));
        if button(ui, p, rr, "reset_app", "reset", false) {
            ACCENT.store(PRESETS[0], Ordering::Relaxed);
            self.cfg.anim = 2;
        }
    }

    fn page_viewport(&mut self, ui: &mut Ui, p: &Painter, a: Rect) {
        let w = a.width();
        let b = sbox(p, Rect::from_min_size(a.min, vec2(w, 92.0)), "antialiasing");
        let (_, c) = row(p, b.left(), b.top(), w, "multisampling", "smooths the edges of the mesh. higher is smoother and costs more gpu time.", 230.0);
        seg(ui, p, c, "aa", &["off", "2x", "4x", "8x"], &mut self.cfg.aa, Some(3));
        let y0 = a.top() + 104.0;
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), y0), vec2(w, 280.0)), "render");
        let mut y = b.top();
        let line = |p: &Painter, r: Rect| p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, Color32::from_gray(0x1E)));
        let (r1, c) = row(p, b.left(), y, w, "mode", "", 230.0);
        seg(ui, p, c, "render", &["real-time", "path tracing"], &mut self.cfg.render, None);
        line(p, r1);
        y = r1.bottom();
        let (r2, c) = row(p, b.left(), y, w, "samples", "one sample per pixel each frame while the view is still, up to 4096. moving the camera or editing starts again.", 230.0);
        let frac = 512.0 / 4096.0;
        let bar = Rect::from_min_size(pos2(c.left(), c.center().y - 5.0), vec2(c.width() - 80.0, 10.0));
        blocks(p, bar, frac, if self.cfg.render == 1 { acc() } else { TEXT });
        txt(p, pos2(c.right(), c.center().y), Align2::RIGHT_CENTER, "512 / 4096", F::Mono, 11.0, TEXT, 0.0);
        line(p, r2);
        y = r2.bottom();
        let (r3, c) = row(p, b.left(), y, w, "denoise while moving", "a fast filter cleans the picture on every frame, so it never shows grain.", 230.0);
        lbl(p, pos2(c.right(), c.center().y), Align2::RIGHT_CENTER, "always on", 8.0, DIM);
        line(p, r3);
        y = r3.bottom();
        let (_, c) = row(p, b.left(), y, w, "ai denoise when still", "runs once the view stops.", 230.0);
        seg(ui, p, Rect::from_min_size(pos2(c.right() - 92.0, c.top()), vec2(92.0, 28.0)), "ai", &["on", "off"], &mut self.cfg.ai_denoise, None);
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), y0 + 292.0), vec2(w, 92.0)), "statistics");
        let (_, c) = row(p, b.left(), b.top(), w, "show statistics", "frame time, fps and cpu / gpu usage in the viewport.", 230.0);
        seg(ui, p, Rect::from_min_size(pos2(c.right() - 92.0, c.top()), vec2(92.0, 28.0)), "stats", &["on", "off"], &mut self.cfg.stats, None);
    }

    fn page_libraries(&mut self, ui: &mut Ui, p: &Painter, a: Rect) {
        let w = a.width();
        let n = self.cfg.folders.len();
        let b = sbox(p, Rect::from_min_size(a.min, vec2(w, 30.0 + n as f32 * 44.0 + 52.0)), "your folders");
        let mut remove = None;
        let mut make_default = None;
        for (i, (path, def)) in self.cfg.folders.iter().enumerate() {
            let r = Rect::from_min_size(pos2(b.left(), b.top() + i as f32 * 44.0), vec2(w, 44.0));
            let h = an(ui, ("fold", i), ui.rect_contains_pointer(r));
            p.rect_filled(r.shrink(1.0), 0.0, alpha(HOVER, h));
            txt(p, pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, path, F::Mono, 11.0, TEXT, 0.0);
            let rm = Rect::from_min_size(pos2(r.right() - 90.0, r.center().y - 12.0), vec2(76.0, 24.0));
            if button(ui, p, rm, &format!("rm{i}"), "remove", false) {
                remove = Some(i);
            }
            let d = Rect::from_min_size(pos2(rm.left() - 112.0, rm.top()), vec2(104.0, 24.0));
            if *def {
                let tr = lbl(p, d.center(), Align2::CENTER_CENTER, "default", 7.0, acc());
                p.rect_stroke(tr.expand2(vec2(8.0, 6.0)), 0.0, Stroke::new(1.0, acc()), StrokeKind::Outside);
            } else if button(ui, p, d, &format!("def{i}"), "set default", false) {
                make_default = Some(i);
            }
            p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, Color32::from_gray(0x1E)));
        }
        if let Some(i) = make_default {
            for (j, f) in self.cfg.folders.iter_mut().enumerate() {
                f.1 = j == i;
            }
        }
        if let Some(i) = remove {
            self.cfg.folders.remove(i);
        }
        let by = b.top() + n as f32 * 44.0 + 12.0;
        if button(ui, p, Rect::from_min_size(pos2(b.left() + 14.0, by), vec2(110.0, 28.0)), "addf", "add folder...", false) && self.cfg.folders.len() < 5 {
            self.cfg.folders.push(("E:\\shared\\library", false));
        }
        button(ui, p, Rect::from_min_size(pos2(b.left() + 132.0, by), vec2(86.0, 28.0)), "rescan", "rescan", false);
        let y = b.bottom() + 12.0;
        let b = sbox(p, Rect::from_min_size(pos2(a.left(), y), vec2(w, 30.0 + 2.0 * 44.0)), "found on this computer");
        for (i, (path, n)) in [("C:\\ProgramData\\materials", "312 assets"), ("D:\\downloads\\brush packs", "48 assets")].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(b.left(), b.top() + i as f32 * 44.0), vec2(w, 44.0));
            txt(p, pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, path, F::Mono, 11.0, SOFT, 0.0);
            lbl(p, pos2(r.right() - 110.0, r.center().y), Align2::RIGHT_CENTER, n, 7.0, FAINT);
            button(ui, p, Rect::from_min_size(pos2(r.right() - 90.0, r.center().y - 12.0), vec2(76.0, 24.0)), &format!("found{i}"), "add", false);
        }
    }
}

/// Read-only: the industry-standard keys the app follows (rebinding is not there yet).
fn page_shortcuts(p: &Painter, a: Rect) {
    let groups: [(&str, &[(&str, &str)]); 4] = [
        ("general", &[("new", "ctrl+n"), ("open", "ctrl+o"), ("save", "ctrl+s"), ("save as", "ctrl+shift+s"), ("undo", "ctrl+z"), ("redo", "ctrl+y"), ("export textures", "ctrl+shift+e"), ("hide ui", "tab")]),
        ("brush", &[("increase size", "]"), ("decrease size", "["), ("color picker", "i"), ("lazy mouse", "d")]),
        ("tools", &[("paint", "1"), ("eraser", "2"), ("projection", "3"), ("polygon fill", "4"), ("smudge", "5"), ("clone", "6"), ("pick material", "p")]),
        ("viewport", &[("perspective", "f5"), ("orthographic", "f6"), ("next channel", "c"), ("material", "m"), ("frame mesh", "f")]),
    ];
    let cw = (a.width() - 14.0) / 2.0;
    let mut ys = [a.top(), a.top()];
    for (gi, (title, rows)) in groups.iter().enumerate() {
        let col = if gi % 2 == 0 { 0 } else { 1 };
        let x = a.left() + col as f32 * (cw + 14.0);
        let h = 30.0 + rows.len() as f32 * 26.0 + 6.0;
        let b = sbox(p, Rect::from_min_size(pos2(x, ys[col]), vec2(cw, h)), title);
        for (i, (name, key)) in rows.iter().enumerate() {
            let y = b.top() + 16.0 + i as f32 * 26.0;
            lbl(p, pos2(x + 14.0, y), Align2::LEFT_CENTER, name, 8.0, SOFT);
            let kr = lbl(p, pos2(x + cw - 16.0, y), Align2::RIGHT_CENTER, key, 8.0, TEXT);
            p.rect_stroke(kr.expand2(vec2(6.0, 4.0)), 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Outside);
        }
        ys[col] += h + 12.0;
    }
    let y = ys[0].max(ys[1]);
    let tr = lbl(p, pos2(a.left(), y + 4.0), Align2::LEFT_TOP, "rebinding keys · not yet", 7.0, FAINT);
    p.rect_stroke(tr.expand2(vec2(6.0, 4.0)), 0.0, Stroke::new(1.0, Color32::from_gray(0x4A)), StrokeKind::Outside);
}
