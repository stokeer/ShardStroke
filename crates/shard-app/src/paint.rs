//! CPU texture-space paint target.
//!
//! Memory-lean layout: a solid (or checker) **base** colour plus a straight-alpha RGBA8 **paint**
//! overlay (4 bytes/texel). The final image is resolved on demand for a dirty rectangle (GPU upload)
//! or in full (export), so no third full-size buffer is kept.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Paint,
    Eraser,
}

#[derive(Clone, Copy)]
pub struct Brush {
    /// Radius in screen pixels.
    pub radius: f32,
    pub hardness: f32,
    pub flow: f32,
    pub opacity: f32,
    /// Stamp spacing as a percentage of the brush diameter (Painter default: 20).
    pub spacing: f32,
    pub color: [f32; 3],
}

impl Default for Brush {
    fn default() -> Self {
        Self { radius: 14.0, hardness: 0.5, flow: 1.0, opacity: 1.0, spacing: 20.0, color: [0.9, 0.15, 0.1] }
    }
}

#[derive(Clone, PartialEq)]
enum Base {
    Solid([u8; 3]),
    /// Low-resolution composited base colour (`w x h`, RGB8), sampled nearest-neighbour.
    Image { w: u32, h: u32, rgb: Vec<u8> },
    Checker,
}

