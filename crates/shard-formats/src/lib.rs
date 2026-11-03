//! Readers for Substance 3D Painter data files.
//!
//! * [`h5`]: minimal read-only HDF5 reader (container of `.spp/.spt/.spsm/.spmsk/.sppr`).
//! * [`alg`]: decoder of the "Alg" reflective serialization used by the members.
//! * [`enums`]: value -> name tables (blend modes, projections, channel types).
//!
//! Format documentation: `docs/FORMATS.md`. Only data files are read; no Adobe code is involved.

pub mod alg;
pub mod assbin;
pub mod enums;
pub mod h5;
pub mod image;
pub mod sbsar;
pub mod texcache;

use std::fs::File;
use std::path::Path;

/// A Painter container file (`.spp`, `.spt`, `.spsm`, `.spmsk`, `.sppr`).
pub struct Container {
    h5: h5::H5File<File>,
    datasets: Vec<h5::Dataset>,
}

impl Container {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, h5::Error> {
        let mut h5 = h5::H5File::open(path)?;
        let datasets = h5.datasets()?;
        Ok(Self { h5, datasets })
    }

    pub fn members(&self) -> impl Iterator<Item = &h5::Dataset> {
        self.datasets.iter()
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, h5::Error> {
        let ds = self
            .datasets
            .iter()
            .find(|d| d.path == path)
            .cloned()
            .ok_or_else(|| h5::Error::NotFound(path.to_string()))?;
        self.h5.read(&ds)
    }

    /// First `n` bytes of a member (header peeking without reading large payloads).
    pub fn read_prefix(&mut self, path: &str, n: usize) -> Result<Vec<u8>, h5::Error> {
        let ds = self
            .datasets
            .iter()
            .find(|d| d.path == path)
            .cloned()
            .ok_or_else(|| h5::Error::NotFound(path.to_string()))?;
        self.h5.read_prefix(&ds, n)
    }

    /// Meshes and material names of the project scene (`paint/scene.assbin`).
    pub fn scene(&mut self) -> Result<assbin::Scene, h5::Error> {
        if !self.datasets.iter().any(|d| d.path == "paint/scene.assbin") {
            return Ok(assbin::Scene::default());
        }
        let b = self.read("paint/scene.assbin")?;
        Ok(assbin::parse_scene(&b))
    }

    /// Meshes stored by the project (`paint/scene.assbin`), empty if the member is absent.
    pub fn scene_meshes(&mut self) -> Result<Vec<assbin::SceneMesh>, h5::Error> {
        if !self.datasets.iter().any(|d| d.path == "paint/scene.assbin") {
            return Ok(Vec::new());
        }
        let b = self.read("paint/scene.assbin")?;
        Ok(assbin::parse_meshes(&b))
    }

    /// Channels Painter cached for a texture set (`sceneMaterialUID`): `(channel type, member path)`, the first
    /// stack of the set. Empty when the project has no cache for it.
    pub fn texture_cache_channels(&self, scene_material_uid: u64) -> Vec<(u32, String)> {
        let prefix = format!("texture-cache/{scene_material_uid}/");
        let mut stack: Option<&str> = None;
        let mut out = Vec::new();
        for d in &self.datasets {
            let Some(rest) = d.path.strip_prefix(&prefix) else { continue };
            let Some((st, ch)) = rest.split_once('/') else { continue };
            if *stack.get_or_insert(st) != st {
                continue;
            }
            if let Ok(c) = ch.parse::<u32>() {
                out.push((c, d.path.clone()));
            }
        }
        out.sort();
        out
    }

    /// One cached channel of a texture set, decoded (rows top-down).
    pub fn texture_cache(&mut self, scene_material_uid: u64, channel: u32) -> Option<texcache::CacheImage> {
        let path = self.texture_cache_channels(scene_material_uid).into_iter().find(|(c, _)| *c == channel)?.1;
        texcache::decode(&self.read(&path).ok()?)
    }

    /// Read and decode an Alg member.
    pub fn decode(&mut self, path: &str) -> Result<alg::Value, Box<dyn std::error::Error>> {
        let b = self.read(path)?;
        Ok(alg::decode(&b)?)
    }
}
