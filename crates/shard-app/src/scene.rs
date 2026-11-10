//! Scene state: mesh, paint texture, brush, camera. No GPU or UI types here.

use std::path::Path;

use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::mesh::Mesh;
use crate::paint::{Brush, PaintTexture, Tool};

pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: Vec3,
    /// Offset of the projection centre in NDC: the scene fills the whole window and is centred in the free
    /// area the windows leave open.
    pub center: Vec2,
    /// Vertical field of view, degrees (Painter's default 45; Display Settings > Camera).
    pub fov: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self { yaw: 0.6, pitch: 0.35, dist: 4.4, target: Vec3::ZERO, center: Vec2::ZERO, fov: 45.0 }
    }
    pub fn eye(&self) -> Vec3 {
        self.target
            + self.dist
                * Vec3::new(
                    self.pitch.cos() * self.yaw.sin(),
                    self.pitch.sin(),
                    self.pitch.cos() * self.yaw.cos(),
                )
    }
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let v = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        // Painter's default camera: 45 degrees of vertical field of view (Display Settings > Camera)
        let p = Mat4::perspective_rh(self.fov.clamp(5.0, 120.0).to_radians(), aspect, 0.05, 100.0);
        Mat4::from_translation(Vec3::new(self.center.x, self.center.y, 0.0)) * p * v
    }
    pub fn ray(&self, aspect: f32, ndc: Vec2) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let a = inv * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let b = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        (a, (b - a).normalize())
    }
}

/// Resolution of every texture set's paint target.
pub const TEX_SIZE: u32 = 1024;

