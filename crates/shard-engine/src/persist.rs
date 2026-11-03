//! ShardStroke project files (`.shard`).
//!
//! A project file stores what the user edited on top of a base scene: the layer stack of every texture
//! set (strokes, polygon-fill hits and procedural settings included), the hand-painted overlay of every
//! set, and the session (camera, brush). The base (mesh, mesh maps, embedded resources) is the Painter
//! `.spp` or the mesh the session started from, referenced by path: opening a `.shard` opens the base and
//! applies the edits.
//!
//! Container: a ZIP archive (deflate) with `manifest.json`, `document.json`, `view.json` and
//! `paint/<set>.png`. Writing is atomic (temp file, then rename), so a crash never corrupts a project.
//! `manifest.version` is bumped on incompatible changes; readers refuse newer versions.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use shard_model::Layer;
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "shard-project";
pub const VERSION: u32 = 1;

/// The scene the edits apply to.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Base {
    /// `spp` (Painter project), `obj` (mesh file) or `none`.
    pub kind: String,
    /// Absolute path when saved.
    pub path: String,
    /// Path relative to the project file, tried when the absolute one no longer exists.
    pub rel: Option<String>,
    /// Size of the base file when saved (a mismatch is reported, not fatal).
    pub len: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub app: String,
    pub name: String,
    pub base: Base,
    pub sets: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SetDoc {
    pub name: String,
    pub layers: Vec<Layer>,
    /// Layers added in the session at the top of the stack.
    pub session_layers: usize,
}

/// Hand-painted overlay of a set: straight-alpha RGBA8, rows top-first.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintOverlay {
    pub set: usize,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ProjectFile {
    pub manifest: Manifest,
    pub sets: Vec<SetDoc>,
    pub overlays: Vec<PaintOverlay>,
    /// Session state owned by the app (camera, brush...), opaque here.
    pub view: serde_json::Value,
}

impl Base {
    /// Describe `path` as the base of a project stored at `project`.
    pub fn of(kind: &str, path: &Path, project: &Path) -> Base {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let rel = project
            .parent()
            .and_then(|d| std::fs::canonicalize(d).ok())
            .and_then(|d| rel_path(&abs, &d))
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        Base {
            kind: kind.to_string(),
            path: strip_verbatim(&abs).to_string_lossy().into_owned(),
            rel,
            len: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        }
    }

    /// Where the base is now: the stored absolute path, else the relative one next to `project`.
    pub fn resolve(&self, project: &Path) -> Option<PathBuf> {
        let abs = PathBuf::from(&self.path);
        if abs.exists() {
            return Some(abs);
        }
        let rel = self.rel.as_ref()?;
        let p = project.parent()?.join(rel);
        p.exists().then_some(p)
    }
}

