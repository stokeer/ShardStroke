//! Corpus tests against a local Painter install (skipped when the path does not exist).
//! Override the location with `PAINTER_RESOURCES`.

use std::path::{Path, PathBuf};

use shard_formats::{alg, Container};

fn resources() -> Option<PathBuf> {
    let p = std::env::var("PAINTER_RESOURCES")
        .unwrap_or_else(|_| r"C:\Program Files\Adobe\Adobe Substance 3D Painter\resources".into());
    let p = PathBuf::from(p);
    p.exists().then_some(p)
}

fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, exts, out);
            } else if let Some(x) = p.extension().and_then(|x| x.to_str()) {
                if exts.contains(&x.to_ascii_lowercase().as_str()) {
                    out.push(p);
                }
            }
        }
    }
}

#[test]
fn every_container_member_decodes() {
    let Some(root) = resources() else { return };
    let mut files = Vec::new();
    walk(&root.join("starter_assets"), &["spsm", "spmsk", "sppr", "spt"], &mut files);
    assert!(!files.is_empty());
    let (mut ok, mut bad) = (0usize, Vec::new());
    for f in &files {
        let mut c = Container::open(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let names: Vec<(String, u64)> = c.members().map(|d| (d.path.clone(), d.byte_len())).collect();
        for (n, len) in names {
            if len > 64_000_000 {
                continue;
            }
            let b = c.read(&n).unwrap();
            if !alg::is_alg(&b) {
                continue;
            }
            match alg::decode(&b) {
                Ok(_) => ok += 1,
                Err(e) => bad.push(format!("{}:{n}: {e}", f.display())),
            }
        }
    }
    eprintln!("files={} alg members ok={ok} bad={}", files.len(), bad.len());
    assert!(bad.is_empty(), "{} failures, first: {:?}", bad.len(), bad.first());
}

#[test]
fn export_presets_decode_and_lumberyard_matches_python() {
    let Some(root) = resources() else { return };
    let dir = root.join("starter_assets").join("export-presets");
    let mut files = Vec::new();
    walk(&dir, &["spexp"], &mut files);
    // `.alg_meta/` holds per-file metadata (not Alg-encoded presets).
    files.retain(|p| !p.components().any(|c| c.as_os_str() == ".alg_meta"));
    assert!(files.len() >= 33, "expected at least 33 export presets, found {}", files.len());
    for f in &files {
        let b = std::fs::read(f).unwrap();
        alg::decode(&b).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
    }
    let b = std::fs::read(dir.join("Amazon Lumberyard.spexp")).unwrap();
    let v = alg::decode(&b).unwrap();
    let preset = v.as_object().unwrap();
    assert_eq!(preset.class, "DataExportPreset");
    let maps = preset.get("maps").unwrap().as_list().unwrap();
    assert_eq!(maps.len(), 3);
    let m0 = maps[0].as_object().unwrap();
    assert_eq!(m0.get("path").unwrap().as_str().unwrap(), "$mesh_$textureSet_diff(_$colorSpace)(.$udim)");
    let ch = m0.get("channels").unwrap().as_list().unwrap();
    assert_eq!(ch.len(), 4);
    let c0 = ch[0].as_object().unwrap();
    assert_eq!(c0.get("mapId"), Some(&alg::Value::QUint(24)));
    assert_eq!(c0.get("channelSrc").and_then(|v| v.as_i64()), Some(0));
}

#[test]
fn user_project_document_decodes_if_present() {
    let p = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("board.spp")).unwrap_or_default();
    if !p.exists() {
        return;
    }
    let mut c = Container::open(p).unwrap();
    let v = c.decode("paint/document.bin").unwrap();
    let doc = v.as_object().unwrap();
    assert_eq!(doc.class, "DataDocument");
    let mats = doc.get("materials").unwrap().as_list().unwrap();
    assert!(!mats.is_empty());
}