pub struct Scene {
    pub mesh: Mesh,
    /// One paint target per texture set (material); `mesh.tri_mat` selects the set of a triangle.
    pub texs: Vec<PaintTexture>,
    pub brush: Brush,
    pub tool: Tool,
    pub cam: Camera,
    pub mesh_name: String,
    /// File the mesh was loaded from when it is a loose mesh (not a project scene).
    pub mesh_path: Option<std::path::PathBuf>,
    /// Texture set receiving paint (only triangles of this set are painted, like in Painter).
    pub active_set: usize,
    /// Texture sets hidden in the viewport (bit per set): not drawn, and the brush and the eyedropper see
    /// through them.
    pub hidden_sets: u32,
    /// The only texture set shown (Alt+Q / the isolate button); the brush sees only it too.
    pub isolated: Option<usize>,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            mesh: Mesh::uv_sphere(48, 96),
            texs: vec![PaintTexture::new(TEX_SIZE, TEX_SIZE)],
            brush: Brush::default(),
            tool: Tool::Paint,
            cam: Camera::new(),
            mesh_name: "Sphere".into(),
            mesh_path: None,
            active_set: 0,
            hidden_sets: 0,
            isolated: None,
        }
    }

    /// (Re)create one paint target per texture set of the current mesh (default resolution).
    fn reset_texture_sets(&mut self) {
        let n = self.mesh.set_count();
        self.texs = (0..n).map(|_| PaintTexture::new(TEX_SIZE, TEX_SIZE)).collect();
    }

    /// Resize the paint targets to the given per-set resolutions (paint is discarded).
    pub fn set_resolutions(&mut self, res: &[(u32, u32)]) {
        let n = self.mesh.set_count();
        self.texs = (0..n)
            .map(|i| {
                let (w, h) = res.get(i).copied().unwrap_or((TEX_SIZE, TEX_SIZE));
                PaintTexture::new(w.clamp(64, 4096), h.clamp(64, 4096))
            })
            .collect();
    }

    /// Start recording the active set's tiles for undo.
    pub fn begin_stroke(&mut self) {
        if let Some(t) = self.texs.get_mut(self.active_set) {
            t.begin_stroke();
        }
    }

    /// Finish the stroke; the active set index and the tiles it overwrote.
    pub fn end_stroke(&mut self) -> Option<(usize, Vec<crate::paint::TileSnap>)> {
        let set = self.active_set;
        self.texs.get_mut(set)?.end_stroke().map(|t| (set, t))
    }

    /// Nearest hit on a visible texture set.
    pub fn hit(&self, o: Vec3, d: Vec3) -> Option<crate::mesh::Hit> {
        let (hidden, only) = (self.hidden_sets, self.isolated);
        self.mesh.raycast_where(o, d, |t| {
            self.mesh.tri_mat.get(t).is_none_or(|&s| {
                only.is_none_or(|i| i == s as usize) && (s >= 32 || (hidden >> s) & 1 == 0)
            })
        })
    }

    /// Colour seen under `ndc` (the surface's base colour with the paint over it), for the eyedropper.
    pub fn sample_color(&self, aspect: f32, ndc: Vec2) -> Option<[f32; 3]> {
        let (o, d) = self.cam.ray(aspect, ndc);
        let hit = self.hit(o, d)?;
        let set = (*self.mesh.tri_mat.get(hit.tri as usize)? as usize).min(self.texs.len().checked_sub(1)?);
        Some(self.texs[set].color_at_uv(hit.uv.into()))
    }

    pub fn clear_paint(&mut self) {
        for t in &mut self.texs {
            t.clear_paint();
        }
    }

    /// Replace the mesh with the scene stored in a Painter project (`.spp`).
    pub fn load_project_mesh(&mut self, path: &Path) -> Result<usize, String> {
        let mut c = shard_formats::Container::open(path).map_err(|e| e.to_string())?;
        let scene = c.scene().map_err(|e| e.to_string())?;
        if scene.meshes.is_empty() {
            return Err("project has no readable scene mesh".into());
        }
        let n = scene.meshes.len();
        self.mesh = Mesh::from_scene(&scene);
        // the scene keeps Painter's own coordinates: the camera looks at its centre from a distance that frames it
        let (centre, radius) = self.mesh.bounds();
        self.cam.target = centre;
        self.cam.dist = 4.4 * (radius / 1.2).clamp(0.05, 50.0);
        self.mesh_path = None;
        self.mesh_name = format!(
            "{} ({} meshes, {} sets)",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("project"),
            n,
            self.mesh.set_count()
        );
        self.reset_texture_sets();
        Ok(n)
    }

    pub fn load_mesh(&mut self, path: &Path) -> Result<(), String> {
        self.mesh = Mesh::load_obj(path)?;
        self.mesh_path = Some(path.to_path_buf());
        self.mesh_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("mesh").to_string();
        self.reset_texture_sets();
        Ok(())
    }

    /// Projection painting: paint a stroke segment from `prev` to `ndc` (normalized device coords in
    /// the viewport). The brush is a sphere in 3D space whose screen size is `brush.radius` pixels,
    /// so the footprint on the surface is undistorted regardless of the UV layout.
    /// `vp_px` is the viewport size in pixels. Returns the new "previous" position.
    pub fn paint_stroke(&mut self, aspect: f32, vp_px: (f32, f32), ndc: Vec2, prev: Option<Vec2>) -> Option<Vec2> {
        let diam_ndc = 2.0 * self.brush.radius / (vp_px.1 * 0.5).max(1.0);
        let step = (diam_ndc * self.brush.spacing / 100.0).max(1e-4);
        let mut pts = Vec::new();
        match prev {
            Some(p) => {
                let d = ndc - p;
                let len = Vec2::new(d.x * aspect, d.y).length();
                let n = (len / step).ceil().clamp(1.0, 400.0) as u32;
                for i in 1..=n {
                    pts.push(p + d * (i as f32 / n as f32));
                }
            }
            None => pts.push(ndc),
        }
        let mut any = false;
        for p in pts {
            any |= self.stamp_ndc(aspect, vp_px, p);
        }
        if any || prev.is_some() {
            Some(ndc)
        } else {
            None
        }
    }

    fn stamp_ndc(&mut self, aspect: f32, vp_px: (f32, f32), ndc: Vec2) -> bool {
        let (o, d) = self.cam.ray(aspect, ndc);
        let Some(hit) = self.hit(o, d) else { return false };
        let eye = self.cam.eye();
        let dist = (hit.point - eye).length();
        let world_per_px = 2.0 * (self.cam.fov.clamp(5.0, 120.0).to_radians() * 0.5).tan() * dist / vp_px.1.max(1.0);
        let r = (self.brush.radius * world_per_px).max(1e-5);
        self.stamp_surface(hit.point, r, d);
        true
    }

    /// Rasterize every triangle near `centre` into texture space; texels whose 3D position lies
    /// within `r` of `centre` (and that face the viewer) receive paint.
    fn stamp_surface(&mut self, centre: Vec3, r: f32, view_dir: Vec3) {
        let brush = self.brush;
        let tool = self.tool;
        let r2 = r * r;
        // dirty rectangle per texture set
        let mut bbs: Vec<(u32, u32, u32, u32)> = vec![(u32::MAX, u32::MAX, 0, 0); self.texs.len()];
        let mut tris = Vec::new();
        self.mesh.query_sphere(centre, r, |t| tris.push(t));
        let verts = &self.mesh.vertices;
        for ti in tris {
            let set = (self.mesh.tri_mat[ti] as usize).min(self.texs.len() - 1);
            if set != self.active_set {
                continue; // only the active texture set is paintable
            }
            let (tw_i, th_i) = (self.texs[set].w, self.texs[set].h);
            let (tw, th) = (tw_i as f32, th_i as f32);
            let t = &self.mesh.indices[3 * ti..3 * ti + 3];
            let (v0, v1, v2) = (&verts[t[0] as usize], &verts[t[1] as usize], &verts[t[2] as usize]);
            let (p0, p1, p2) = (Vec3::from(v0.pos), Vec3::from(v1.pos), Vec3::from(v2.pos));
            let t0 = Vec2::new(v0.uv[0] * tw, (1.0 - v0.uv[1]) * th);
            let t1 = Vec2::new(v1.uv[0] * tw, (1.0 - v1.uv[1]) * th);
            let t2 = Vec2::new(v2.uv[0] * tw, (1.0 - v2.uv[1]) * th);
            let edge = |a: Vec2, b: Vec2, c: Vec2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            let area = edge(t0, t1, t2);
            if area.abs() < 1e-6 {
                continue;
            }
            let min = t0.min(t1).min(t2);
            let max = t0.max(t1).max(t2);
            let x0 = (min.x.floor() as i64).max(0);
            let y0 = (min.y.floor() as i64).max(0);
            let x1 = (max.x.ceil() as i64).min(tw_i as i64 - 1);
            let y1 = (max.y.ceil() as i64).min(th_i as i64 - 1);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = edge(t1, t2, p) / area;
                    let w1 = edge(t2, t0, p) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let pos = p0 * w0 + p1 * w1 + p2 * w2;
                    let d2 = (pos - centre).length_squared();
                    if d2 >= r2 {
                        continue;
                    }
                    let n = Vec3::from(v0.normal) * w0 + Vec3::from(v1.normal) * w1 + Vec3::from(v2.normal) * w2;
                    if n.dot(view_dir) >= 0.0 {
                        continue; // facing away from the viewer
                    }
                    let a = crate::paint::falloff(d2.sqrt() / r, brush.hardness) * brush.flow * brush.opacity;
                    if a <= 0.0 {
                        continue;
                    }
                    self.texs[set].blend_texel(x as u32, y as u32, a.clamp(0.0, 1.0), brush.color, tool);
                    let bb = &mut bbs[set];
                    bb.0 = bb.0.min(x as u32);
                    bb.1 = bb.1.min(y as u32);
                    bb.2 = bb.2.max(x as u32 + 1);
                    bb.3 = bb.3.max(y as u32 + 1);
                }
            }
        }
        for (set, bb) in bbs.iter().enumerate() {
            if bb.2 > bb.0 && bb.3 > bb.1 {
                self.texs[set].touch(bb.0, bb.1, bb.2, bb.3);
            }
        }
    }

    /// Recompose the base colour from a layer model and show it as the base of every texture set.
    pub fn set_base_from_document(&mut self, doc: &shard_model::Document) -> Vec<String> {
        let c = shard_compose::compose(doc, 4, 4, &[0]);
        let px = c.channels[&0].pixel(0, 0);
        for t in &mut self.texs {
            t.set_base_solid([px[0], px[1], px[2]].map(shard_compose::linear_to_srgb));
        }
        c.unsupported
    }
}