/// `\\?\C:\x` (what canonicalize returns on Windows) as `C:\x`.
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// `target` relative to the directory `from` (both absolute and canonical), when they share a root.
fn rel_path(target: &Path, from: &Path) -> Option<PathBuf> {
    let t: Vec<_> = target.components().collect();
    let f: Vec<_> = from.components().collect();
    let common = t.iter().zip(&f).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return None;
    }
    let mut out = PathBuf::new();
    for _ in common..f.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    Some(out)
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba {
        return Err("paint overlay is not RGBA".into());
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

pub fn save(path: &Path, p: &ProjectFile) -> Result<(), String> {
    let tmp = path.with_extension("shard.tmp");
    {
        let f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut put = |name: &str, data: &[u8]| -> Result<(), String> {
            z.start_file(name, opt).map_err(|e| e.to_string())?;
            z.write_all(data).map_err(|e| e.to_string())
        };
        put("manifest.json", &serde_json::to_vec_pretty(&p.manifest).map_err(|e| e.to_string())?)?;
        put("document.json", &serde_json::to_vec(&p.sets).map_err(|e| e.to_string())?)?;
        put("view.json", &serde_json::to_vec_pretty(&p.view).map_err(|e| e.to_string())?)?;
        for o in &p.overlays {
            put(&format!("paint/{}.png", o.set), &encode_png(o.w, o.h, &o.rgba)?)?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<ProjectFile, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| format!("not a ShardStroke project: {e}"))?;
    let mut read = |name: &str| -> Result<Vec<u8>, String> {
        let mut e = z.by_name(name).map_err(|e| format!("{name}: {e}"))?;
        let mut v = Vec::new();
        e.read_to_end(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    };
    let manifest: Manifest = serde_json::from_slice(&read("manifest.json")?).map_err(|e| format!("manifest: {e}"))?;
    if manifest.format != FORMAT {
        return Err(format!("unknown project format '{}'", manifest.format));
    }
    if manifest.version > VERSION {
        return Err(format!("project saved by a newer ShardStroke (format version {})", manifest.version));
    }
    let sets: Vec<SetDoc> = serde_json::from_slice(&read("document.json")?).map_err(|e| format!("document: {e}"))?;
    let view = read("view.json").ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(serde_json::Value::Null);
    let mut overlays = Vec::new();
    for set in 0..sets.len() {
        if let Ok(b) = read(&format!("paint/{set}.png")) {
            let (w, h, rgba) = decode_png(&b)?;
            overlays.push(PaintOverlay { set, w, h, rgba });
        }
    }
    // ids handed out from now on must not collide with the ones stored in the file
    let mut max = 0u64;
    fn walk(ls: &[Layer], max: &mut u64) {
        fn eff(es: &[shard_model::Effect], max: &mut u64) {
            for e in es {
                *max = (*max).max(e.uid);
                if let shard_model::EffectKind::Group { children } = &e.kind {
                    eff(children, max);
                }
            }
        }
        for l in ls {
            *max = (*max).max(l.uid);
            eff(&l.content, max);
            if let Some(m) = &l.mask {
                eff(&m.effects, max);
            }
            walk(&l.children, max);
        }
    }
    for s in &sets {
        walk(&s.layers, &mut max);
    }
    shard_model::reserve_uids(max + 1);
    Ok(ProjectFile { manifest, sets, overlays, view })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shard_model::*;

    fn sample_layer(name: &str) -> Layer {
        let stroke = Stroke {
            keys: (0..8).map(|i| StrokeKey { pos: [i as f32 / 8.0, 0.25], time: i as f32 * 0.01, kind: 0 }).collect(),
            view: [1.5; 16],
            projection: [0.5; 16],
            seed: 77,
            view_type: 1,
            lazy_distance: 0.0,
            pressure: vec![0.1, 0.9],
            duplicate_count: 0,
            duplicate_matrix: None,
        };
        let mut tweaks = std::collections::BTreeMap::new();
        tweaks.insert("$randomseed".to_string(), Tweak::Int(1234));
        tweaks.insert("balance".to_string(), Tweak::Float(0.7));
        tweaks.insert("color".to_string(), Tweak::Float3([0.1, 0.2, 0.3]));
        Layer {
            uid: next_uid(),
            kind: LayerKind::Layer,
            name: name.into(),
            visible: false,
            blend: vec![ChannelBlend { mode: BlendMode::MULTIPLY, channels: ChannelSet(u128::MAX), opacity: 0.5 }],
            content: vec![
                Effect {
                    uid: 7,
                    visible: true,
                    channels: ChannelSet((1 << 0) | (1 << 100)),
                    label: Some("Fill".into()),
                    blend: vec![],
                    kind: EffectKind::Fill(Fill {
                        projection: Projection::Triplanar,
                        filtering: 2,
                        uv: UvTransform { scale: Some([2.0, 3.0]), rotation: Some(45.0), wrap: 3, ..Default::default() },
                        sources: vec![
                            Source::Uniform { channels: ChannelSet(1), color: Some([0.5, 0.25, 0.125]), opacity: 1.0 },
                            Source::Procedural {
                                channels: ChannelSet(2),
                                procedural: Procedural {
                                    resource: Some("file:C:/x/y.sbsar".into()),
                                    tweaks,
                                    resolution: ResolutionOverride { mode: 3, log2_offset: -2, manual: [256, 512] },
                                    ..Default::default()
                                },
                                opacity: 0.75,
                            },
                            Source::Reference { channels: ChannelSet(1), opacity: 1.0, target: 99, levels: None },
                        ],
                        projector: None,
                    }),
                },
                Effect {
                    uid: 8,
                    visible: true,
                    channels: ChannelSet(1),
                    label: None,
                    blend: vec![],
                    kind: EffectKind::Paint(PaintAction { strokes: vec![stroke; 3].into(), ..Default::default() }),
                },
            ],
            mask: Some(Mask {
                explicit: true,
                enabled: true,
                initial: 0,
                effects: vec![Effect {
                    uid: 9,
                    visible: true,
                    channels: ChannelSet(1),
                    label: None,
                    blend: vec![],
                    kind: EffectKind::Decal(
                        vec![DecalHit {
                            uid: 5,
                            position: [0.2, 0.8],
                            rect: None,
                            proj_view_inv: [0.25; 16],
                            color: [1.0, 0.0, 0.5],
                            granularity: 1,
                            paint_through: 0,
                            view_type: 2,
                        }]
                        .into(),
                    ),
                }],
            }),
            children: vec![],
        }
    }

    #[test]
    fn project_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("shard_persist_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.shard");
        let mut group = sample_layer("Group");
        group.kind = LayerKind::Group;
        group.children.push(sample_layer("child"));
        let sets = vec![
            SetDoc { name: "A".into(), layers: vec![sample_layer("one"), group], session_layers: 1 },
            SetDoc { name: "B".into(), layers: vec![], session_layers: 0 },
        ];
        let mut rgba = vec![0u8; 8 * 4 * 4];
        rgba[0..4].copy_from_slice(&[10, 20, 30, 255]);
        let pf = ProjectFile {
            manifest: Manifest {
                format: FORMAT.into(),
                version: VERSION,
                app: "test".into(),
                name: "t".into(),
                base: Base::of("none", &dir.join("missing.spp"), &path),
                sets: vec!["A".into(), "B".into()],
            },
            sets: sets.clone(),
            overlays: vec![PaintOverlay { set: 0, w: 8, h: 4, rgba: rgba.clone() }],
            view: serde_json::json!({"camera": [0.6, 0.35, 4.4]}),
        };
        save(&path, &pf).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.manifest, pf.manifest);
        assert_eq!(back.sets, sets);
        assert_eq!(back.overlays, pf.overlays);
        assert_eq!(back.view["camera"][1], 0.35);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn newer_format_is_refused() {
        let dir = std::env::temp_dir().join(format!("shard_persist_v_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("n.shard");
        let pf = ProjectFile {
            manifest: Manifest {
                format: FORMAT.into(),
                version: VERSION + 1,
                app: "x".into(),
                name: "n".into(),
                base: Base::default(),
                sets: vec![],
            },
            sets: vec![],
            overlays: vec![],
            view: serde_json::Value::Null,
        };
        save(&path, &pf).unwrap();
        assert!(load(&path).unwrap_err().contains("newer"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
