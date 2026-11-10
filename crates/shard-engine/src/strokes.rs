//! Replay of recorded paint strokes (`DataStroke`) into a texture-space buffer.
//!
//! Each stroke carries the camera it was painted with. Its keys are cast through that camera onto
//! the mesh; stamps are placed along the resulting 3D path at `spacing * diameter` and splatted onto
//! the texels of one texture set whose surface point lies within the brush radius (hard/soft falloff
//! from the brush `hardness`, `flow` per stamp, stroke `opacity` as cap). Conventions in
//! docs/FORMATS.md 7.5k.

use std::cell::RefCell;

use glam::{Mat4, Vec3, Vec4};
use shard_model::{PaintAction, Stroke};

use crate::mesh::Mesh;

struct Texel {
    idx: u32,
    pos: Vec3,
    normal: Vec3,
}

/// Surface samples of one texture set at one resolution, in a uniform 3D grid.
struct Cloud {
    w: u32,
    h: u32,
    texels: Vec<Texel>,
    lo: Vec3,
    cell: f32,
    dim: usize,
    cells: Vec<Vec<u32>>,
    /// Typical world-space size of a texel.
    texel_size: f32,
    /// World-space size of a texel on every triangle of the mesh (0 where the triangle is not of this set): the UV
    /// islands of one set are scaled very differently, and a brush smaller than the texels of its island has to
    /// reach the nearest texel centre all the same.
    tri_texel: Vec<f32>,
    /// Texel (index into `texels`) of every image index, `u32::MAX` where the set has no surface.
    by_idx: Vec<u32>,
}

impl Cloud {
    fn build(mesh: &Mesh, set: usize, w: u32, h: u32) -> Self {
        let mut filled = vec![false; (w * h) as usize];
        let mut texels: Vec<Texel> = Vec::new();
        let mut area = 0.0f64;
        let mut tri_texel = vec![0.0f32; mesh.indices.len() / 3];
        // a position already in a texel (to tell a second surface from the border of the same one), texels in all
        let mut first_pos: std::collections::HashMap<u32, Vec3> = std::collections::HashMap::new();
        // A texel covered by several surfaces (overlapping UVs) takes paint from all of them, as in Painter (with
        // the whole test vehicle scene read: interior metallic 1.65 -> 1.10, its hand-painted mask 2.6 -> 1.9, and the
        // hand-written signature appears). SHARD_SHARED_TEXELS=0: only the first surface over a texel.
        let shared_texels = std::env::var("SHARD_SHARED_TEXELS").map(|v| v != "0").unwrap_or(true);
        let mut unique = 0usize;
        for t in 0..mesh.indices.len() / 3 {
            if mesh.tri_mat.get(t).copied() != Some(set as u32) {
                continue;
            }
            let i = &mesh.indices[3 * t..3 * t + 3];
            let vs = [&mesh.vertices[i[0] as usize], &mesh.vertices[i[1] as usize], &mesh.vertices[i[2] as usize]];
            let p: [Vec3; 3] = [Vec3::from(vs[0].pos), Vec3::from(vs[1].pos), Vec3::from(vs[2].pos)];
            let n: [Vec3; 3] = [Vec3::from(vs[0].normal), Vec3::from(vs[1].normal), Vec3::from(vs[2].normal)];
            area += 0.5 * (p[1] - p[0]).cross(p[2] - p[0]).length() as f64;
            // texture space: x = u * w, y = (1 - v) * h
            let uv: [[f32; 2]; 3] = [
                [vs[0].uv[0] * w as f32, (1.0 - vs[0].uv[1]) * h as f32],
                [vs[1].uv[0] * w as f32, (1.0 - vs[1].uv[1]) * h as f32],
                [vs[2].uv[0] * w as f32, (1.0 - vs[2].uv[1]) * h as f32],
            ];
            let (x0, x1) = (uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max));
            let (y0, y1) = (uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max));
            let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
            if det.abs() < 1e-9 {
                continue;
            }
            // world area of the triangle over its area in texels
            let a3 = 0.5 * (p[1] - p[0]).cross(p[2] - p[0]).length();
            tri_texel[t] = (2.0 * a3 / det.abs()).sqrt();
            let texel_world = tri_texel[t];
            let (xa, xb) = ((x0.floor().max(0.0)) as u32, (x1.ceil().min(w as f32 - 1.0)) as u32);
            let (ya, yb) = ((y0.floor().max(0.0)) as u32, (y1.ceil().min(h as f32 - 1.0)) as u32);
            for y in ya..=yb {
                for x in xa..=xb {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                    let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                    let l2 = 1.0 - l0 - l1;
                    const E: f32 = -0.02;
                    if l0 < E || l1 < E || l2 < E {
                        continue;
                    }
                    let idx = y * w + x;
                    // Texels shared by several surfaces (mirrored parts, copies on one island) keep a sample on each:
                    // paint on any of them lands in the texel, like in Painter. A second sample on the very surface
                    // already in the texel (shared triangle borders) is not kept.
                    let pos = p[0] * l0 + p[1] * l1 + p[2] * l2;
                    if filled[idx as usize] {
                        if !shared_texels {
                            continue;
                        }
                        let dup = first_pos
                            .get(&idx)
                            .is_some_and(|q: &Vec3| q.distance_squared(pos) < (texel_world * 1.5) * (texel_world * 1.5));
                        if dup {
                            continue;
                        }
                    } else {
                        first_pos.insert(idx, pos);
                        unique += 1;
                    }
                    filled[idx as usize] = true;
                    let normal = (n[0] * l0 + n[1] * l1 + n[2] * l2).normalize_or_zero();
                    texels.push(Texel { idx, pos, normal });
                }
            }
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for t in &texels {
            lo = lo.min(t.pos);
            hi = hi.max(t.pos);
        }
        if texels.is_empty() {
            lo = Vec3::ZERO;
            hi = Vec3::ONE;
        }
        let dim = 48usize;
        let cell = ((hi - lo).max_element() / dim as f32).max(1e-6);
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); dim * dim * dim];
        for (k, t) in texels.iter().enumerate() {
            let c = ((t.pos - lo) / cell).floor().clamp(Vec3::ZERO, Vec3::splat(dim as f32 - 1.0));
            cells[(c.z as usize * dim + c.y as usize) * dim + c.x as usize].push(k as u32);
        }
        let texel_size = (area / unique.max(1) as f64).sqrt() as f32;
        let mut by_idx = vec![u32::MAX; (w * h) as usize];
        for (k, t) in texels.iter().enumerate() {
            by_idx[t.idx as usize] = k as u32;
        }
        Cloud { w, h, texels, lo, cell, dim, cells, texel_size, tri_texel, by_idx }
    }

    fn query(&self, centre: Vec3, r: f32, mut f: impl FnMut(&Texel)) {
        let lo = ((centre - Vec3::splat(r) - self.lo) / self.cell).floor().max(Vec3::ZERO);
        let hi = ((centre + Vec3::splat(r) - self.lo) / self.cell).floor().min(Vec3::splat(self.dim as f32 - 1.0));
        if hi.x < lo.x || hi.y < lo.y || hi.z < lo.z {
            return;
        }
        let r2 = r * r;
        for z in lo.z as usize..=hi.z as usize {
            for y in lo.y as usize..=hi.y as usize {
                for x in lo.x as usize..=hi.x as usize {
                    for &k in &self.cells[(z * self.dim + y) * self.dim + x] {
                        let t = &self.texels[k as usize];
                        if t.pos.distance_squared(centre) <= r2 {
                            f(t);
                        }
                    }
                }
            }
        }
    }
}

fn falloff(d: f32, hardness: f32) -> f32 {
    if d >= 1.0 {
        0.0
    } else if d <= hardness {
        1.0
    } else {
        let s = 1.0 - (d - hardness) / (1.0 - hardness).max(1e-4);
        s * s * (3.0 - 2.0 * s)
    }
}

/// Surfaces a painted key is blocked by.
enum Occluders {
    /// Every triangle of the scene.
    All,
    /// The triangles of the texture set that is being painted, and the listed sets.
    Own(Vec<u32>),
}

/// Stroke replay for one texture set of a mesh.
pub struct SetStrokes<'a> {
    pub mesh: &'a Mesh,
    pub set: usize,
    /// Aspect ratio of the window the strokes were painted in (positions are normalised by its height).
    aspect: f32,
    /// Shift of the key positions of strokes (SHARD_STROKE_XOFF / SHARD_STROKE_YOFF), in viewport fractions.
    xoff: f32,
    yoff: f32,
    cloud: RefCell<Option<Cloud>>,
    /// What hides the surface behind it for the keys of a stroke.
    occluders: Occluders,
    /// Triangle groups for decal selection: (mesh element id, UV chunk id) per triangle, and the triangles of
    /// every group.
    groups: RefCell<Option<std::sync::Arc<Groups>>>,
}

impl<'a> SetStrokes<'a> {
    pub fn new(mesh: &'a Mesh, set: usize, aspect: f32) -> Self {
        // Key positions are normalised by the viewport on both axes (NDC = 2 * position - 1): a test screen's dabs
        // and letters match the export with a sharp optimum at 1.0 (aspect 1.3, the spread-based estimate, loses
        // them). SHARD_STROKE_ASPECT=auto uses the estimate, a number overrides it.
        let aspect = match std::env::var("SHARD_STROKE_ASPECT").ok().as_deref() {
            Some("auto") => aspect,
            Some(v) => v.parse::<f32>().unwrap_or(1.0),
            None => 1.0,
        };
        let envf = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        Self {
            mesh,
            set,
            aspect: aspect.clamp(0.5, 3.0),
            xoff: envf("SHARD_STROKE_XOFF", 0.0),
            yoff: envf("SHARD_STROKE_YOFF", 0.0),
            cloud: RefCell::new(None),
            // Only the painted set's own triangles hide what is behind them (measured on a test vehicle: the interior was
            // painted with the hull sets out of the way -- the interior export gets 3.3 -> 2.6 of base colour MAE and
            // 5.9 -> 4.8 of roughness, the exterior sets stay equal or improve). SHARD_PAINT_OCCLUDERS=all|<set list>.
            occluders: match std::env::var("SHARD_PAINT_OCCLUDERS").ok().as_deref() {
                Some("all") => Occluders::All,
                None | Some("own") => Occluders::Own(Vec::new()),
                Some(list) => Occluders::Own(list.split(',').filter_map(|t| t.trim().parse().ok()).collect()),
            },
            groups: RefCell::new(None),
        }
    }

