//! Parity with the Python importer (`tools/import_spsm.py --report`) on the starter_assets corpus.
//! Skipped when no Painter install is present.

use std::path::{Path, PathBuf};

use shard_model::import::import_file;
use shard_model::{LayerKind, Stats};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Some(x) = p.extension().and_then(|x| x.to_str()) {
                if matches!(x.to_ascii_lowercase().as_str(), "spsm" | "spmsk" | "sppr") {
                    out.push(p);
                }
            }
        }
    }
}

#[test]
fn corpus_matches_python_report() {
    let root = PathBuf::from(
        std::env::var("PAINTER_RESOURCES")
            .unwrap_or_else(|_| r"C:\Program Files\Adobe\Adobe Substance 3D Painter\resources".into()),
    )
    .join("starter_assets");
    if !root.exists() {
        return;
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert_eq!(files.len(), 460);
    let mut st = Stats::default();
    for f in &files {
        let d = import_file(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        st.add_document(&d);
    }
    // Numbers from `python tools/import_spsm.py --report` (docs/FORMATS.md 7.5e).
    assert_eq!(st.layers, 684);
    assert_eq!(st.groups, 135);
    assert_eq!(st.bitmaps, 18);
    assert_eq!(st.effects.get("fill"), Some(&1004));
    assert_eq!(st.effects.get("group"), Some(&379));
    assert_eq!(st.effects.get("filter"), Some(&191));
    assert_eq!(st.effects.get("levels"), Some(&70));
    assert_eq!(st.effects.get("paint"), Some(&30));
    assert_eq!(st.effects.get("anchor"), Some(&7));
    assert_eq!(st.effects.get("eraser"), Some(&1));
    assert_eq!(st.sources.get("uniform"), Some(&1958));
    assert_eq!(st.sources.get("procedural"), Some(&701));
    assert_eq!(st.sources.get("bitmap"), Some(&18));
    assert_eq!(st.sources.get("reference"), Some(&10));
    assert_eq!(st.procedurals.len(), 188);
    assert_eq!(st.procedurals.get("mg_mask_editor"), Some(&92));
}

#[test]
fn board_project_stack_reads_blend_names() {
    let p = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("board.spp")).unwrap_or_default();
    if !p.exists() {
        return;
    }
    let d = import_file(p).unwrap();
    // First top-level group is "Folder 1" with layers incl. 'Fill layer 1' = Multiply @ BaseColor 0.46.
    let folder = d.layers.iter().find(|l| l.name == "Folder 1").expect("Folder 1");
    assert_eq!(folder.kind, LayerKind::Group);
    let l = folder.children.iter().find(|l| l.name == "Fill layer 1").expect("Fill layer 1");
    assert_eq!(l.blend.len(), 1);
    assert_eq!(l.blend[0].mode.name(), "Multiply");
    assert_eq!(l.blend[0].channels.names(), vec!["BaseColor".to_string()]);
    assert!((l.blend[0].opacity - 0.46).abs() < 0.01);
    let dirt = folder.children.iter().find(|l| l.name == "Dirt").unwrap();
    assert_eq!(dirt.blend[0].mode.name(), "Normal");
    let pass = folder.children.iter().find(|l| l.name == "Fill layer 4").unwrap();
    assert_eq!(pass.blend[0].mode.name(), "Passthrough");
}

fn strokes_of<'a>(es: &'a [shard_model::Effect], out: &mut Vec<&'a shard_model::Stroke>) {
    use shard_model::EffectKind;
    for e in es {
        match &e.kind {
            EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => out.extend(a.strokes.iter()),
            EffectKind::Group { children } => strokes_of(children, out),
            _ => {}
        }
    }
}

fn layer_strokes<'a>(ls: &'a [shard_model::Layer], out: &mut Vec<&'a shard_model::Stroke>) {
    for l in ls {
        strokes_of(&l.content, out);
        if let Some(m) = &l.mask {
            strokes_of(&m.effects, out);
        }
        layer_strokes(&l.children, out);
    }
}

/// Recorded strokes decode with sane cameras: row-vector matrices whose last column is (0,0,0,1)
/// (view) and a perspective projection with w = -z (docs/FORMATS.md 7.5k); orthographic cameras exist too.
#[test]
fn stroke_cameras_are_well_formed() {
    let root = PathBuf::from(
        std::env::var("PAINTER_RESOURCES")
            .unwrap_or_else(|_| r"C:\Program Files\Adobe\Adobe Substance 3D Painter\resources".into()),
    )
    .join("starter_assets");
    if !root.exists() {
        return;
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut strokes = Vec::new();
    let docs: Vec<_> = files.iter().filter_map(|f| import_file(f).ok()).collect();
    for d in &docs {
        layer_strokes(&d.layers, &mut strokes);
    }
    assert!(!strokes.is_empty(), "the starter corpus has painted strokes");
    for s in &strokes {
        assert!(!s.keys.is_empty());
        assert!(s.keys.windows(2).all(|w| w[1].time >= w[0].time), "key times increase");
        if s.view_type == 0 {
            // 2D (UV space) stroke: no camera stored
            assert!(s.view.iter().all(|v| *v == 0.0));
            continue;
        }
        let v = &s.view;
        let ok = v[3].abs() < 1e-5 && v[7].abs() < 1e-5 && v[11].abs() < 1e-5 && (v[15] - 1.0).abs() < 1e-5;
        assert!(ok, "view_type={} view={:?} proj={:?} keys={}", s.view_type, s.view, s.projection, s.keys.len());
        let w = s.projection[11];
        assert!((w + 1.0).abs() < 1e-5 || w.abs() < 1e-5, "perspective (w = -z) or orthographic (w = 1) projection");
        assert!(s.keys.windows(2).all(|w| w[1].time >= w[0].time), "key times increase");
    }
}
