//! Triangle mesh + BVH (ray picking and sphere queries for projection painting).

use glam::{Vec2, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Texture set (material) index; selects the layer of the paint texture array.
    pub mat: u32,
    /// Tangent frame stored by Painter (the one the normal bakes were made in); zero when unknown.
    pub tangent: [f32; 3],
    pub bitangent: [f32; 3],
}

pub struct Hit {
    /// Triangle index (into `indices / 3` and `tri_mat`).
    pub tri: u32,
    pub t: f32,
    pub uv: Vec2,
    pub point: Vec3,
}

#[derive(Clone, Copy)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    left: u32,
    right: u32,
    /// leaf: first index into `order`
    first: u32,
    /// 0 = internal node, otherwise triangle count of the leaf
    count: u32,
}

#[derive(Clone)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Texture set index of every triangle.
    pub tri_mat: Vec<u32>,
    /// Names of the texture sets (materials), at least one.
    pub set_names: Vec<String>,
    /// Per texture set: `(first index, index count)` into `indices` (triangles are sorted by set).
    pub set_ranges: Vec<(u32, u32)>,
    nodes: Vec<Node>,
    order: Vec<u32>,
}

const LEAF_SIZE: usize = 4;

impl Mesh {
    /// Cheap fingerprint of the geometry (counts plus a sample of vertices and indices): tells two loaded
    /// meshes apart for the caches that outlive one composition.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.vertices.len(), self.indices.len(), self.tri_mat.len()).hash(&mut h);
        for v in self.vertices.iter().step_by(61) {
            for c in v.pos.iter().chain(v.uv.iter()) {
                c.to_bits().hash(&mut h);
            }
        }
        for i in self.indices.iter().step_by(97) {
            i.hash(&mut h);
        }
        for t in self.tri_mat.iter().step_by(89) {
            t.hash(&mut h);
        }
        h.finish()
    }

    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        let n = indices.len() / 3;
        let mut m = Mesh {
            set_ranges: vec![(0, indices.len() as u32)],
            vertices,
            indices,
            tri_mat: vec![0; n],
            set_names: vec!["Material".to_string()],
            nodes: Vec::new(),
            order: Vec::new(),
        };
        m.build_bvh();
        m
    }

    /// Number of texture sets.
    pub fn set_count(&self) -> usize {
        self.set_names.len().max(1)
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Center on the origin and scale so the bounding sphere has radius 1.
    pub fn normalize(&mut self) {
        if self.vertices.is_empty() {
            return;
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            let p = Vec3::from(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let c = (lo + hi) * 0.5;
        let r = ((hi - lo) * 0.5).max_element().max(1e-6);
        for v in &mut self.vertices {
            v.pos = ((Vec3::from(v.pos) - c) / r).to_array();
        }
        self.build_bvh();
    }

    pub fn recompute_normals(vertices: &mut [Vertex], indices: &[u32]) {
        let mut acc = vec![Vec3::ZERO; vertices.len()];
        for t in indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let n = (Vec3::from(vertices[b].pos) - Vec3::from(vertices[a].pos))
                .cross(Vec3::from(vertices[c].pos) - Vec3::from(vertices[a].pos));
            acc[a] += n;
            acc[b] += n;
            acc[c] += n;
        }
        for (v, n) in vertices.iter_mut().zip(acc) {
            v.normal = n.normalize_or_zero().to_array();
        }
    }

    pub fn uv_sphere(stacks: u32, slices: u32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for i in 0..=stacks {
            let v = i as f32 / stacks as f32;
            let phi = v * std::f32::consts::PI;
            for j in 0..=slices {
                let u = j as f32 / slices as f32;
                let th = u * std::f32::consts::TAU;
                let p = Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
                vertices.push(Vertex { pos: p.to_array(), normal: p.to_array(), uv: [u, 1.0 - v], mat: 0, tangent: [0.0; 3], bitangent: [0.0; 3] });
            }
        }
        for i in 0..stacks {
            for j in 0..slices {
                let a = i * (slices + 1) + j;
                let b = a + slices + 1;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        Mesh::new(vertices, indices)
    }

    pub fn load_obj(path: &std::path::Path) -> Result<Self, String> {
        let opts = tobj::LoadOptions { triangulate: true, single_index: true, ..Default::default() };
        let (models, _) = tobj::load_obj(path, &opts).map_err(|e| e.to_string())?;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut need_normals = false;
        for m in models {
            let mesh = m.mesh;
            let base = vertices.len() as u32;
            for i in 0..mesh.positions.len() / 3 {
                let pos = [mesh.positions[3 * i], mesh.positions[3 * i + 1], mesh.positions[3 * i + 2]];
                let normal = if mesh.normals.len() >= 3 * (i + 1) {
                    [mesh.normals[3 * i], mesh.normals[3 * i + 1], mesh.normals[3 * i + 2]]
                } else {
                    need_normals = true;
                    [0.0, 1.0, 0.0]
                };
                let uv = if mesh.texcoords.len() >= 2 * (i + 1) {
                    [mesh.texcoords[2 * i], mesh.texcoords[2 * i + 1]]
                } else {
                    [0.0, 0.0]
                };
                vertices.push(Vertex { pos, normal, uv, mat: 0, tangent: [0.0; 3], bitangent: [0.0; 3] });
            }
            indices.extend(mesh.indices.iter().map(|i| i + base));
        }
        if need_normals {
            Self::recompute_normals(&mut vertices, &indices);
        } else {
            orient_triangles(&vertices, &mut indices);
        }
        let mut m = Mesh::new(vertices, indices);
        m.normalize();
        Ok(m)
    }

    /// Merge the meshes of a Painter project scene into one mesh (UVs and authored normals kept).
    ///
    /// The authored vertex normals stored in `scene.assbin` are used when every mesh has them
    /// (hard-surface models rely on custom normals across chamfers). Otherwise normals are derived
    /// with crease-angle smoothing (see [`smooth_by_angle`]).
    pub fn from_scene(scene: &shard_formats::assbin::Scene) -> Self {
        let meshes = &scene.meshes;
        let mut pos: Vec<[f32; 3]> = Vec::new();
        let mut uv: Vec<[f32; 2]> = Vec::new();
        let mut nor: Vec<[f32; 3]> = Vec::new();
        let mut tan: Vec<[f32; 3]> = Vec::new();
        let mut bit: Vec<[f32; 3]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut tri_mat: Vec<u32> = Vec::new();
        let all_normals = meshes.iter().all(|m| m.normals.is_some()) && std::env::var_os("SHARD_RECOMPUTE_NORMALS").is_none();
        let all_tangents = all_normals
            && std::env::var_os("SHARD_DERIVATIVE_TANGENTS").is_none()
            && meshes.iter().all(|m| m.tangents.is_some() && m.bitangents.is_some());
        for m in meshes {
            let base = pos.len() as u32;
            for (i, p) in m.positions.iter().enumerate() {
                pos.push(*p);
                uv.push(m.uvs.as_ref().map(|u| u[i]).unwrap_or([0.0, 0.0]));
                if let Some(n) = &m.normals {
                    nor.push(n[i]);
                }
                if let (Some(t), Some(b)) = (&m.tangents, &m.bitangents) {
                    tan.push(t[i]);
                    bit.push(b[i]);
                }
            }
            indices.extend(m.indices.iter().map(|i| i + base));
            tri_mat.extend(std::iter::repeat(m.material).take(m.indices.len() / 3));
        }
        let (mut vertices, mut indices) = if all_normals {
            let v: Vec<Vertex> = (0..pos.len())
                .map(|i| Vertex {
                    pos: pos[i],
                    normal: nor[i],
                    uv: uv[i],
                    mat: 0,
                    tangent: if all_tangents { tan[i] } else { [0.0; 3] },
                    bitangent: if all_tangents { bit[i] } else { [0.0; 3] },
                })
                .collect();
            (v, indices)
        } else {
            smooth_by_angle(&pos, &uv, &indices, 40.0)
        };
        orient_triangles(&vertices, &mut indices); // swaps within a triangle only: triangle order is kept
        for (t, tri) in indices.chunks_exact(3).enumerate() {
            for &i in tri {
                vertices[i as usize].mat = tri_mat[t];
            }
        }
        let set_names: Vec<String> = if scene.materials.is_empty() {
            vec!["Material".to_string()]
        } else {
            scene.materials.clone()
        };
        // sort triangles by texture set so every set is one contiguous draw range
        let nsets = set_names.len().max(1);
        let ntri = indices.len() / 3;
        let mut order: Vec<usize> = (0..ntri).collect();
        order.sort_by_key(|&t| (tri_mat[t] as usize).min(nsets - 1));
        let mut sorted_idx = Vec::with_capacity(indices.len());
        let mut sorted_mat = Vec::with_capacity(ntri);
        for &t in &order {
            sorted_idx.extend_from_slice(&indices[3 * t..3 * t + 3]);
            sorted_mat.push((tri_mat[t] as usize).min(nsets - 1) as u32);
        }
        let mut ranges = vec![(0u32, 0u32); nsets];
        let mut first = 0u32;
        for s in 0..nsets {
            let count = sorted_mat.iter().filter(|&&m| m as usize == s).count() as u32 * 3;
            ranges[s] = (first, count);
            first += count;
        }
        let mut mesh = Mesh::new(vertices, sorted_idx);
        mesh.tri_mat = sorted_mat;
        mesh.set_names = set_names;
        mesh.set_ranges = ranges;
        // The scene of a project is stored in the frame Painter's cameras were recorded in (the strokes' view and
        // projection matrices, decal hits): it must not be re-centred. The a test vehicle's scene spans z in [-1, 1] and is
        // centred at y = +0.0075, not at its box centre: re-centring moved every stroke by that much, which is about
        // the diameter of a scratch brush. SHARD_NORMALIZE_SCENE=1 restores the old behaviour.
        if std::env::var_os("SHARD_NORMALIZE_SCENE").is_some() {
            mesh.normalize();
        }
        mesh
    }

    /// Centre and radius of the bounding sphere around the box of the vertices (for framing the camera).
    pub fn bounds(&self) -> (Vec3, f32) {
        if self.vertices.is_empty() {
            return (Vec3::ZERO, 1.0);
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            let p = Vec3::from(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        ((lo + hi) * 0.5, ((hi - lo) * 0.5).length().max(1e-6))
    }

    // ------------------------------------------------------------------ BVH

    fn tri_pos(&self, t: usize) -> (Vec3, Vec3, Vec3) {
        let i = &self.indices[3 * t..3 * t + 3];
        (
            Vec3::from(self.vertices[i[0] as usize].pos),
            Vec3::from(self.vertices[i[1] as usize].pos),
            Vec3::from(self.vertices[i[2] as usize].pos),
        )
    }

    fn build_bvh(&mut self) {
        let n = self.triangle_count();
        self.nodes.clear();
        self.order = (0..n as u32).collect();
        if n == 0 {
            return;
        }
        let mut lo = Vec::with_capacity(n);
        let mut hi = Vec::with_capacity(n);
        let mut cen = Vec::with_capacity(n);
        for t in 0..n {
            let (a, b, c) = self.tri_pos(t);
            let (l, h) = (a.min(b).min(c), a.max(b).max(c));
            lo.push(l);
            hi.push(h);
            cen.push((l + h) * 0.5);
        }
        self.nodes.reserve(2 * n / LEAF_SIZE + 2);
        let mut order = std::mem::take(&mut self.order);
        let mut nodes = std::mem::take(&mut self.nodes);
        build_rec(&mut nodes, &mut order, &lo, &hi, &cen, 0, n);
        self.order = order;
        self.nodes = nodes;
    }

    /// The BVH packed for a GPU traversal: two `[f32; 4]` per node, `[lo, a]` and `[hi, b]` with `a` / `b` (as
    /// bits) the children of an inner node, or `first` / `count | 1 << 31` of a leaf into the returned triangle order.
    pub fn gpu_bvh(&self) -> (Vec<[f32; 4]>, &[u32]) {
        let mut out = Vec::with_capacity(self.nodes.len() * 2);
        for n in &self.nodes {
            let (a, b) = if n.count == 0 { (n.left, n.right) } else { (n.first, n.count | 1 << 31) };
            out.push([n.lo.x, n.lo.y, n.lo.z, f32::from_bits(a)]);
            out.push([n.hi.x, n.hi.y, n.hi.z, f32::from_bits(b)]);
        }
        (out, &self.order)
    }

    /// Nearest ray/triangle intersection (Moller-Trumbore) using the BVH.
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        self.raycast_where(origin, dir, |_| true)
    }

    /// Nearest intersection among the triangles `keep` accepts (the others are transparent to the ray).
    pub fn raycast_where(&self, origin: Vec3, dir: Vec3, keep: impl Fn(usize) -> bool) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        let mut best_t = f32::MAX;
        let mut best: Option<(usize, f32, f32)> = None;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni as usize];
            if !ray_aabb(origin, inv, n.lo, n.hi, best_t) {
                continue;
            }
            if n.count == 0 {
                stack.push(n.left);
                stack.push(n.right);
            } else {
                for k in 0..n.count {
                    let t = self.order[(n.first + k) as usize] as usize;
                    if !keep(t) {
                        continue;
                    }
                    let (p0, p1, p2) = self.tri_pos(t);
                    if let Some((tt, u, v)) = ray_tri(origin, dir, p0, p1, p2) {
                        if tt < best_t {
                            best_t = tt;
                            best = Some((t, u, v));
                        }
                    }
                }
            }
        }
        let (t, u, v) = best?;
        let i = &self.indices[3 * t..3 * t + 3];
        let (v0, v1, v2) = (&self.vertices[i[0] as usize], &self.vertices[i[1] as usize], &self.vertices[i[2] as usize]);
        let w = 1.0 - u - v;
        let uv = Vec2::from(v0.uv) * w + Vec2::from(v1.uv) * u + Vec2::from(v2.uv) * v;
        Some(Hit { tri: t as u32, t: best_t, uv, point: origin + dir * best_t })
    }

    /// Call `f(triangle_index)` for every triangle whose bounds touch the sphere.
    pub fn query_sphere(&self, centre: Vec3, r: f32, mut f: impl FnMut(usize)) {
        if self.nodes.is_empty() {
            return;
        }
        let r2 = r * r;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni as usize];
            if (centre.clamp(n.lo, n.hi) - centre).length_squared() > r2 {
                continue;
            }
            if n.count == 0 {
                stack.push(n.left);
                stack.push(n.right);
            } else {
                for k in 0..n.count {
                    f(self.order[(n.first + k) as usize] as usize);
                }
            }
        }
    }
}