    /// Brush diameter in mesh units (`brushSizeSpace = 1`: scene space).
    fn diameter(action: &PaintAction) -> f32 {
        let k = std::env::var("SHARD_BRUSH_K").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        (action.brush.size * k).max(1e-5)
    }

    /// 3D points `(position, view direction)` of the stroke keys that hit the mesh.
    fn path(&self, s: &Stroke) -> Vec<(Vec3, Vec3)> {
        self.path_tris(s).into_iter().map(|(p, d, _, _)| (p, d)).collect()
    }

    /// Like [`Self::path`], with the triangle every key hit.
    /// Like [`Self::path`], with the triangle every key hit and the pen pressure at that key (1 without a tablet).
    fn path_tris(&self, s: &Stroke) -> Vec<(Vec3, Vec3, u32, f32)> {
        let v = Mat4::from_cols_array(&s.view);
        let p = Mat4::from_cols_array(&s.projection);
        let inv = (p * v).inverse();
        let mut own_path = Vec::with_capacity(s.keys.len());
        let mut all_path = Vec::with_capacity(s.keys.len());
        // per key: the hit on this set seen through the other sets, and the nearest hit of all
        let mut pairs: Vec<(Option<Vec3>, Option<Vec3>)> = Vec::with_capacity(s.keys.len());
        for (ki, k) in s.keys.iter().enumerate() {
            let pressure = s.pressure.get(ki).copied().unwrap_or(1.0).clamp(0.0, 1.0);
            // square-pixel NDC: the projection has aspect 1 and u runs 0..aspect across the window
            let (x, y) = (2.0 * (k.pos[0] + self.xoff) - self.aspect, 2.0 * (k.pos[1] + self.yoff) - 1.0);
            let n = inv * Vec4::new(x, y, -1.0, 1.0);
            let f = inv * Vec4::new(x, y, 1.0, 1.0);
            let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
            let dir = (f - n).normalize_or_zero();
            if dir == Vec3::ZERO {
                continue;
            }
            let all = self.mesh.raycast(n, dir);
            let own = match &self.occluders {
                Occluders::All => None,
                Occluders::Own(extra) => {
                    let set = self.set as u32;
                    self.mesh.raycast_where(n, dir, |t| {
                        let m = self.mesh.tri_mat.get(t).copied();
                        m == Some(set) || m.is_some_and(|m| extra.contains(&m))
                    })
                }
            };
            if let Some(h) = &all {
                all_path.push((h.point, dir, h.tri, pressure));
            }
            if let Some(h) = &own {
                own_path.push((h.point, dir, h.tri, pressure));
            }
            pairs.push((own.map(|h| h.point), all.map(|h| h.point)));
        }
        if matches!(self.occluders, Occluders::All) {
            return all_path;
        }
        // Seen through the other sets, a stroke the artist drew on a surface it could see stays on one surface (the
        // a test vehicle's signature on the interior, drawn with the hull hidden); one drawn over another set while it was in
        // the way scatters over whatever lies behind (the `Base` stroke drawn on the turret). The second keeps the
        // nearest surfaces (SHARD_OWN_JUMP: how much longer the steps behind may be, default 2.5).
        let limit = std::env::var("SHARD_OWN_JUMP").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.5);
        let (mut own_len, mut all_len) = (0.0f32, 0.0f32);
        for w in pairs.windows(2) {
            if let ((Some(o0), Some(a0)), (Some(o1), Some(a1))) = (w[0], w[1]) {
                own_len += o0.distance(o1);
                all_len += a0.distance(a1);
            }
        }
        if std::env::var_os("SHARD_TRACE_JUMP").is_some() {
            let own_direct = pairs.iter().filter(|(o, a)| o.is_some() && o == a).count();
            let max_step = pairs
                .windows(2)
                .filter_map(|w| match (w[0].0, w[1].0) {
                    (Some(a), Some(b)) => Some(a.distance(b)),
                    _ => None,
                })
                .fold(0.0f32, f32::max);
            eprintln!(
                "[jump] keys {} own {} direct {} own_len {:.4} all_len {:.4} ratio {:.1} max_own_step {:.4}",
                s.keys.len(), own_path.len(), own_direct, own_len, all_len, own_len / all_len.max(1e-6), max_step
            );
        }
        // the scattered case: a step behind the other sets far longer than the stroke's usual step
        let (mut steps, mut max_step) = (0usize, 0.0f32);
        for w in pairs.windows(2) {
            if let (Some(a), Some(b)) = (w[0].0, w[1].0) {
                steps += 1;
                max_step = max_step.max(a.distance(b));
            }
        }
        let mean_step = own_len / steps.max(1) as f32;
        let scattered = max_step > 0.03 && max_step > 8.0 * mean_step && own_len > limit * all_len.max(1e-6);
        if scattered {
            all_path
        } else {
            own_path
        }
    }
}

/// Connected components of the triangles: two triangles are joined when they share a vertex key.
fn components(n_tris: usize, keys: impl Fn(usize) -> [[i64; 5]; 3]) -> Vec<u32> {
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut parent: Vec<u32> = (0..n_tris as u32).collect();
    let mut seen: std::collections::HashMap<[i64; 5], u32> = std::collections::HashMap::new();
    for t in 0..n_tris {
        for k in keys(t) {
            match seen.get(&k) {
                Some(&o) => {
                    let (a, b) = (find(&mut parent, t as u32), find(&mut parent, o));
                    if a != b {
                        parent[a as usize] = b;
                    }
                }
                None => {
                    seen.insert(k, t as u32);
                }
            }
        }
    }
    (0..n_tris as u32).map(|t| find(&mut parent, t)).collect()
}

/// UV chunk id of every triangle (triangles that share a vertex position and UV belong to one chunk): the
/// unit a "UV chunk" polygon-fill click selects.
pub fn uv_chunks(m: &crate::mesh::Mesh) -> Vec<u32> {
    let q = |v: f32| (v * 2000.0).round() as i64;
    let vkey = |i: u32| -> [i64; 5] {
        let v = &m.vertices[i as usize];
        [q(v.pos[0]), q(v.pos[1]), q(v.pos[2]), q(v.uv[0] * 0.5), q(v.uv[1] * 0.5)]
    };
    components(m.indices.len() / 3, |t| {
        let i = &m.indices[3 * t..3 * t + 3];
        [vkey(i[0]), vkey(i[1]), vkey(i[2])]
    })
}

/// Triangle groups of a mesh a polygon-fill click selects (see [`SetStrokes::decal`]).
pub struct Groups {
    /// Mesh element (triangles sharing vertex positions) id of every triangle.
    element: Vec<u32>,
    /// UV chunk (triangles sharing position and UV) id of every triangle.
    chunk: Vec<u32>,
    /// Triangles of each element / chunk id.
    element_tris: std::collections::HashMap<u32, Vec<u32>>,
    chunk_tris: std::collections::HashMap<u32, Vec<u32>>,
}

/// Groups of the meshes seen so far, by a fingerprint of the geometry: building them is a union-find over the
/// whole mesh and every compose job of the same document would repeat it.
fn shared_groups(m: &crate::mesh::Mesh) -> std::sync::Arc<Groups> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<(u64, std::sync::Arc<Groups>)>>> = std::sync::OnceLock::new();
    let key = m.fingerprint();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(g) = cache.lock().ok().and_then(|c| c.iter().find(|(k, _)| *k == key).map(|(_, g)| g.clone())) {
        return g;
    }
    let q = |v: f32| (v * 2000.0).round() as i64;
    let vkey = |i: u32, with_uv: bool| -> [i64; 5] {
        let v = &m.vertices[i as usize];
        if with_uv {
            [q(v.pos[0]), q(v.pos[1]), q(v.pos[2]), q(v.uv[0] * 0.5), q(v.uv[1] * 0.5)]
        } else {
            [q(v.pos[0]), q(v.pos[1]), q(v.pos[2]), 0, 0]
        }
    };
    let n = m.indices.len() / 3;
    let element = components(n, |t| {
        let i = &m.indices[3 * t..3 * t + 3];
        [vkey(i[0], false), vkey(i[1], false), vkey(i[2], false)]
    });
    let chunk = components(n, |t| {
        let i = &m.indices[3 * t..3 * t + 3];
        [vkey(i[0], true), vkey(i[1], true), vkey(i[2], true)]
    });
    let index = |ids: &[u32]| {
        let mut map: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
        for (t, id) in ids.iter().enumerate() {
            map.entry(*id).or_default().push(t as u32);
        }
        map
    };
    let g = std::sync::Arc::new(Groups { element_tris: index(&element), chunk_tris: index(&chunk), element, chunk });
    if let Ok(mut c) = cache.lock() {
        if c.len() >= 4 {
            c.remove(0);
        }
        c.push((key, g.clone()));
    }
    g
}

