//! The layer-by-layer comparison reads what the Painter plugin writes. Without Painter at hand, feed it our own
//! export in the plugin's layout and expect no error: this checks the plumbing (file names, json, png, metrics).

use std::path::Path;

use shard_engine::measure;
use shard_engine::project::Project;
use shard_model::{Layer, LayerKind};

fn leaves(ls: &[Layer], out: &mut Vec<String>) {
    for l in ls.iter().rev() {
        if l.kind == LayerKind::Group {
            leaves(&l.children, out);
        } else {
            out.push(l.name.clone());
        }
    }
}

fn write_png(path: &Path, w: u32, h: u32, color: png::ColorType, data: &[u8]) {
    let f = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(data).unwrap();
}

#[test]
fn ladder_of_our_own_export_has_no_error() {
    let spp = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("screen.spp")).unwrap_or_default();
    if !spp.exists() {
        return; // corpus file not present on this machine
    }
    let proj = Project::open(spp).unwrap();
    let (set, res) = (0usize, 128u32);
    let mut names = Vec::new();
    leaves(&proj.sets[set].layers, &mut names);
    assert!(!names.is_empty());
    let last = names.len() - 1;

    let root = std::env::temp_dir().join(format!("shard_ladder_{}", std::process::id()));
    let tag = measure::slug(&proj.sets[set].name);
    let dir = root.join("ladder").join(&tag).join(format!("{last:03}_1_{}", measure::slug(&names[last])));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(root.join("layers")).unwrap();
    let ladder: Vec<_> =
        names.iter().enumerate().map(|(i, n)| serde_json::json!({"step": i, "uid": i, "name": n, "has_mask": false, "parents": []})).collect();
    std::fs::write(root.join("layers").join(format!("{tag}.json")), serde_json::json!({"ladder": ladder}).to_string()).unwrap();

    // what the plugin would have exported for the last step: the whole stack
    let comp = proj.compose(set, &[0, 7, 13], res);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let base = &comp.channels[&0];
    let rgb: Vec<u8> = base.data.chunks_exact(base.n).flat_map(|p| [0, 1, 2].map(|c| q(shard_compose::linear_to_srgb(p[c.min(base.n - 1)])))).collect();
    write_png(&dir.join("BaseColor.png"), res, res, png::ColorType::Rgb, &rgb);
    for (bit, file) in [(7u32, "Roughness.png"), (13, "Metallic.png")] {
        let img = &comp.channels[&bit];
        let l: Vec<u8> = img.data.chunks_exact(img.n).map(|p| q(p[0])).collect();
        write_png(&dir.join(file), res, res, png::ColorType::Grayscale, &l);
    }

    let rows = measure::measure_ladder(&proj, set, &root, res, res).expect("ladder");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.step, last);
    assert!(r.name_matches, "the leaf order must match the plugin's");
    assert!(r.base.unwrap() < 1.0 && r.rough.unwrap() < 1.0 && r.metal.unwrap() < 1.0, "{r:?}");
    std::fs::remove_dir_all(&root).ok();
}