/// Axis-aligned dirty rectangle in texels, `[x0, x1) x [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect4 {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect4 {
    fn union(self, o: Rect4) -> Rect4 {
        Rect4 { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
}

/// Undo granularity: tiles of `TILE x TILE` texels of the paint overlay.
pub const TILE: u32 = 64;

/// RGBA8 content of one overlay tile (clipped at the texture border), row-major.
#[derive(Clone)]
pub struct TileSnap {
    pub tx: u32,
    pub ty: u32,
    pub data: Vec<u8>,
}

pub struct PaintTexture {
    pub w: u32,
    pub h: u32,
    /// Copy-on-write recording of the stroke in progress (see `begin_stroke`).
    recording: bool,
    tile_seen: Vec<bool>,
    saved: Vec<TileSnap>,
    base: Base,
    paint: Vec<u8>,
    /// Some texel was painted since the last clear (GPU then needs the full-size texture).
    painted: bool,
    /// Everything must be re-uploaded (resize, base change, clear).
    pub full_dirty: bool,
    /// Region painted since the last upload.
    pub dirty_rect: Option<Rect4>,
}

impl PaintTexture {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            recording: false,
            tile_seen: Vec::new(),
            saved: Vec::new(),
            base: Base::Checker,
            paint: vec![0; (w * h * 4) as usize],
            painted: false,
            full_dirty: true,
            dirty_rect: None,
        }
    }

    /// Size of the GPU texture that represents this target right now. An unpainted solid base is a
    /// single texel (saves 16 MB per 2K set); an unpainted composited base image stays at its own
    /// (smaller) resolution; painted or checker targets need the full texture.
    pub fn gpu_dims(&self) -> (u32, u32) {
        if self.painted {
            return (self.w, self.h);
        }
        match &self.base {
            Base::Solid(_) => (1, 1),
            Base::Image { w, h, .. } => (*w, *h),
            Base::Checker => (self.w, self.h),
        }
    }

    /// RGBA8 of the base image at its own resolution (only meaningful while unpainted).
    pub fn resolve_base_rgba(&self) -> Vec<u8> {
        match &self.base {
            Base::Image { rgb, .. } => rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            Base::Solid(c) => vec![c[0], c[1], c[2], 255],
            Base::Checker => self.resolve_full(),
        }
    }

    /// Does the base already hold a composited image (not a flat colour or the checker)?
    pub fn has_image_base(&self) -> bool {
        matches!(self.base, Base::Image { .. })
    }

    pub fn is_dirty(&self) -> bool {
        self.full_dirty || self.dirty_rect.is_some()
    }

    pub fn clear_dirty(&mut self) {
        self.full_dirty = false;
        self.dirty_rect = None;
    }

    pub fn set_base_solid(&mut self, rgb: [f32; 3]) {
        self.base = Base::Solid([to8(rgb[0]), to8(rgb[1]), to8(rgb[2])]);
        self.full_dirty = true;
    }

    /// Spatially varying base colour from a (possibly smaller) RGB float image, `n` floats per pixel.
    pub fn set_base_image(&mut self, w: u32, h: u32, n: usize, data: &[f32]) {
        let rgb: Vec<u8> = data
            .chunks_exact(n)
            .flat_map(|p| [to8(p[0]), to8(p[1.min(n - 1)]), to8(p[2.min(n - 1)])])
            .collect();
        self.base = Base::Image { w, h, rgb };
        self.full_dirty = true;
    }

    /// Spatially varying base colour from 8-bit sRGB pixels (`rgb`, 3 bytes per pixel).
    pub fn set_base_rgb8(&mut self, w: u32, h: u32, rgb: Vec<u8>) {
        self.base = Base::Image { w, h, rgb };
        self.full_dirty = true;
    }

    fn tiles_x(&self) -> u32 {
        (self.w + TILE - 1) / TILE
    }

    fn read_tile(&self, tx: u32, ty: u32) -> TileSnap {
        let (x0, y0) = (tx * TILE, ty * TILE);
        let (x1, y1) = ((x0 + TILE).min(self.w), (y0 + TILE).min(self.h));
        let mut data = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
        for y in y0..y1 {
            let i = ((y * self.w + x0) * 4) as usize;
            data.extend_from_slice(&self.paint[i..i + ((x1 - x0) * 4) as usize]);
        }
        TileSnap { tx, ty, data }
    }

    /// Start recording the tiles a stroke modifies (copy-on-write, for undo).
    pub fn begin_stroke(&mut self) {
        let n = (self.tiles_x() * ((self.h + TILE - 1) / TILE)) as usize;
        self.tile_seen.clear();
        self.tile_seen.resize(n, false);
        self.saved.clear();
        self.recording = true;
    }

    /// Finish the stroke: the original content of every tile it touched (None when nothing changed).
    pub fn end_stroke(&mut self) -> Option<Vec<TileSnap>> {
        self.recording = false;
        if self.saved.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.saved))
        }
    }

    /// Write `tiles` into the overlay and return what they replaced (undo <-> redo are symmetric).
    pub fn swap_tiles(&mut self, tiles: Vec<TileSnap>) -> Vec<TileSnap> {
        let mut prev = Vec::with_capacity(tiles.len());
        for t in tiles {
            prev.push(self.read_tile(t.tx, t.ty));
            let (x0, y0) = (t.tx * TILE, t.ty * TILE);
            let (x1, y1) = ((x0 + TILE).min(self.w), (y0 + TILE).min(self.h));
            let row = ((x1 - x0) * 4) as usize;
            for (k, y) in (y0..y1).enumerate() {
                let i = ((y * self.w + x0) * 4) as usize;
                self.paint[i..i + row].copy_from_slice(&t.data[k * row..(k + 1) * row]);
            }
            self.touch(x0, y0, x1, y1);
        }
        self.painted = true; // the full-size GPU texture must exist to show the restored paint
        self.full_dirty = true;
        prev
    }

    /// Straight-alpha RGBA8 paint overlay and its size (for export).
    pub fn overlay(&self) -> (&[u8], u32, u32) {
        (&self.paint, self.w, self.h)
    }

    /// True when something was painted since the last clear.
    pub fn has_paint(&self) -> bool {
        self.painted
    }

    /// Replace the hand-painted overlay with a saved one (nearest-neighbour when the size differs).
    pub fn set_overlay(&mut self, w: u32, h: u32, rgba: &[u8]) {
        if rgba.len() < (w * h * 4) as usize || w == 0 || h == 0 {
            return;
        }
        if (w, h) == (self.w, self.h) {
            let n = self.paint.len();
            self.paint.copy_from_slice(&rgba[..n]);
        } else {
            for y in 0..self.h {
                let sy = (y as u64 * h as u64 / self.h as u64) as u32;
                for x in 0..self.w {
                    let sx = (x as u64 * w as u64 / self.w as u64) as u32;
                    let (s, d) = (((sy * w + sx) * 4) as usize, ((y * self.w + x) * 4) as usize);
                    self.paint[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
                }
            }
        }
        self.painted = self.paint.chunks_exact(4).any(|p| p[3] != 0);
        self.full_dirty = true;
    }

    pub fn clear_paint(&mut self) {
        self.paint.fill(0);
        self.painted = false;
        self.full_dirty = true;
    }

    fn base_at(&self, x: u32, y: u32) -> [u8; 3] {
        match &self.base {
            Base::Solid(c) => *c,
            Base::Image { w, h, rgb } => {
                let sx = (x as u64 * *w as u64 / self.w as u64).min(*w as u64 - 1) as u32;
                let sy = (y as u64 * *h as u64 / self.h as u64).min(*h as u64 - 1) as u32;
                let i = ((sy * *w + sx) * 3) as usize;
                [rgb[i], rgb[i + 1], rgb[i + 2]]
            }
            Base::Checker => {
                let c = if ((x / 64) + (y / 64)) % 2 == 0 { 200u8 } else { 150u8 };
                [c, c, c]
            }
        }
    }

    /// Final RGBA8 for the rectangle (row-major, tightly packed).
    pub fn resolve_rect(&self, r: Rect4) -> Vec<u8> {
        let (x1, y1) = (r.x1.min(self.w), r.y1.min(self.h));
        let mut out = Vec::with_capacity(((x1 - r.x0) * (y1 - r.y0) * 4) as usize);
        for y in r.y0..y1 {
            for x in r.x0..x1 {
                let i = ((y * self.w + x) * 4) as usize;
                let a = self.paint[i + 3] as f32 / 255.0;
                let b = self.base_at(x, y);
                for c in 0..3 {
                    let v = b[c] as f32 + (self.paint[i + c] as f32 - b[c] as f32) * a;
                    out.push((v + 0.5) as u8);
                }
                out.push(255);
            }
        }
        out
    }

    /// Final colour of one texel (base under the painted overlay), sRGB 0..1; `uv` in mesh UV space.
    pub fn color_at_uv(&self, uv: [f32; 2]) -> [f32; 3] {
        let x = ((uv[0].rem_euclid(1.0) * self.w as f32) as u32).min(self.w - 1);
        let y = (((1.0 - uv[1]).rem_euclid(1.0) * self.h as f32) as u32).min(self.h - 1);
        let px = self.resolve_rect(Rect4 { x0: x, y0: y, x1: x + 1, y1: y + 1 });
        [px[0], px[1], px[2]].map(|v| v as f32 / 255.0)
    }

    pub fn resolve_full(&self) -> Vec<u8> {
        self.resolve_rect(Rect4 { x0: 0, y0: 0, x1: self.w, y1: self.h })
    }

    /// Blend one texel with source alpha `a_src` (already including falloff/flow/opacity).
    pub fn blend_texel(&mut self, x: u32, y: u32, a_src: f32, color: [f32; 3], tool: Tool) {
        if self.recording {
            let ti = ((y / TILE) * self.tiles_x() + x / TILE) as usize;
            if !self.tile_seen[ti] {
                self.tile_seen[ti] = true;
                let snap = self.read_tile(x / TILE, y / TILE);
                self.saved.push(snap);
            }
        }
        self.painted = true;
        let i = ((y * self.w + x) * 4) as usize;
        let a_dst = self.paint[i + 3] as f32 / 255.0;
        match tool {
            Tool::Eraser => self.paint[i + 3] = to8(a_dst * (1.0 - a_src)),
            Tool::Paint => {
                let a_out = a_src + a_dst * (1.0 - a_src);
                if a_out > 0.0 {
                    for c in 0..3 {
                        let dst = self.paint[i + c] as f32 / 255.0;
                        let v = (color[c] * a_src + dst * a_dst * (1.0 - a_src)) / a_out;
                        self.paint[i + c] = to8(v);
                    }
                }
                self.paint[i + 3] = to8(a_out);
            }
        }
    }

    /// Mark the texel rectangle `[x0, x1) x [y0, y1)` as needing a GPU upload.
    pub fn touch(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        let r = Rect4 { x0, y0, x1: x1.min(self.w), y1: y1.min(self.h) };
        self.dirty_rect = Some(match self.dirty_rect {
            Some(d) => d.union(r),
            None => r,
        });
    }

    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        save_png(path, self.w, self.h, &self.resolve_full())
    }
}

