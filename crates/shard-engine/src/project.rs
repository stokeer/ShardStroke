//! A Painter project opened headlessly: stacks per texture set, mesh, embedded resources and bakes.

use std::path::{Path, PathBuf};

use shard_model::Layer;

use crate::bitmaps::ProjectBitmaps;
use crate::compose;
use crate::mesh::Mesh;

/// One texture set (material): its layer stack and texture size.
#[derive(Clone)]
pub struct SetData {
    pub name: String,
    pub layers: Vec<Layer>,
    pub resolution: (u32, u32),
    /// `sceneMaterialUID`: the set's channels in Painter's texture cache (0 = unknown).
    pub cache_uid: u64,
}

pub struct Project {
    pub path: PathBuf,
    pub name: String,
    pub mesh: Mesh,
    /// One entry per texture set of the mesh, in mesh order.
    pub sets: Vec<SetData>,
    /// Resource URLs of the project (`resources.toc`), used to find bakes by name.
    pub bake_urls: Vec<String>,
    /// Aspect of the window the strokes were painted in (see `strokes::estimate_aspect`).
    pub stroke_aspect: f32,
}

impl Project {
    pub fn open(path: &Path) -> Result<Project, String> {
        let stacks = shard_model::import::import_project_stacks(path).map_err(|e| e.to_string())?;
        let mut c = shard_formats::Container::open(path).map_err(|e| e.to_string())?;
        let scene = c.scene().map_err(|e| e.to_string())?;
        if scene.meshes.is_empty() {
            return Err("project has no readable scene mesh".into());
        }
        let mesh = Mesh::from_scene(&scene);
        let names = mesh.set_names.clone();
        let mut sets: Vec<SetData> = names
            .iter()
            .map(|n| match stacks.iter().find(|s| &s.name == n) {
                Some(s) => SetData { name: n.clone(), layers: s.layers.clone(), resolution: s.resolution, cache_uid: s.uid },
                None => SetData { name: n.clone(), layers: Vec::new(), resolution: (1024, 1024), cache_uid: 0 },
            })
            .collect();
        if sets.is_empty() {
            sets.push(SetData { name: "Material".into(), layers: Vec::new(), resolution: (1024, 1024), cache_uid: 0 });
        }
        let mut us = Vec::new();
        for s in &sets {
            compose::collect_key_u(&s.layers, &mut us);
        }
        let mut bake_urls = ProjectBitmaps::open(path, 1024).map(|b| b.resource_urls()).unwrap_or_default();
        // the mesh maps each texture set is assigned in the project
        for st in &stacks {
            bake_urls.extend(compose::assignment_records(&st.name, &st.mesh_maps));
        }
        Ok(Project {
            path: path.to_path_buf(),
            name: path.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string(),
            mesh,
            sets,
            bake_urls,
            stroke_aspect: crate::strokes::estimate_aspect(us),
        })
    }

    /// Evaluate channels `bits` of texture set `set` at `res` x `res`, exactly as the app's worker does.
    pub fn compose(&self, set: usize, bits: &[u32], res: u32) -> shard_compose::Composite {
        self.compose_layers(set, &self.sets[set].layers, bits, res)
    }

    /// [`Self::compose`] for an edited copy of the stack of `set`.
    pub fn compose_layers(&self, set: usize, layers: &[shard_model::Layer], bits: &[u32], res: u32) -> shard_compose::Composite {
        let st = &self.sets[set];
        let bitmaps = ProjectBitmaps::open(&self.path, if res > 1024 { 2048 } else { 1024 });
        compose::compose_job(
            &compose::document(&st.name, layers),
            bitmaps.as_ref(),
            &self.bake_urls,
            &st.name,
            &self.mesh,
            set,
            bits,
            res,
            self.stroke_aspect,
        )
    }

    /// Final tangent-space normal of a set at `res`: mesh normal map + Height (as a normal) + Normal channel.
    pub fn normal(&self, set: usize, res: u32) -> shard_compose::Image {
        let st = &self.sets[set];
        let comp = self.compose(set, &[1, 22], res);
        let bitmaps = ProjectBitmaps::open(&self.path, (res * 2).clamp(1024, 4096));
        compose::assemble_job_normal(bitmaps.as_ref(), &self.bake_urls, &st.name, res, comp.channels.get(&1), comp.channels.get(&22), Some((&self.mesh, set)))
            .unwrap_or_else(|| shard_compose::Image::filled(res, res, &[0.5, 0.5, 1.0]))
    }

    /// Baked mesh map of a set (decoded up to `max_dim`), found by name.
    pub fn mesh_map(&self, set: usize, map: shard_compose::MeshMap, max_dim: u32) -> Option<shard_compose::Image> {
        let url = compose::find_bake_url(&self.bake_urls, &self.sets.get(set)?.name, map)?;
        ProjectBitmaps::open(&self.path, max_dim)?.bitmap_full(url, max_dim)
    }
}
