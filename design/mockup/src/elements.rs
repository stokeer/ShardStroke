//! More of the Elements page: texture set list + settings (12), assets shelf (13), history + log (15) as
//! dockable panels, and the Export textures dialog (16).

use super::*;

pub const SETS: [(&str, u32); 8] =
    [("hull", 4096), ("turret", 4096), ("wheels", 2048), ("tracks", 2048), ("interior", 2048), ("glass", 1024), ("decals", 2048), ("details", 1024)];
/// The set still composing in the background (stripes).
const COMPOSING: usize = 3;

pub const HISTORY: [&str; 7] =
    ["Project opened", "Add fill layer", "Paint stroke", "Paint stroke", "Opacity of Rust grunge", "Paint stroke", "Hide Dust"];

pub fn log_lines() -> Vec<(f32, String)> {
    [
        (0.4, "Opening turret_v2.shard"),
        (1.9, "8 texture set(s), 111,420 triangles"),
        (2.3, "Composing 'hull' at 1024x1024 in the background..."),
        (6.8, "'hull' composed at 4096x4096"),
        (12.0, "Active texture set: turret"),
        (31.5, "Picked colour #8C5A3C"),
        (48.2, "Environment rotation 84°"),
        (60.7, "Sharpen: not rendered by the viewport yet"),
        (61.1, "Exported 6 file(s) to D:\\out in 3.4 s"),
    ]
    .into_iter()
    .map(|(t, s)| (t, s.to_string()))
    .collect()
}

/// (name, kind, usable today)
const ASSETS: [(&str, &str, bool); 10] = [
    ("Iron Rust", "smart material", true),
    ("Steel Rust Old", "smart material", true),
    ("Rust Edges", "smart mask", true),
    ("Rust Fine", "material", false),
    ("Rust Coarse", "material", false),
    ("Grunge Rust Fine", "procedural", false),
    ("rust_leaks.png", "texture", false),
    ("Rust Spots", "alpha", false),
    ("Rusty Brush", "tool preset", false),
    ("rusty_warehouse.exr", "environment", true),
];
const KINDS: [(&str, &str); 10] = [
    ("all", ""),
    ("smart mat", "smart material"),
    ("smart mask", "smart mask"),
    ("materials", "material"),
    ("textures", "texture"),
    ("alphas", "alpha"),
    ("env", "environment"),
    ("presets", "tool preset"),
    ("procedural", "procedural"),
    ("effects", "effect"),
];

/// Moving diagonal stripes (a set or layer still computing).
fn stripes(p: &Painter, r: Rect, t: f32, c: Color32) {
    let sp = p.with_clip_rect(r.intersect(p.clip_rect()));
    let mut x = r.left() - r.height() + (t * 20.0).rem_euclid(12.0) - 12.0;
    while x < r.right() {
        sp.add(Shape::convex_polygon(
            vec![pos2(x, r.bottom()), pos2(x + 6.0, r.bottom()), pos2(x + 6.0 + r.height(), r.top()), pos2(x + r.height(), r.top())],
            c,
            Stroke::NONE,
        ));
        x += 12.0;
    }
}

/// Label + a read-only drop-list look.
fn field(p: &Painter, r: Rect, name: &str, value: &str, arrow: bool) {
    lbl(p, pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, name, 7.0, DIM);
    let v = Rect::from_min_max(pos2(r.left() + r.width() * 0.45, r.top() + 3.0), r.max - vec2(8.0, 3.0));
    frame(p, v, BG, BORDER2);
    txt(p, pos2(v.left() + 8.0, v.center().y), Align2::LEFT_CENTER, value, F::Mono, 10.0, TEXT, 0.0);
    if arrow {
        txt(p, pos2(v.right() - 8.0, v.center().y), Align2::RIGHT_CENTER, "▾", F::Mono, 10.0, DIM, 0.0);
    }
}

pub struct Export {
    pub open: bool,
    pub k: f32,
    pick: [bool; 8],
    kind: usize,
    /// Export started at this time (progress), if running.
    run: Option<f64>,
}

