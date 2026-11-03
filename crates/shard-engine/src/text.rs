//! Text stencils (`DataSourceText`): the text of a paint action rasterised into a square alpha image.
//!
//! The stencil is what a stroke reveals: its alpha, projected through the view the stroke was painted in,
//! multiplies the brush coverage. The image is square, the text is centred and scaled to fit the area left
//! by the auto padding, keeping its aspect.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use shard_model::TextStencil;

/// Square alpha image (`n` x `n`, row 0 on top).
pub struct StencilImage {
    pub n: usize,
    pub alpha: Vec<f32>,
}

impl StencilImage {
    /// Bilinear alpha at `(u, v)` in 0..1 (v down); 0 outside.
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return 0.0;
        }
        let (x, y) = (u * self.n as f32 - 0.5, v * self.n as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| {
            let (xi, yi) = (xi.clamp(0.0, self.n as f32 - 1.0) as usize, yi.clamp(0.0, self.n as f32 - 1.0) as usize);
            self.alpha[yi * self.n + xi]
        };
        let a = at(x0, y0) * (1.0 - tx) + at(x0 + 1.0, y0) * tx;
        let b = at(x0, y0 + 1.0) * (1.0 - tx) + at(x0 + 1.0, y0 + 1.0) * tx;
        a * (1.0 - ty) + b * ty
    }
}

/// Font file of a `resource://system_fonts/<name>?version=...` url.
fn font_path(url: &str) -> Option<std::path::PathBuf> {
    let name = url.rsplit('/').next()?.split('?').next()?.trim_end_matches(".ttf");
    let dirs = [
        std::env::var_os("WINDIR").map(|w| std::path::PathBuf::from(w).join("Fonts")),
        Some(std::path::PathBuf::from(r"C:\Windows\Fonts")),
        Some(std::path::PathBuf::from("/usr/share/fonts/truetype/msttcorefonts")),
    ];
    for d in dirs.into_iter().flatten() {
        for cand in [format!("{name}.ttf"), format!("{name}.TTF")] {
            let p = d.join(cand);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Rasterise `t` at `n` x `n`. `None` when the font cannot be found or the text is empty.
pub fn render_text(t: &TextStencil, n: usize) -> Option<StencilImage> {
    let lines: Vec<&str> = t.text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    // lines that carry ink; trailing empty lines do not take part in the layout (a text ends with a line
    // break when typed with Enter)
    let first = lines.iter().position(|l| !l.is_empty())?;
    let last = lines.iter().rposition(|l| !l.is_empty())?;
    let lines = &lines[first..=last];
    let data = std::fs::read(font_path(&t.font)?).ok()?;
    let font = FontVec::try_from_vec(data).ok()?;
    let px = 256.0f32; // layout scale; the result is rescaled to the stencil box
    let scaled = font.as_scaled(PxScale::from(px));
    // a line is as tall as the font's ascent + descent + gap times the line spacing; the whole box is what
    // gets centred in the stencil
    // calibrated against a test screen's letter masks (best correlation 0.9 at 0.93)
    let lk = std::env::var("SHARD_LINE_K").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.93);
    let line_h = (scaled.ascent() - scaled.descent() + scaled.line_gap()) * t.line_spacing.max(0.1) * lk;

    // glyph outlines placed in layout space (x right, y down), ink bounds measured on the way
    struct Placed {
        glyph: ab_glyph::Glyph,
    }
    let mut placed: Vec<Placed> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let mut x = 0.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for ch in line.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let mut g = id.with_scale(PxScale::from(px));
            // the text sits at the top of its line box; the extra line spacing is below it
            g.position = ab_glyph::point(x, scaled.ascent() + li as f32 * line_h);
            x += scaled.h_advance(id) + t.letter_spacing * px;
            prev = Some(id);
            placed.push(Placed { glyph: g });
        }
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    let outlined: Vec<_> = placed.iter().filter_map(|p| font.outline_glyph(p.glyph.clone())).collect();
    for o in &outlined {
        let b = o.px_bounds();
        lo = [lo[0].min(b.min.x), lo[1].min(b.min.y)];
        hi = [hi[0].max(b.max.x), hi[1].max(b.max.y)];
    }
    if outlined.is_empty() || hi[0] <= lo[0] || hi[1] <= lo[1] {
        return None;
    }
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    // fit into the square minus the auto padding, aspect preserved: the ink width and the line box height
    // both have to fit; the ink is centred horizontally and the line box vertically
    let pad = if t.auto_size { t.padding[0].max(t.padding[1]) } else { 0.0 };
    let k = std::env::var("SHARD_TEXT_FILL").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.92);
    let avail = (1.0 - 2.0 * pad) * n as f32 * k;
    let box_h = lines.len() as f32 * line_h;
    let s = avail / w.max(h);
    let ox = (n as f32 - w * s) * 0.5;
    // layout y of the box centre is box_h / 2; ink top is lo[1]
    let oy = n as f32 * 0.5 + (lo[1] - box_h * 0.5) * s;
    let mut alpha = vec![0.0f32; n * n];
    // draw at the layout scale into a scratch buffer, then resample with the fit scale
    let (sw, sh) = (w.ceil() as usize + 2, h.ceil() as usize + 2);
    let mut scratch = vec![0.0f32; sw * sh];
    for o in &outlined {
        let b = o.px_bounds();
        o.draw(|x, y, c| {
            let (px_, py_) = (x as f32 + b.min.x - lo[0], y as f32 + b.min.y - lo[1]);
            let (ix, iy) = (px_ as usize, py_ as usize);
            if ix < sw && iy < sh {
                let d = &mut scratch[iy * sw + ix];
                *d = (*d + c).min(1.0);
            }
        });
    }
    for y in 0..n {
        for x in 0..n {
            let (lx, ly) = ((x as f32 + 0.5 - ox) / s, (y as f32 + 0.5 - oy) / s);
            if lx < 0.0 || ly < 0.0 || lx >= w || ly >= h {
                continue;
            }
            // bilinear lookup in the scratch buffer
            let (fx, fy) = (lx - 0.5, ly - 0.5);
            let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (tx, ty) = (fx - fx.floor(), fy - fy.floor());
            let at = |xi: usize, yi: usize| scratch[yi.min(sh - 1) * sw + xi.min(sw - 1)];
            let a = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let b = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            alpha[y * n + x] = a * (1.0 - ty) + b * ty;
        }
    }
    Some(StencilImage { n, alpha })
}
