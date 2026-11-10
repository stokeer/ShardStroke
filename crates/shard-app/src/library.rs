//! Asset library scan (the "shelf"): classifies files of the configured library folders.
//!
//! Layout follows Painter's: `<library>/<category>/.../<asset>.<ext>` with categories such as
//! `smart-materials`, `smart-masks`, `materials`, `textures`, `alphas`, `environments`, `presets`.

use std::path::{Path, PathBuf};

use crate::config::Library;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum AssetKind {
    SmartMaterial,
    SmartMask,
    Material,
    Texture,
    Alpha,
    Environment,
    Preset,
    Procedural,
    Effect,
}

impl AssetKind {
    pub const ALL: [AssetKind; 9] = [
        AssetKind::SmartMaterial,
        AssetKind::SmartMask,
        AssetKind::Material,
        AssetKind::Texture,
        AssetKind::Alpha,
        AssetKind::Environment,
        AssetKind::Preset,
        AssetKind::Procedural,
        AssetKind::Effect,
    ];

    /// Icon of the kind in the Assets window's filter row.
    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            AssetKind::SmartMaterial => Icon::Sphere,
            AssetKind::SmartMask => Icon::Mask,
            AssetKind::Material => Icon::Material,
            AssetKind::Texture => Icon::Grid,
            AssetKind::Alpha => Icon::Alpha,
            AssetKind::Environment => Icon::Environment,
            AssetKind::Preset => Icon::Brush,
            AssetKind::Procedural => Icon::Wand,
            AssetKind::Effect => Icon::PostFx,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AssetKind::SmartMaterial => "Smart materials",
            AssetKind::SmartMask => "Smart masks",
            AssetKind::Material => "Materials",
            AssetKind::Texture => "Textures",
            AssetKind::Alpha => "Alphas",
            AssetKind::Environment => "Environments",
            AssetKind::Preset => "Tool presets",
            AssetKind::Procedural => "Procedurals",
            AssetKind::Effect => "Effects",
        }
    }

    fn from_folder(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "smart-materials" => AssetKind::SmartMaterial,
            "smart-masks" => AssetKind::SmartMask,
            "materials" => AssetKind::Material,
            "textures" => AssetKind::Texture,
            "alphas" => AssetKind::Alpha,
            "environments" => AssetKind::Environment,
            "presets" => AssetKind::Preset,
            "procedurals" | "generators" => AssetKind::Procedural,
            "effects" => AssetKind::Effect,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub path: PathBuf,
    pub kind: AssetKind,
    /// Index into the configured library list.
    pub library: usize,
}

const EXTENSIONS: [&str; 13] =
    ["spsm", "spmsk", "sbsar", "sppr", "abr", "abrimg", "png", "tga", "jpg", "jpeg", "exr", "hdr", "svg"];

/// A user may point at the install root, `resources` or the assets folder itself.
fn resolve_root(p: &Path) -> PathBuf {
    for sub in [&["resources", "starter_assets"][..], &["starter_assets"][..], &["assets"][..]] {
        let mut c = p.to_path_buf();
        for s in sub {
            c.push(s);
        }
        if c.is_dir() {
            return c;
        }
    }
    p.to_path_buf()
}

fn walk(dir: &Path, kind: AssetKind, library: usize, out: &mut Vec<Asset>, depth: u32) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, kind, library, out, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if EXTENSIONS.contains(&ext.as_str()) {
                let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                out.push(Asset { name, path: p, kind, library });
            }
        }
    }
}

/// Scan every library; assets are returned sorted by kind and name.
pub fn scan(libs: &[Library]) -> Vec<Asset> {
    let mut out = Vec::new();
    for (i, lib) in libs.iter().enumerate() {
        let root = resolve_root(&lib.path);
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let folder = e.file_name().to_string_lossy().to_string();
            if let Some(kind) = AssetKind::from_folder(&folder) {
                walk(&p, kind, i, &mut out, 0);
            }
        }
    }
    out.sort_by(|a, b| (a.kind as u8, a.name.to_ascii_lowercase()).cmp(&(b.kind as u8, b.name.to_ascii_lowercase())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Library;

    #[test]
    fn scans_painter_starter_assets() {
        let p = PathBuf::from(r"C:\Program Files\Adobe\Adobe Substance 3D Painter\resources\starter_assets");
        if !p.is_dir() {
            return;
        }
        // pointing at the install root works as well as at the assets folder
        for root in [p.clone(), p.parent().unwrap().parent().unwrap().to_path_buf()] {
            let libs = [Library { name: "starter".into(), path: root, default: true }];
            let assets = scan(&libs);
            let count = |k: AssetKind| assets.iter().filter(|a| a.kind == k).count();
            assert_eq!(count(AssetKind::SmartMaterial), 121);
            assert_eq!(count(AssetKind::SmartMask), 66);
            assert_eq!(count(AssetKind::Environment), 64);
            assert!(count(AssetKind::Material) > 100);
            assert!(assets.iter().all(|a| a.path.is_file()));
        }
    }
}
