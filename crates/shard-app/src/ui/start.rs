//! Start screen, a popup over the main window (as in design/mockup): opens when the program starts without a
//! project and from the logo at the top left; Esc or a click outside closes it.
//!
//! Left: the shard under a purple gradient, with SHARDSTROKE rolling diagonally behind it (moving only at the
//! advanced animation level). Right: the two ways to start (material authoring is not built yet), the quick
//! actions and the recent projects.

use super::*;
use egui::Pos2;

/// (title, footer, key, ascii art, enabled)
const CARDS: [(&str, &str, &str, &str, &str, &str, bool); 2] = [
    (
        "[01] // MATERIAL AUTHORING",
        "NEW MATERIAL",
        "BUILD A TILEABLE PROCEDURAL MATERIAL\nFROM A NODE GRAPH. NOT BUILT YET.",
        "GRAPH · 2048 · TILEABLE",
        "",
        "[NOISE]--+\n         +--[BLEND]--[HEIGHT]--+\n[TILES]--+                     |\n                               +--[OUT]\n[GRUNGE]--[LEVELS]-------------+",
        false,
    ),
    (
        "[02] // 3D TEXTURING",
        "NEW 3D TEXTURING",
        "IMPORT A MESH AND PAINT\nLAYERED MATERIALS ON IT.",
        "OBJ",
        "",
        "    +------+     LAYERS [03]\n   /|     /|       PAINT\n  +------+ |       FILL\n  | +----|-+       MASK\n  |/     |/\n  +------+",
        true,
    ),
];

fn txt(p: &egui::Painter, pos: Pos2, align: Align2, s: &str, size: f32, color: Color32) -> Rect {
    p.text(pos, align, s, FontId::proportional(size), color)
}

/// The shard: hexagon with a triangular notch, with the start screen's effects (a solid ring that cuts what is
/// behind it, then a short glow of its colour).
fn shard(p: &egui::Painter, center: Pos2, h: f32, fill: Color32, ring: Color32) {
    let s = h / 201.68;
    let o = center - vec2(70.0, 100.84) * s;
    let pt = |x: f32, y: f32| o + vec2(x, y) * s;
    let hex = vec![pt(70.0, 0.0), pt(140.0, 70.0), pt(111.0, 178.23), pt(70.0, 201.68), pt(29.0, 178.23), pt(0.0, 70.0)];
    let grow = |d: f32| -> Vec<Pos2> {
        let n = hex.len();
        let normal = |a: Pos2, b: Pos2| {
            let e = (b - a).normalized();
            vec2(e.y, -e.x)
        };
        (0..n)
            .map(|i| {
                let n1 = normal(hex[(i + n - 1) % n], hex[i]);
                let n2 = normal(hex[i], hex[(i + 1) % n]);
                let m = (n1 + n2).normalized();
                hex[i] + m * (d / m.dot(n1).max(0.3))
            })
            .collect()
    };
    p.add(egui::Shape::convex_polygon(grow(15.0), ring, Stroke::NONE));
    for k in (1..=5).rev() {
        let a = 0.22 * (1.0 - k as f32 / 6.0);
        p.add(egui::Shape::convex_polygon(grow(k as f32), Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), (a * 255.0) as u8), Stroke::NONE));
    }
    p.add(egui::Shape::convex_polygon(hex.clone(), fill, Stroke::NONE));
    p.add(egui::Shape::convex_polygon(vec![pt(75.0, 25.0), pt(115.0, 58.0), pt(86.0, 69.5)], ring, Stroke::NONE));
}

const BAND: f32 = 0.35;
const CELL: egui::Vec2 = egui::Vec2::new(10.0, 14.0);

