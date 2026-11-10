//! Approximate evaluation of Substance procedurals used by masks.
//!
//! The real graphs are compiled Substance assemblies (`SBAM`, see docs/FORMATS.md 7.5i). Until an
//! evaluator for them exists, the most common graphs are approximated here with the same tweak
//! semantics (scale, balance, contrast, invert, seed): the result is similar in character, not
//! pixel-identical. Everything that goes through this module is reported in
//! [`crate::Composite::approximated`], never silently.

use std::sync::Arc;

use shard_model::{Procedural, Tweak};

use crate::Image;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum MeshMap {
    AmbientOcclusion,
    Curvature,
    WorldNormal,
    NormalBase,
    Position,
    /// White where the UV layout of the texture set has surface, black elsewhere (`mesh_uv_mask`).
    UvMask,
}

/// Baked mesh maps of the texture set being composed.
pub trait MeshMaps {
    fn mesh_map(&self, map: MeshMap) -> Option<Arc<Image>>;

    /// Bitmap `index` embedded in the compiled graph of `resource` (the source texture of grunges and of
    /// the edge-wear generator). Components are 0..1; `None` when the graph carries none.
    fn embedded(&self, _resource: &str, _index: usize) -> Option<Arc<Image>> {
        None
    }

    /// Evaluate `p` exactly with an external Substance renderer at `w` x `h` (powers of two). `content`
    /// is the picture a filter graph processes. `None` when no renderer is available or it failed.
    /// Surface point and normal of every texel at `w` x `h`, for tri-planar and planar projections.
    fn surface(&self, _w: u32, _h: u32) -> Option<Arc<Surface>> {
        None
    }

    /// Identity of everything this provider's results depend on (project, texture set, mesh, renderer on or
    /// off, resolution class). Evaluated masks are kept between compositions under it; `None` = do not keep.
    fn cache_token(&self) -> Option<u64> {
        None
    }

    fn render_graph(
        &self,
        _p: &Procedural,
        _output: Option<&str>,
        _w: u32,
        _h: u32,
        _content: Option<&Image>,
    ) -> Option<Arc<Image>> {
        None
    }
}

/// Size a graph is rendered at for a `w` x `h` context: the source's resolution override, capped.
pub fn render_size(p: &Procedural, w: u32, h: u32) -> Option<(u32, u32)> {
    let (rw, rh) = p.resolution.target(w, h);
    let (rw, rh) = (rw.min(2048), rh.min(2048));
    (rw.is_power_of_two() && rh.is_power_of_two() && rw >= 16 && rh >= 16).then_some((rw, rh))
}

/// How a fill projects its texture: the kind, the 3D placement when it has one, and whether the texture
/// repeats outside the unit square.
#[derive(Clone, Copy)]
pub struct ProjSpec {
    pub kind: shard_model::Projection,
    pub projector: Option<shard_model::Projector>,
    pub repeat: bool,
}

impl ProjSpec {
    pub fn of(f: &shard_model::Fill) -> Self {
        ProjSpec { kind: f.projection, projector: f.projector, repeat: f.uv.wrap != 0 }
    }
}

