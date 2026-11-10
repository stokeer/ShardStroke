//! The viewport side of the Elements page: controls on the brush toolbar (04), the shading popover with the
//! environment picker (05) and post effects (06), display + shader settings panels (14), the resource picker
//! (10) and the right-click quick panel (19). The environment, exposure, rotation and background drive the
//! viewport backdrop so the controls visibly do something.

use super::*;

/// (name, sky, ground)
pub const ENVS: [(&str, [u8; 3], [u8; 3]); 11] = [
    ("industrial_hall_02", [0x3A, 0x3A, 0x40], [0x14, 0x13, 0x12]),
    ("meadow_noon", [0x6C, 0x8C, 0xB4], [0x2A, 0x34, 0x1E]),
    ("night_street", [0x14, 0x18, 0x2C], [0x0C, 0x0A, 0x0A]),
    ("studio_soft", [0x5A, 0x5A, 0x5A], [0x26, 0x26, 0x26]),
    ("overcast_field", [0x70, 0x74, 0x78], [0x2E, 0x30, 0x28]),
    ("sunset_dunes", [0x9A, 0x5A, 0x3A], [0x3A, 0x26, 0x1A]),
    ("forest_shade", [0x2E, 0x3E, 0x2A], [0x12, 0x18, 0x10]),
    ("white_cyclorama", [0x9A, 0x9A, 0x9A], [0x6A, 0x6A, 0x6A]),
    ("parking_garage", [0x30, 0x32, 0x2E], [0x16, 0x16, 0x14]),
    ("harbor_dawn", [0x5A, 0x64, 0x84], [0x1C, 0x22, 0x2C]),
    ("workshop_warm", [0x5A, 0x44, 0x30], [0x1E, 0x16, 0x10]),
];

const VIEWS: [&str; 6] = ["material", "base color", "roughness", "metallic", "normal", "height"];

/// (name, kind)
const RESOURCES: [(&str, &str); 10] = [
    ("Grunge Map 004", "procedural"),
    ("Grunge Map 012", "procedural"),
    ("Grunge Rust Fine", "procedural"),
    ("Grunge Scratches", "procedural"),
    ("Grunge Spots", "procedural"),
    ("grunge_leaks.png", "texture"),
    ("grunge_streaks.png", "texture"),
    ("grunge_dirt.tga", "texture · png only"),
    ("Grunge Brush", "alpha"),
    ("Grunge Splat", "alpha"),
];
const ANCHORS: [&str; 3] = ["Edge wear mask", "Rust grunge height", "Base paint color"];

pub struct View {
    pub pt: usize,
    pt_t0: f64,
    pub pop: bool,
    pop_k: f32,
    pop_anchor: Rect,
    tab: usize,
    pub env: usize,
    env_open: bool,
    env_find: String,
    pub rot: f32,
    pub exp: f32,
    pub bg: usize,
    pub aa: usize,
    pub ao: f32,
    cc: bool,
    cc_v: [f32; 6],
    bloom: bool,
    bloom_v: [f32; 3],
    vignette: bool,
    grain: bool,
    fov: f32,
    emissive: f32,
    pub show: usize,
    quick: Option<Pos2>,
    quick_k: f32,
    eraser: bool,
    color: Color32,
    picker: bool,
    picker_k: f32,
    picker_tab: usize,
    picker_grid: bool,
    picker_sel: usize,
    picker_find: String,
    pub rough_src: &'static str,
    pub hide_ui: bool,
    /// Quick panel sections, top to bottom: colour, brush, recent brushes, recent colours, project colours.
    qorder: [usize; 5],
    arrange: bool,
}