/// Diagonal ASCII wipe over `r` at progress `k` (0 covered, 1 revealed): a band of characters running from the
/// top-left corner to the bottom-right, dense at its leading edge and thinning out toward the revealed side. No
/// background: what is not revealed yet stays see-through ([`clip_diagonal`]).
/// Card art on a fixed character grid (the pixel font is proportional, so a plain text block loses its columns),
/// animated at the advanced level: card 0 (node graph) sends a pulse along its wires to OUT, card 1 (mesh +
/// layers) sweeps a glint over the cube's edges and walks the cursor down PAINT / FILL / MASK.
fn ascii_art(p: &egui::Painter, origin: Pos2, art: &str, card: usize, t: f32, base: Color32, lit: Color32) {
    const CELL: egui::Vec2 = egui::Vec2::new(7.0, 12.0);
    let font = FontId::monospace(10.0);
    let moving = theme::heavy();
    let width = art.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
    // the layer row the cursor is on (cycles every 0.9 s; PAINT when still)
    let cursor_row = if moving { 1 + (t / 0.9) as usize % 3 } else { 1 };
    for (row, line) in art.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        for (col, &c) in chars.iter().enumerate() {
            if c == ' ' {
                continue;
            }
            let wire = matches!(c, '-' | '+' | '|' | '/');
            let glow = if !moving {
                0.0
            } else if card == 0 && wire {
                // a pulse travelling left to right along the wires
                let head = (t * 18.0).rem_euclid(width + 10.0) - 4.0;
                (1.0 - (col as f32 - head).abs() / 3.0).max(0.0)
            } else if card == 1 && wire && col < 14 {
                // a glint sweeping the cube diagonally
                let head = (t * 9.0).rem_euclid(26.0) - 6.0;
                (1.0 - ((col + row) as f32 - head).abs() / 2.5).max(0.0)
            } else {
                0.0
            };
            // the layer label under the cursor
            let label = card == 1 && col >= 19 && row == cursor_row;
            let color = if label { lit } else { w::mix(base, lit, glow) };
            let at = origin + vec2(col as f32 * CELL.x + CELL.x * 0.5, row as f32 * CELL.y);
            p.text(at, Align2::CENTER_TOP, c, font.clone(), color);
        }
        if card == 1 && row == cursor_row {
            p.text(origin + vec2(17.0 * CELL.x + CELL.x * 0.5, row as f32 * CELL.y), Align2::CENTER_TOP, '>', font.clone(), lit);
        }
    }
}

fn ascii_wipe(p: &egui::Painter, r: Rect, k: f32) {
    const RAMP: [&str; 9] = ["@", "#", "%", "*", "+", "=", "-", ":", "."];
    let (cw, ch) = (CELL.x, CELL.y);
    let front = k * (1.0 + BAND);
    let span = r.width() + r.height();
    let font = FontId::monospace(11.0);
    let acc = theme::accent();
    let mut y = r.min.y;
    while y < r.max.y {
        let mut x = r.min.x;
        while x < r.max.x {
            let d = ((x - r.min.x) + (y - r.min.y)) / span;
            // t: 1 = revealed, 0 = still covered
            let t = ((front - d) / BAND).clamp(0.0, 1.0);
            if t < 1.0 {
                let cell = Rect::from_min_size(pos2(x, y), vec2(cw, ch)).intersect(r);
                if t > 0.0 {
                    let g = RAMP[((t * RAMP.len() as f32) as usize).min(RAMP.len() - 1)];
                    let col = w::mix(acc, theme::TEXT_DIM, t);
                    let a = ((1.0 - t) * 0.8 + 0.2).min(1.0);
                    p.text(cell.center(), Align2::CENTER_CENTER, g, font.clone(), Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (a * 255.0) as u8));
                }
            }
            x += cw;
        }
        y += ch;
    }
}

/// Keeps only the revealed part of the start screen's drawing (shapes after `first`): row by row of the wipe's
/// cells, everything left of the band's trailing edge. The rest of the screen shows through.
fn clip_diagonal(ctx: &egui::Context, layer: egui::LayerId, first: usize, r: Rect, k: f32) {
    let front = k * (1.0 + BAND) - BAND;
    let span = r.width() + r.height();
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        let shapes: Vec<egui::epaint::ClippedShape> = list.all_entries().cloned().collect();
        let mut out = egui::layers::PaintList::default();
        for (i, cs) in shapes.into_iter().enumerate() {
            if i < first {
                out.add(cs.clip_rect, cs.shape);
                continue;
            }
            let b = cs.shape.visual_bounding_rect();
            if !b.is_finite() {
                continue;
            }
            // the rows of the screen (and its glow around it) that this shape touches
            let top = r.min.y - 60.0;
            let row0 = ((b.min.y - top) / CELL.y).floor().max(0.0) as i32;
            let row1 = ((b.max.y - top) / CELL.y).ceil() as i32;
            for row in row0..row1.max(row0 + 1) {
                let y0 = top + row as f32 * CELL.y;
                let yc = y0 + CELL.y * 0.5;
                let x_rev = r.min.x + front * span - (yc - r.min.y);
                if x_rev <= b.min.x {
                    continue;
                }
                let strip = Rect::from_min_max(pos2(r.min.x - 60.0, y0), pos2(x_rev, y0 + CELL.y));
                let clip = cs.clip_rect.intersect(strip);
                if clip.is_positive() {
                    out.add(clip, cs.shape.clone());
                }
            }
        }
        *list = out;
    });
}