impl Default for Export {
    fn default() -> Self {
        Export { open: false, k: 0.0, pick: [true, true, true, false, false, false, false, false], kind: 0, run: None }
    }
}

impl App {
    pub(crate) fn sets_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "texture set list", Some(SETS.len()), "");
        let t = ui.input(|i| i.time) as f32;
        let st_t = if heavy() { t } else { 0.0 };
        let mut y = inner.top() + 4.0;
        for (i, (name, size)) in SETS.iter().enumerate() {
            let row = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 28.0));
            y += 28.0;
            if row.bottom() > inner.bottom() {
                break;
            }
            let resp = ui.interact(row, Id::new(("set", i)), Sense::click());
            if resp.clicked() && self.active_set != i {
                self.active_set = i;
                self.log.push((t, format!("Active texture set: {name}")));
            }
            let s = an(ui, ("set_s", i), self.active_set == i);
            let h = an(ui, ("set_h", i), resp.hovered());
            p.rect_filled(row, 0.0, mix(alpha(HOVER, h), SEL, s));
            p.rect_filled(Rect::from_min_size(row.min, vec2(2.0, row.height() * s)), 0.0, acc());
            p.circle_stroke(pos2(row.left() + 16.0, row.center().y), 5.0, Stroke::new(1.2, DIM));
            p.circle_filled(pos2(row.left() + 16.0, row.center().y), 2.4, DIM);
            let nr = lbl(p, pos2(row.left() + 32.0, row.center().y), Align2::LEFT_CENTER, name, 8.0, mix(SOFT, Color32::WHITE, s));
            if i == COMPOSING {
                let st = Rect::from_min_size(pos2(nr.right() + 8.0, row.center().y - 7.0), vec2(80.0, 14.0));
                stripes(p, st, st_t, alpha(acc(), 0.5));
                lbl(p, st.center(), Align2::CENTER_CENTER, "composing", 6.0, TEXT);
            }
            lbl(p, pos2(row.right() - 10.0, row.center().y), Align2::RIGHT_CENTER, "main shader", 6.0, FAINT);
            txt(p, pos2(row.right() - 96.0, row.center().y), Align2::RIGHT_CENTER, &size.to_string(), F::Mono, 10.0, DIM, 0.0);
        }
        let fy = y + 6.0;
        if fy + 20.0 < inner.bottom() {
            lbl(p, pos2(inner.left() + 10.0, fy + 8.0), Align2::LEFT_CENTER, &format!("active texture set: {}", SETS[self.active_set].0), 7.0, FAINT);
        }
        // settings of the active set, when the panel is tall enough
        let sy = fy + 26.0;
        if sy + 7.0 * 28.0 + 70.0 < inner.bottom() {
            p.hline(inner.x_range(), sy, Stroke::new(1.0, BORDER));
            lbl(p, pos2(inner.left() + 10.0, sy + 16.0), Align2::LEFT_CENTER, &format!("texture set settings // {}", SETS[self.active_set].0), 8.0, TEXT);
            let size = SETS[self.active_set].1.to_string();
            let rows = [
                ("size", size.as_str(), true),
                ("name", SETS[self.active_set].0, false),
                ("shader instance", "Main shader", true),
                ("normal mixing", "Replace", true),
                ("height to normal", "Smooth (Sobel)", true),
                ("ao mixing", "Replace", true),
                ("uv padding", "UV Space Neighbor", true),
            ];
            let mut y = sy + 32.0;
            for (n, v, a) in rows {
                field(p, Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 28.0)), n, v, a);
                y += 28.0;
            }
            button(ui, p, Rect::from_min_size(pos2(inner.left() + 10.0, y + 8.0), vec2(150.0, 26.0)), "bake", "bake mesh maps", false);
        }
    }

    pub(crate) fn history_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "history", Some(HISTORY.len()), "");
        let now = ui.input(|i| i.time) as f32;
        for (i, s) in HISTORY.iter().enumerate() {
            let row = Rect::from_min_size(pos2(inner.left(), inner.top() + 4.0 + i as f32 * 26.0), vec2(inner.width(), 26.0));
            if row.bottom() > inner.bottom() {
                break;
            }
            let resp = ui.interact(row, Id::new(("hist", i)), Sense::click());
            if resp.clicked() && self.history_pos != i {
                self.history_pos = i;
                self.log.push((now, format!("Back to step {i:02}: {s}")));
            }
            let cur = an(ui, ("hist_c", i), self.history_pos == i);
            let h = an(ui, ("hist_h", i), resp.hovered());
            let undone = i > self.history_pos;
            p.rect_filled(row, 0.0, mix(alpha(HOVER, h), SEL, cur));
            p.rect_filled(Rect::from_min_size(row.min, vec2(2.0, row.height() * cur)), 0.0, acc());
            txt(p, pos2(row.left() + 12.0, row.center().y), Align2::LEFT_CENTER, &format!("{i:02}"), F::Mono, 10.0, FAINT, 0.0);
            let label = if undone { format!("{s} (undone)") } else { s.to_string() };
            txt(p, pos2(row.left() + 40.0, row.center().y), Align2::LEFT_CENTER, &label, F::Ui, 12.0, if undone { FAINT } else { mix(SOFT, Color32::WHITE, cur) }, 0.0);
            if self.history_pos == i {
                let tr = lbl(p, pos2(row.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, "now", 6.0, acc());
                p.rect_stroke(tr.expand2(vec2(5.0, 3.0)), 0.0, Stroke::new(1.0, acc()), StrokeKind::Outside);
            }
        }
    }

    pub(crate) fn log_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "log", Some(self.log.len()), "");
        let bar = Rect::from_min_size(inner.min, vec2(inner.width(), 34.0));
        if button(ui, p, Rect::from_min_size(bar.min + vec2(8.0, 5.0), vec2(64.0, 24.0)), "log_copy", "copy", false) {
            let text: String = self.log.iter().map(|(t, s)| format!("{t:.1}s {s}\n")).collect();
            ui.ctx().copy_text(text);
        }
        if button(ui, p, Rect::from_min_size(bar.min + vec2(78.0, 5.0), vec2(64.0, 24.0)), "log_clear", "clear", false) {
            self.log.clear();
        }
        let list = Rect::from_min_max(pos2(inner.left(), bar.bottom()), inner.max);
        let lp = p.with_clip_rect(list.intersect(p.clip_rect()));
        // newest at the bottom, like a console
        let rows = (list.height() / 20.0) as usize;
        let start = self.log.len().saturating_sub(rows);
        for (n, (t, s)) in self.log[start..].iter().enumerate() {
            let y = list.top() + 10.0 + n as f32 * 20.0;
            txt(&lp, pos2(list.left() + 10.0, y), Align2::LEFT_CENTER, &format!("{t:>5.1}s"), F::Mono, 10.0, FAINT, 0.0);
            let warn = s.contains("not rendered");
            txt(&lp, pos2(list.left() + 64.0, y), Align2::LEFT_CENTER, s, F::Mono, 10.0, if warn { WARN } else { SOFT }, 0.0);
        }
    }

    pub(crate) fn assets_panel(&mut self, ui: &mut Ui, p: &Painter, r: Rect) {
        let inner = card(p, r, "assets", None, "libraries...");
        lbl(p, pos2(inner.left() + 60.0, r.top() + 15.0), Align2::LEFT_CENTER, "[2,184]", 8.0, FAINT);
        // search + kind filters
        let sr = Rect::from_min_size(inner.min + vec2(8.0, 6.0), vec2(180.0, 24.0));
        frame(p, sr, BG, BORDER2);
        ui.put(sr.shrink2(vec2(8.0, 3.0)), egui::TextEdit::singleline(&mut self.asset_find).frame(false).font(fid(F::Px, 10.0)).text_color(TEXT).hint_text("SEARCH"));
        let mut x = sr.right() + 8.0;
        for (i, (s, _)) in KINDS.iter().enumerate() {
            let w = s.len() as f32 * 9.0 + 18.0;
            let c = Rect::from_min_size(pos2(x, sr.top()), vec2(w, 24.0));
            x += w + 4.0;
            if c.right() > inner.right() {
                break;
            }
            let resp = ui.interact(c, Id::new(("kind", i)), Sense::click());
            if resp.clicked() {
                self.asset_kind = i;
            }
            let on = an(ui, ("kind_on", i), self.asset_kind == i || resp.hovered());
            frame(p, c, mix(BG, TEXT, on), mix(BORDER2, TEXT, on));
            lbl(p, c.center(), Align2::CENTER_CENTER, s, 7.0, mix(DIM, BG, on));
        }
        // the list, in as many columns as fit
        let list = Rect::from_min_max(pos2(inner.left(), sr.bottom() + 8.0), inner.max - vec2(0.0, 24.0));
        let q = self.asset_find.to_lowercase();
        let want = KINDS[self.asset_kind].1;
        let items: Vec<_> = ASSETS.iter().filter(|a| (want.is_empty() || a.1 == want) && a.0.to_lowercase().contains(&q)).collect();
        let cols = ((list.width() / 300.0) as usize).max(1);
        let cw = list.width() / cols as f32;
        let mut hover_path = None;
        for (n, (name, kind, ok)) in items.iter().enumerate() {
            let (c, rr) = (n % cols, n / cols);
            let row = Rect::from_min_size(pos2(list.left() + c as f32 * cw, list.top() + rr as f32 * 30.0), vec2(cw - 4.0, 28.0));
            if row.bottom() > list.bottom() {
                break;
            }
            let resp = ui.interact(row, Id::new(("asset", n, *name)), Sense::click());
            let h = an(ui, ("asset_h", *name), resp.hovered());
            p.rect_filled(row, 0.0, alpha(HOVER, h));
            let th = Rect::from_center_size(pos2(row.left() + 16.0, row.center().y), vec2(20.0, 20.0));
            frame(p, th, Color32::from_gray(0x2A + (n as u8 * 7) % 30), BORDER2);
            txt(p, pos2(th.right() + 10.0, row.center().y), Align2::LEFT_CENTER, name, F::Ui, 12.0, if *ok { TEXT } else { DIM }, 0.0);
            lbl(p, pos2(row.right() - 70.0, row.center().y), Align2::RIGHT_CENTER, kind, 6.0, FAINT);
            if *ok {
                if button(ui, p, Rect::from_min_size(pos2(row.right() - 58.0, row.top() + 3.0), vec2(54.0, 22.0)), &format!("use{name}"), "use", false) || resp.double_clicked() {
                    let t = ui.input(|i| i.time) as f32;
                    self.log.push((t, format!("Used {name}")));
                }
            } else {
                lbl(p, pos2(row.right() - 8.0, row.center().y), Align2::RIGHT_CENTER, "not yet", 6.0, Color32::from_gray(0x4A));
            }
            if resp.hovered() {
                hover_path = Some(format!("D:\\assets\\materials\\{name}"));
            }
        }
        let foot = hover_path.unwrap_or_else(|| "libraries: 2,184 asset(s) in 2 location(s) · double-click to use · hover shows the path".into());
        lbl(p, pos2(inner.left() + 10.0, inner.bottom() - 12.0), Align2::LEFT_CENTER, &foot, 6.5, FAINT);
    }

    /// Export textures: modal like the settings.
    pub(crate) fn export_modal(&mut self, ctx: &egui::Context, now: f64) {
        let k = smooth(self.export.k);
        let full = ctx.screen_rect();
        egui::Area::new(Id::new("export_modal")).fixed_pos(full.min).order(egui::Order::Tooltip).show(ctx, |ui| {
            let (_, back) = ui.allocate_exact_size(full.size(), Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha((150.0 * k) as u8));
            ui.multiply_opacity(k);
            let size = vec2(940.0, 640.0).min(full.size() - vec2(64.0, 64.0));
            let r = Rect::from_center_size(full.center() + vec2(0.0, 18.0 * (1.0 - k)), size);
            let p = ui.painter().clone();
            glow(&p, r, Color32::BLACK, 0.6, 40);
            win_frame(&p, r, CARD, BORDER2);
            self.export_body(ui, &p, r, now);
            let outside = back.clicked() && ui.ctx().input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
            if self.export.run.is_none() && (outside || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))) {
                self.export.open = false;
            }
        });
    }

    fn export_body(&mut self, ui: &mut Ui, p: &Painter, r: Rect, now: f64) {
        let hd = Rect::from_min_size(r.min, vec2(r.width(), 40.0));
        p.hline(hd.x_range(), hd.bottom(), Stroke::new(1.0, BORDER));
        lbl(p, pos2(hd.left() + 16.0, hd.center().y), Align2::LEFT_CENTER, "export textures", 9.0, Color32::WHITE);
        lbl(p, pos2(hd.right() - 16.0, hd.center().y), Align2::RIGHT_CENTER, "esc", 7.0, FAINT);
        // texture sets
        let left = Rect::from_min_max(pos2(r.left(), hd.bottom()), pos2(r.left() + 300.0, r.bottom()));
        p.vline(left.right(), left.y_range(), Stroke::new(1.0, BORDER));
        lbl(p, pos2(left.left() + 16.0, left.top() + 22.0), Align2::LEFT_CENTER, &format!("texture sets [{:02}]", SETS.len()), 7.0, FAINT);
        if button(ui, p, Rect::from_min_size(pos2(left.right() - 116.0, left.top() + 11.0), vec2(50.0, 22.0)), "ex_none", "none", false) {
            self.export.pick = [false; 8];
        }
        if button(ui, p, Rect::from_min_size(pos2(left.right() - 60.0, left.top() + 11.0), vec2(46.0, 22.0)), "ex_all", "all", false) {
            self.export.pick = [true; 8];
        }
        for (i, (name, size)) in SETS.iter().enumerate() {
            let row = Rect::from_min_size(pos2(left.left(), left.top() + 44.0 + i as f32 * 30.0), vec2(left.width() - 1.0, 30.0));
            let resp = ui.interact(row, Id::new(("ex_set", i)), Sense::click());
            if resp.clicked() {
                self.export.pick[i] = !self.export.pick[i];
            }
            let h = an(ui, ("ex_set_h", i), resp.hovered());
            let on = an(ui, ("ex_set_on", i), self.export.pick[i]);
            p.rect_filled(row, 0.0, alpha(HOVER, h));
            let cb = Rect::from_center_size(pos2(row.left() + 22.0, row.center().y), vec2(12.0, 12.0));
            glow(p, cb, acc(), 0.5 * on, 4);
            p.rect_filled(cb, 0.0, alpha(acc(), on));
            p.rect_stroke(cb, 0.0, Stroke::new(1.0, mix(Color32::from_gray(0x6A), acc(), on)), StrokeKind::Inside);
            lbl(p, pos2(row.left() + 40.0, row.center().y), Align2::LEFT_CENTER, name, 8.0, mix(SOFT, Color32::WHITE, on));
            txt(p, pos2(row.right() - 16.0, row.center().y), Align2::RIGHT_CENTER, &size.to_string(), F::Mono, 10.0, FAINT, 0.0);
        }
        // output + format
        let x = left.right() + 18.0;
        let w = r.right() - x - 18.0;
        let mut y = hd.bottom() + 16.0;
        lbl(p, pos2(x, y + 6.0), Align2::LEFT_CENTER, "output", 7.0, FAINT);
        y += 20.0;
        field(p, Rect::from_min_size(pos2(x - 10.0, y), vec2(w - 100.0, 30.0)), "directory", "D:\\textures\\turret_v2", false);
        button(ui, p, Rect::from_min_size(pos2(x + w - 92.0, y + 3.0), vec2(92.0, 24.0)), "browse", "browse...", false);
        y += 32.0;
        field(p, Rect::from_min_size(pos2(x - 10.0, y), vec2(w + 10.0, 30.0)), "template", "PBR Metallic Roughness", true);
        y += 38.0;
        lbl(p, pos2(x, y + 6.0), Align2::LEFT_CENTER, "files per set", 7.0, FAINT);
        y += 18.0;
        let files = [("$set_BaseColor", "rgb"), ("$set_Roughness", "grey"), ("$set_Metallic", "grey"), ("$set_Normal", "rgb"), ("$set_AO", "grey")];
        let fb = Rect::from_min_size(pos2(x, y), vec2(w, files.len() as f32 * 22.0));
        frame(p, fb, PANEL, BORDER);
        for (n, (f, c)) in files.iter().enumerate() {
            let fy = fb.top() + 11.0 + n as f32 * 22.0;
            txt(p, pos2(fb.left() + 10.0, fy), Align2::LEFT_CENTER, f, F::Mono, 10.0, TEXT, 0.0);
            lbl(p, pos2(fb.right() - 10.0, fy), Align2::RIGHT_CENTER, c, 6.0, FAINT);
        }
        y = fb.bottom() + 18.0;
        lbl(p, pos2(x, y + 6.0), Align2::LEFT_CENTER, "format", 7.0, FAINT);
        y += 20.0;
        lbl(p, pos2(x, y + 13.0), Align2::LEFT_CENTER, "file type", 7.0, DIM);
        settings::seg(ui, p, Rect::from_min_size(pos2(x + w * 0.45 - 10.0, y), vec2(w * 0.55 + 10.0, 26.0)), "ex_kind", &["png", "tga", "jpeg", "tiff", "bmp", "exr"], &mut self.export.kind, None);
        y += 32.0;
        for (n, v) in [("bit depth", "8 bits"), ("size", "Each set's own"), ("padding", "Dilation infinite"), ("normal map", "From template (OpenGL)")] {
            field(p, Rect::from_min_size(pos2(x - 10.0, y), vec2(w + 10.0, 28.0)), n, v, true);
            y += 30.0;
        }
        // footer: cancel / export, then a progress bar while it runs
        let n = self.export.pick.iter().filter(|b| **b).count();
        let foot = Rect::from_min_max(pos2(left.right() + 1.0, r.bottom() - 52.0), r.max);
        p.hline(foot.x_range(), foot.top(), Stroke::new(1.0, BORDER));
        if let Some(t0) = self.export.run {
            let f = ((now - t0) / 1.6) as f32;
            let bar = Rect::from_min_size(pos2(foot.left() + 18.0, foot.center().y - 6.0), vec2(foot.width() - 200.0, 12.0));
            blocks(p, bar, smooth(f), acc());
            lbl(p, pos2(bar.right() + 14.0, foot.center().y), Align2::LEFT_CENTER, &format!("writing {}/{}", ((f * (n * 5) as f32) as usize).min(n * 5), n * 5), 7.0, TEXT);
            if f >= 1.0 {
                self.export.run = None;
                self.export.open = false;
                self.log.push((now as f32, format!("Exported {} file(s) to D:\\textures\\turret_v2", n * 5)));
            }
        } else {
            let label = format!("export {n} set(s)");
            if button(ui, p, Rect::from_min_size(pos2(foot.right() - 170.0, foot.top() + 12.0), vec2(152.0, 30.0)), "ex_go", &label, true) && n > 0 {
                self.export.run = Some(now);
            }
            if button(ui, p, Rect::from_min_size(pos2(foot.right() - 264.0, foot.top() + 12.0), vec2(86.0, 30.0)), "ex_cancel", "cancel", false) {
                self.export.open = false;
            }
        }
    }
}