/// Rotation (world = R * local) of the Euler angles of a projector. Qt order: roll (Z), pitch (X), yaw (Y).
fn projector_rotation(e: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = e[0].sin_cos();
    let (sy, cy) = e[1].sin_cos();
    let (sz, cz) = e[2].sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| {
        let mut r = [[0.0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        r
    };
    static ORDER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    match ORDER.get_or_init(|| std::env::var("SHARD_EULER_ORDER").unwrap_or_default()).as_str() {
        "xyz" => mul(rz, mul(ry, rx)),
        "zyx" => mul(rx, mul(ry, rz)),
        "xzy" => mul(ry, mul(rz, rx)),
        _ => mul(ry, mul(rx, rz)),
    }
}

/// Texture lookups of one surface texel through a projector: up to three bilinear taps (a tri-planar
/// projection blends the three axes), each already through the fill's UV transform and the wrap / clamp rule.
#[derive(Clone, Copy)]
pub struct Taps {
    count: usize,
    uv: [(f32, f32); 3],
    weight: [f32; 3],
}

impl Taps {
    /// Value of component `c` (of `n` requested) of `img` at these taps.
    #[inline]
    pub fn sample(&self, img: &Image, c: usize, n: usize) -> f32 {
        let mut acc = 0.0;
        for k in 0..self.count {
            acc += self.weight[k] * crate::sample(img, self.uv[k].0, self.uv[k].1, c, n);
        }
        acc
    }
}

/// A planar / tri-planar projector prepared for per-texel lookups: rotation, size and the experiment knobs
/// are worked out once instead of for every texel and component.
pub struct ProjPrep {
    r: [[f32; 3]; 3],
    position: [f32; 3],
    size: [f32; 3],
    repeat: bool,
    planar: bool,
    planar_mode: u32,
    pw: f32,
}

impl ProjPrep {
    /// `None` when the fill has no 3D placement (the caller then uses the UV mapping).
    pub fn new(spec: &ProjSpec) -> Option<ProjPrep> {
        use shard_model::Projection;
        let pr = spec.projector?;
        if !matches!(spec.kind, Projection::Planar | Projection::Triplanar) {
            return None;
        }
        // experiment knobs, read once
        struct Knobs {
            k: f32,
            kp: f32,
            pw: f32,
            planar: u32,
        }
        static KNOBS: std::sync::OnceLock<Knobs> = std::sync::OnceLock::new();
        let kn = KNOBS.get_or_init(|| {
            let f = |n: &str, d: f32| std::env::var(n).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
            Knobs {
                k: f("SHARD_PROJ_K", 2.0),
                kp: f("SHARD_PROJ_KPOS", 1.0),
                pw: f("SHARD_TRI_POW", 4.0),
                planar: f("SHARD_PLANAR", 0.0) as u32,
            }
        });
        // projector units are fractions of the scene: the normalised mesh spans 2 of them across (its longest
        // half extent is 1), so a unit size covers the whole object; the position is in mesh units and the
        // size is a half extent
        Some(ProjPrep {
            r: projector_rotation(pr.euler),
            position: [pr.position[0] * kn.kp, pr.position[1] * kn.kp, pr.position[2] * kn.kp],
            size: [
                (pr.size[0] * kn.k).abs().max(1e-6),
                (pr.size[1] * kn.k).abs().max(1e-6),
                (pr.size[2] * kn.k).abs().max(1e-6),
            ],
            repeat: pr.wrap != 0,
            planar: spec.kind == Projection::Planar,
            planar_mode: kn.planar,
            pw: kn.pw,
        })
    }

    /// Does texel `i` of the surface fall inside the projector's picture (the unit square across the projection
    /// axis)? `SHARD_PLANAR_DEPTH=1` also bounds it along the axis.
    pub fn inside(&self, s: &Surface, i: usize) -> bool {
        let r = &self.r;
        let d = [s.pos[i][0] - self.position[0], s.pos[i][1] - self.position[1], s.pos[i][2] - self.position[2]];
        let c = [
            (r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2]) / self.size[0],
            (r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2]) / self.size[1],
            (r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2]) / self.size[2],
        ];
        static DEPTH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let depth = *DEPTH.get_or_init(|| std::env::var_os("SHARD_PLANAR_DEPTH").is_some());
        c[0].abs() <= 0.5 && c[1].abs() <= 0.5 && (!depth || c[2].abs() <= 0.5)
    }

    /// The texture taps of texel `i` of the surface for an image of `iw` x `ih` texels.
    pub fn taps(&self, s: &Surface, i: usize, xf: &UvXform, iw: u32, ih: u32) -> Taps {
        let r = &self.r;
        let d = [s.pos[i][0] - self.position[0], s.pos[i][1] - self.position[1], s.pos[i][2] - self.position[2]];
        // local = R^T d
        let local = [
            r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2],
            r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2],
            r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2],
        ];
        // centred coordinates in projector units
        let c = [local[0] / self.size[0], local[1] / self.size[1], local[2] / self.size[2]];
        let look = |u: f32, v: f32| {
            let (u, v) = xf.map(u + 0.5, v + 0.5);
            // outside the box the edge texel continues: clamp to texel centres so the bilinear lookup does not
            // wrap around to the opposite edge
            let (hu, hv) = (0.5 / iw as f32, 0.5 / ih as f32);
            if self.repeat {
                (u, v)
            } else {
                (u.clamp(hu, 1.0 - hu), v.clamp(hv, 1.0 - hv))
            }
        };
        // The texture is laid on the surface as seen from outside along the projection axis, with the image's
        // up pointing along +y on the side faces (fitted per half axis against a tri-planar mask exported by
        // Painter, correlation 0.92 - 0.997): +x faces u = -z, -x faces u = +z, +z faces u = +x, -z faces
        // u = -x, v = -y; top / bottom faces u = +x, v = +/-z.
        if self.planar {
            // experiment: SHARD_PLANAR bit0 swaps the axes, bit1 flips u, bit2 flips v (default 0)
            let m = self.planar_mode;
            let (a, b) = if m & 1 != 0 { (-c[1], c[0]) } else { (c[0], -c[1]) };
            let (a, b) = (if m & 2 != 0 { -a } else { a }, if m & 4 != 0 { -b } else { b });
            return Taps { count: 1, uv: [look(a, b), (0.0, 0.0), (0.0, 0.0)], weight: [1.0, 0.0, 0.0] };
        }
        let nn = s.nrm[i];
        let nl = [
            r[0][0] * nn[0] + r[1][0] * nn[1] + r[2][0] * nn[2],
            r[0][1] * nn[0] + r[1][1] * nn[1] + r[2][1] * nn[2],
            r[0][2] * nn[0] + r[1][2] * nn[1] + r[2][2] * nn[2],
        ];
        let pow = |x: f32| {
            let a = x.abs();
            if self.pw == 4.0 {
                let a2 = a * a;
                a2 * a2
            } else {
                a.powf(self.pw)
            }
        };
        let w = [pow(nl[0]), pow(nl[1]), pow(nl[2])];
        let sum = (w[0] + w[1] + w[2]).max(1e-9);
        let sg = |v: f32| if v < 0.0 { -1.0f32 } else { 1.0 };
        let uvs = [(-sg(nl[0]) * c[2], -c[1]), (c[0], sg(nl[1]) * c[2]), (sg(nl[2]) * c[0], -c[1])];
        let mut t = Taps { count: 0, uv: [(0.0, 0.0); 3], weight: [0.0; 3] };
        for a in 0..3 {
            if w[a] > 1e-4 * sum {
                t.uv[t.count] = look(uvs[a].0, uvs[a].1);
                t.weight[t.count] = w[a] / sum;
                t.count += 1;
            }
        }
        t
    }
}

/// Sample `img` through a planar / tri-planar projector at texel `i` of the surface. `None` when the
/// projection has no 3D placement (the caller then uses the UV mapping). Per-texel callers should prepare
/// a [`ProjPrep`] once instead.
pub fn project_sample(img: &Image, c_i: usize, n: usize, s: &Surface, i: usize, xf: &UvXform, spec: &ProjSpec) -> Option<f32> {
    let prep = ProjPrep::new(spec)?;
    Some(prep.taps(s, i, xf, img.w, img.h).sample(img, c_i, n))
}

/// Tri-planar parameters (experiment knobs: `SHARD_TRI_SCALE`, `SHARD_TRI_POW`, `SHARD_TRI_UNIFORM`).
fn tri_params() -> (f32, f32, bool) {
    let f = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
    (f("SHARD_TRI_SCALE", 1.0), f("SHARD_TRI_POW", 4.0), std::env::var_os("SHARD_TRI_UNIFORM").is_some())
}

/// Sample `img` projected from the three object axes and blended by the surface normal.
/// `tiling`/`offset` are the fill's UV transform values applied in projection space.
pub fn triplanar_sample(img: &Image, c: usize, n: usize, s: &Surface, i: usize, xf: &UvXform) -> f32 {
    let (k, pw, uniform) = tri_params();
    let ext = [(s.hi[0] - s.lo[0]).max(1e-6), (s.hi[1] - s.lo[1]).max(1e-6), (s.hi[2] - s.lo[2]).max(1e-6)];
    let m = ext[0].max(ext[1]).max(ext[2]);
    let p = s.pos[i];
    let q = [
        (p[0] - s.lo[0]) / if uniform { m } else { ext[0] },
        (p[1] - s.lo[1]) / if uniform { m } else { ext[1] },
        (p[2] - s.lo[2]) / if uniform { m } else { ext[2] },
    ];
    let nn = s.nrm[i];
    let w = [nn[0].abs().powf(pw), nn[1].abs().powf(pw), nn[2].abs().powf(pw)];
    let sum = (w[0] + w[1] + w[2]).max(1e-9);
    let uvs = [(q[1], q[2]), (q[0], q[2]), (q[0], q[1])];
    let mut acc = 0.0;
    for a in 0..3 {
        if w[a] <= 1e-4 * sum {
            continue;
        }
        let (u, v) = xf.map(uvs[a].0 * k, uvs[a].1 * k);
        acc += w[a] / sum * crate::sample(img, u, v, c, n);
    }
    acc
}