fn vgrad(p: &egui::Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(m));
}

impl AppState {
    pub fn open_start(&mut self) {
        self.start_open = true;
    }

    /// Draws the start screen while it is open or fading out.
    pub(super) fn start_screen(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        // advanced: a quick diagonal ASCII wipe; basic: a fade; off: at once
        let wipe = theme::heavy();
        let step = if theme::animation() == 0 { 1.0 } else { dt / if wipe { 0.3 } else { 0.22 } };
        self.start_k = if self.start_open { (self.start_k + step).min(1.0) } else { (self.start_k - step).max(0.0) };
        // SHARD_SHOT_WIPE=k (diagnostic): the open screen held at progress k of its wipe
        if let Some(v) = std::env::var("SHARD_SHOT_WIPE").ok().and_then(|v| v.parse::<f32>().ok()) {
            self.start_k = v.clamp(0.01, 1.0);
        }
        if self.start_k <= 0.0 {
            return;
        }
        let k = self.start_k * self.start_k * (3.0 - 2.0 * self.start_k);
        let full = ctx.screen_rect();
        let mut close = false;
        let mut first_shape = 0;
        let mut wipe_rect = None;
        egui::Area::new(egui::Id::new("start_screen")).order(egui::Order::Foreground).fixed_pos(full.min).fade_in(false).show(ctx, |ui| {
            let (_, back) = ui.allocate_exact_size(full.size(), Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha((170.0 * k) as u8));
            first_shape = ui.painter().add(egui::Shape::Noop).0;
            if !wipe {
                ui.multiply_opacity(k);
            }
            let size = vec2(1280.0, 780.0).min(full.size() - vec2(64.0, 64.0));
            let lift = if wipe { 0.0 } else { 18.0 * (1.0 - k) };
            let r = Rect::from_center_size(full.center() + vec2(0.0, lift), size);
            let p = ui.painter().clone();
            theme::glow(&p, r, Color32::BLACK, 0.6, 40);
            p.rect_filled(r, 0.0, theme::BG_VIEWPORT);
            p.rect_stroke(r, 0.0, Stroke::new(theme::WINDOW_STROKE, theme::BORDER_STRONG), egui::StrokeKind::Inside);
            close |= self.start_body(ui, &p, r.shrink(20.0));
            if wipe && self.start_k < 1.0 {
                wipe_rect = Some(r);
                let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("start_wipe")));
                ascii_wipe(&lp, r, self.start_k);
            }
            let outside = back.clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|q| !r.contains(q));
            close |= outside || ui.input(|i| i.key_pressed(egui::Key::Escape));
        });
        if let Some(r) = wipe_rect {
            clip_diagonal(ctx, egui::LayerId::new(egui::Order::Foreground, egui::Id::new("start_screen")), first_shape, r, self.start_k);
            ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("start_wipe")));
        }
        self.patch_notes(ctx);
        if close && !self.patch_notes_open {
            self.start_open = false;
        }
        // Escape closes the notes first, then the start screen
        if close && self.patch_notes_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.patch_notes_open = false;
        }
        if self.start_open || self.start_k > 0.0 {
            ctx.request_repaint();
        }
    }

    /// What this version brings, over the start screen.
    fn patch_notes(&mut self, ctx: &egui::Context) {
        if !self.patch_notes_open || !self.start_open {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("PATCH NOTES").size(10.0))
            .id(egui::Id::new("patch_notes"))
            .order(egui::Order::Tooltip)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(ctx.screen_rect().height() * 0.6).show(ui, |ui| {
                    for line in include_str!("../../assets/PATCH_NOTES.txt").lines() {
                        let t = line.trim_end();
                        if t.is_empty() {
                            ui.add_space(6.0);
                        } else if !t.starts_with('-') && !t.starts_with(' ') {
                            // a heading
                            ui.label(RichText::new(t.to_uppercase()).size(10.0).color(theme::accent_hover()));
                        } else {
                            ui.label(RichText::new(t).size(10.0).color(theme::TEXT));
                        }
                    }
                });
            });
        self.patch_notes_open = open;
    }

    /// The content; true when an action closes the screen.
    fn start_body(&mut self, ui: &mut egui::Ui, p: &egui::Painter, full: Rect) -> bool {
        let mut close = false;
        let t = if theme::heavy() { ui.input(|i| i.time) as f32 } else { 0.0 };
        // ---- left: the shard under the gradient, the name rolling behind it
        let left = Rect::from_min_size(full.min, vec2((full.width() * 0.34).max(340.0), full.height()));
        p.rect_filled(left, 0.0, theme::PANEL);
        p.rect_stroke(left, 0.0, Stroke::new(theme::WINDOW_STROKE, theme::BORDER), egui::StrokeKind::Inside);
        txt(p, left.min + vec2(22.0, 22.0), Align2::LEFT_TOP, "[00] // START", 10.0, theme::TEXT_DIM);
        txt(p, pos2(left.max.x - 22.0, left.min.y + 22.0), Align2::RIGHT_TOP, "V0.1 PREVIEW", 10.0, theme::TEXT_DISABLED);
        let art = Rect::from_min_max(pos2(left.min.x + 2.0, left.min.y + 40.0), pos2(left.max.x - 2.0, left.max.y - 112.0));
        let base = Color32::from_rgb(18, 18, 20);
        p.rect_filled(art, 0.0, base);
        {
            let cp = p.with_clip_rect(art.intersect(p.clip_rect()));
            let a = -30f32.to_radians();
            let d = vec2(a.cos(), a.sin());
            let n = vec2(-a.sin(), a.cos());
            let unit = "SHARDSTROKE - ";
            let font = FontId::proportional(22.0);
            let seg = cp.layout_no_wrap(unit.to_string(), font.clone(), Color32::WHITE).size().x;
            let line = unit.repeat(10);
            let reach = art.size().length();
            let rows = (reach / 34.0) as i32 + 2;
            let col = Color32::from_rgb(48, 43, 64);
            for k in -rows / 2..=rows / 2 {
                let phase = (t * 22.0 + k as f32 * seg * 0.37).rem_euclid(seg);
                let start = art.center() + n * (k as f32 * 34.0) - d * (reach * 0.5 + seg) + d * phase;
                let g = cp.layout_no_wrap(line.clone(), font.clone(), col);
                cp.add(egui::epaint::TextShape::new(start, g, col).with_angle(a));
            }
        }
        shard(p, art.center(), art.height().min(art.width()) * 0.5, Color32::from_rgb(76, 34, 140), base);
        // the gradient lies over the text and the shard
        vgrad(p, art, Color32::from_rgba_unmultiplied(42, 32, 68, 90), Color32::from_rgba_unmultiplied(21, 21, 23, 170));
        // the app's name: the one text not in the pixel font
        p.text(pos2(left.min.x + 22.0, left.max.y - 82.0), Align2::LEFT_TOP, "SHARDSTROKE", FontId::new(26.0, egui::FontFamily::Name("title".into())), theme::TEXT);
        txt(p, pos2(left.min.x + 22.0, left.max.y - 38.0), Align2::LEFT_CENTER, "MATERIAL AUTHORING + 3D TEXTURING", 10.0, theme::TEXT_DIM);
        let pn = Rect::from_min_size(pos2(left.max.x - 118.0, left.max.y - 50.0), vec2(104.0, 24.0));
        if ui.put(pn, egui::Button::new(RichText::new("PATCH NOTES").size(10.0))).clicked() {
            self.patch_notes_open = !self.patch_notes_open;
        }

        // ---- right column
        let rx = left.max.x + 18.0;
        let right = Rect::from_min_max(pos2(rx, full.min.y), full.max);
        let cw = (right.width() - 18.0) / 2.0;
        let ch = (right.height() * 0.33).clamp(200.0, 260.0);
        for (i, (head, title, body, foot, _key, ascii, enabled)) in CARDS.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(rx + i as f32 * (cw + 18.0), right.min.y), vec2(cw, ch));
            let resp = ui.interact(r, ui.id().with(("start_card", i)), if enabled { Sense::click() } else { Sense::hover() });
            let resp = if enabled { resp } else { resp.on_hover_text("The material graph is not built yet") };
            let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, theme::anim(0.16));
            // hover: 2 px accent border and the kit's soft glow
            theme::glow(p, r, theme::accent(), 0.26 * h, 12);
            p.rect_filled(r, 0.0, theme::PANEL_ALT);
            let [ar, ag, ab] = theme::accent_rgb();
            vgrad(p, r, Color32::from_rgba_unmultiplied(ar / 3, ag / 3, ab / 3, (h * 60.0) as u8), Color32::TRANSPARENT);
            p.rect_stroke(r, 0.0, Stroke::new(1.0 + h, w::mix(theme::BORDER, theme::accent(), h)), egui::StrokeKind::Inside);
            let sq = Rect::from_min_size(r.min + vec2(18.0, 20.0), vec2(9.0, 9.0));
            theme::glow(p, sq, theme::accent(), h, 4);
            p.rect_filled(sq, 0.0, w::mix(theme::BORDER_STRONG, theme::accent(), h));
            let dim = if enabled { 1.0 } else { 0.45 };
            let fade = |c: Color32| w::mix(Color32::TRANSPARENT, c, dim);
            txt(p, pos2(sq.max.x + 12.0, sq.center().y), Align2::LEFT_CENTER, head, 10.0, fade(theme::TEXT_DIM));
            let base = fade(w::mix(theme::TEXT_DISABLED, theme::TEXT, h));
            ascii_art(p, r.min + vec2(18.0, 48.0), ascii, i, t, base, fade(theme::accent_hover()));
            txt(p, pos2(r.min.x + 18.0, r.max.y - 92.0), Align2::LEFT_BOTTOM, title, 18.0, fade(w::mix(theme::TEXT, Color32::WHITE, h)));
            p.text(pos2(r.min.x + 18.0, r.max.y - 76.0), Align2::LEFT_TOP, body, FontId::proportional(10.0), fade(theme::TEXT_DIM));
            txt(p, pos2(r.min.x + 18.0, r.max.y - 18.0), Align2::LEFT_BOTTOM, foot, 10.0, fade(theme::TEXT_DISABLED));
            if resp.clicked() && enabled {
                self.import_mesh_dialog();
                close = true;
            }
        }
        // quick actions
        let by = right.min.y + ch + 16.0;
        let bw = (right.width() - 3.0 * 12.0) / 4.0;
        let actions: [(Icon, &str, &str, bool); 4] = [
            (Icon::Folder, "OPEN", "CTRL+O", true),
            (Icon::Cube, "IMPORT MESH", "", true),
            (Icon::Grid, "FROM TEMPLATE", "", false),
            (Icon::Sliders, "SETTINGS", "", true),
        ];
        for (i, (icon, label, key, enabled)) in actions.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(rx + i as f32 * (bw + 12.0), by), vec2(bw, 38.0));
            let resp = ui.interact(r, ui.id().with(("start_act", i)), if enabled { Sense::click() } else { Sense::hover() });
            let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, theme::anim(0.16));
            let col = if enabled { w::mix(theme::TEXT, theme::BG_VIEWPORT, h) } else { theme::TEXT_DISABLED };
            p.rect_filled(r, 0.0, w::mix(theme::PANEL_ALT, theme::TEXT, h));
            p.rect_stroke(r, 0.0, Stroke::new(1.0, w::mix(theme::BORDER, theme::TEXT, h)), egui::StrokeKind::Inside);
            crate::icons::draw(p, Rect::from_center_size(pos2(r.min.x + 20.0, r.center().y), vec2(16.0, 16.0)), icon, col);
            txt(p, pos2(r.min.x + 36.0, r.center().y), Align2::LEFT_CENTER, label, 10.0, col);
            if !key.is_empty() {
                txt(p, pos2(r.max.x - 12.0, r.center().y), Align2::RIGHT_CENTER, key, 8.0, w::mix(theme::TEXT_DISABLED, theme::BG_VIEWPORT, h));
            }
            if resp.clicked() && enabled {
                match i {
                    0 => self.open_dialog(),
                    1 => self.import_mesh_dialog(),
                    _ => self.settings_open = true,
                }
                close = true;
            }
        }
        // recent projects
        let lr = Rect::from_min_max(pos2(rx, by + 54.0), right.max);
        p.rect_filled(lr, 0.0, theme::PANEL_ALT);
        p.rect_stroke(lr, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        let recents: Vec<std::path::PathBuf> = self.config.recent_projects.clone();
        let t1 = txt(p, lr.min + vec2(16.0, 20.0), Align2::LEFT_CENTER, "RECENT", 10.0, theme::TEXT);
        txt(p, pos2(t1.max.x + 6.0, t1.center().y), Align2::LEFT_CENTER, &format!("[{:02}]", recents.len()), 10.0, theme::TEXT_DISABLED);
        let fr = Rect::from_min_size(pos2(lr.max.x - 196.0, lr.min.y + 8.0), vec2(180.0, 24.0));
        p.rect_filled(fr, 0.0, theme::WELL);
        p.rect_stroke(fr, 0.0, Stroke::new(1.0, theme::BORDER_STRONG), egui::StrokeKind::Inside);
        ui.put(fr.shrink2(vec2(8.0, 3.0)), egui::TextEdit::singleline(&mut self.start_find).frame(false).hint_text("FIND..."));
        let hy = lr.min.y + 52.0;
        let cols = [lr.min.x + 66.0, lr.max.x - 240.0, lr.max.x - 130.0];
        for (x, s) in cols.iter().zip(["NAME", "HAS", "OPENED"]) {
            txt(p, pos2(*x, hy), Align2::LEFT_CENTER, s, 8.0, theme::TEXT_DISABLED);
        }
        if recents.is_empty() {
            txt(p, pos2(cols[0], hy + 40.0), Align2::LEFT_CENTER, "NOTHING OPENED YET", 10.0, theme::TEXT_DISABLED);
        }
        let q = self.start_find.to_lowercase();
        let mut y = hy + 16.0;
        for (i, path) in recents.iter().enumerate() {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
            if !name.to_lowercase().contains(&q) || y + 44.0 > lr.max.y {
                continue;
            }
            let r = Rect::from_min_size(pos2(lr.min.x + 2.0, y), vec2(lr.width() - 4.0, 44.0));
            y += 45.0;
            let exists = path.exists();
            let resp = ui.interact(r, ui.id().with(("start_recent", i)), Sense::click()).on_hover_text(path.display().to_string());
            let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim(0.16));
            p.rect_filled(r, 0.0, w::mix(if i == 0 { theme::SELECTED } else { Color32::TRANSPARENT }, theme::WIDGET_HOVER, h));
            let th = Rect::from_center_size(pos2(lr.min.x + 31.0, r.center().y), vec2(30.0, 30.0));
            p.rect_filled(th, 0.0, Color32::from_gray(40 + (i as u8 * 9) % 40));
            p.rect_stroke(th, 0.0, Stroke::new(1.0, if i == 0 { theme::TEXT } else { theme::BORDER_STRONG }), egui::StrokeKind::Inside);
            let nr = txt(p, pos2(cols[0] + 4.0 * h, r.center().y), Align2::LEFT_CENTER, &name.to_uppercase(), 10.0, if i == 0 { theme::TEXT } else { w::mix(theme::TEXT_DIM, theme::TEXT, h) });
            if !exists {
                txt(p, pos2(nr.max.x + 8.0, r.center().y), Align2::LEFT_CENTER, "[MISSING]", 8.0, theme::ERROR);
            }
            let tag = "3D";
            let tr = txt(p, pos2(cols[1] + 5.0, r.center().y), Align2::LEFT_CENTER, tag, 8.0, theme::TEXT);
            p.rect_stroke(tr.expand2(vec2(5.0, 4.0)), 0.0, Stroke::new(1.0, if i == 0 { theme::TEXT } else { theme::BORDER_STRONG }), egui::StrokeKind::Outside);
            let when = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                .map(|d| {
                    let m = d.as_secs() / 60;
                    match m {
                        0..=89 => format!("{m} MIN AGO"),
                        90..=2879 => format!("{} H AGO", m / 60),
                        _ => format!("{} DAYS AGO", m / 1440),
                    }
                })
                .unwrap_or_else(|| "-".into());
            txt(p, pos2(cols[2], r.center().y), Align2::LEFT_CENTER, &when, 10.0, theme::TEXT_DIM);
            if resp.clicked() && exists {
                self.request_open(path.clone());
                close = true;
            }
        }
        close
    }
}