/// Make the winding of every triangle agree with its vertex normals (counter-clockwise = front),
/// so back-face culling and single-sided picking behave consistently even when the source file's
/// winding is mixed.
pub fn orient_triangles(vertices: &[Vertex], indices: &mut [u32]) {
    for t in indices.chunks_exact_mut(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let p0 = Vec3::from(vertices[a].pos);
        let g = (Vec3::from(vertices[b].pos) - p0).cross(Vec3::from(vertices[c].pos) - p0);
        let n = Vec3::from(vertices[a].normal) + Vec3::from(vertices[b].normal) + Vec3::from(vertices[c].normal);
        if g.dot(n) < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// Build per-corner vertices with crease-angle smoothed normals.
///
/// Faces are grouped by quantized *position* (so UV seams do not create hard edges). For every
/// triangle corner the normal is the area-weighted sum of the face normals around that position
/// whose angle to the corner's own face is below `crease_deg`.
pub fn smooth_by_angle(pos: &[[f32; 3]], uv: &[[f32; 2]], idx: &[u32], crease_deg: f32) -> (Vec<Vertex>, Vec<u32>) {
    use std::collections::HashMap;
    let q = |p: &[f32; 3]| ((p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64, (p[2] * 1e5).round() as i64);
    let mut weld: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut weld_id = vec![0u32; pos.len()];
    for (i, p) in pos.iter().enumerate() {
        let n = weld.len() as u32;
        weld_id[i] = *weld.entry(q(p)).or_insert(n);
    }
    let ntri = idx.len() / 3;
    // face normals (area-weighted, unnormalized) and unit normals
    let mut fnorm = Vec::with_capacity(ntri);
    let mut funit = Vec::with_capacity(ntri);
    for t in 0..ntri {
        let (a, b, c) = (idx[3 * t] as usize, idx[3 * t + 1] as usize, idx[3 * t + 2] as usize);
        let n = (Vec3::from(pos[b]) - Vec3::from(pos[a])).cross(Vec3::from(pos[c]) - Vec3::from(pos[a]));
        fnorm.push(n);
        funit.push(n.normalize_or_zero());
    }
    let mut around: Vec<Vec<u32>> = vec![Vec::new(); weld.len()];
    for t in 0..ntri {
        for k in 0..3 {
            around[weld_id[idx[3 * t + k] as usize] as usize].push(t as u32);
        }
    }
    let cos_c = crease_deg.to_radians().cos();
    let mut vertices = Vec::with_capacity(idx.len());
    let mut out_idx = Vec::with_capacity(idx.len());
    for t in 0..ntri {
        for k in 0..3 {
            let vi = idx[3 * t + k] as usize;
            let mut n = Vec3::ZERO;
            for &f in &around[weld_id[vi] as usize] {
                if funit[f as usize].dot(funit[t]) >= cos_c {
                    n += fnorm[f as usize];
                }
            }
            let n = if n.length_squared() > 0.0 { n.normalize() } else { funit[t] };
            out_idx.push(vertices.len() as u32);
            vertices.push(Vertex { pos: pos[vi], normal: n.to_array(), uv: uv[vi], mat: 0, tangent: [0.0; 3], bitangent: [0.0; 3] });
        }
    }
    (vertices, out_idx)
}

fn build_rec(
    nodes: &mut Vec<Node>,
    order: &mut [u32],
    lo: &[Vec3],
    hi: &[Vec3],
    cen: &[Vec3],
    start: usize,
    end: usize,
) -> u32 {
    let mut nlo = Vec3::splat(f32::MAX);
    let mut nhi = Vec3::splat(f32::MIN);
    for &t in &order[start..end] {
        nlo = nlo.min(lo[t as usize]);
        nhi = nhi.max(hi[t as usize]);
    }
    let idx = nodes.len() as u32;
    nodes.push(Node { lo: nlo, hi: nhi, left: 0, right: 0, first: start as u32, count: (end - start) as u32 });
    if end - start <= LEAF_SIZE {
        return idx;
    }
    let ext = nhi - nlo;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    order[start..end].sort_unstable_by(|&a, &b| {
        cen[a as usize][axis].partial_cmp(&cen[b as usize][axis]).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mid = start + (end - start) / 2;
    let left = build_rec(nodes, order, lo, hi, cen, start, mid);
    let right = build_rec(nodes, order, lo, hi, cen, mid, end);
    let n = &mut nodes[idx as usize];
    n.left = left;
    n.right = right;
    n.count = 0;
    idx
}

fn ray_aabb(o: Vec3, inv: Vec3, lo: Vec3, hi: Vec3, tmax: f32) -> bool {
    let t1 = (lo - o) * inv;
    let t2 = (hi - o) * inv;
    let tmin = t1.min(t2).max_element().max(0.0);
    let tm = t1.max(t2).min_element().min(tmax);
    tmin <= tm
}

fn ray_tri(o: Vec3, d: Vec3, p0: Vec3, p1: Vec3, p2: Vec3) -> Option<(f32, f32, f32)> {
    let e1 = p1 - p0;
    let e2 = p2 - p0;
    let pv = d.cross(e2);
    let det = e1.dot(pv);
    if det.abs() < 1e-12 {
        return None;
    }
    if det < 0.0 {
        return None; // back-facing (single-sided picking, matches back-face culling)
    }
    let inv = 1.0 / det;
    let tv = o - p0;
    let u = tv.dot(pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = tv.cross(e1);
    let v = d.dot(qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(qv) * inv;
    if t <= 1e-5 {
        return None;
    }
    Some((t, u, v))
}