impl SetStrokes<'_> {
    fn ensure_groups(&self) {
        if self.groups.borrow().is_some() {
            return;
        }
        *self.groups.borrow_mut() = Some(shared_groups(self.mesh));
    }

    /// Ray of a decal hit: viewport position (0..1, y down) through the inverse view-projection.
    /// Triangles a rectangle selection (`DataDecalHitRect`) reaches: what the camera of the hit sees inside the
    /// rectangle (nearest surface per pixel; every triangle in it when the hit paints through). Software
    /// rasterisation of the whole mesh into the rectangle: the polygon-fill tool reads an id buffer the same way.
    fn rect_triangles(&self, h: &shard_model::DecalHit) -> Vec<u32> {
        let Some([c0, c1]) = h.rect else { return Vec::new() };
        // same screen convention as the clicks: x = 2 px - 1 (window aspect 1), y up
        let up = std::env::var_os("SHARD_DECAL_Y_DOWN").is_none();
        let to_ndc = |p: [f32; 2]| (2.0 * p[0] - 1.0, if up { 2.0 * p[1] - 1.0 } else { 1.0 - 2.0 * p[1] });
        let (a, b) = (to_ndc(c0), to_ndc(c1));
        // A rectangle can lie past the edge of the unit view, like the stroke keys do (the window was wider than
        // the projection's square): the camera extends there. Clamping to -1..1 dropped a test pistol's two marquees
        // at x = -0.09 that cut the frame out of the metal (SHARD_DECAL_RECT_CLAMP=1: the old clamp).
        let lim = if std::env::var_os("SHARD_DECAL_RECT_CLAMP").is_some() { 1.0 } else { 4.0 };
        let (x0, x1) = (a.0.min(b.0).max(-lim), a.0.max(b.0).min(lim));
        let (y0, y1) = (a.1.min(b.1).max(-lim), a.1.max(b.1).min(lim));
        if x1 <= x0 || y1 <= y0 {
            return Vec::new();
        }
        let view_proj = Mat4::from_cols_array(&h.proj_view_inv).inverse();
        // raster cells: ~1.5k across the whole view, at least 4 cells on the short side
        let res = std::env::var("SHARD_DECAL_RECT_RES").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1536.0);
        let (w, hh) = ((((x1 - x0) * 0.5 * res).ceil() as usize).max(4), (((y1 - y0) * 0.5 * res).ceil() as usize).max(4));
        // Every marquee reaches what is behind the surface too (with the whole scene read -- see the assbin reader --
        // a test vehicle's interior masks become exact: a hand-painted mask layer 10.8 -> 0.0, base colour 5.1 -> 3.6; the old
        // "only white ones" rule was fitted while 62 meshes were missing). SHARD_DECAL_RECT_THROUGH=white|black|visible.
        let white = h.color[0] > 0.5;
        let through = h.paint_through != 0
            || match std::env::var("SHARD_DECAL_RECT_THROUGH").ok().as_deref() {
                Some("white") => white,
                Some("black") => !white,
                Some("visible") => false,
                _ => true,
            };
        let cull = std::env::var_os("SHARD_DECAL_RECT_CULL").is_some();
        let zclip = std::env::var_os("SHARD_DECAL_RECT_ZCLIP").is_some();
        let mesh = self.mesh;
        let n_tris = mesh.indices.len() / 3;
        // clip-space positions of every vertex once
        let clip: Vec<Vec4> = mesh
            .vertices
            .iter()
            .map(|v| view_proj * Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0))
            .collect();
        let mut depth = vec![f32::INFINITY; w * hh];
        let mut id = vec![u32::MAX; w * hh];
        let mut touched: Vec<u32> = Vec::new();
        let watch: Vec<u32> = std::env::var("SHARD_WATCH_CHUNKS")
            .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect())
            .unwrap_or_default();
        let mut covered: std::collections::BTreeMap<u32, u32> = Default::default();
        let watched_chunk = |t: usize| -> Option<u32> {
            let g = self.groups.borrow();
            let c = g.as_ref()?.chunk[t];
            watch.contains(&c).then_some(c)
        };
        let mut skips: std::collections::BTreeMap<(u32, &str), u32> = Default::default();
        let sx = w as f32 / (x1 - x0);
        let sy = hh as f32 / (y1 - y0);
        // the other texture sets are not in the picture (measured: the sets that cover the model from outside would
        // hide every interior chunk; SHARD_DECAL_ALL_SETS=1 keeps them) -- SHARD_DECAL_OCCLUDERS=a,b: those sets
        // still hide what is behind them
        let own = std::env::var_os("SHARD_DECAL_ALL_SETS").is_none();
        let set_id = self.set as u32;
        let occluders: Vec<u32> = std::env::var("SHARD_DECAL_OCCLUDERS")
            .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect())
            .unwrap_or_default();
        for t in 0..n_tris {
            if own {
                let m = mesh.tri_mat.get(t).copied();
                if m != Some(set_id) && !m.is_some_and(|m| occluders.contains(&m)) {
                    continue;
                }
            }
            let i = &mesh.indices[3 * t..3 * t + 3];
            let (c0, c1, c2) = (clip[i[0] as usize], clip[i[1] as usize], clip[i[2] as usize]);
            let wc = if watch.is_empty() { None } else { watched_chunk(t) };
            let mut skip = |why: &'static str| {
                if let Some(c) = wc {
                    *skips.entry((c, why)).or_default() += 1;
                }
            };
            if c0.w <= 1e-6 || c1.w <= 1e-6 || c2.w <= 1e-6 {
                skip("behind");
                continue; // behind the camera (no clipping: those triangles are at the screen edge anyway)
            }
            let p = [
                (c0.x / c0.w, c0.y / c0.w, c0.z / c0.w),
                (c1.x / c1.w, c1.y / c1.w, c1.z / c1.w),
                (c2.x / c2.w, c2.y / c2.w, c2.z / c2.w),
            ];
            let (tx0, tx1) = (p[0].0.min(p[1].0).min(p[2].0), p[0].0.max(p[1].0).max(p[2].0));
            let (ty0, ty1) = (p[0].1.min(p[1].1).min(p[2].1), p[0].1.max(p[1].1).max(p[2].1));
            if tx1 < x0 || tx0 > x1 || ty1 < y0 || ty0 > y1 {
                skip("outside");
                continue;
            }
            // A marquee that reaches through needs no depth: a triangle is in it when its picture meets the rectangle
            // (separating axes: the bounding boxes overlap, and no triangle edge has the whole rectangle on its outer
            // side). Rasterising missed triangles seen edge-on and slivers that cover no cell centre: a test tank's
            // `ExhaustGrid` marquee, looking along Z, left the side panels (x or y constant) unselected (a hull set
            // base +11). SHARD_DECAL_RECT_RASTER=1: the raster test.
            if through && std::env::var_os("SHARD_DECAL_RECT_RASTER").is_none() {
                let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                let separated = (0..3).any(|k| {
                    let (a, b, c) = (p[k], p[(k + 1) % 3], p[(k + 2) % 3]);
                    let n = (-(b.1 - a.1), b.0 - a.0);
                    if n.0 * n.0 + n.1 * n.1 < 1e-20 {
                        return false; // a collapsed edge separates nothing
                    }
                    let side = |q: (f32, f32)| (q.0 - a.0) * n.0 + (q.1 - a.1) * n.1;
                    let tri = side((c.0, c.1));
                    // the triangle lies on the side of `tri` (a segment: on the line itself)
                    if tri > 0.0 {
                        corners.iter().all(|&q| side(q) < 0.0)
                    } else if tri < 0.0 {
                        corners.iter().all(|&q| side(q) > 0.0)
                    } else {
                        corners.iter().all(|&q| side(q) > 0.0) || corners.iter().all(|&q| side(q) < 0.0)
                    }
                });
                if separated {
                    skip("outside (edges)");
                } else {
                    touched.push(t as u32);
                }
                continue;
            }
            // winding in screen space: counter-clockwise faces the camera
            let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-12 {
                skip("edge-on");
                continue;
            }
            if cull && !through && area < 0.0 {
                continue;
            }
            // pixel coordinates inside the rectangle
            let q = p.map(|(x, y, z)| ((x - x0) * sx, (y - y0) * sy, z));
            let (bx0, bx1) = (
                q[0].0.min(q[1].0).min(q[2].0).floor().max(0.0) as usize,
                (q[0].0.max(q[1].0).max(q[2].0).ceil().max(0.0) as usize).min(w - 1),
            );
            let (by0, by1) = (
                q[0].1.min(q[1].1).min(q[2].1).floor().max(0.0) as usize,
                (q[0].1.max(q[1].1).max(q[2].1).ceil().max(0.0) as usize).min(hh - 1),
            );
            let det = (q[1].1 - q[2].1) * (q[0].0 - q[2].0) + (q[2].0 - q[1].0) * (q[0].1 - q[2].1);
            if det.abs() < 1e-12 {
                continue;
            }
            let mut hit_any = false;
            for y in by0..=by1 {
                for x in bx0..=bx1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let l0 = ((q[1].1 - q[2].1) * (px - q[2].0) + (q[2].0 - q[1].0) * (py - q[2].1)) / det;
                    let l1 = ((q[2].1 - q[0].1) * (px - q[2].0) + (q[0].0 - q[2].0) * (py - q[2].1)) / det;
                    let l2 = 1.0 - l0 - l1;
                    if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                        continue;
                    }
                    let z = l0 * q[0].2 + l1 * q[1].2 + l2 * q[2].2;
                    // SHARD_DECAL_RECT_ZCLIP=1: what lies outside the depth range of the camera is not in the picture
                    if zclip && !(-1.0..=1.0).contains(&z) {
                        continue;
                    }
                    let k = y * w + x;
                    if !watch.is_empty() {
                        *covered.entry(t as u32).or_default() += 1;
                    }
                    if through {
                        hit_any = true;
                    } else if z < depth[k] {
                        depth[k] = z;
                        id[k] = t as u32;
                    }
                }
            }
            if through && hit_any {
                touched.push(t as u32);
            } else if through {
                skip("no cell centre");
            }
        }
        if !skips.is_empty() {
            eprintln!("[rect {:.3},{:.3}->{:.3},{:.3}] skipped triangles of watched chunks: {:?}", c0[0], c0[1], c1[0], c1[1], skips);
        }
        if !watch.is_empty() {
            let chunk = self.groups.borrow().as_ref().map(|g| g.chunk.clone()).unwrap_or_default();
            let mut per: std::collections::BTreeMap<u32, (u32, u32)> = Default::default();
            for (&t, &n) in &covered {
                let c = chunk[t as usize];
                if watch.contains(&c) {
                    per.entry(c).or_default().0 += n;
                }
            }
            for &t in id.iter() {
                if t != u32::MAX && watch.contains(&chunk[t as usize]) {
                    per.entry(chunk[t as usize]).or_default().1 += 1;
                }
            }
            if !per.is_empty() {
                eprintln!("[rect {:.3},{:.3}->{:.3},{:.3}] cells (all, visible) per watched chunk: {:?}", c0[0], c0[1], c1[0], c1[1], per);
            }
        }
        if through {
            return touched;
        }
        let mut seen: std::collections::BTreeSet<u32> = id.into_iter().filter(|t| *t != u32::MAX).collect();
        std::mem::take(&mut seen).into_iter().collect()
    }

    fn hit_ray(&self, h: &shard_model::DecalHit) -> Option<(Vec3, Vec3)> {
        let inv = Mat4::from_cols_array(&h.proj_view_inv);
        // same convention as the stroke keys: normalised by the window height, y up, square-pixel NDC
        let up = std::env::var_os("SHARD_DECAL_Y_DOWN").is_none();
        // measured: with a window aspect of 1 (positions are 0..1 on both axes) every click lands on the mesh
        let wide = std::env::var("SHARD_DECAL_ASPECT").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let xoff = std::env::var("SHARD_DECAL_XOFF").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let yoff = std::env::var("SHARD_DECAL_YOFF").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let (px, py) = (h.position[0] + xoff, h.position[1] + yoff);
        let (x, y) = (2.0 * px - wide, if up { 2.0 * py - 1.0 } else { 1.0 - 2.0 * py });
        let n = inv * Vec4::new(x, y, -1.0, 1.0);
        let f = inv * Vec4::new(x, y, 1.0, 1.0);
        let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
        let dir = (f - n).normalize_or_zero();
        (dir != Vec3::ZERO).then_some((n, dir))
    }
}