/// Exact evaluation through the external renderer, resampled through the fill's UV transform (UV projection)
/// or projected from the surface (tri-planar).
fn rendered_field(
    p: &Procedural,
    output: Option<&str>,
    w: u32,
    h: u32,
    maps: &dyn MeshMaps,
    xf: &UvXform,
    spec: &ProjSpec,
) -> Option<Vec<f32>> {
    let (rw, rh) = render_size(p, w, h)?;
    let img = maps.render_graph(p, output, rw, rh, None)?;
    // a 3D projector places the texture exactly; without one the old bbox-normalised tri-planar is an
    // experiment (SHARD_TRIPLANAR)
    let surface = if spec.projector.is_some() && matches!(spec.kind, shard_model::Projection::Planar | shard_model::Projection::Triplanar)
        && std::env::var_os("SHARD_NO_PROJECTOR").is_none()
    {
        maps.surface(w, h)
    } else if spec.kind == shard_model::Projection::Triplanar && std::env::var_os("SHARD_TRIPLANAR").is_some() {
        maps.surface(w, h)
    } else {
        None
    };
    let prep = surface.as_ref().and_then(|_| ProjPrep::new(spec));
    // the UV lookups read the mip level of the texture that matches their footprint
    let uv_img = mip_for(img.clone(), xf, w);
    let mut out = vec![0.0f32; (w * h) as usize];
    par_rows(&mut out, w as usize, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w as usize + x;
            *o = match &surface {
                Some(s) if s.covered[i] => match &prep {
                    Some(pr) => pr.taps(s, i, xf, img.w, img.h).sample(&img, 0, 1),
                    None => triplanar_sample(&img, 0, 1, s, i, xf),
                },
                _ => {
                    let (u, v) = xf.map((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    crate::sample(&uv_img, u, v, 0, 1)
                }
            };
        }
    });
    Some(out)
}

/// Geometry seen from texture space: per texel the object-space position and unit normal of the surface
/// point it stands for (`covered` = the texel belongs to an island). Used by the non-UV projections.
pub struct Surface {
    pub w: u32,
    pub h: u32,
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub covered: Vec<bool>,
    /// Bounding box of the whole scene (object space).
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

pub struct NoMaps;

impl MeshMaps for NoMaps {
    fn mesh_map(&self, _: MeshMap) -> Option<Arc<Image>> {
        None
    }
}

fn tweak<'a>(p: &'a Procedural, name: &str) -> Option<&'a Tweak> {
    p.tweaks.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

fn tf(p: &Procedural, name: &str, default: f32) -> f32 {
    match tweak(p, name) {
        Some(Tweak::Float(f)) => *f,
        Some(Tweak::Int(i)) => *i as f32,
        Some(Tweak::Bool(b)) => *b as i32 as f32,
        _ => default,
    }
}

fn hash(ix: i32, iy: i32, seed: u32) -> f32 {
    let mut h = (ix as u32).wrapping_mul(0x9E37_79B1) ^ (iy as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Tileable value noise: `period` lattice cells across the unit square.
fn value_noise(u: f32, v: f32, period_x: i32, period_y: i32, seed: u32) -> f32 {
    let (x, y) = (u * period_x as f32, v * period_y as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let at = |dx: i32, dy: i32| hash((x0 as i32 + dx).rem_euclid(period_x), (y0 as i32 + dy).rem_euclid(period_y), seed);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bot = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bot - top) * sy
}

/// Sparse thin scratches: one random segment per lattice cell (tileable), 1 on the line, 0 elsewhere.
fn scratch_field(u: f32, v: f32, cells: i32, seed: u32, quantity: f32, width: f32, length: f32) -> f32 {
    let (x, y) = (u * cells as f32, v * cells as f32);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let mut best = 0.0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (ix, iy) = ((cx + dx).rem_euclid(cells), (cy + dy).rem_euclid(cells));
            // roughly one scratch in two cells
            if hash(ix, iy, seed ^ 0x51) > quantity {
                continue;
            }
            let px = (cx + dx) as f32 + hash(ix, iy, seed ^ 0x11);
            let py = (cy + dy) as f32 + hash(ix, iy, seed ^ 0x23);
            let ang = hash(ix, iy, seed ^ 0x37) * std::f32::consts::PI;
            let half = (0.35 + 0.9 * hash(ix, iy, seed ^ 0x49)) * length;
            let (ax, ay) = (ang.cos(), ang.sin());
            // distance from the point to the segment (p +- half * dir)
            let (rx, ry) = (x - px, y - py);
            let t = (rx * ax + ry * ay).clamp(-half, half);
            let (qx, qy) = (rx - ax * t, ry - ay * t);
            let d = (qx * qx + qy * qy).sqrt();
            let thick = (0.06 + 0.07 * hash(ix, iy, seed ^ 0x5d)) * width;
            let strength = 0.5 + 0.5 * hash(ix, iy, seed ^ 0x6b);
            best = best.max((1.0 - d / thick).clamp(0.0, 1.0) * strength);
        }
    }
    best
}

fn fbm(u: f32, v: f32, base_x: i32, base_y: i32, octaves: u32, seed: u32) -> f32 {
    let (mut amp, mut sum, mut norm) = (1.0f32, 0.0f32, 0.0f32);
    let (mut px, mut py) = (base_x, base_y);
    for o in 0..octaves {
        sum += amp * value_noise(u, v, px, py, seed.wrapping_add(o * 101));
        norm += amp;
        amp *= 0.5;
        px *= 2;
        py *= 2;
    }
    sum / norm
}

/// Histogram shaping shared by the noise graphs: balance shifts, contrast stretches, invert flips.
fn shape(v: f32, balance: f32, contrast: f32, invert: bool) -> f32 {
    let mut x = v + (balance - 0.5);
    x = (x - 0.5) * (1.0 + 3.0 * contrast) + 0.5;
    x = x.clamp(0.0, 1.0);
    if invert {
        1.0 - x
    } else {
        x
    }
}

fn resample(img: &Image, w: u32, h: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            out.push(crate::sample(img, u, v, 0, 1));
        }
    }
    out
}

/// What kind of approximation was used (for reporting).
pub struct Generated {
    pub values: Vec<f32>,
    pub approx: String,
}