impl Default for View {
    fn default() -> Self {
        View {
            pt: 0,
            pt_t0: 0.0,
            pop: false,
            pop_k: 0.0,
            pop_anchor: Rect::NOTHING,
            tab: 0,
            env: 0,
            env_open: false,
            env_find: String::new(),
            rot: 84.0,
            exp: 0.0,
            bg: 0,
            aa: 2,
            ao: 0.6,
            cc: true,
            cc_v: [0.0, 1.1, 1.0, 0.0, 0.0, 0.0],
            bloom: true,
            bloom_v: [2.0, 1.0, 0.5],
            vignette: false,
            grain: false,
            fov: 45.0,
            emissive: 1.0,
            show: 0,
            quick: None,
            quick_k: 0.0,
            eraser: false,
            color: Color32::from_rgb(0xE6, 0x26, 0x19),
            picker: false,
            picker_k: 0.0,
            picker_tab: 0,
            picker_grid: false,
            picker_sel: 0,
            picker_find: String::new(),
            rough_src: "Grunge Map 004",
            hide_ui: false,
            qorder: [0, 1, 2, 3, 4],
            arrange: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Sphere {
    Raster,
    PathTrace,
}

/// The app's render-mode sphere: concentric discs lit from the upper left; the path traced one is glossy.
fn paint_sphere(p: &Painter, c: Pos2, r: f32, kind: Sphere) {
    let (dark, lit) = match kind {
        Sphere::Raster => (58.0, 205.0),
        Sphere::PathTrace => (26.0, 150.0),
    };
    let fade = |v: f32| Color32::from_gray(v.clamp(0.0, 255.0) as u8);
    const STEPS: usize = 14;
    for i in 0..STEPS {
        let t = i as f32 / STEPS as f32;
        p.circle_filled(c + vec2(-0.32, -0.34) * r * t, r * (1.0 - t * 0.82), fade(dark + (lit - dark) * t.powf(1.4)));
    }
    if kind == Sphere::PathTrace {
        let band: Vec<Pos2> = (0..=12)
            .map(|k| {
                let a = std::f32::consts::PI * (0.12 + 0.76 * k as f32 / 12.0);
                c + vec2(-a.cos() * r * 0.86, a.sin() * r * 0.36)
            })
            .collect();
        p.add(Shape::line(band, Stroke::new(r * 0.16, fade(118.0))));
        p.circle_stroke(c, r - 0.6, Stroke::new(1.0, fade(110.0)));
        p.circle_filled(c + vec2(-0.36, -0.38) * r, r * 0.17, fade(255.0));
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Block slider over any range, with its own value format.
fn vslider(ui: &mut Ui, p: &Painter, r: Rect, id: &str, label: &str, v: &mut f32, lo: f32, hi: f32, fmt: &dyn Fn(f32) -> String) {
    let mut t = (*v - lo) / (hi - lo);
    let resp = ui.interact(r, Id::new(id), Sense::click_and_drag());
    let h = an(ui, (id, "h"), resp.hovered() || resp.dragged());
    let d = an(ui, (id, "d"), resp.dragged());
    p.rect_filled(r, 0.0, alpha(HOVER, h));
    let lw = (label.len() as f32 * 9.0 + 16.0).max(r.width() * 0.32);
    let bar = Rect::from_min_max(pos2(r.left() + lw, r.center().y - 5.0), pos2(r.right() - 50.0, r.center().y + 5.0));
    if let Some(q) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            t = ((q.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            *v = lo + t * (hi - lo);
        }
    }
    let shown = anim_value(ui.ctx(), Id::new((id, "v")), t, if resp.dragged() { 0.0 } else { 0.12 });
    lbl(p, pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, label, 8.0, mix(DIM, TEXT, h));
    blocks(p, bar, shown, mix(TEXT, acc(), d));
    txt(p, pos2(r.right() - 6.0, r.center().y), Align2::RIGHT_CENTER, &fmt(*v), F::Mono, 11.0, TEXT, 0.0);
}

/// Accent check box with a label; returns the row rect.
fn check(ui: &mut Ui, p: &Painter, r: Rect, id: &str, label: &str, note: &str, on: &mut bool) {
    let resp = ui.interact(r, Id::new(id), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let h = an(ui, (id, "h"), resp.hovered());
    let o = an(ui, (id, "o"), *on);
    p.rect_filled(r, 0.0, alpha(HOVER, h));
    let cb = Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(12.0, 12.0));
    glow(p, cb, acc(), 0.5 * o, 4);
    p.rect_filled(cb, 0.0, alpha(acc(), o));
    p.rect_stroke(cb, 0.0, Stroke::new(1.0, mix(Color32::from_gray(0x6A), acc(), o)), StrokeKind::Inside);
    let lr = txt(p, pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, label, F::Ui, 12.0, mix(SOFT, Color32::WHITE, o), 0.0);
    if !note.is_empty() {
        lbl(p, pos2(lr.right() + 10.0, r.center().y), Align2::LEFT_CENTER, note, 6.0, FAINT);
    }
}

fn section(p: &Painter, x: f32, y: f32, w: f32, s: &str) {
    lbl(p, pos2(x + 8.0, y + 10.0), Align2::LEFT_CENTER, s, 7.0, FAINT);
    p.hline((x + 8.0 + s.len() as f32 * 10.0)..=(x + w - 8.0), y + 10.0, Stroke::new(1.0, Color32::from_gray(0x24)));
}

fn env_thumb(p: &Painter, r: Rect, e: usize) {
    let (_, sky, ground) = ENVS[e];
    vgrad(p, Rect::from_min_max(r.min, pos2(r.right(), r.center().y)), rgb(sky), mix(rgb(sky), rgb(ground), 0.6));
    p.rect_filled(Rect::from_min_max(pos2(r.left(), r.center().y), r.max), 0.0, rgb(ground));
}

fn res_thumb(p: &Painter, r: Rect, seed: usize) {
    let cells = 8;
    let (cw, ch) = (r.width() / cells as f32, r.height() / cells as f32);
    for j in 0..cells {
        for i in 0..cells {
            let v = ((i * 7 + j * 13 + seed * 31) % 17) as f32 / 17.0;
            let g = (30.0 + v * (90.0 + seed as f32 * 8.0)).min(200.0) as u8;
            p.rect_filled(Rect::from_min_size(pos2(r.left() + i as f32 * cw, r.top() + j as f32 * ch), vec2(cw + 0.5, ch + 0.5)), 0.0, Color32::from_gray(g));
        }
    }
}

/// Opens a floating box under `anchor` on the Foreground layer; returns its rect.
fn popup(ctx: &egui::Context, id: &str, r: Rect, k: f32, body: impl FnOnce(&mut Ui, &Painter)) {
    egui::Area::new(Id::new(id)).fixed_pos(r.min).order(egui::Order::Foreground).show(ctx, |ui| {
        ui.multiply_opacity(k);
        ui.allocate_exact_size(r.size(), Sense::hover());
        let p = ui.painter().clone();
        glow(&p, r, Color32::BLACK, 0.6, 20);
        win_frame(&p, r, CARD, BORDER2);
        body(ui, &p);
    });
}

impl App {
    /// Viewport backdrop: the environment's sky / ground, turned by rotation, brightened by exposure.
    pub(crate) fn viewport_bg(&self, ui: &Ui, p: &Painter, view: Rect) {
        let v = &self.view;
        if v.bg == 1 {
            p.rect_filled(view, 0.0, Color32::from_gray(0x2A));
            return;
        }
        let (_, sky, ground) = ENVS[v.env];
        let k = 0.35 * (2f32).powf(v.exp);
        let dim = |c: [u8; 3]| Color32::from_rgb((c[0] as f32 * k).min(255.0) as u8, (c[1] as f32 * k).min(255.0) as u8, (c[2] as f32 * k).min(255.0) as u8);
        let horizon = view.top() + view.height() * (0.45 + 0.1 * (v.rot.to_radians()).sin());
        let ctx = ui.ctx();
        let hz = anim_value(&ctx, Id::new("horizon"), horizon, 0.3);
        vgrad(p, Rect::from_min_max(view.min, pos2(view.right(), hz)), dim(sky), mix(dim(sky), dim(ground), 0.5));
        vgrad(p, Rect::from_min_max(pos2(view.left(), hz), view.max), mix(dim(sky), dim(ground), 0.5), dim(ground));
        if v.vignette {
            for i in 0..12 {
                p.rect_stroke(view.shrink(i as f32 * 6.0), 0.0, Stroke::new(6.0, Color32::from_black_alpha((60 - i * 5) as u8)), StrokeKind::Inside);
            }
        }
    }

    /// Statistics overlay in the viewport corner.
    pub(crate) fn viewport_stats(&self, ui: &Ui, p: &Painter, view: Rect) {
        let mut y = view.bottom() - 8.0;
        let now = ui.input(|i| i.time);
        if self.view.pt == 1 {
            let n = (((now - self.view.pt_t0) * 180.0) as u32).min(4096);
            lbl(p, pos2(view.left() + 8.0, y), Align2::LEFT_BOTTOM, &format!("path tracing {n} / 4096 samples"), 7.0, acc());
            y -= 14.0;
            if n >= 256 {
                lbl(p, pos2(view.left() + 8.0, y), Align2::LEFT_BOTTOM, "ai denoised at 256", 7.0, FAINT);
                y -= 14.0;
            }
        }
        if self.cfg.stats == 0 {
            lbl(p, pos2(view.left() + 8.0, y), Align2::LEFT_BOTTOM, "cpu 3.1 ms · gpu 8.42 ms", 7.0, FAINT);
            lbl(p, pos2(view.left() + 8.0, y - 14.0), Align2::LEFT_BOTTOM, "61 fps · 16.4 ms", 7.0, FAINT);
        }
        if self.view.show > 0 {
            lbl(p, pos2(view.center().x, view.top() + 50.0), Align2::CENTER_TOP, &format!("showing: {}", VIEWS[self.view.show]), 8.0, TEXT);
        }
    }

    /// Plain right click on the free viewport opens the quick panel there.
    pub(crate) fn viewport_input(&mut self, ui: &mut Ui, view: Rect) {
        let resp = ui.interact(view, Id::new("viewport"), Sense::click());
        if resp.secondary_clicked() {
            self.view.quick = resp.interact_pointer_pos();
            self.view.quick_k = 0.0;
        }
    }

    /// Right end of the brush toolbar: render switch, shading popover, channel shown. Returns the width used.
    pub(crate) fn viewport_controls(&mut self, ui: &mut Ui, p: &Painter, bar: Rect) -> f32 {
        let now = ui.input(|i| i.time);
        let w = bar.width();
        let r = bar.shrink(4.0);
        // real-time / path tracing: the app's two shaded spheres
        for (i, kind) in [Sphere::Raster, Sphere::PathTrace].into_iter().enumerate() {
            let b = Rect::from_min_size(pos2(r.left() + i as f32 * 30.0, r.top()), vec2(26.0, 26.0));
            let resp = ui.interact(b, Id::new(("vp_sphere", i)), Sense::click());
            let on = an(ui, ("vp_sphere_on", i), self.view.pt == i);
            let h = an(ui, ("vp_sphere_h", i), resp.hovered());
            p.rect_filled(b, 0.0, mix(alpha(HOVER, h), alpha(acc(), 0.25), on));
            p.rect_stroke(b, 0.0, Stroke::new(1.0, alpha(acc(), on)), StrokeKind::Inside);
            paint_sphere(p, b.center(), 26.0 * 0.32, kind);
            if resp.clicked() && self.view.pt != i {
                self.view.pt = i;
                self.view.pt_t0 = now;
            }
        }
        // the chevron opens the shading popover
        let sb = Rect::from_min_size(pos2(r.left() + 60.0, r.top()), vec2(18.0, 26.0));
        let resp = ui.interact(sb, Id::new("vp_shade"), Sense::click());
        let h = an(ui, "vp_shade", resp.hovered() || self.view.pop);
        p.rect_filled(sb, 0.0, mix(Color32::TRANSPARENT, alpha(acc(), 0.25), h));
        icons::draw(p, sb.shrink2(vec2(2.0, 6.0)), icons::Icon::ChevronDown, mix(TEXT, acc(), h));
        if resp.clicked() {
            self.view.pop = !self.view.pop;
            self.view.pop_k = 0.0;
            self.view.pop_anchor = sb;
        }
        // post effects quick toggle
        p.vline(sb.right() + 6.0, r.y_range(), Stroke::new(1.0, BORDER));
        let pb = Rect::from_min_size(pos2(sb.right() + 12.0, r.top()), vec2(26.0, 26.0));
        let presp = ui.interact(pb, Id::new("vp_post"), Sense::click());
        let on = self.view.cc || self.view.bloom;
        let ph = an(ui, "vp_post", presp.hovered());
        let po = an(ui, "vp_post_on", on);
        p.rect_filled(pb, 0.0, mix(alpha(HOVER, ph), alpha(acc(), 0.25), po));
        p.rect_stroke(pb, 0.0, Stroke::new(1.0, alpha(acc(), po)), StrokeKind::Inside);
        icons::draw(p, pb.shrink(4.0), icons::Icon::PostFx, mix(DIM, TEXT, po.max(ph)));
        if presp.clicked() {
            let v = !on;
            self.view.cc = v;
            self.view.bloom = v;
        }
        // channel shown (C / M)
        let cr = Rect::from_min_max(pos2(pb.right() + 6.0, r.top()), pos2(r.right(), r.top() + 26.0));
        let cresp = ui.interact(cr, Id::new("vp_view"), Sense::click());
        let chh = an(ui, "vp_view", cresp.hovered());
        frame(p, cr, mix(BG, TEXT, chh), mix(BORDER2, TEXT, chh));
        lbl(p, pos2(cr.left() + 8.0, cr.center().y), Align2::LEFT_CENTER, VIEWS[self.view.show], 8.0, mix(TEXT, BG, chh));
        txt(p, pos2(cr.right() - 8.0, cr.center().y), Align2::RIGHT_CENTER, "▾", F::Mono, 10.0, mix(DIM, BG, chh), 0.0);
        if cresp.clicked() {
            self.view.show = (self.view.show + 1) % VIEWS.len();
        }
        w
    }

    /// Industry-standard keys of the viewport and modes (reference 12.19).
    pub(crate) fn viewport_keys(&mut self, ctx: &egui::Context, now: f64) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let (tab, c, shift, m, f9, f10) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Tab),
                i.key_pressed(egui::Key::C),
                i.modifiers.shift,
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::F9),
                i.key_pressed(egui::Key::F10),
            )
        });
        if tab {
            self.view.hide_ui = !self.view.hide_ui;
        }
        let tool_key = ctx.input(|i| {
            if i.modifiers.ctrl {
                return None;
            }
            [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::P]
                .iter()
                .position(|k| i.key_pressed(*k))
        });
        if let (Some(t), Mode::Texture) = (tool_key, self.mode) {
            if self.tool != t {
                self.tool = t;
                self.tool_t = now;
            }
        }
        if c {
            let n = VIEWS.len() - 1;
            self.view.show = if shift { (self.view.show + n - 2) % n + 1 } else { self.view.show % n + 1 };
        }
        if m {
            self.view.show = 0;
        }
        if f9 {
            self.set_mode(Mode::Texture, now);
        }
        if f10 {
            self.set_mode(Mode::Texture, now);
            if self.view.pt == 0 {
                self.view.pt = 1;
                self.view.pt_t0 = now;
            }
        }
    }

    /// All the floating pieces of the viewport, drawn on top.
    pub(crate) fn viewport_overlays(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        if self.view.pop {
            approach(&mut self.view.pop_k, true, dt, 0.14);
            self.shading_popover(ctx);
        }
        if self.view.quick.is_some() {
            approach(&mut self.view.quick_k, true, dt, 0.12);
            self.quick_panel(ctx);
        }
        approach(&mut self.view.picker_k, self.view.picker, dt, 0.2);
        if self.view.picker_k > 0.0 {
            self.resource_picker(ctx);
        }
    }

    fn shading_popover(&mut self, ctx: &egui::Context) {
        let k = smooth(self.view.pop_k);
        let a = self.view.pop_anchor;
        let w = 360.0;
        let r = Rect::from_min_size(pos2(a.right() - w, a.bottom() + 6.0 - 6.0 * (1.0 - k)), vec2(w, 560.0));
        let env_r = Rect::from_min_size(pos2(r.left() - 256.0, r.top()), vec2(250.0, 420.0));
        popup(ctx, "shading_pop", r, k, |ui, p| {
            // tabs
            for (i, s) in ["viewport shading", "post effects"].into_iter().enumerate() {
                let tr = Rect::from_min_size(pos2(r.left() + 1.0 + i as f32 * (w / 2.0 - 1.0), r.top() + 1.0), vec2(w / 2.0 - 1.0, 30.0));
                let resp = ui.interact(tr, Id::new(("pop_tab", i)), Sense::click());
                if resp.clicked() {
                    self.view.tab = i;
                }
                let on = an(ui, ("pop_tab", i), self.view.tab == i);
                let h = an(ui, ("pop_tab_h", i), resp.hovered());
                p.rect_filled(tr, 0.0, mix(alpha(HOVER, h), PANEL, on));
                p.rect_filled(Rect::from_min_max(pos2(tr.left(), tr.bottom() - 2.0), tr.max), 0.0, alpha(acc(), on));
                lbl(p, tr.center(), Align2::CENTER_CENTER, s, 8.0, mix(DIM, TEXT, on));
            }
            let x = r.left();
            let mut y = r.top() + 40.0;
            let row = |y: f32| Rect::from_min_size(pos2(x + 2.0, y), vec2(w - 4.0, 26.0));
            if self.view.tab == 0 {
                section(p, x, y, w, "render");
                y += 22.0;
                let before = self.view.pt;
                settings::seg(ui, p, Rect::from_min_size(pos2(x + 10.0, y), vec2(w - 20.0, 26.0)), "pop_rt", &["real-time", "path tracing"], &mut self.view.pt, None);
                if before != self.view.pt {
                    self.view.pt_t0 = ui.input(|i| i.time);
                }
                y += 36.0;
                section(p, x, y, w, "environment");
                y += 22.0;
                let er = Rect::from_min_size(pos2(x + 10.0, y), vec2(w - 20.0, 40.0));
                let eresp = ui.interact(er, Id::new("env_banner"), Sense::click());
                let eh = an(ui, "env_banner", eresp.hovered() || self.view.env_open);
                env_thumb(p, er, self.view.env);
                p.rect_stroke(er, 0.0, Stroke::new(1.0 + eh, mix(BORDER2, acc(), eh)), StrokeKind::Inside);
                lbl(p, pos2(er.left() + 10.0, er.center().y), Align2::LEFT_CENTER, &format!("{} ▸", ENVS[self.view.env].0), 8.0, Color32::WHITE);
                if eresp.clicked() {
                    self.view.env_open = !self.view.env_open;
                }
                y += 46.0;
                vslider(ui, p, row(y), "rot", "rotation", &mut self.view.rot, 0.0, 360.0, &|v| format!("{v:.0}°"));
                y += 28.0;
                vslider(ui, p, row(y), "exp", "exposure ev", &mut self.view.exp, -3.0, 3.0, &|v| format!("{v:+.2}"));
                y += 32.0;
                lbl(p, pos2(x + 14.0, y + 13.0), Align2::LEFT_CENTER, "background", 7.0, DIM);
                settings::seg(ui, p, Rect::from_min_size(pos2(x + w * 0.45, y), vec2(w * 0.55 - 10.0, 26.0)), "pop_bg", &["env", "color"], &mut self.view.bg, None);
                y += 32.0;
                lbl(p, pos2(x + 14.0, y + 13.0), Align2::LEFT_CENTER, "profile", 7.0, DIM);
                let pr = Rect::from_min_size(pos2(x + w * 0.45, y), vec2(w * 0.55 - 10.0, 26.0));
                frame(p, pr, BG, BORDER2);
                txt(p, pos2(pr.left() + 8.0, pr.center().y), Align2::LEFT_CENTER, "ACES - sRGB", F::Mono, 10.0, TEXT, 0.0);
                y += 36.0;
                section(p, x, y, w, "antialiasing");
                y += 22.0;
                settings::seg(ui, p, Rect::from_min_size(pos2(x + 10.0, y), vec2(w - 20.0, 26.0)), "pop_aa", &["off", "2x", "4x", "8x"], &mut self.view.aa, Some(3));
                y += 36.0;
                section(p, x, y, w, "shading");
                y += 22.0;
                vslider(ui, p, row(y), "ao", "ao intensity", &mut self.view.ao, 0.0, 1.0, &|v| format!("{v:.2}"));
            } else {
                section(p, x, y, w, "tone & color");
                y += 22.0;
                lbl(p, pos2(x + 14.0, y + 13.0), Align2::LEFT_CENTER, "tone mapping", 7.0, DIM);
                lbl(p, pos2(r.right() - 14.0, y + 13.0), Align2::RIGHT_CENTER, "aces filmic", 7.0, TEXT);
                y += 28.0;
                check(ui, p, row(y), "cc", "Color correction", "", &mut self.view.cc);
                y += 28.0;
                if self.view.cc {
                    let names = ["brightness", "contrast", "gamma", "saturation"];
                    let ranges = [(-1.0, 1.0), (0.0, 2.0), (0.2, 3.0), (-1.0, 1.0)];
                    for (i, n) in names.iter().enumerate() {
                        vslider(ui, p, row(y), &format!("cc{i}"), n, &mut self.view.cc_v[i], ranges[i].0, ranges[i].1, &|v| format!("{v:.2}"));
                        y += 26.0;
                    }
                }
                y += 6.0;
                section(p, x, y, w, "light & lens");
                y += 22.0;
                check(ui, p, row(y), "bloom", "Bloom", "glow of bright areas", &mut self.view.bloom);
                y += 28.0;
                if self.view.bloom {
                    for (i, n) in ["strength", "threshold", "falloff"].iter().enumerate() {
                        let hi = if i == 2 { 1.0 } else { 4.0 };
                        vslider(ui, p, row(y), &format!("bl{i}"), n, &mut self.view.bloom_v[i], 0.0, hi, &|v| format!("{v:.2}"));
                        y += 26.0;
                    }
                }
                check(ui, p, row(y), "vig", "Vignette", "", &mut self.view.vignette);
                y += 28.0;
                section(p, x, y, w, "film");
                y += 22.0;
                check(ui, p, row(y), "grain", "Film grain", "", &mut self.view.grain);
                y += 32.0;
                section(p, x, y, w, "not rendered yet [06]");
                y += 20.0;
                for s in ["glare", "sharpen · on in this project", "depth of field", "chromatic aberration", "lens flare", "color grading (lut)"] {
                    if y + 16.0 > r.bottom() {
                        break;
                    }
                    lbl(p, pos2(x + 14.0, y + 6.0), Align2::LEFT_CENTER, s, 7.0, Color32::from_gray(0x55));
                    y += 16.0;
                }
            }
        });
        // environment picker beside the popover
        let ek = an_ctx(ctx, "env_open", self.view.env_open && self.view.tab == 0);
        if ek > 0.01 {
            let er = env_r.translate(vec2(10.0 * (1.0 - ek), 0.0));
            popup(ctx, "env_pick", er, k * ek, |ui, p| {
                lbl(p, pos2(er.left() + 12.0, er.top() + 16.0), Align2::LEFT_CENTER, &format!("environment [{}]", ENVS.len()), 8.0, TEXT);
                let fr = Rect::from_min_size(pos2(er.left() + 10.0, er.top() + 30.0), vec2(er.width() - 20.0, 24.0));
                frame(p, fr, BG, BORDER2);
                ui.put(fr.shrink2(vec2(8.0, 3.0)), egui::TextEdit::singleline(&mut self.view.env_find).frame(false).font(fid(F::Px, 10.0)).text_color(TEXT).hint_text("FIND"));
                let q = self.view.env_find.to_lowercase();
                let mut y = fr.bottom() + 6.0;
                for (i, (name, _, _)) in ENVS.iter().enumerate() {
                    if !name.contains(&q) || y + 30.0 > er.bottom() - 18.0 {
                        continue;
                    }
                    let row = Rect::from_min_size(pos2(er.left() + 1.0, y), vec2(er.width() - 2.0, 30.0));
                    y += 30.0;
                    let resp = ui.interact(row, Id::new(("env", i)), Sense::click());
                    let h = an(ui, ("env_h", i), resp.hovered());
                    let on = an(ui, ("env_on", i), self.view.env == i);
                    p.rect_filled(row, 0.0, mix(alpha(HOVER, h), SEL, on));
                    env_thumb(p, Rect::from_center_size(pos2(row.left() + 26.0, row.center().y), vec2(36.0, 20.0)), i);
                    txt(p, pos2(row.left() + 52.0, row.center().y), Align2::LEFT_CENTER, name, F::Mono, 10.0, mix(SOFT, Color32::WHITE, on.max(h)), 0.0);
                    if resp.clicked() {
                        self.view.env = i;
                    }
                }
                lbl(p, pos2(er.left() + 12.0, er.bottom() - 10.0), Align2::LEFT_CENTER, "click to use · only .exr for now", 6.0, FAINT);
            });
        }
        let out = ctx.input(|i| i.pointer.any_click())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q) && !a.contains(q) && !(self.view.env_open && env_r.contains(q)));
        if (out && self.view.pop_k >= 1.0) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.view.pop = false;
            self.view.env_open = false;
        }
    }

    fn quick_panel(&mut self, ctx: &egui::Context) {
        let Some(at) = self.view.quick else { return };
        let k = smooth(self.view.quick_k);
        let w = 280.0;
        let screen = ctx.screen_rect();
        let mut r = Rect::from_min_size(at, vec2(w, 410.0));
        r = r.translate(vec2((screen.right() - 8.0 - r.right()).min(0.0), (screen.bottom() - 8.0 - r.bottom()).min(0.0)));
        let r = r.translate(vec2(0.0, -6.0 * (1.0 - k)));
        popup(ctx, "quick", r, k, |ui, p| {
            lbl(p, pos2(r.left() + 12.0, r.top() + 16.0), Align2::LEFT_CENTER, "quick panel", 8.0, TEXT);
            // the menu button turns the panel into its section list
            let mb = Rect::from_min_size(pos2(r.right() - 32.0, r.top() + 4.0), vec2(24.0, 24.0));
            let mresp = ui.interact(mb, Id::new("q_menu"), Sense::click());
            let mh = an(ui, "q_menu", mresp.hovered() || self.view.arrange);
            p.rect_filled(mb, 0.0, alpha(TEXT, mh));
            icons::draw(p, mb.shrink(5.0), icons::Icon::Menu, mix(DIM, BG, mh));
            if mresp.clicked() {
                self.view.arrange = !self.view.arrange;
            }
            let f = an(ui, "q_arrange", self.view.arrange);
            let mut body = p.clone();
            body.multiply_opacity(1.0 - f);
            let mut list = p.clone();
            list.multiply_opacity(f);
            if self.view.arrange {
                self.quick_sections(ui, &list, r);
            } else {
                self.quick_body(ui, &body, r);
            }
        });
        let out = ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
        if (out && self.view.quick_k >= 1.0) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.view.quick = None;
            self.view.arrange = false;
        }
    }

    /// The panel itself: its sections in the user's order.
    fn quick_body(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let w = r.width();
        let mut y = r.top() + 32.0;
        for s in self.view.qorder {
            match s {
                // colour: swatch, hex and a hue strip
                0 => {
                    let sw = Rect::from_min_size(pos2(r.left() + 12.0, y), vec2(44.0, 44.0));
                    frame(p, sw, self.view.color, BORDER2);
                    let c = self.view.color;
                    txt(p, pos2(sw.right() + 12.0, sw.top() + 12.0), Align2::LEFT_CENTER, &format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()), F::Mono, 12.0, TEXT, 0.0);
                    let hs = Rect::from_min_max(pos2(sw.right() + 12.0, sw.top() + 26.0), pos2(r.right() - 12.0, sw.bottom()));
                    let n = 36;
                    for i in 0..n {
                        let col: Color32 = egui::ecolor::Hsva::new(i as f32 / n as f32, 0.85, 0.9, 1.0).into();
                        let cell = Rect::from_min_size(pos2(hs.left() + i as f32 * hs.width() / n as f32, hs.top()), vec2(hs.width() / n as f32 + 0.5, hs.height()));
                        p.rect_filled(cell, 0.0, col);
                    }
                    let hr = ui.interact(hs, Id::new("q_hue"), Sense::click_and_drag());
                    if let Some(q) = hr.interact_pointer_pos() {
                        if hr.clicked() || hr.dragged() {
                            let hue = ((q.x - hs.left()) / hs.width()).clamp(0.0, 0.999);
                            self.view.color = egui::ecolor::Hsva::new(hue, 0.85, 0.9, 1.0).into();
                        }
                    }
                    y = sw.bottom() + 10.0;
                }
                // brush sliders + paint / eraser
                1 => {
                    let names = ["size", "flow", "opacity", "spacing"];
                    for i in 0..4 {
                        let row = Rect::from_min_size(pos2(r.left() + 2.0, y), vec2(w - 4.0, 26.0));
                        let mut v = self.tool_v[self.tool.min(1)][i];
                        slider(ui, p, row, &format!("q{i}"), names[i], &mut v, 100.0, "");
                        self.tool_v[self.tool.min(1)][i] = v;
                        y += 27.0;
                    }
                    y += 4.0;
                    let seg_r = Rect::from_min_size(pos2(r.left() + 12.0, y), vec2(w - 24.0, 26.0));
                    settings::seg(ui, p, seg_r, "q_tool", &["paint", "eraser"], &mut (self.view.eraser as usize), None);
                    if ui.interact(seg_r, Id::new("q_tool_flip"), Sense::click()).clicked() {
                        self.view.eraser = !self.view.eraser;
                        self.tool = if self.view.eraser { 1 } else { 0 };
                    }
                    y += 36.0;
                }
                // recent brushes
                2 => {
                    lbl(p, pos2(r.left() + 12.0, y + 6.0), Align2::LEFT_CENTER, "recent brushes", 6.5, FAINT);
                    y += 14.0;
                    for (i, s) in [14.0f32, 60.0, 120.0, 6.0].iter().enumerate() {
                        let br = Rect::from_min_size(pos2(r.left() + 12.0 + i as f32 * 44.0, y), vec2(40.0, 30.0));
                        let resp = ui.interact(br, Id::new(("qbrush", i)), Sense::click());
                        let h = an(ui, ("qbrush", i), resp.hovered());
                        frame(p, br, mix(BG, HOVER, h), mix(BORDER2, TEXT, h));
                        p.circle_filled(br.center() - vec2(0.0, 3.0), (s / 12.0).clamp(1.5, 8.0), TEXT);
                        txt(p, pos2(br.center().x, br.bottom() - 6.0), Align2::CENTER_CENTER, &format!("{s:.0}"), F::Mono, 8.0, DIM, 0.0);
                        if resp.clicked() {
                            self.tool_v[self.tool.min(1)][0] = *s;
                        }
                    }
                    y += 38.0;
                }
                // recent / project colours
                _ => {
                    let (title, cols) = if s == 3 {
                        ("recent colours", [0xE62619u32, 0x8C5A3C, 0x2E3B2F, 0xD0D0D0, 0x3E4A3C, 0x6B4A36])
                    } else {
                        ("project colours", [0x5E6B36, 0x1E1E1E, 0xE2E2E2, 0x4A5548, 0x2A2A2A, 0x6A6A6A])
                    };
                    lbl(p, pos2(r.left() + 12.0, y + 6.0), Align2::LEFT_CENTER, title, 6.5, FAINT);
                    y += 14.0;
                    for (i, c) in cols.iter().enumerate() {
                        let col = Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, *c as u8);
                        let cr = Rect::from_min_size(pos2(r.left() + 12.0 + i as f32 * 30.0, y), vec2(26.0, 22.0));
                        let resp = ui.interact(cr, Id::new((title, i)), Sense::click());
                        let h = an(ui, (title, i, "h"), resp.hovered());
                        frame(p, cr.expand(h), col, mix(BORDER2, TEXT, h));
                        if resp.clicked() {
                            self.view.color = col;
                        }
                    }
                    y += 30.0;
                }
            }
        }
    }

    /// Section list: move each section up or down; reset or done.
    fn quick_sections(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        const NAMES: [&str; 5] = ["colour picker", "brush and tool", "recent brushes", "recent colours", "project colours"];
        lbl(p, pos2(r.left() + 12.0, r.top() + 44.0), Align2::LEFT_CENTER, "sections, top to bottom", 6.5, FAINT);
        let mut swap = None;
        for (n, s) in self.view.qorder.iter().enumerate() {
            // rows glide to their new place
            let ty = anim_value(ui.ctx(), Id::new(("q_row", *s)), r.top() + 58.0 + n as f32 * 36.0, 0.18);
            let row = Rect::from_min_size(pos2(r.left() + 8.0, ty), vec2(r.width() - 16.0, 32.0));
            frame(p, row, PANEL, BORDER);
            txt(p, pos2(row.left() + 12.0, row.center().y), Align2::LEFT_CENTER, NAMES[*s], F::Ui, 12.0, TEXT, 0.0);
            for (k, (icon, d)) in [(icons::Icon::ChevronUp, -1i32), (icons::Icon::ChevronDown, 1)].into_iter().enumerate() {
                let b = Rect::from_min_size(pos2(row.right() - 56.0 + k as f32 * 26.0, row.top() + 4.0), vec2(24.0, 24.0));
                let target = n as i32 + d;
                let ok = (0..5).contains(&target);
                let resp = ui.interact(b, Id::new(("q_mv", *s, k)), Sense::click());
                let h = an(ui, ("q_mv", *s, k), resp.hovered() && ok);
                p.rect_filled(b, 0.0, alpha(TEXT, h));
                icons::draw(p, b.shrink(6.0), icon, if ok { mix(SOFT, BG, h) } else { Color32::from_gray(0x40) });
                if resp.clicked() && ok {
                    swap = Some((n, target as usize));
                }
            }
        }
        if let Some((a, b)) = swap {
            self.view.qorder.swap(a, b);
        }
        let by = r.top() + 58.0 + 5.0 * 36.0 + 10.0;
        if button(ui, p, Rect::from_min_size(pos2(r.left() + 12.0, by), vec2(90.0, 28.0)), "q_reset", "reset", false) {
            self.view.qorder = [0, 1, 2, 3, 4];
        }
        if button(ui, p, Rect::from_min_size(pos2(r.right() - 102.0, by), vec2(90.0, 28.0)), "q_done", "done", true) {
            self.view.arrange = false;
        }
    }

    /// Viewport > Viewport shading: opens the popover under the top-right corner.
    pub(crate) fn view_open_shading(&mut self, screen: Rect) {
        self.view.pop = true;
        self.view.pop_k = 0.0;
        self.view.pop_anchor = Rect::from_min_size(pos2(screen.right() - 470.0, 34.0), vec2(26.0, 26.0));
    }

    pub(crate) fn view_open_env(&mut self) {
        self.view.tab = 0;
        self.view.env_open = true;
    }

    pub(crate) fn open_picker(&mut self) {
        self.view.picker = true;
    }

    fn resource_picker(&mut self, ctx: &egui::Context) {
        let k = smooth(self.view.picker_k);
        let full = ctx.screen_rect();
        egui::Area::new(Id::new("picker_modal")).fixed_pos(full.min).order(egui::Order::Tooltip).show(ctx, |ui| {
            let (_, back) = ui.allocate_exact_size(full.size(), Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha((150.0 * k) as u8));
            ui.multiply_opacity(k);
            let r = Rect::from_center_size(full.center() + vec2(0.0, 18.0 * (1.0 - k)), vec2(820.0, 560.0).min(full.size() - vec2(64.0, 64.0)));
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 40);
            win_frame(&p, r, CARD, BORDER2);
            let hd = Rect::from_min_size(r.min, vec2(r.width(), 40.0));
            p.hline(hd.x_range(), hd.bottom(), Stroke::new(1.0, BORDER));
            let t = lbl(&p, pos2(hd.left() + 16.0, hd.center().y), Align2::LEFT_CENTER, "select a resource", 9.0, Color32::WHITE);
            lbl(&p, pos2(t.right() + 10.0, hd.center().y), Align2::LEFT_CENTER, "// roughness of rust grunge", 7.0, FAINT);
            lbl(&p, pos2(hd.right() - 16.0, hd.center().y), Align2::RIGHT_CENTER, "esc", 7.0, FAINT);
            // tabs + find + view
            let mut x = r.left() + 12.0;
            let ty = hd.bottom() + 10.0;
            for (i, s) in ["resources", "anchor points [03]"].into_iter().enumerate() {
                let tw = s.len() as f32 * 9.5 + 24.0;
                let tr = Rect::from_min_size(pos2(x, ty), vec2(tw, 28.0));
                x += tw + 4.0;
                let resp = ui.interact(tr, Id::new(("pk_tab", i)), Sense::click());
                if resp.clicked() {
                    self.view.picker_tab = i;
                    self.view.picker_sel = 0;
                }
                let on = an(ui, ("pk_tab", i), self.view.picker_tab == i || resp.hovered());
                frame(&p, tr, mix(BG, TEXT, on), mix(BORDER2, TEXT, on));
                lbl(&p, tr.center(), Align2::CENTER_CENTER, s, 7.5, mix(DIM, BG, on));
            }
            let fr = Rect::from_min_size(pos2(x + 8.0, ty), vec2(200.0, 28.0));
            frame(&p, fr, BG, BORDER2);
            ui.put(fr.shrink2(vec2(8.0, 4.0)), egui::TextEdit::singleline(&mut self.view.picker_find).frame(false).font(fid(F::Px, 10.0)).text_color(TEXT).hint_text("FIND"));
            let mut g = self.view.picker_grid as usize;
            settings::seg(ui, &p, Rect::from_min_size(pos2(r.right() - 140.0 - 280.0, ty), vec2(110.0, 28.0)), "pk_view", &["list", "grid"], &mut g, None);
            self.view.picker_grid = g == 1;
            // items
            let list = Rect::from_min_max(pos2(r.left() + 12.0, ty + 40.0), pos2(r.right() - 292.0, r.bottom() - 12.0));
            let q = self.view.picker_find.to_lowercase();
            let items: Vec<(usize, &str, &str)> = if self.view.picker_tab == 0 {
                RESOURCES.iter().enumerate().filter(|(_, (n, _))| n.to_lowercase().contains(&q)).map(|(i, (n, k))| (i, *n, *k)).collect()
            } else {
                ANCHORS.iter().enumerate().filter(|(_, n)| n.to_lowercase().contains(&q)).map(|(i, n)| (i, *n, "anchor point")).collect()
            };
            let mut use_now = false;
            for (n, (i, name, kind)) in items.iter().enumerate() {
                let cell = if self.view.picker_grid {
                    let cols = (list.width() / 120.0) as usize;
                    Rect::from_min_size(pos2(list.left() + (n % cols) as f32 * 120.0, list.top() + (n / cols) as f32 * 120.0), vec2(114.0, 114.0))
                } else {
                    Rect::from_min_size(pos2(list.left(), list.top() + n as f32 * 34.0), vec2(list.width(), 32.0))
                };
                if cell.bottom() > list.bottom() {
                    break;
                }
                let resp = ui.interact(cell, Id::new(("pk", self.view.picker_tab, i)), Sense::click());
                let h = an(ui, ("pk_h", self.view.picker_tab, *i), resp.hovered());
                let on = an(ui, ("pk_on", self.view.picker_tab, *i), self.view.picker_sel == *i);
                glow(&p, cell, acc(), 0.4 * on, 4);
                frame(&p, cell, mix(mix(PANEL, HOVER, h), SEL, on), mix(BORDER, acc(), on));
                if self.view.picker_grid {
                    res_thumb(&p, Rect::from_min_size(cell.min + vec2(8.0, 8.0), vec2(98.0, 72.0)), *i);
                    txt(&p, pos2(cell.left() + 8.0, cell.bottom() - 24.0), Align2::LEFT_CENTER, name, F::Ui, 11.0, TEXT, 0.0);
                    lbl(&p, pos2(cell.left() + 8.0, cell.bottom() - 10.0), Align2::LEFT_CENTER, kind, 5.5, FAINT);
                } else {
                    res_thumb(&p, Rect::from_center_size(pos2(cell.left() + 20.0, cell.center().y), vec2(24.0, 24.0)), *i);
                    txt(&p, pos2(cell.left() + 42.0, cell.center().y), Align2::LEFT_CENTER, name, F::Ui, 12.0, TEXT, 0.0);
                    lbl(&p, pos2(cell.right() - 10.0, cell.center().y), Align2::RIGHT_CENTER, kind, 6.5, FAINT);
                }
                if resp.clicked() {
                    self.view.picker_sel = *i;
                }
                if resp.double_clicked() {
                    self.view.picker_sel = *i;
                    use_now = true;
                }
            }
            // detail
            let dr = Rect::from_min_max(pos2(r.right() - 280.0, ty + 40.0), r.max - vec2(12.0, 12.0));
            frame(&p, dr, PANEL, BORDER);
            let sel = self.view.picker_sel;
            let (name, kind) = if self.view.picker_tab == 0 { RESOURCES[sel.min(RESOURCES.len() - 1)] } else { (ANCHORS[sel.min(2)], "anchor point") };
            res_thumb(&p, Rect::from_min_size(dr.min + vec2(12.0, 12.0), vec2(dr.width() - 24.0, dr.width() - 24.0)), sel);
            let mut y = dr.top() + dr.width() + 4.0;
            txt(&p, pos2(dr.left() + 12.0, y), Align2::LEFT_CENTER, name, F::Ui, 14.0, Color32::WHITE, 0.0);
            y += 22.0;
            lbl(&p, pos2(dr.left() + 12.0, y), Align2::LEFT_CENTER, &format!("source: starter library · {kind}"), 6.5, FAINT);
            y += 16.0;
            lbl(&p, pos2(dr.left() + 12.0, y), Align2::LEFT_CENTER, "1024 · tileable", 6.5, FAINT);
            let ub = Rect::from_min_size(pos2(dr.left() + 12.0, dr.bottom() - 62.0), vec2(dr.width() - 24.0, 30.0));
            if button(ui, &p, ub, "pk_use", "use this resource", true) {
                use_now = true;
            }
            lbl(&p, pos2(dr.center().x, dr.bottom() - 16.0), Align2::CENTER_CENTER, "double-click to use it right away", 6.0, FAINT);
            if use_now {
                self.view.rough_src = name;
                self.view.picker = false;
                let t = ui.input(|i| i.time) as f32;
                self.log.push((t, format!("Roughness of Rust grunge: {name}")));
            }
            let outside = back.clicked() && ui.ctx().input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
            if outside || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
                self.view.picker = false;
            }
        });
    }

    pub(crate) fn display_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "display settings", None, "");
        let (x, w) = (inner.left(), inner.width());
        let row = |y: f32| Rect::from_min_size(pos2(x + 2.0, y), vec2(w - 4.0, 26.0));
        let mut y = inner.top() + 6.0;
        section(p, x, y, w, "environment");
        y += 22.0;
        let er = Rect::from_min_size(pos2(x + 10.0, y), vec2(w - 20.0, 30.0));
        let resp = ui.interact(er, Id::new("disp_env"), Sense::click());
        env_thumb(p, er, self.view.env);
        p.rect_stroke(er, 0.0, Stroke::new(1.0, BORDER2), StrokeKind::Inside);
        lbl(p, pos2(er.left() + 10.0, er.center().y), Align2::LEFT_CENTER, &format!("{} ▾", ENVS[self.view.env].0), 8.0, Color32::WHITE);
        if resp.clicked() {
            self.view.env = (self.view.env + 1) % ENVS.len();
        }
        y += 36.0;
        vslider(ui, p, row(y), "d_rot", "rotation", &mut self.view.rot, 0.0, 360.0, &|v| format!("{v:.0}°"));
        y += 28.0;
        vslider(ui, p, row(y), "d_exp", "exposure (ev)", &mut self.view.exp, -3.0, 3.0, &|v| format!("{v:+.2}"));
        y += 32.0;
        section(p, x, y, w, "camera");
        y += 22.0;
        vslider(ui, p, row(y), "d_fov", "field of view", &mut self.view.fov, 10.0, 120.0, &|v| format!("{v:.0}°"));
        y += 32.0;
        section(p, x, y, w, "viewer");
        y += 22.0;
        vslider(ui, p, row(y), "d_ao", "ao intensity", &mut self.view.ao, 0.0, 1.0, &|v| format!("{v:.2}"));
        y += 30.0;
        if y + 26.0 < inner.bottom() {
            settings::seg(ui, p, Rect::from_min_size(pos2(x + 10.0, y), vec2(w - 20.0, 26.0)), "d_aa", &["off", "2x", "4x", "8x"], &mut self.view.aa, Some(3));
        }
    }

    pub(crate) fn shader_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "shader settings", None, "");
        let (x, w) = (inner.left(), inner.width());
        let mut y = inner.top() + 6.0;
        section(p, x, y, w, "shader");
        y += 22.0;
        let sr = Rect::from_min_size(pos2(x + 10.0, y), vec2(w - 20.0, 26.0));
        frame(p, sr, BG, BORDER2);
        txt(p, pos2(sr.left() + 8.0, sr.center().y), Align2::LEFT_CENTER, "PBR - Metallic Roughness", F::Mono, 10.0, TEXT, 0.0);
        txt(p, pos2(sr.right() - 8.0, sr.center().y), Align2::RIGHT_CENTER, "▾", F::Mono, 10.0, DIM, 0.0);
        y += 36.0;
        section(p, x, y, w, "parameters");
        y += 22.0;
        vslider(ui, p, Rect::from_min_size(pos2(x + 2.0, y), vec2(w - 4.0, 26.0)), "s_ao", "ao intensity", &mut self.view.ao, 0.0, 1.0, &|v| format!("{v:.2}"));
        y += 28.0;
        vslider(ui, p, Rect::from_min_size(pos2(x + 2.0, y), vec2(w - 4.0, 26.0)), "s_em", "emissive intensity", &mut self.view.emissive, 0.0, 4.0, &|v| format!("{v:.2}"));
    }
}

fn an_ctx(ctx: &egui::Context, id: &str, on: bool) -> f32 {
    smooth(ctx.animate_bool_with_time(Id::new(id), on, FADE))
}
