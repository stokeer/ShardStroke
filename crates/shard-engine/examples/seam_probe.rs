//! Island-border health of a texture set: covered texels (the app's padding source) whose composed channel or
//! baked map is black, split into border ring / interior.
//!
//! usage: seam_probe PROJECT.spp SET_INDEX [RES]

use shard_engine::project::Project;
use shard_engine::raster::{coverage, erode};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p = Project::open(std::path::Path::new(&a[0])).expect("open");
    let set: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let res: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(2048);
    let cov = coverage(&p.mesh, set, res, res);
    let inner = erode(&cov, res, res);
    let report = |label: &str, img: &shard_compose::Image| {
        if img.w != res || img.h != res {
            println!("{label}: {}x{} (not {res})", img.w, img.h);
            return;
        }
        let (mut ring, mut ring_black, mut core, mut core_black) = (0usize, 0usize, 0usize, 0usize);
        for i in 0..(res * res) as usize {
            if !cov[i] {
                continue;
            }
            let v = &img.data[i * img.n..i * img.n + img.n.min(3)];
            let black = v.iter().all(|c| *c < 0.02);
            if inner[i] {
                core += 1;
                core_black += black as usize;
            } else {
                ring += 1;
                ring_black += black as usize;
            }
        }
        println!(
            "{label:<12} border ring {ring:>8} texels, black {ring_black:>7} ({:.1}%) | interior {core:>8}, black {core_black:>7} ({:.2}%)",
            100.0 * ring_black as f64 / ring.max(1) as f64,
            100.0 * core_black as f64 / core.max(1) as f64
        );
    };
    println!("set {set} '{}' at {res}", p.sets[set].name);
    // SEAM_PREFIX=1: base colour of the stack cut after each top-level layer (index order of the file)
    if std::env::var_os("SEAM_PREFIX").is_some() {
        let layers = &p.sets[set].layers;
        for k in 1..=layers.len() {
            let c = p.compose_layers(set, &layers[..k], &[0], res);
            if let Some(img) = c.channels.get(&0) {
                report(&format!("..{k} {}", layers[k - 1].name), img);
            }
        }
        for k in 0..layers.len() {
            let c = p.compose_layers(set, &layers[k..], &[0], res);
            if let Some(img) = c.channels.get(&0) {
                report(&format!("{k}.. {}", layers[k].name), img);
            }
        }
        return;
    }
    let c = p.compose(set, &[0, 7, 13], res);
    for (bit, name) in [(0u32, "base"), (7, "roughness"), (13, "metallic")] {
        if let Some(img) = c.channels.get(&bit) {
            report(name, img);
        }
    }
    for (map, name) in [(shard_compose::MeshMap::AmbientOcclusion, "bake AO"), (shard_compose::MeshMap::NormalBase, "bake normal")] {
        if let Some(img) = p.mesh_map(set, map, res) {
            report(name, &img);
        }
    }
}