/// Colour filters that need the three channels together (`n` = 3, interleaved RGB). None: not one.
pub fn filter_color(p: &Procedural, px: &mut [f32], n: usize) -> Option<String> {
    if n != 3 {
        return None;
    }
    match p.graph_name()? {
        "hsl_perceptive" | "hsl" => {
            // 0.5 is neutral for all three (interface defaults)
            let hue = (tf(p, "hue", 0.5) - 0.5) * 360.0;
            let sat = tf(p, "saturation", 0.5) * 2.0;
            let lum = (tf(p, "luminosity", 0.5) - 0.5) * 2.0;
            for c in px.chunks_exact_mut(3) {
                let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
                let h = (h + hue).rem_euclid(360.0);
                let s = (s * sat).clamp(0.0, 1.0);
                let l = if lum < 0.0 { l * (1.0 + lum) } else { l + (1.0 - l) * lum };
                let (r, g, b) = hsl_to_rgb(h, s, l.clamp(0.0, 1.0));
                c[0] = r;
                c[1] = g;
                c[2] = b;
            }
            Some("hsl: hue/saturation/luminosity".into())
        }
        _ => None,
    }
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) * 0.5;
    let d = mx - mn;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s.clamp(0.0, 1.0), l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    (r + m, g + m, b + m)
}

/// Constant value of one output of a material-style graph (several outputs: base colour, roughness...).
pub struct OutputValue {
    pub values: Vec<f32>,
    pub note: String,
}

fn tf3(p: &Procedural, name: &str, default: [f32; 3]) -> [f32; 3] {
    match tweak(p, name) {
        Some(Tweak::Float3(v)) => *v,
        Some(Tweak::Float4(v)) => [v[0], v[1], v[2]],
        Some(Tweak::Float(f)) => [*f; 3],
        _ => default,
    }
}

/// Approximated value of output `output` of a material graph with `n` components. Colour inputs of these
/// graphs are stored as sRGB fractions; the compositor works in linear light.
pub fn generate_output(p: &Procedural, output: &str, n: usize) -> Option<OutputValue> {
    let name = p.graph_name()?;
    let srgb_lin = |c: [f32; 3]| c.map(|v| crate::srgb_to_linear(v.clamp(0.0, 1.0)));
    let grey_or_rgb = |c: [f32; 3]| -> Vec<f32> {
        if n == 3 {
            c.to_vec()
        } else {
            vec![(c[0] + c[1] + c[2]) / 3.0]
        }
    };
    let (values, what) = match (name, output) {
        ("steel_rust_and_wear", "basecolor" | "diffuse") => {
            let c = tf3(p, "metal_color", [0.352682, 0.347707, 0.372549]);
            // luminosity 0.5 is neutral; the offset is added in display (sRGB) space
            let lum = (tf(p, "luminosity", 0.5) - 0.5) * 2.0;
            (grey_or_rgb(srgb_lin(c.map(|v| v + lum))), "steel: metal colour + luminosity")
        }
        ("steel_rust_and_wear", "roughness" | "glossiness") => {
            let r = tf(p, "metal_roughness", 0.27451) + tf(p, "dirt", 0.4) * 0.13;
            (vec![r.clamp(0.0, 1.0); n.max(1)], "steel: metal roughness + dirt")
        }
        ("steel_rust_and_wear", "metallic") => (vec![1.0; n.max(1)], "steel: fully metallic"),
        ("coated_metal", "basecolor" | "diffuse") => {
            let c = tf3(p, "color", [0.385057, 0.351895, 0.278199]);
            (grey_or_rgb(srgb_lin(c)), "coated metal: base colour (flakes ignored)")
        }
        ("coated_metal", "roughness" | "glossiness") => (vec![tf(p, "roughness", 0.35); n.max(1)], "coated metal: roughness"),
        ("coated_metal", "metallic") => (vec![1.0; n.max(1)], "coated metal: metallic"),
        _ => return None,
    };
    Some(OutputValue { values, note: format!("{name}: {what}") })
}

/// Fill `out` (rows of `width`) in parallel: `f(row index, row)`.
pub(crate) fn par_rows(out: &mut [f32], width: usize, f: impl Fn(usize, &mut [f32]) + Sync) {
    use rayon::prelude::*;
    let width = width.max(1);
    let rows = out.len() / width;
    if rows < 64 {
        for (y, row) in out.chunks_mut(width).enumerate() {
            f(y, row);
        }
        return;
    }
    // blocks of rows keep the per-task overhead small next to a row of work
    let block = 8;
    out.par_chunks_mut(width * block).enumerate().for_each(|(bi, chunk)| {
        for (k, row) in chunk.chunks_mut(width).enumerate() {
            f(bi * block + k, row);
        }
    });
}

/// Single-channel output of a generator graph, or None when the graph is not covered.
/// Fill-layer UV transformation applied to a generator's domain: tiling, rotation (degrees about the
/// pivot) and offset. `map` takes a texture coordinate to the coordinate the generator is sampled at.
#[derive(Clone, Copy)]
pub struct UvXform {
    scale: [f32; 2],
    offset: [f32; 2],
    pivot: [f32; 2],
    cos: f32,
    sin: f32,
}

impl UvXform {
    pub fn new(uv: &shard_model::UvTransform) -> Self {
        // Stored in radians and turning the other way than this mapping's angle (a test vehicle's `Color Variation 2`
        // holds 1.3089970 = 75 degrees: read as degrees, or with the other sign, its tri-planar clouds had
        // correlation 0.06 / -0.14 with Painter's; as it is now 0.993). SHARD_UV_ROT_SIGN, SHARD_UV_ROT_DEGREES=1.
        let raw = uv.rotation.unwrap_or(0.0);
        let rot = if std::env::var_os("SHARD_UV_ROT_DEGREES").is_some() { raw.to_radians() } else { raw }
            * std::env::var("SHARD_UV_ROT_SIGN").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(-1.0);
        Self {
            scale: uv.scale.unwrap_or([1.0, 1.0]),
            offset: uv.position.unwrap_or([0.0, 0.0]),
            pivot: uv.pivot.unwrap_or([0.5, 0.5]),
            cos: (-rot).cos(),
            sin: (-rot).sin(),
        }
    }

    /// Texels of an `img_w` wide texture that one texel of an `out_w` wide target spans (> 1: minified).
    pub fn footprint(&self, img_w: u32, out_w: u32) -> f32 {
        self.scale[0].abs().max(self.scale[1].abs()) * img_w as f32 / out_w.max(1) as f32
    }

    #[inline]
    pub fn map(&self, u: f32, v: f32) -> (f32, f32) {
        let (cx, cy) = (u - self.pivot[0], v - self.pivot[1]);
        let (rx, ry) = (cx * self.cos - cy * self.sin, cx * self.sin + cy * self.cos);
        (
            rx * self.scale[0] + self.pivot[0] - self.offset[0],
            ry * self.scale[1] + self.pivot[1] - self.offset[1],
        )
    }
}