/// Soft-brush falloff: 1 inside `hardness`, smooth-step to 0 at the rim (`d` in 0..1).
pub fn falloff(d: f32, hardness: f32) -> f32 {
    if d >= 1.0 {
        0.0
    } else if d <= hardness {
        1.0
    } else {
        let s = 1.0 - (d - hardness) / (1.0 - hardness).max(1e-4);
        s * s * (3.0 - 2.0 * s)
    }
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn save_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_tiles_restore_the_overlay() {
        let mut t = PaintTexture::new(150, 130); // not a multiple of the tile size
        t.begin_stroke();
        for (x, y) in [(3, 4), (70, 5), (149, 129), (64, 64)] {
            t.blend_texel(x, y, 1.0, [1.0, 0.0, 0.0], Tool::Paint);
        }
        let tiles = t.end_stroke().expect("tiles recorded");
        assert_eq!(tiles.len(), 4, "four distinct tiles were touched");
        let painted = t.overlay().0.to_vec();
        assert!(painted.iter().any(|&b| b != 0));
        // undo: the overlay becomes empty again; the returned tiles redo the stroke exactly
        let redo = t.swap_tiles(tiles);
        assert!(t.overlay().0.iter().all(|&b| b == 0));
        t.swap_tiles(redo);
        assert_eq!(t.overlay().0, &painted[..]);
    }
}