/// Alpha of a textured brush (`knife_01`, `alpha_dirt_03`...): square, row 0 on top, spanning the stamp diameter.
struct BrushShape {
    n: usize,
    data: Vec<f32>,
}

impl BrushShape {
    fn from_image(img: &shard_compose::Image) -> Option<Self> {
        if img.w != img.h || img.w < 4 || img.n == 0 {
            return None;
        }
        let n = img.w as usize;
        Some(Self { n, data: (0..n * n).map(|i| img.data[i * img.n].clamp(0.0, 1.0)).collect() })
    }

    /// Bilinear sample at `(u, v)` in 0..1 (v down); nothing outside the image.
    fn sample(&self, u: f32, v: f32) -> f32 {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return 0.0;
        }
        let n = self.n as i32;
        let (x, y) = (u * self.n as f32 - 0.5, v * self.n as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let g = |xi: i32, yi: i32| -> f32 {
            if xi < 0 || yi < 0 || xi >= n || yi >= n {
                0.0
            } else {
                self.data[yi as usize * self.n + xi as usize]
            }
        };
        let (xi, yi) = (x0 as i32, y0 as i32);
        let top = g(xi, yi) * (1.0 - fx) + g(xi + 1, yi) * fx;
        let bottom = g(xi, yi + 1) * (1.0 - fx) + g(xi + 1, yi + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// Small deterministic generator of the per-stamp jitters (seeded by the stroke's own `randomSeed`).
struct StampRng(u64);

impl StampRng {
    fn new(seed: u32) -> Self {
        let mut s = (seed as u64 ^ 0x9E37_79B9_7F4A_7C15).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        if s == 0 {
            s = 0x1234_5678_9ABC_DEF1;
        }
        Self(s)
    }

    /// Uniform in [0, 1).
    fn next(&mut self) -> f32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((v >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// Orientation conventions of textured stamps (diagnostics: SHARD_ROT_SIGN, SHARD_FOLLOW_BASE in radians).
struct StampKnobs {
    rot_sign: f32,
    follow_base: f32,
}

impl StampKnobs {
    fn from_env() -> Self {
        let f = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        Self { rot_sign: f("SHARD_ROT_SIGN", 1.0), follow_base: f("SHARD_FOLLOW_BASE", 0.0) }
    }
}

impl shard_compose::StrokeRaster for SetStrokes<'_> {
    fn cache_token(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.mesh.fingerprint(), self.set, self.aspect.to_bits()).hash(&mut h);
        (self.xoff.to_bits(), self.yoff.to_bits()).hash(&mut h);
        match &self.occluders {
            Occluders::All => 0u8.hash(&mut h),
            Occluders::Own(v) => (1u8, v).hash(&mut h),
        }
        Some(h.finish())
    }

    fn decal(&self, hits: &[shard_model::DecalHit], buf: &mut [f32], w: u32, h: u32, _value: f32) -> bool {
        let t_decal = std::time::Instant::now();
        self.ensure_groups();
        if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
            eprintln!("[time]         decal: groups ready after {:?} ({} hits)", t_decal.elapsed(), hits.len());
        }
        let groups = self.groups.borrow().clone().unwrap();
        let (element, chunk) = (&groups.element, &groups.chunk);
        let mesh = self.mesh;
        let n_tris = element.len();
        let mut any = false;
        let (mut missed, mut skipped, mut tris_total) = (0, 0, 0usize);
        let mut ray_time = std::time::Duration::ZERO;
        let mut history: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
        if let Ok(spec) = std::env::var("SHARD_PROBE_RAY") {
            // "y,z" pairs separated by ';': cast along -X from +x, report the chunk found
            for pair in spec.split(';') {
                let v: Vec<f32> = pair.split(',').filter_map(|t| t.trim().parse().ok()).collect();
                if v.len() == 2 {
                    let o = Vec3::new(10.0, v[0], v[1]);
                    if let Some(r) = self.mesh.raycast(o, Vec3::new(-1.0, 0.0, 0.0)) {
                        let size = chunk.iter().filter(|c| **c == chunk[r.tri as usize]).count();
                        eprintln!("[probe ray] y={} z={} -> chunk {} ({} tris) at x={:.2}", v[0], v[1], chunk[r.tri as usize], size, r.point.x);
                    }
                }
            }
        }
        if let Ok(spec) = std::env::var("SHARD_PROBE_CHUNKS") {
            for id in spec.split(',').filter_map(|t| t.trim().parse::<u32>().ok()) {
                let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for (t, c) in chunk.iter().enumerate() {
                    if *c != id {
                        continue;
                    }
                    for k in 0..3 {
                        let v = &mesh.vertices[mesh.indices[3 * t + k] as usize];
                        for a in 0..2 {
                            lo[a] = lo[a].min(v.uv[a]);
                            hi[a] = hi[a].max(v.uv[a]);
                        }
                        plo = plo.min(Vec3::from(v.pos));
                        phi = phi.max(Vec3::from(v.pos));
                    }
                }
                eprintln!("[chunk {id}] uv ({:.2},{:.2})-({:.2},{:.2}) pos ({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})", lo[0], lo[1], hi[0], hi[1], plo.x, plo.y, plo.z, phi.x, phi.y, phi.z);
            }
        }
        if std::env::var_os("SHARD_DECAL_INIT1").is_some() {
            buf.iter_mut().for_each(|v| *v = 1.0);
        }
        // hits are applied in order: white ones paint, black ones erase what came before
        let reverse = std::env::var_os("SHARD_DECAL_REVERSE").is_some();
        let ordered: Vec<&shard_model::DecalHit> = if reverse { hits.iter().rev().collect() } else { hits.iter().collect() };
        for hit in ordered {
            if hit.view_type == 0 {
                skipped += 1;
                continue;
            }
            let t_ray = std::time::Instant::now();
            let gran = match (hit.granularity, std::env::var("SHARD_DECAL_G3").ok().as_deref()) {
                (3, Some("element")) => 2,
                (2, Some("chunk")) => 3,
                (g, _) => g,
            };
            // the triangles the hit selects: what the ray finds (clicks), what the rectangle sees (marquee),
            // grown to the granularity of the tool
            let mut selected_owned: Vec<u32> = Vec::new();
            let t;
            // A rectangle selection (`DataDecalHitRect`, a marquee) is read as the click at the centre of the view
            // it used to be (the old reading, which measured better): how Painter combines the marquee with
            // the visible surface is not understood yet (SHARD_DECAL_RECT=1 selects what the rectangle sees).
            if hit.rect.is_some() && std::env::var_os("SHARD_GROUP_CLICKS_LEGACY").is_none() {
                let seen = self.rect_triangles(hit);
                ray_time += t_ray.elapsed();
                let Some(&first) = seen.first() else {
                    if std::env::var_os("SHARD_TRACE_DECAL").is_some() {
                        eprintln!("[decal] uid={} {} RECT {:?} pos=({:.3},{:.3}) sees nothing", hit.uid, if hit.color[0] > 0.5 { "W" } else { "B" }, hit.rect, hit.position[0], hit.position[1]);
                    }
                    missed += 1;
                    continue;
                };
                any = true;
                t = first as usize;
                match gran {
                    2 | 3 => {
                        let ids: std::collections::BTreeSet<u32> =
                            seen.iter().map(|&tri| if gran == 2 { element[tri as usize] } else { chunk[tri as usize] }).collect();
                        for id in ids {
                            let tris = if gran == 2 { groups.element_tris.get(&id) } else { groups.chunk_tris.get(&id) };
                            if let Some(tris) = tris {
                                selected_owned.extend_from_slice(tris);
                            }
                        }
                    }
                    _ => selected_owned = seen,
                }
                history.entry(chunk[t]).or_default().push(if hit.color[0] > 0.5 { 'W' } else { 'B' });
            } else {
                let Some((o, d)) = self.hit_ray(hit) else { continue };
                // the other texture sets are transparent to the click (they may have been hidden)
                let own = std::env::var_os("SHARD_DECAL_ALL_SETS").is_none()
                    && std::env::var_os("SHARD_DECAL_CLICK_ALL_SETS").is_none()
                    && std::env::var_os("SHARD_GROUP_CLICKS_LEGACY").is_none();
                let set_id = self.set as u32;
                let Some(r) = self.mesh.raycast_where(o, d, |t| !own || mesh.tri_mat.get(t).copied() == Some(set_id)) else {
                    if std::env::var_os("SHARD_TRACE_DECAL").is_some() {
                        eprintln!("[decal] uid={} {} click pos=({:.3},{:.3}) misses the set", hit.uid, if hit.color[0] > 0.5 { "W" } else { "B" }, hit.position[0], hit.position[1]);
                    }
                    missed += 1;
                    continue;
                };
                ray_time += t_ray.elapsed();
                any = true;
                t = r.tri as usize;
                history.entry(chunk[t]).or_default().push(if hit.color[0] > 0.5 { 'W' } else { 'B' });
                let single = [t as u32];
                let tris: &[u32] = match gran {
                    2 => groups.element_tris.get(&element[t]).map(Vec::as_slice).unwrap_or(&single),
                    3 => groups.chunk_tris.get(&chunk[t]).map(Vec::as_slice).unwrap_or(&single),
                    _ => &single,
                };
                selected_owned.extend_from_slice(tris);
            }
            let selected: &[u32] = &selected_owned;
            if std::env::var_os("SHARD_TRACE_DECAL").is_some() {
                let mut mine: std::collections::BTreeSet<u32> = Default::default();
                for &tri in selected {
                    if mesh.tri_mat.get(tri as usize).copied() == Some(self.set as u32) {
                        mine.insert(chunk[tri as usize]);
                    }
                }
                eprintln!(
                    "[decal] uid={} {} {} gran={} thru={} view={} pos=({:.3},{:.3}){} -> {} tris, {} chunks of this set",
                    hit.uid,
                    if hit.color[0] > 0.5 { "W" } else { "B" },
                    if hit.rect.is_some() { "RECT " } else { "click" },
                    hit.granularity,
                    hit.paint_through,
                    hit.view_type,
                    hit.position[0],
                    hit.position[1],
                    hit.rect.map(|r| format!(" {:.3},{:.3} -> ({:.3},{:.3})", r[0][0], r[0][1], r[1][0], r[1][1])).unwrap_or_default(),
                    selected.len(),
                    mine.len()
                );
            }
            // SHARD_WATCH_CHUNKS=a,b: which hits select those chunks (and with what colour)
            if let Ok(spec) = std::env::var("SHARD_WATCH_CHUNKS") {
                let watch: Vec<u32> = spec.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                let mut hit_chunks: std::collections::BTreeSet<u32> = Default::default();
                for &tri in selected {
                    if watch.contains(&chunk[tri as usize]) {
                        hit_chunks.insert(chunk[tri as usize]);
                    }
                }
                if !hit_chunks.is_empty() {
                    eprintln!(
                        "[watch] uid={} {} {} selects chunks {:?} ({} tris)",
                        hit.uid,
                        if hit.color[0] > 0.5 { "W" } else { "B" },
                        if hit.rect.is_some() { "RECT" } else { "click" },
                        hit_chunks,
                        selected.len()
                    );
                }
            }
            // SHARD_DECAL_RECT_INVERT=1: a rectangle's colour is the opposite of what it does (experiment)
            let value = if hit.rect.is_some() && std::env::var_os("SHARD_DECAL_RECT_INVERT").is_some() {
                1.0 - hit.color[0]
            } else {
                hit.color[0]
            };
            let probe: Option<(usize, usize)> = std::env::var("SHARD_PROBE_TEXEL").ok().and_then(|v| {
                let mut it = v.split(',').filter_map(|t| t.trim().parse::<f32>().ok());
                let (u, vv) = (it.next()?, it.next()?);
                Some(((u * w as f32) as usize, (vv * h as f32) as usize))
            });
            for &t in selected {
                let t = t as usize;
                if mesh.tri_mat.get(t).copied() != Some(self.set as u32) {
                    continue;
                }
                tris_total += 1;
                let i = &mesh.indices[3 * t..3 * t + 3];
                let uv: [[f32; 2]; 3] = [0, 1, 2].map(|k| {
                    let v = &mesh.vertices[i[k] as usize];
                    [v.uv[0] * w as f32, (1.0 - v.uv[1]) * h as f32]
                });
                let (x0, x1) = (uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max));
                let (y0, y1) = (uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max));
                let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
                if det.abs() < 1e-9 {
                    continue;
                }
                let (xa, xb) = (x0.floor().max(0.0) as u32, x1.ceil().min(w as f32 - 1.0) as u32);
                let (ya, yb) = (y0.floor().max(0.0) as u32, y1.ceil().min(h as f32 - 1.0) as u32);
                for y in ya..=yb {
                    for x in xa..=xb {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                        let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                        let l2 = 1.0 - l0 - l1;
                        const E: f32 = -0.02;
                        if l0 >= E && l1 >= E && l2 >= E {
                            buf[(y * w + x) as usize] = value;
                        }
                    }
                }
            }
            if let Some((px, py)) = probe {
                if px < w as usize && py < h as usize {
                    eprintln!(
                        "[texel] after {} chunk {} ({} tris): {}",
                        if hit.color[0] > 0.5 { "W" } else { "B" }, chunk[t], selected.len(), buf[py * w as usize + px]
                    );
                }
            }
        }
        if let Some(dir) = std::env::var_os("SHARD_DECAL_REPORT") {
            // chunk id of every texel (u32 little endian, 0xFFFFFFFF = none), the final mask and the hit
            // histories: lets an offline script compare the decal result with Painter's exports
            let dir = std::path::PathBuf::from(dir);
            let mut ids = vec![u32::MAX; (w * h) as usize];
            for t in 0..n_tris {
                if mesh.tri_mat.get(t).copied() != Some(self.set as u32) {
                    continue;
                }
                let i = &mesh.indices[3 * t..3 * t + 3];
                let uv: [[f32; 2]; 3] = [0, 1, 2].map(|k| {
                    let v = &mesh.vertices[i[k] as usize];
                    [v.uv[0] * w as f32, (1.0 - v.uv[1]) * h as f32]
                });
                let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
                if det.abs() < 1e-9 {
                    continue;
                }
                let (x0, x1) = (uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max));
                let (y0, y1) = (uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max));
                for y in (y0.floor().max(0.0) as u32)..=(y1.ceil().min(h as f32 - 1.0) as u32) {
                    for x in (x0.floor().max(0.0) as u32)..=(x1.ceil().min(w as f32 - 1.0) as u32) {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                        let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                        if l0 >= -0.02 && l1 >= -0.02 && 1.0 - l0 - l1 >= -0.02 {
                            ids[(y * w + x) as usize] = chunk[t];
                        }
                    }
                }
            }
            let _ = std::fs::create_dir_all(&dir);
            let tag = format!("{}_{}_{}", hits.len(), hits.first().map(|h| h.uid).unwrap_or(0), w);
            let bytes: Vec<u8> = ids.iter().flat_map(|v| v.to_le_bytes()).collect();
            let _ = std::fs::write(dir.join(format!("chunks_{tag}.bin")), bytes);
            let fin: Vec<u8> = buf.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8).collect();
            let _ = std::fs::write(dir.join(format!("mask_{tag}.bin")), fin);
            let mut txt = String::new();
            for (c, hist) in &history {
                txt.push_str(&format!("{c} {hist}
"));
            }
            let _ = std::fs::write(dir.join(format!("hist_{tag}.txt")), txt);
        }
        if std::env::var_os("SHARD_TRACE").is_some() {
            eprintln!("[decal] {} hits: {missed} missed the mesh, {skipped} 2D; {tris_total} triangle fills", hits.len());
        }
        if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
            eprintln!(
                "[time]         decal: {} hits, rays {:?}, total {:?}, {tris_total} triangle fills",
                hits.len(),
                ray_time,
                t_decal.elapsed()
            );
        }
        any
    }

    fn paint(
        &self,
        action: &PaintAction,
        buf: &mut [f32],
        w: u32,
        h: u32,
        value: f32,
        revert: Option<f32>,
        stencil_image: Option<&shard_compose::Image>,
        brush_shape: Option<&shard_compose::Image>,
    ) {
        if self.cloud.borrow().as_ref().map(|c| (c.w, c.h)) != Some((w, h)) {
            *self.cloud.borrow_mut() = Some(Cloud::build(self.mesh, self.set, w, h));
        }
        let cloud_ref = self.cloud.borrow();
        let cloud = cloud_ref.as_ref().unwrap();
        let b = &action.brush;
        let diameter = Self::diameter(action);
        let hardness = b.hardness().unwrap_or(0.5).clamp(0.0, 0.99);
        // the stamps follow the brush spacing (a fraction of the diameter); only a spacing finer than the texels
        // themselves is relaxed
        let step = b.spacing.max(0.01) * diameter;
        // Backface culling: `cullingAngle` is a fraction of 180 degrees (0.5 = only what faces away is skipped);
        // read as a cosine it skipped every surface past 60 degrees and lost marks along slanted faces (a test vehicle
        // interior: interior mask MAE 4.5 -> 4.3, interior metallic 2.05 -> 1.96). SHARD_CULL_COS=<c> overrides
        // the cosine a surface must reach against the view (-2 = no culling).
        let cull = match std::env::var("SHARD_CULL_COS").ok().and_then(|v| v.parse::<f32>().ok()) {
            Some(c) => c,
            None if b.culling_enabled => (b.culling_angle.clamp(0.0, 1.0) * std::f32::consts::PI).cos(),
            None => -2.0,
        };
        let cap = action.opacity.clamp(0.0, 1.0);
        // alpha of a textured brush: a square image that spans the diameter of the stamp
        let shape: Option<BrushShape> = brush_shape.and_then(BrushShape::from_image);
        let knobs = StampKnobs::from_env();
        // SHARD_NO_PRESSURE=1: every key at full pressure (the old replay)
        let no_pressure = std::env::var_os("SHARD_NO_PRESSURE").is_some();
        let no_subtexel = std::env::var_os("SHARD_NO_SUBTEXEL").is_some();
        let subtexel_pow = std::env::var("SHARD_SUBTEXEL_POW").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        if std::env::var_os("SHARD_PROBE_PAINT").is_some() && std::env::var_os("SHARD_TRACE_STROKES").is_some() {
            eprintln!("[probe-paint] buffer {w}x{h}");
        }
        // SHARD_PROBE_PAINT="u,v": the texel at that fraction of the texture (y down), whatever the resolution
        let probe: Option<usize> = std::env::var("SHARD_PROBE_PAINT").ok().and_then(|v| {
            let c: Vec<f32> = v.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            (c.len() == 2).then(|| {
                let (x, y) = (((c[0] * w as f32) as u32).min(w - 1), ((c[1] * h as f32) as u32).min(h - 1));
                (y * w + x) as usize
            })
        });
        // a stencil limits the stroke to the image placed in the view; one we cannot rasterise paints nothing
        // (a plain mark would be wrong everywhere)
        let stencil: Option<(crate::text::StencilImage, [f32; 9])> = match (&action.stencil, action.proj) {
            (Some(shard_model::Stencil::Text(t)), Some(m)) => match crate::text::render_text(t, 256) {
                Some(img) => Some((img, m)),
                None => return,
            },
            (Some(shard_model::Stencil::Procedural(_) | shard_model::Stencil::Bitmap { .. }), Some(m)) => match stencil_image {
                // the rendered graph: its first channel is the alpha (square images)
                Some(img) if img.w == img.h && img.w > 0 => {
                    let n = img.w as usize;
                    let alpha = (0..n * n).map(|i| img.data[i * img.n]).collect();
                    Some((crate::text::StencilImage { n, alpha }, m))
                }
                _ => return,
            },
            (Some(_), _) => return,
            _ => None,
        };
        let mut cov = vec![0.0f32; buf.len()];
        let mut touched: Vec<u32> = Vec::new();
        for stroke in &action.strokes {
            if stroke.keys.is_empty() {
                continue;
            }
            if stroke.view_type == 0 {
                // Painted in the 2D view: the keys are UV coordinates (v up) and the brush size a fraction of the
                // texture's width; every texel under the stamp takes the paint, whatever surface it belongs to.
                if std::env::var_os("SHARD_NO_2D_STROKES").is_some() {
                    continue;
                }
                let (wf, hf) = (w as f32, h as f32);
                // measured on a test vehicle's `Base` (bars painted in the 2D view): the marks are 0.7 of the stored size wide
                let k2d = std::env::var("SHARD_2D_SIZE_K").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.7);
                let r_px = (diameter * 0.5 * wf * k2d).max(0.75);
                let step_px = (b.spacing.max(0.01) * 2.0 * r_px).max(0.5);
                let pts: Vec<(f32, f32)> = stroke.keys.iter().map(|k| (k.pos[0] * wf, (1.0 - k.pos[1]) * hf)).collect();
                let mut rng = StampRng::new(stroke.seed);
                let mut stamp2d = |cx: f32, cy: f32, heading: f32, cov: &mut Vec<f32>, touched: &mut Vec<u32>| {
                    let r = [rng.next(), rng.next(), rng.next()];
                    let rr = r_px * (1.0 - b.size_jitter.clamp(0.0, 1.0) * r[0]);
                    let flow = b.flow * (1.0 - b.flow_jitter.clamp(0.0, 1.0) * r[1]);
                    let mut angle = knobs.rot_sign * (b.rotation + b.rotation_jitter * (2.0 * r[2] - 1.0));
                    if b.follow_path {
                        angle += heading + knobs.follow_base;
                    }
                    let (sa, ca) = angle.sin_cos();
                    let reach = if shape.is_some() { rr * std::f32::consts::SQRT_2 } else { rr };
                    let (x0, x1) = ((cx - reach).floor().max(0.0) as i64, (cx + reach).ceil().min(wf - 1.0) as i64);
                    let (y0, y1) = ((cy - reach).floor().max(0.0) as i64, (cy + reach).ceil().min(hf - 1.0) as i64);
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let (dx, dy) = (x as f32 + 0.5 - cx, -(y as f32 + 0.5 - cy));
                            let a = match &shape {
                                None => falloff((dx * dx + dy * dy).sqrt() / rr, hardness),
                                Some(img) => {
                                    let (xr, yr) = (dx * ca + dy * sa, -dx * sa + dy * ca);
                                    img.sample(0.5 + xr / (2.0 * rr), 0.5 - yr / (2.0 * rr))
                                }
                            } * flow;
                            if a <= 0.0 {
                                continue;
                            }
                            let i = (y as u32 * w + x as u32) as usize;
                            let cv = &mut cov[i];
                            if *cv == 0.0 {
                                touched.push(i as u32);
                            }
                            *cv = 1.0 - (1.0 - *cv) * (1.0 - a);
                        }
                    }
                };
                let mut carry = 0.0f32;
                for i in 0..pts.len() {
                    let (px, py) = pts[i];
                    if i == 0 {
                        let h0 = pts.get(1).map(|n| (-(n.1 - py)).atan2(n.0 - px)).unwrap_or(0.0);
                        stamp2d(px, py, h0, &mut cov, &mut touched);
                        continue;
                    }
                    let (qx, qy) = pts[i - 1];
                    let (sx, sy) = (px - qx, py - qy);
                    let len = (sx * sx + sy * sy).sqrt();
                    let heading = (-sy).atan2(sx);
                    let mut d = step_px - carry;
                    while d <= len {
                        let t = d / len.max(1e-9);
                        stamp2d(qx + sx * t, qy + sy * t, heading, &mut cov, &mut touched);
                        d += step_px;
                    }
                    carry = len - (d - step_px);
                }
                for &i in &touched {
                    let a = cov[i as usize].min(cap);
                    let dst = &mut buf[i as usize];
                    match revert {
                        Some(base) => *dst += (base - *dst) * a,
                        None => *dst += (value - *dst) * a,
                    }
                    cov[i as usize] = 0.0;
                }
                touched.clear();
                continue;
            }
            let path3 = self.path_tris(stroke);
            let path: Vec<(Vec3, Vec3)> = path3.iter().map(|(p, d, _, _)| (*p, *d)).collect();
            // pen pressure scales the size and / or the flow between the brush's minimum and 1 (154 of a test vehicle's
            // paint actions were drawn with a tablet: a 0.0093 scratch brush lays 1 texel lines, not 7)
            let press_size = |p: f32| if b.pressure_size && !no_pressure { b.pressure_size_min + (1.0 - b.pressure_size_min) * p } else { 1.0 };
            let press_flow = |p: f32| if b.pressure_flow && !no_pressure { b.pressure_flow_min + (1.0 - b.pressure_flow_min) * p } else { 1.0 };
            // SHARD_DUMP_STROKE_HITS=<brush size>: where the keys of the strokes of that brush land (every texture set)
            if let Some(want) = std::env::var("SHARD_DUMP_STROKE_HITS").ok().and_then(|v| v.parse::<f32>().ok()) {
                if (b.size - want).abs() < 2e-5 && !stroke.keys.is_empty() {
                    let v = Mat4::from_cols_array(&stroke.view);
                    let eye = v.inverse().transform_point3(Vec3::ZERO);
                    let inv = (Mat4::from_cols_array(&stroke.projection) * v).inverse();
                    let mut sets: std::collections::BTreeMap<u32, usize> = Default::default();
                    let (mut first, mut last) = (None, None);
                    // keys that reach this set's own surface when nothing else hides it
                    let set = self.set as u32;
                    let (mut own_n, mut ulo, mut uhi, mut vlo, mut vhi) = (0usize, f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                    for k in &stroke.keys {
                        let (x, y) = (2.0 * (k.pos[0] + self.xoff) - self.aspect, 2.0 * (k.pos[1] + self.yoff) - 1.0);
                        let n = inv * Vec4::new(x, y, -1.0, 1.0);
                        let f = inv * Vec4::new(x, y, 1.0, 1.0);
                        let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
                        let dir = (f - n).normalize_or_zero();
                        if let Some(h) = self.mesh.raycast_where(n, dir, |t| self.mesh.tri_mat.get(t).copied() == Some(set)) {
                            own_n += 1;
                            ulo = ulo.min(h.uv.x);
                            uhi = uhi.max(h.uv.x);
                            vlo = vlo.min(h.uv.y);
                            vhi = vhi.max(h.uv.y);
                        }
                        if let Some(h) = self.mesh.raycast(n, dir) {
                            *sets.entry(self.mesh.tri_mat.get(h.tri as usize).copied().unwrap_or(99)).or_default() += 1;
                            if first.is_none() {
                                first = Some((h.point, h.uv));
                            }
                            last = Some((h.point, h.uv));
                        }
                    }
                    // SHARD_DUMP_UV="u0,v0,u1,v1": the keys of this stroke whose own-set hit falls in that UV box
                    if let Some(bx) = std::env::var("SHARD_DUMP_UV").ok().map(|v| v.split(',').filter_map(|t| t.trim().parse::<f32>().ok()).collect::<Vec<_>>()) {
                        if bx.len() == 4 {
                            for (ki, k) in stroke.keys.iter().enumerate() {
                                let (x, y) = (2.0 * (k.pos[0] + self.xoff) - self.aspect, 2.0 * (k.pos[1] + self.yoff) - 1.0);
                                let n = inv * Vec4::new(x, y, -1.0, 1.0);
                                let f = inv * Vec4::new(x, y, 1.0, 1.0);
                                let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
                                let dir = (f - n).normalize_or_zero();
                                if let Some(h) = self.mesh.raycast_where(n, dir, |t| self.mesh.tri_mat.get(t).copied() == Some(set)) {
                                    if h.uv.x >= bx[0] && h.uv.x <= bx[2] && h.uv.y >= bx[1] && h.uv.y <= bx[3] {
                                        eprintln!(
                                            "[dump-uv] stroke keys {} seed {:08x}: key {} pos ({:.4},{:.4}) -> p({:.3},{:.3},{:.3}) uv({:.4},{:.4}) tri {}",
                                            stroke.keys.len(), stroke.seed, ki, k.pos[0], k.pos[1], h.point.x, h.point.y, h.point.z, h.uv.x, h.uv.y, h.tri
                                        );
                                    }
                                }
                            }
                        }
                    }
                    eprintln!(
                        "[dump-hits] size {:.4} keys {} eye ({:.3},{:.3},{:.3}) first-hit sets {:?} own-set hits {} uv u[{:.3},{:.3}] v[{:.3},{:.3}] first {:?} last {:?}",
                        b.size, stroke.keys.len(), eye.x, eye.y, eye.z, sets, own_n, ulo, uhi, vlo, vhi,
                        first.map(|(p, uv)| format!("p({:.3},{:.3},{:.3}) uv({:.3},{:.3})", p.x, p.y, p.z, uv.x, uv.y)),
                        last.map(|(p, uv)| format!("p({:.3},{:.3},{:.3}) uv({:.3},{:.3})", p.x, p.y, p.z, uv.x, uv.y)),
                    );
                }
            }
            if std::env::var_os("SHARD_TRACE_STROKES").is_some() {
                // where the stroke lands: UV box of the keys that hit the mesh (this texture set or not)
                let (mut lo, mut hi, mut hits_set, mut hits) = ([f32::MAX; 2], [f32::MIN; 2], 0, 0);
                let v = Mat4::from_cols_array(&stroke.view);
                let p = Mat4::from_cols_array(&stroke.projection);
                let inv = (p * v).inverse();
                for k in &stroke.keys {
                    let (x, y) = (2.0 * k.pos[0] - self.aspect, 2.0 * k.pos[1] - 1.0);
                    let n = inv * Vec4::new(x, y, -1.0, 1.0);
                    let f = inv * Vec4::new(x, y, 1.0, 1.0);
                    let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
                    if let Some(h) = self.mesh.raycast(n, (f - n).normalize_or_zero()) {
                        hits += 1;
                        if self.mesh.tri_mat.get(h.tri as usize).copied() == Some(self.set as u32) {
                            hits_set += 1;
                            for a in 0..2 {
                                lo[a] = lo[a].min(h.uv[a]);
                                hi[a] = hi[a].max(h.uv[a]);
                            }
                        }
                    }
                }
                eprintln!(
                    "[stroke] keys {} hit {} (set {}) view_type {} diameter {:.4} hardness {:.2} spacing {:.2} flow {:.2} cap {:.2} cull {:.2} uv u[{:.3},{:.3}] v[{:.3},{:.3}]",
                    stroke.keys.len(), hits, hits_set, stroke.view_type, diameter, hardness, b.spacing, b.flow, cap,
                    cull, lo[0], hi[0], lo[1], hi[1]
                );
            }
            // camera axes of the stroke in world space: stamps are laid on the screen plane and projected along the view
            let gv = Mat4::from_cols_array(&stroke.view);
            let right = gv.row(0).truncate().normalize_or_zero();
            let up = gv.row(1).truncate().normalize_or_zero();
            let mut rng = StampRng::new(stroke.seed);
            // The stencil's alpha where a surface point lies in the view of the stroke. Read per surface sample, not
            // per texel: a texel shared by mirrored parts (a test pistol's grip, magazine) belongs to several surfaces,
            // and the one kept for the texel can sit far off the screen -- the magazine text came out empty.
            let vp_st = Mat4::from_cols_array(&stroke.projection) * Mat4::from_cols_array(&stroke.view);
            let st_flip_v = std::env::var_os("SHARD_STENCIL_NOFLIPV").is_none();
            let stencil_at = |p: Vec3| -> f32 {
                let Some((img, m)) = &stencil else { return 1.0 };
                let c = vp_st * Vec4::new(p.x, p.y, p.z, 1.0);
                if c.w.abs() < 1e-9 {
                    return 0.0;
                }
                let (x, y) = (c.x / c.w, c.y / c.w);
                // row-vector 3x3: [x y 1] * M
                let u = x * m[0] + y * m[3] + m[6];
                let v = x * m[1] + y * m[4] + m[7];
                img.sample(u, if st_flip_v { 1.0 - v } else { v })
            };
            let per_sample_stencil = std::env::var_os("SHARD_STENCIL_PER_TEXEL").is_none();
            // Symmetry (`duplicateCount` copies through `duplicateMatrix`^k, a mirror x -> -x on a test pistol): every stamp
            // is laid again at the transformed place, with the transformed direction and axes; the stencil is read where
            // the original stamp would have been. 176 strokes of a test pistol's grip and cuts had their second half
            // missing. SHARD_NO_SYMMETRY=1 lays only the original.
            let copies: Vec<(Mat4, Mat4)> = {
                let mut v = vec![(Mat4::IDENTITY, Mat4::IDENTITY)];
                if let (true, Some(m)) = (stroke.duplicate_count > 0 && std::env::var_os("SHARD_NO_SYMMETRY").is_none(), stroke.duplicate_matrix) {
                    let m = Mat4::from_cols_array(&m);
                    let mut acc = Mat4::IDENTITY;
                    for _ in 0..stroke.duplicate_count.min(16) {
                        acc = m * acc;
                        v.push((acc, acc.inverse()));
                    }
                }
                v
            };
            // `heading`: direction of the stroke on the screen plane at this stamp (radians), for "follow path"
            let mut stamp = |centre: Vec3, dir: Vec3, heading: f32, tri: u32, press: f32, cov: &mut Vec<f32>, touched: &mut Vec<u32>| {
                let local = cloud.tri_texel.get(tri as usize).copied().filter(|v| *v > 0.0).unwrap_or(cloud.texel_size);
                let r = [rng.next(), rng.next(), rng.next(), rng.next(), rng.next()];
                let size = diameter * (1.0 - b.size_jitter.clamp(0.0, 1.0) * r[0]) * press_size(press);
                let mut flow = b.flow * (1.0 - b.flow_jitter.clamp(0.0, 1.0) * r[1]) * press_flow(press);
                let radius = (size * 0.5).max(local * 0.75);
                // a stamp thinner than the texels is widened to reach one, and covers it only by its share of the
                // width (Painter's sub-texel scratches are faint lines, not full-strength pixels)
                if !no_subtexel {
                    flow *= ((size * 0.5) / radius).clamp(0.0, 1.0).powf(subtexel_pow);
                }
                let mut angle = knobs.rot_sign * (b.rotation + b.rotation_jitter * (2.0 * r[2] - 1.0));
                if b.follow_path {
                    angle += heading + knobs.follow_base;
                }
                let mut c = centre;
                if b.position_jitter > 0.0 {
                    let j = b.position_jitter * size;
                    c += right * ((2.0 * r[3] - 1.0) * j) + up * ((2.0 * r[4] - 1.0) * j);
                }
                let (sa, ca) = angle.sin_cos();
                let reach = if shape.is_some() { radius * std::f32::consts::SQRT_2 } else { radius };
                for (k, (tm, inv)) in copies.iter().enumerate() {
                    let mirrored = k > 0;
                    let (c, dir, right, up) = if mirrored {
                        (
                            tm.transform_point3(c),
                            tm.transform_vector3(dir).normalize_or_zero(),
                            tm.transform_vector3(right),
                            tm.transform_vector3(up),
                        )
                    } else {
                        (c, dir, right, up)
                    };
                    cloud.query(c, reach, |t| {
                        if -t.normal.dot(dir) < cull {
                            return;
                        }
                        let d = t.pos - c;
                        let a = match &shape {
                            None => falloff(d.length() / radius, hardness) * flow,
                            Some(img) => {
                                // the stamp lies on the screen plane; what is deeper than the stamp's radius is not reached
                                if d.dot(dir).abs() > radius {
                                    return;
                                }
                                let (x, y) = (d.dot(right), d.dot(up));
                                let (xr, yr) = (x * ca + y * sa, -x * sa + y * ca);
                                img.sample(0.5 + xr / (2.0 * radius), 0.5 - yr / (2.0 * radius)) * flow
                            }
                        };
                        let a = if per_sample_stencil {
                            a * stencil_at(if mirrored { inv.transform_point3(t.pos) } else { t.pos })
                        } else {
                            a
                        };
                        if a <= 0.0 {
                            return;
                        }
                        let cv = &mut cov[t.idx as usize];
                        if *cv == 0.0 {
                            touched.push(t.idx);
                        }
                        *cv = 1.0 - (1.0 - *cv) * (1.0 - a);
                    });
                }
            };
            // direction of a step on the screen plane
            let heading_of = |from: Vec3, to: Vec3| -> f32 {
                let d = to - from;
                d.dot(up).atan2(d.dot(right))
            };
            let mut carry = 0.0f32;
            for (i, &(pt, dir)) in path.iter().enumerate() {
                let (tri, pr) = (path3[i].2, path3[i].3);
                if i == 0 {
                    let h0 = path.get(1).map(|n| heading_of(pt, n.0)).unwrap_or(0.0);
                    stamp(pt, dir, h0, tri, pr, &mut cov, &mut touched);
                    continue;
                }
                let pr0 = path3[i - 1].3;
                let (pp, _) = path[i - 1];
                let seg = pt - pp;
                let len = seg.length();
                if len > diameter * 8.0 + 0.05 {
                    // discontinuity (lifted/jumped): start a new run at this key
                    let h0 = path.get(i + 1).map(|n| heading_of(pt, n.0)).unwrap_or(0.0);
                    stamp(pt, dir, h0, tri, pr, &mut cov, &mut touched);
                    carry = 0.0;
                    continue;
                }
                let heading = heading_of(pp, pt);
                // the spacing is a fraction of the stamp's current size: a light pen stroke lays its smaller stamps
                // closer together (a fixed step left dotted lines where Painter draws a continuous scratch)
                let seg_step = (step * press_size(pr0.min(pr))).max(cloud.texel_size * 0.35);
                let mut d = seg_step - carry.min(seg_step);
                while d <= len {
                    let t = d / len.max(1e-9);
                    stamp(pp + seg * t, dir, heading, tri, pr0 + (pr - pr0) * t, &mut cov, &mut touched);
                    d += seg_step;
                }
                carry = len - (d - seg_step);
            }
            // the stencil: its alpha at the position of the texel in the view of the stroke
            let vp = Mat4::from_cols_array(&stroke.projection) * Mat4::from_cols_array(&stroke.view);
            let st_flip = std::env::var_os("SHARD_STENCIL_NOFLIPV").is_none();
            let envf = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            let (st_du, st_dv) = (envf("SHARD_STENCIL_DU"), envf("SHARD_STENCIL_DV"));
            let st_alpha = |i: u32| -> f32 {
                let Some((img, m)) = &stencil else { return 1.0 };
                let k = cloud.by_idx[i as usize];
                if k == u32::MAX {
                    return 0.0;
                }
                let p = cloud.texels[k as usize].pos;
                let c = vp * Vec4::new(p.x, p.y, p.z, 1.0);
                if c.w.abs() < 1e-9 {
                    return 0.0;
                }
                let (x, y) = (c.x / c.w, c.y / c.w);
                // row-vector 3x3: [x y 1] * M
                let u = x * m[0] + y * m[3] + m[6] + st_du;
                let v = x * m[1] + y * m[4] + m[7] + st_dv;
                img.sample(u, if st_flip { 1.0 - v } else { v })
            };
            if std::env::var_os("SHARD_TRACE_STROKES").is_some() && stencil.is_some() {
                // SHARD_TRACE_RAY=1: every surface the ray of the first key crosses (element, distance, uv, facing)
                if let (Some(k), true) = (stroke.keys.first(), std::env::var_os("SHARD_TRACE_RAY").is_some()) {
                    self.ensure_groups();
                    let g = self.groups.borrow();
                    let el = &g.as_ref().unwrap().element;
                    let inv = vp.inverse();
                    let (x, y) = (2.0 * (k.pos[0] + self.xoff) - self.aspect, 2.0 * (k.pos[1] + self.yoff) - 1.0);
                    let n = inv * Vec4::new(x, y, -1.0, 1.0);
                    let f = inv * Vec4::new(x, y, 1.0, 1.0);
                    let (o, fp) = (n.truncate() / n.w, f.truncate() / f.w);
                    let d = (fp - o).normalize_or_zero();
                    let m = self.mesh;
                    let mut hits: Vec<(f32, u32, [f32; 2], f32)> = Vec::new();
                    for t in 0..m.indices.len() / 3 {
                        let i = &m.indices[3 * t..3 * t + 3];
                        let (a, b, c) = (
                            Vec3::from(m.vertices[i[0] as usize].pos),
                            Vec3::from(m.vertices[i[1] as usize].pos),
                            Vec3::from(m.vertices[i[2] as usize].pos),
                        );
                        // Moller-Trumbore
                        let (e1, e2) = (b - a, c - a);
                        let pv = d.cross(e2);
                        let det = e1.dot(pv);
                        if det.abs() < 1e-12 {
                            continue;
                        }
                        let tv = o - a;
                        let u = tv.dot(pv) / det;
                        let qv = tv.cross(e1);
                        let v = d.dot(qv) / det;
                        if u < 0.0 || v < 0.0 || u + v > 1.0 {
                            continue;
                        }
                        let tt = e2.dot(qv) / det;
                        if tt > 0.0 {
                            let uv0 = m.vertices[i[0] as usize].uv;
                            let uv1 = m.vertices[i[1] as usize].uv;
                            let uv2 = m.vertices[i[2] as usize].uv;
                            let w = 1.0 - u - v;
                            let uv = [uv0[0] * w + uv1[0] * u + uv2[0] * v, uv0[1] * w + uv1[1] * u + uv2[1] * v];
                            let facing = -e1.cross(e2).normalize_or_zero().dot(d);
                            hits.push((tt, el[t], uv, facing));
                        }
                    }
                    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
                    // SHARD_TRACE_VIEW=DIR: the stroke camera's picture (elements in false colours, keys in white)
                    if let Some(dir) = std::env::var_os("SHARD_TRACE_VIEW") {
                        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                        let idx = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let s = 512usize;
                        let mut depth = vec![f32::INFINITY; s * s];
                        let mut col = vec![[16u8, 16, 16]; s * s];
                        for t in 0..m.indices.len() / 3 {
                            let i = &m.indices[3 * t..3 * t + 3];
                            let q: Vec<(f32, f32, f32)> = i
                                .iter()
                                .map(|&vi| {
                                    let p = m.vertices[vi as usize].pos;
                                    let c = vp * Vec4::new(p[0], p[1], p[2], 1.0);
                                    (((c.x / c.w) * 0.5 + 0.5) * s as f32, (0.5 - (c.y / c.w) * 0.5) * s as f32, c.z / c.w)
                                })
                                .collect();
                            let det = (q[1].1 - q[2].1) * (q[0].0 - q[2].0) + (q[2].0 - q[1].0) * (q[0].1 - q[2].1);
                            if det.abs() < 1e-9 {
                                continue;
                            }
                            let (x0, x1) = (q.iter().map(|v| v.0).fold(f32::MAX, f32::min).max(0.0), q.iter().map(|v| v.0).fold(f32::MIN, f32::max).min(s as f32 - 1.0));
                            let (y0, y1) = (q.iter().map(|v| v.1).fold(f32::MAX, f32::min).max(0.0), q.iter().map(|v| v.1).fold(f32::MIN, f32::max).min(s as f32 - 1.0));
                            if x1 < x0 || y1 < y0 {
                                continue;
                            }
                            let h = el[t].wrapping_mul(2654435761);
                            let c = [64 + (h & 127) as u8, 64 + ((h >> 8) & 127) as u8, 64 + ((h >> 16) & 127) as u8];
                            for y in y0 as usize..=y1 as usize {
                                for x in x0 as usize..=x1 as usize {
                                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                                    let l0 = ((q[1].1 - q[2].1) * (px - q[2].0) + (q[2].0 - q[1].0) * (py - q[2].1)) / det;
                                    let l1 = ((q[2].1 - q[0].1) * (px - q[2].0) + (q[0].0 - q[2].0) * (py - q[2].1)) / det;
                                    let l2 = 1.0 - l0 - l1;
                                    if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                                        continue;
                                    }
                                    let z = l0 * q[0].2 + l1 * q[1].2 + l2 * q[2].2;
                                    let k = y * s + x;
                                    if z < depth[k] {
                                        depth[k] = z;
                                        col[k] = c;
                                    }
                                }
                            }
                        }
                        for kk in &stroke.keys {
                            let (x, y) = (2.0 * (kk.pos[0] + self.xoff) - self.aspect, 2.0 * (kk.pos[1] + self.yoff) - 1.0);
                            let (px, py) = (((x * 0.5 + 0.5) * s as f32) as i64, ((0.5 - y * 0.5) * s as f32) as i64);
                            if (0..s as i64).contains(&px) && (0..s as i64).contains(&py) {
                                col[py as usize * s + px as usize] = [255, 255, 255];
                            }
                        }
                        let mut b = format!("P6\n{s} {s}\n255\n").into_bytes();
                        b.extend(col.iter().flatten());
                        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("view_{idx:03}.ppm")), b);
                    }
                    let list: Vec<String> =
                        hits.iter().take(8).map(|h| format!("t {:.3} el {} uv ({:.3},{:.3}) face {:+.2}", h.0, h.1, h.2[0], h.2[1], h.3)).collect();
                    eprintln!("[ray] key0 crosses {} surfaces: {}", hits.len(), list.join(" | "));
                }
                // the first key, where its ray landed and where that point projects back
                if let (Some(k), Some(hit)) = (stroke.keys.first(), path.first()) {
                    let c = vp * Vec4::new(hit.0.x, hit.0.y, hit.0.z, 1.0);
                    eprintln!(
                        "[stencil] key0 ndc ({:.3},{:.3}) -> hit {:?} -> ndc ({:.3},{:.3}) w {:.3}",
                        2.0 * k.pos[0] - 1.0,
                        2.0 * k.pos[1] - 1.0,
                        hit.0,
                        c.x / c.w,
                        c.y / c.w,
                        c.w
                    );
                }
                let (mut n_on, mut lo, mut hi, mut umin, mut umax, mut vmin, mut vmax) = (0, [u32::MAX; 2], [0u32; 2], f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                let (mut tlo, mut thi) = ([u32::MAX; 2], [0u32; 2]);
                for &i in &touched {
                    let a = st_alpha(i);
                    let (x, y) = (i % w, i / w);
                    tlo = [tlo[0].min(x), tlo[1].min(y)];
                    thi = [thi[0].max(x), thi[1].max(y)];
                    if a > 0.05 {
                        n_on += 1;
                        lo = [lo[0].min(x), lo[1].min(y)];
                        hi = [hi[0].max(x), hi[1].max(y)];
                    }
                    // stencil uv extent of the touched texels
                    let k = cloud.by_idx[i as usize];
                    if k != u32::MAX {
                        let p = cloud.texels[k as usize].pos;
                        let c = vp * Vec4::new(p.x, p.y, p.z, 1.0);
                        if let Some((_, m)) = &stencil {
                            let (x, y) = (c.x / c.w, c.y / c.w);
                            let (u, v) = (x * m[0] + y * m[3] + m[6], x * m[1] + y * m[4] + m[7]);
                            umin = umin.min(u);
                            umax = umax.max(u);
                            vmin = vmin.min(v);
                            vmax = vmax.max(v);
                        }
                    }
                }
                eprintln!(
                    "[stencil] touched {} (box ({},{})-({},{})) on {} texel box ({},{})-({},{}) stencil uv u[{:.2},{:.2}] v[{:.2},{:.2}]",
                    touched.len(), tlo[0], tlo[1], thi[0], thi[1], n_on, lo[0], lo[1], hi[0], hi[1], umin, umax, vmin, vmax
                );
            }
            // SHARD_PROBE_PAINT="x,y": report the strokes that change that texel (which layer: see the [paint] trace line)
            // the probe watches a small neighbourhood (a 1 texel line may miss the texel itself)
            let probe_cells: Vec<usize> = match probe {
                Some(i) => {
                    let (px, py) = ((i % w as usize) as i32, (i / w as usize) as i32);
                    let mut v = Vec::new();
                    for dy in -3..=3 {
                        for dx in -3..=3 {
                            let (x, y) = (px + dx, py + dy);
                            if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                                v.push(y as usize * w as usize + x as usize);
                            }
                        }
                    }
                    v
                }
                None => Vec::new(),
            };
            let probe_before: Vec<f32> = probe_cells.iter().map(|&i| buf[i]).collect();
            if std::env::var_os("SHARD_TRACE_APPLY").is_some() {
                eprintln!("[apply] keys {} path {} touched {} diameter {:.4} texel {:.5}", stroke.keys.len(), path.len(), touched.len(), diameter, cloud.texel_size);
            }
            // apply the finished stroke
            for &i in &touched {
                let a = cov[i as usize].min(cap) * if per_sample_stencil { 1.0 } else { st_alpha(i) };
                let dst = &mut buf[i as usize];
                match revert {
                    Some(base) => *dst += (base - *dst) * a,
                    None => *dst += (value - *dst) * a,
                }
                cov[i as usize] = 0.0;
            }
            if !probe_cells.is_empty() {
                let changed: Vec<(usize, f32, f32)> = probe_cells
                    .iter()
                    .zip(&probe_before)
                    .filter(|(&i, &before)| (buf[i] - before).abs() > 0.005)
                    .map(|(&i, &before)| (i, before, buf[i]))
                    .collect();
                if let Some((i, before, after)) = changed.first() {
                    eprintln!(
                        "[probe-paint] {} texels near the probe changed (e.g. {} {:.3} -> {:.3}): stroke of {} keys, seed {:08x}, brush size {:.4} flow {:.2} spacing {:.2} value {:.2}",
                        changed.len(), i, before, after, stroke.keys.len(), stroke.seed, b.size, b.flow, b.spacing, value
                    );
                }
            }
            touched.clear();
        }
    }
}

/// Window aspect ratio of the painting session, from the horizontal spread of all stroke keys: the
/// strokes stay roughly centred, so `aspect ~ u_min + u_max` (percentiles guard against outliers).
pub fn estimate_aspect(mut us: Vec<f32>) -> f32 {
    if us.len() < 50 {
        return 1.0;
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |f: f32| us[((us.len() - 1) as f32 * f) as usize];
    (q(0.01) + q(0.99)).clamp(0.6, 3.0)
}