/// The mip level of `img` a texture lookup through `xf` into an `out_w` wide target reads: halved (box) while
/// one target texel spans two texels or more. Painter filters its texture lookups (BilinearHQ, mip-mapped); a
/// plain bilinear tap of a tiled texture aliased into noise (a test tank's `noise 01` mask, tiled 5 times: the same
/// mean, a correlation of 0.26 with Painter's). SHARD_NO_MIP=1 samples the full picture.
pub fn mip_for(img: Arc<Image>, xf: &UvXform, out_w: u32) -> Arc<Image> {
    if std::env::var_os("SHARD_NO_MIP").is_some() {
        return img;
    }
    let mut cur = img;
    while xf.footprint(cur.w, out_w) >= 2.0 && cur.w >= 2 && cur.h >= 2 {
        let (w, h, n) = ((cur.w / 2) as usize, (cur.h / 2) as usize, cur.n);
        let sw = cur.w as usize;
        let mut data = vec![0.0f32; w * h * n];
        for y in 0..h {
            for x in 0..w {
                for c in 0..n {
                    let at = |xx: usize, yy: usize| cur.data[(yy * sw + xx) * n + c];
                    data[(y * w + x) * n + c] =
                        (at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1)) * 0.25;
                }
            }
        }
        cur = Arc::new(Image { w: w as u32, h: h as u32, n, data });
    }
    cur
}

/// [`generate`] at the source's resolution override (Resolution / Factor of the FILL panel): the
/// graph is evaluated at the overridden size and resampled to the context, so 1/4 looks soft and x2
/// is supersampled, as in Painter. Mesh-aware generators (`mg_*`) always use the context size.
pub fn generate_scaled(
    p: &Procedural,
    w: u32,
    h: u32,
    maps: &dyn MeshMaps,
    uv: &shard_model::UvTransform,
    spec: &ProjSpec,
) -> Option<Generated> {
    if std::env::var_os("SHARD_NO_ORACLE").is_none() {
        let xf = UvXform::new(uv);
        if let Some(values) = rendered_field(p, None, w, h, maps, &xf, spec) {
            return Some(Generated { values, approx: format!("{}: Substance engine (exact)", p.graph_name().unwrap_or("?")) });
        }
    }
    let (gw, gh) = p.resolution.target(w, h);
    let gw = gw.min(4096);
    let gh = gh.min(4096);
    if (gw, gh) == (w, h) || p.graph_name().is_some_and(|n| n.starts_with("mg_")) {
        return generate(p, w, h, maps, uv);
    }
    let mut g = generate(p, gw, gh, maps, uv)?;
    g.values = resample_map(&g.values, gw as usize, gh as usize, w as usize, h as usize);
    Some(g)
}

