//! `cargo run --release -p shard-formats --example scene_meshes -- PROJECT.spp`: bounding box of every mesh of the
//! scene of a Painter project, in the scene's own units, sorted by how far they reach on the vertical axis.
//! Used to find which part of the scene Painter centres the normalised view of its camera on.
use shard_formats::Container;

fn main() {
    let path = std::env::args().nth(1).expect("project path");
    let mut c = Container::open(&path).expect("open");
    let scene = c.scene().expect("scene");
    // `--centroids OUT`: f32 triples (mesh, u, v) of every triangle's UV centroid
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--centroids") {
        let mut out: Vec<u8> = Vec::new();
        for (mi, m) in scene.meshes.iter().enumerate() {
            let Some(uvs) = &m.uvs else { continue };
            for t in m.indices.chunks_exact(3) {
                let (mut u, mut v) = (0.0f32, 0.0f32);
                let mut nrm = [0.0f32; 3];
                for &k in t {
                    u += uvs[k as usize][0] / 3.0;
                    v += uvs[k as usize][1] / 3.0;
                    if let Some(ns) = &m.normals {
                        for a in 0..3 {
                            nrm[a] += ns[k as usize][a] / 3.0;
                        }
                    }
                }
                for x in [mi as f32, u, v, nrm[0], nrm[1], nrm[2]] {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        std::fs::write(&args[i + 1], out).expect("write");
        return;
    }
    // `--failed`: mesh-like records the reader could not parse
    if args.iter().any(|a| a == "--failed") {
        let raw = c.read("paint/scene.assbin").expect("assbin");
        let all = shard_formats::assbin::mesh_candidates(&raw);
        let bad: Vec<_> = all.iter().filter(|r| !r.4).collect();
        println!("{} parsed, {} mesh-like records failed", all.len() - bad.len(), bad.len());
        for r in bad.iter().take(40) {
            println!("  @{} '{}' flags {:#x} verts {}", r.0, r.1, r.2, r.3);
        }
        return;
    }
    // `--uv`: one line per mesh: index, material, material name, triangles, UV box
    if std::env::args().any(|a| a == "--uv") {
        for (i, m) in scene.meshes.iter().enumerate() {
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for uv in m.uvs.iter().flatten() {
                for a in 0..2 {
                    lo[a] = lo[a].min(uv[a]);
                    hi[a] = hi[a].max(uv[a]);
                }
            }
            let mname = scene.materials.get(m.material as usize).map(String::as_str).unwrap_or("?");
            println!("{i}	{}	{mname}	{}	{:.4}	{:.4}	{:.4}	{:.4}	{}", m.material, m.indices.len() / 3, lo[0], lo[1], hi[0], hi[1], m.name);
        }
        return;
    }
    let meshes = scene.meshes;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut rows: Vec<(usize, String, u32, usize, [f32; 3], [f32; 3])> = Vec::new();
    for (i, m) in meshes.iter().enumerate() {
        let (mut l, mut h) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &m.positions {
            for a in 0..3 {
                l[a] = l[a].min(p[a]);
                h[a] = h[a].max(p[a]);
            }
        }
        if m.positions.is_empty() {
            continue;
        }
        for a in 0..3 {
            lo[a] = lo[a].min(l[a]);
            hi[a] = hi[a].max(h[a]);
        }
        rows.push((i, m.name.clone(), m.material, m.positions.len(), l, h));
    }
    let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let half = [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5, (hi[2] - lo[2]) * 0.5];
    let r = half[0].max(half[1]).max(half[2]);
    println!("{} meshes; scene AABB ({:.4},{:.4},{:.4})-({:.4},{:.4},{:.4})", rows.len(), lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]);
    println!("centre ({:.4},{:.4},{:.4}) half extents ({:.4},{:.4},{:.4}) r {:.4}", centre[0], centre[1], centre[2], half[0], half[1], half[2], r);
    // normalised y of every mesh, top first
    let ny = |v: f32| (v - centre[1]) / r;
    rows.sort_by(|a, b| b.5[1].partial_cmp(&a.5[1]).unwrap());
    println!("--- highest meshes (normalised y max, min), verts, material, name");
    for (i, name, mat, n, l, h) in rows.iter().take(14) {
        println!("  #{i:<4} y [{:+.4}, {:+.4}]  verts {n:<6} mat {mat} {name}", ny(l[1]), ny(h[1]));
    }
    rows.sort_by(|a, b| a.4[1].partial_cmp(&b.4[1]).unwrap());
    println!("--- lowest meshes");
    for (i, name, mat, n, l, h) in rows.iter().take(14) {
        println!("  #{i:<4} y [{:+.4}, {:+.4}]  verts {n:<6} mat {mat} {name}", ny(l[1]), ny(h[1]));
    }
    rows.sort_by(|a, b| a.4[2].partial_cmp(&b.4[2]).unwrap());
    println!("--- meshes reaching the z minimum / x extremes");
    for (i, name, mat, n, l, h) in rows.iter().take(5) {
        println!("  #{i:<4} z [{:+.4}, {:+.4}] x [{:+.4}, {:+.4}] verts {n:<6} mat {mat} {name}", (l[2] - centre[2]) / r, (h[2] - centre[2]) / r, (l[0] - centre[0]) / r, (h[0] - centre[0]) / r);
    }
}