/// Bilinear resample of a one-component map (box-averaged first when shrinking by 2x or more).
pub(crate) fn resample_map(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut cur: std::borrow::Cow<[f32]> = std::borrow::Cow::Borrowed(src);
    let (mut cw, mut ch) = (sw, sh);
    while cw >= dw * 2 && ch >= dh * 2 {
        let (nw, nh) = (cw / 2, ch / 2);
        let mut next = vec![0.0f32; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let i = (y * 2) * cw + x * 2;
                next[y * nw + x] = (cur[i] + cur[i + 1] + cur[i + cw] + cur[i + cw + 1]) * 0.25;
            }
        }
        cur = std::borrow::Cow::Owned(next);
        cw = nw;
        ch = nh;
    }
    let mut out = vec![0.0f32; dw * dh];
    par_rows(&mut out, dw, |y, row| {
        let fy = ((y as f32 + 0.5) * ch as f32 / dh as f32 - 0.5).clamp(0.0, (ch - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(ch - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) * cw as f32 / dw as f32 - 0.5).clamp(0.0, (cw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(cw - 1);
            let a = cur[y0 * cw + x0] * (1.0 - tx) + cur[y0 * cw + x1] * tx;
            let b = cur[y1 * cw + x0] * (1.0 - tx) + cur[y1 * cw + x1] * tx;
            *o = a * (1.0 - ty) + b * ty;
        }
    });
    out
}

pub fn generate(p: &Procedural, w: u32, h: u32, maps: &dyn MeshMaps, uv: &shard_model::UvTransform) -> Option<Generated> {
    let xf = UvXform::new(uv);
    let name = p.graph_name()?.to_string();
    if std::env::var("SHARD_SKIP_PROC").is_ok_and(|v| v.split(',').any(|x| x == name)) {
        return None; // diagnostic ablation
    }
    let seed = tf(p, "$randomseed", 0.0) as i32 as u32;
    let invert = tf(p, "invert", 0.0) > 0.5;
    let balance = tf(p, "balance", 0.5);
    let contrast = tf(p, "contrast", 0.0);
    let noise = |base: i32, octaves: u32, ridged: bool, stretch_x: i32| -> Vec<f32> {
        let mut out = vec![0.0f32; (w * h) as usize];
        par_rows(&mut out, w as usize, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let (u, v) = xf.map((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let mut n = fbm(u, v, base * stretch_x, base, octaves, seed);
                if ridged {
                    n = 1.0 - (2.0 * n - 1.0).abs();
                }
                *o = shape(n, balance, contrast, invert);
            }
        });
        out
    };
    let g = |values: Vec<f32>, what: &str| Some(Generated { values, approx: format!("{name}: {what}") });
    // shelf grunges are their embedded bitmap plus a small histogram graph: use the real texture
    let bitmap_grunge = name.starts_with("grunge_") || name.starts_with("ratio_grunge_") || name.starts_with("ratio_dirt");
    if bitmap_grunge && std::env::var_os("SHARD_NO_EMBEDDED").is_none() {
        if let Some(img) = p.resource.as_deref().and_then(|r| maps.embedded(r, 0)) {
            let tiling = tf(p, "tiling_amount", 1.0).max(1.0);
            let (flip_h, flip_v) = (tf(p, "flip_horizontal", 0.0) > 0.5, tf(p, "flip_vertical", 0.0) > 0.5);
            let rot45 = tf(p, "rotate_by_45_degrees", 0.0) > 0.5;
            let mut out = vec![0.0f32; (w * h) as usize];
            par_rows(&mut out, w as usize, |y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let (mut u, mut v) = xf.map((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    if flip_h {
                        u = 1.0 - u;
                    }
                    if flip_v {
                        v = 1.0 - v;
                    }
                    if rot45 {
                        // 45 degrees with a sqrt(2) scale keeps the tiling seamless
                        let (a, b) = (u - v, u + v);
                        u = a;
                        v = b;
                    }
                    let t = crate::sample(&img, u * tiling, v * tiling, 0, 1);
                    *o = shape(t, balance, contrast, invert);
                }
            });
            return g(out, "embedded grunge bitmap (exact source), histogram shaping guessed");
        }
    }
    match name.as_str() {
        "clouds_2" | "clouds_3" | "ratio_clouds_1" | "ratio_clouds_2" | "ratio_clouds_3" | "perlin_noise" => {
            let scale = tf(p, "scale", 1.0).max(1.0) as i32;
            g(noise(2 * scale + 1, 6, false, 1), "fractal value noise")
        }
        n if n.starts_with("grunge_scratches") || n.starts_with("ratio_grunge_scratches") => {
            // graph parameters (Scratch_Quantity / Width / Length / Tiling / Double_Scratch of the ratio_ variants)
            let envf = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
            let quantity = (tf(p, "Scratch_Quantity", 0.6) * envf("SHARD_SCR_Q", 0.3)).clamp(0.0, 1.0);
            let width = (tf(p, "Scratch_Width", 0.25) / 0.11 * envf("SHARD_SCR_T", 0.3)).clamp(0.05, 4.0);
            let length = tf(p, "Scratch_Length", 1.0).clamp(0.2, 3.0);
            let double = tf(p, "Double_Scratch", 0.5).clamp(0.0, 1.0);
            let cells = 28 * tf(p, "Scratch_Tiling", 1.0).max(1.0) as i32;
            let mut out = vec![0.0f32; (w * h) as usize];
            par_rows(&mut out, w as usize, |y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let (u, v) = xf.map((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    // two layers of different density so scratches are not all alike
                    let a = scratch_field(u, v, cells, seed, quantity, width, length);
                    let b = scratch_field(u, v, cells * 2, seed.wrapping_add(7), quantity * double, width, length) * 0.6;
                    *o = shape(a.max(b), 0.5 + (balance - 0.5) * 0.4, contrast * 0.5, invert);
                }
            });
            g(out, "random thin scratches")
        }
        n if n.starts_with("grunge_") || n.starts_with("ratio_grunge_") || n.starts_with("ratio_dirt") => {
            g(noise(5, 6, false, 1), "grunge-like fractal noise")
        }
        "bnw_spots_2" | "ratio_moisture_noise" | "anisotropic_noise" => g(noise(10, 4, false, 1), "fractal noise"),
        "mg_curvature" => {
            let map = maps.mesh_map(MeshMap::Curvature)?;
            let raw = resample(&map, w, h);
            let gain = 2.0 + 6.0 * tf(p, "curvature_sharp", 0.5);
            let bal = tf(p, "global_balance", 0.5);
            let con = tf(p, "global_contrast", 0.0);
            let inv = tf(p, "global_invert", 0.0) > 0.5;
            let values = raw
                .into_iter()
                .map(|c| shape(0.5 + (c - 0.5) * gain, 0.5 + (bal - 0.5) * 2.0, con, inv))
                .collect();
            g(values, "baked curvature, levels guessed")
        }
        "mg_metal_edge_wear" => {
            // convex edges of the baked curvature, broken up by grunge and held back in cavities
            let curv = resample(&*maps.mesh_map(MeshMap::Curvature)?, w, h);
            let ao = maps.mesh_map(MeshMap::AmbientOcclusion).map(|m| resample(&*m, w, h));
            let weight = tf(p, "Curvature_Weight", 1.0);
            let level = tf(p, "Wear_Level", 0.75);
            let contrast = tf(p, "Wear_Contrast", 0.5);
            let grunge = tf(p, "Grunge_Amount", 0.3);
            let aom = tf(p, "Ambient_Occlusion_Masking", 0.5);
            let inv = tf(p, "invert", 0.0) > 0.5;
            let scale = tf(p, "grunge_scale", 1.0).max(1.0) as i32;
            // the generator ships its own breakup textures
            let breakup = if std::env::var_os("SHARD_NO_EMBEDDED").is_none() {
                p.resource.as_deref().and_then(|r| maps.embedded(r, 0))
            } else {
                None
            };
            let mut out = vec![0.0f32; (w * h) as usize];
            par_rows(&mut out, w as usize, |y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let i = y * w as usize + x;
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    let n = match &breakup {
                        Some(img) => crate::sample(img, u * scale as f32, v * scale as f32, 0, 1),
                        None => fbm(u, v, 4 * scale, 4 * scale, 5, seed),
                    };
                    let mut e = ((curv[i] - 0.5) * 2.0 * (1.0 + 3.0 * weight)).clamp(0.0, 1.0);
                    e *= 1.0 - grunge * (1.0 - n);
                    if let Some(ao) = &ao {
                        e *= 1.0 - aom * (1.0 - ao[i]);
                    }
                    let t = (1.0 - level) * 0.5;
                    let r = ((e - t) * (1.0 + contrast * 4.0)).clamp(0.0, 1.0);
                    *o = if inv { 1.0 - r } else { r };
                }
            });
            g(out, "metal edge wear: curvature edges + grunge + AO (guessed)")
        }
        "mg_ambient_occlusion" => {
            let map = maps.mesh_map(MeshMap::AmbientOcclusion)?;
            let raw = resample(&map, w, h);
            let bal = tf(p, "global_balance", 0.5);
            let con = tf(p, "global_contrast", 0.0);
            let inv = tf(p, "global_invert", 0.0) > 0.5;
            let values = raw.into_iter().map(|a| shape(1.0 - a, bal, con, inv)).collect();
            g(values, "baked occlusion, levels guessed")
        }
        "gradient_linear_1" => {
            // ramp along u (rotation in quarter turns), tiled, shifted by Position, steepened by Contrast
            let tiling = tf(p, "Tiling", 1.0).max(1.0);
            let pos = tf(p, "Position", 0.5);
            let rot = tf(p, "rotation", 0.0).round() as i32 & 3;
            let con = tf(p, "Contrast", 0.0);
            let inv = tf(p, "invert", 0.0) > 0.5;
            g(
                field(w, h, |u, v| {
                    let (a, b) = match rot {
                        0 => (u, v),
                        1 => (v, 1.0 - u),
                        2 => (1.0 - u, 1.0 - v),
                        _ => (1.0 - v, u),
                    };
                    let _ = b;
                    let t = (a * tiling + (0.5 - pos) + 1.0).rem_euclid(1.0);
                    let x = (t - 0.5) * (1.0 + 6.0 * con) + 0.5;
                    let x = x.clamp(0.0, 1.0);
                    if inv {
                        1.0 - x
                    } else {
                        x
                    }
                }),
                "linear gradient",
            )
        }
        "metallic_grate_wide" => {
            // grid of wide rounded slots, `tile` per axis, optionally turned 45 degrees
            let tile = tf(p, "tile", 8.0).max(1.0);
            let con = tf(p, "Contrast", 0.5);
            let pos = tf(p, "Position", 0.5);
            let rot45 = tf(p, "rotate_45", 0.0) > 0.5;
            let inv = tf(p, "invert", 0.0) > 0.5;
            g(
                field(w, h, |u, v| {
                    let (mut x, mut y) = (u - 0.5, v - 0.5);
                    let t = if rot45 { tile * std::f32::consts::SQRT_2 } else { tile };
                    if rot45 {
                        let (c, s) = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
                        let (rx, ry) = (x * c - y * s, x * s + y * c);
                        x = rx;
                        y = ry;
                    }
                    let (fx, fy) = ((x * t).rem_euclid(1.0) - 0.5, (y * t).rem_euclid(1.0) - 0.5);
                    // wide slot: |fx| < 0.42, |fy| < 0.16 with soft edges
                    let dx = (fx.abs() - 0.42) * 8.0;
                    let dy = (fy.abs() - 0.16) * 8.0;
                    let inside = 1.0 - dx.max(dy).clamp(0.0, 1.0);
                    let x = ((inside - 0.5) * (1.0 + 4.0 * con) + 0.5 + (pos - 0.5) * 0.5).clamp(0.0, 1.0);
                    if inv {
                        1.0 - x
                    } else {
                        x
                    }
                }),
                "grate pattern",
            )
        }
        // N.L approximation worsened the measured Base color of the interior sets: opt-in only
        "light" if std::env::var_os("SHARD_APPROX_LIGHT").is_some() => {
            // N.L of the baked tangent-space normal for a light at (Horizontal_Angle, Vertical_Angle) turns
            let map = maps.mesh_map(MeshMap::NormalBase)?;
            let hz = tf(p, "Horizontal_Angle", 0.25) * std::f32::consts::TAU;
            let vt = tf(p, "Vertical_Angle", 0.125) * std::f32::consts::TAU;
            let l = [vt.cos() * hz.cos(), vt.cos() * hz.sin(), vt.sin()];
            let inv = tf(p, "invert", 0.0) > 0.5;
            let mut values = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    let n = [
                        crate::sample(&map, u, v, 0, 3) * 2.0 - 1.0,
                        crate::sample(&map, u, v, 1, 3) * 2.0 - 1.0,
                        crate::sample(&map, u, v, 2, 3) * 2.0 - 1.0,
                    ];
                    let nl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).clamp(0.0, 1.0);
                    values.push(if inv { 1.0 - nl } else { nl });
                }
            }
            g(values, "N.L from the baked normal map")
        }
        "mg_mask_editor" => {
            // AO + curvature driven smart mask: keep the occlusion term with the global shaping
            let map = maps.mesh_map(MeshMap::AmbientOcclusion)?;
            let raw = resample(&map, w, h);
            let bal = tf(p, "global_balance", 0.5);
            let con = tf(p, "global_contrast", 0.0);
            let inv = tf(p, "global_invert", 0.0) > 0.5;
            let values = raw.into_iter().map(|a| shape(1.0 - a, bal, con, inv)).collect();
            g(values, "mask editor: occlusion term only")
        }
        "mg_dirt" => {
            // dirt gathers in occluded areas and follows a grunge pattern
            let map = maps.mesh_map(MeshMap::AmbientOcclusion);
            let scale = tf(p, "Grunge_Scale", 2.0).max(1.0) as i32;
            let amount = tf(p, "grunge_amount", 0.5);
            let level = tf(p, "dirt_level", 0.5);
            let con = tf(p, "dirt_contrast", 0.3);
            let occl = map.map(|m| resample(&m, w, h));
            let mut values = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    let n = fbm(u, v, 3 * scale + 1, 3 * scale + 1, 6, seed);
                    let o = occl.as_ref().map(|o| 1.0 - o[(y * w + x) as usize]).unwrap_or(0.5);
                    let d = o * (1.0 - amount) + n * amount;
                    values.push(shape(d, 1.0 - level, con, false));
                }
            }
            g(values, "dirt: occlusion + grunge")
        }
        _ => None,
    }
}

/// Evaluate `f(u, v)` over the texture.
fn field(w: u32, h: u32, f: impl Fn(f32, f32) -> f32 + Sync) -> Vec<f32> {
    let mut out = vec![0.0f32; (w * h) as usize];
    par_rows(&mut out, w as usize, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        }
    });
    out
}

/// Wrapping box blur of radius `r` (window of `2r + 1`): a sliding sum per row, rows in parallel, then per
/// column row by row. Cost does not depend on the radius.
fn box_blur(src: &[f32], w: u32, h: u32, r: u32) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let (wu, hu, ru) = (w as usize, h as usize, r as usize);
    let k = (2 * ru + 1) as f64;
    let mut tmp = vec![0.0f32; src.len()];
    par_rows(&mut tmp, wu, |y, row| {
        let line = &src[y * wu..(y + 1) * wu];
        let at = |i: isize| line[i.rem_euclid(wu as isize) as usize] as f64;
        let mut acc: f64 = (-(ru as isize)..=ru as isize).map(&at).sum();
        for (x, o) in row.iter_mut().enumerate() {
            *o = (acc / k) as f32;
            acc += at(x as isize + ru as isize + 1) - at(x as isize - ru as isize);
        }
    });
    let mut out = vec![0.0f32; src.len()];
    let row_of = |i: isize| i.rem_euclid(hu as isize) as usize * wu;
    let mut acc = vec![0.0f64; wu];
    for d in -(ru as isize)..=ru as isize {
        let base = row_of(d);
        for (a, t) in acc.iter_mut().zip(&tmp[base..base + wu]) {
            *a += *t as f64;
        }
    }
    for y in 0..hu {
        for (o, a) in out[y * wu..(y + 1) * wu].iter_mut().zip(&acc) {
            *o = (*a / k) as f32;
        }
        let (add, sub) = (row_of(y as isize + ru as isize + 1), row_of(y as isize - ru as isize));
        for x in 0..wu {
            acc[x] += tmp[add + x] as f64 - tmp[sub + x] as f64;
        }
    }
    out
}

/// The graphs [`filter`] reproduces closely enough to be used instead of the external renderer.
pub fn native_filter(p: &Procedural) -> bool {
    matches!(p.graph_name(), Some("sharpen" | "highpass" | "contrast_luminosity"))
}

/// `sharpen`: the input plus `intensity` times its 4-neighbour Laplacian, clamped, wrapping at the borders.
/// Fitted on the external renderer with noise input: kernel weights -0.350 / 2.400 at intensity 0.35, nothing
/// beyond the 4 neighbours (the renderer quantizes to 8 bits, which accounts for the remaining 4% of texels
/// one level off).
fn sharpen(v: &mut [f32], w: u32, h: u32, k: f32) {
    let (wu, hu) = (w as usize, h as usize);
    if wu == 0 || hu == 0 {
        return;
    }
    let src = v.to_vec();
    par_rows(v, wu, |y, row| {
        let (up, dn) = (((y + hu - 1) % hu) * wu, ((y + 1) % hu) * wu);
        let line = &src[y * wu..(y + 1) * wu];
        for (x, o) in row.iter_mut().enumerate() {
            let (l, r) = (line[(x + wu - 1) % wu], line[(x + 1) % wu]);
            let c = line[x];
            let lap = 4.0 * c - l - r - src[up + x] - src[dn + x];
            *o = (c + k * lap).clamp(0.0, 1.0);
        }
    });
}

/// Gaussian sigma (texels at 1024) of the blur inside `highpass`, measured against the external renderer by
/// fitting noise input at each `Radius` (8-bit mean error 0.3-1.5 levels); in between, log-log interpolation.
const HIGHPASS_SIGMA: [(f32, f32); 6] = [(0.2, 0.399), (0.4, 1.191), (0.8, 2.082), (1.6, 3.864), (3.2, 7.527), (6.4, 17.822)];

fn highpass_sigma(radius: f32) -> f32 {
    let t = &HIGHPASS_SIGMA;
    if radius <= 0.0 {
        return 0.0;
    }
    let (i, j) = match t.iter().position(|(r, _)| *r >= radius) {
        Some(0) => (0, 1),
        Some(k) => (k - 1, k),
        None => (t.len() - 2, t.len() - 1),
    };
    let (r0, s0, r1, s1) = (t[i].0.ln(), t[i].1.ln(), t[j].0.ln(), t[j].1.ln());
    (s0 + (radius.ln() - r0) * (s1 - s0) / (r1 - r0)).exp()
}

/// `highpass`: the input minus its gaussian blur, around 0.5.
fn highpass(v: &mut [f32], w: u32, h: u32, radius: f32) {
    let sigma = highpass_sigma(radius) * w as f32 / 1024.0;
    let b = gaussian_blur(v, w, h, sigma);
    for (x, b) in v.iter_mut().zip(b) {
        *x = (*x - b + 0.5).clamp(0.0, 1.0);
    }
}

/// Wrapping separable gaussian blur (sampled kernel out to 4 sigma; three box passes past 48 taps a side).
pub(crate) fn gaussian_blur(src: &[f32], w: u32, h: u32, sigma: f32) -> Vec<f32> {
    let (wu, hu) = (w as usize, h as usize);
    if sigma < 0.05 || wu == 0 || hu == 0 {
        return src.to_vec();
    }
    let r = (4.0 * sigma).ceil() as usize;
    if r > 48 {
        // box widths whose three passes have this variance (Wells 1986)
        let wi = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt()).floor().max(1.0) as u32;
        let rr = (wi.saturating_sub(1) / 2).max(1);
        let a = box_blur(src, w, h, rr);
        let b = box_blur(&a, w, h, rr);
        return box_blur(&b, w, h, rr);
    }
    let mut k: Vec<f32> = (0..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k[0] + 2.0 * k[1..].iter().sum::<f32>();
    k.iter_mut().for_each(|x| *x /= sum);
    let mut tmp = vec![0.0f32; src.len()];
    par_rows(&mut tmp, wu, |y, row| {
        // the row with `r` wrapped texels on each side, so the taps need no index wrapping
        let line = &src[y * wu..(y + 1) * wu];
        let ext: Vec<f32> = (0..wu + 2 * r).map(|i| line[(i + wu * (r / wu + 1) - r) % wu]).collect();
        for (x, o) in row.iter_mut().enumerate() {
            let c = x + r;
            let mut a = k[0] * ext[c];
            for (i, kv) in k.iter().enumerate().skip(1) {
                a += kv * (ext[c + i] + ext[c - i]);
            }
            *o = a;
        }
    });
    let mut out = vec![0.0f32; src.len()];
    par_rows(&mut out, wu, |y, row| {
        let base = &tmp[y * wu..(y + 1) * wu];
        row.copy_from_slice(base);
        row.iter_mut().for_each(|o| *o *= k[0]);
        for (i, kv) in k.iter().enumerate().skip(1) {
            let (a, b) = (((y + i) % hu) * wu, ((y + hu * (i / hu + 1) - i) % hu) * wu);
            for (x, o) in row.iter_mut().enumerate() {
                *o += kv * (tmp[a + x] + tmp[b + x]);
            }
        }
    });
    out
}

/// Apply a filter graph in place to a single-channel image. Returns a description when handled.
pub fn filter(p: &Procedural, v: &mut [f32], w: u32, h: u32) -> Option<String> {
    let name = p.graph_name()?.to_string();
    let px = |amount: f32| ((amount * w as f32 / 1024.0).round() as i64).clamp(0, 64) as u32;
    match name.as_str() {
        "blur" => {
            let r = px(tf(p, "intensity", 1.0));
            let b = box_blur(v, w, h, r);
            let b = box_blur(&b, w, h, r);
            v.copy_from_slice(&b);
            Some("blur: two box passes".into())
        }
        "sharpen" => {
            sharpen(v, w, h, tf(p, "intensity", 1.0));
            Some("sharpen (native)".into())
        }
        "highpass" => {
            highpass(v, w, h, tf(p, "Radius", 8.0));
            Some("highpass (native)".into())
        }
        "contrast_luminosity" => {
            // A levels: a positive contrast narrows the input range to [C/2, 1 - C/2], a negative one the
            // output range to [-C/2, 1 + C/2]; the luminosity shifts the output range (clamped to 0..1). Exact
            // against the external renderer on noise input (C -0.5 / 0.17 / 0.4, L -0.4 / -0.06 / 0.3).
            let c = tf(p, "Contrast", 0.0);
            let l = tf(p, "Luminosity", 0.0);
            let (ilo, ihi) = if c > 0.0 { (c / 2.0, 1.0 - c / 2.0) } else { (0.0, 1.0) };
            let (olo, ohi) = if c < 0.0 { (-c / 2.0, 1.0 + c / 2.0) } else { (0.0, 1.0) };
            let (olo, ohi) = ((olo + l).clamp(0.0, 1.0), (ohi + l).clamp(0.0, 1.0));
            let span = (ihi - ilo).max(1e-6);
            for x in v.iter_mut() {
                *x = olo + (ohi - olo) * ((*x - ilo) / span).clamp(0.0, 1.0);
            }
            Some("contrast/luminosity (native)".into())
        }
        "hsl_perceptive" => {
            // greyscale: only luminosity applies (0.5 neutral)
            let lum = (tf(p, "luminosity", 0.5) - 0.5) * 2.0;
            for x in v.iter_mut() {
                *x = if lum < 0.0 { *x * (1.0 + lum) } else { *x + (1.0 - *x) * lum };
            }
            Some("hsl: luminosity".into())
        }
        _ => None,
    }
}
