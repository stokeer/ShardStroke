//! SHARD_PROBE_SPP: front-facing (+X) triangles grouped by depth, with their UV rectangle.
use shard_engine::project::Project;
use std::collections::BTreeMap;

#[test]
fn front_faces() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let proj = Project::open(std::path::Path::new(&p)).unwrap();
    let m = &proj.mesh;
    let mut groups: BTreeMap<i32, (usize, f32, [f32; 4], [f32; 4])> = BTreeMap::new();
    for tri in m.indices.chunks_exact(3) {
        let v = [&m.vertices[tri[0] as usize], &m.vertices[tri[1] as usize], &m.vertices[tri[2] as usize]];
        let a = glam::Vec3::from(v[0].pos);
        let (b, c) = (glam::Vec3::from(v[1].pos), glam::Vec3::from(v[2].pos));
        let n = (b - a).cross(c - a);
        let area = n.length() * 0.5;
        let n = n.normalize_or_zero();
        if n.x < 0.95 {
            continue;
        }
        let key = ((a.x + b.x + c.x) / 3.0 * 1000.0).round() as i32;
        let e = groups.entry(key).or_insert((0, 0.0, [f32::MAX, f32::MIN, f32::MAX, f32::MIN], [f32::MAX, f32::MIN, f32::MAX, f32::MIN]));
        e.0 += 1;
        e.1 += area;
        for x in v {
            e.2[0] = e.2[0].min(x.uv[0]);
            e.2[1] = e.2[1].max(x.uv[0]);
            e.2[2] = e.2[2].min(x.uv[1]);
            e.2[3] = e.2[3].max(x.uv[1]);
            e.3[0] = e.3[0].min(x.pos[1]);
            e.3[1] = e.3[1].max(x.pos[1]);
            e.3[2] = e.3[2].min(x.pos[2]);
            e.3[3] = e.3[3].max(x.pos[2]);
        }
    }
    for (k, (n, area, uv, pos)) in groups {
        if area > 0.0005 {
            println!(
                "x={:+.3}  tris {:4}  area {:.4}  uv u[{:.3},{:.3}] v[{:.3},{:.3}]  y[{:.2},{:.2}] z[{:.2},{:.2}]",
                k as f32 / 1000.0, n, area, uv[0], uv[1], uv[2], uv[3], pos[0], pos[1], pos[2], pos[3]
            );
        }
    }
}

/// SHARD_PROBE_SPP: bounding box of the mesh and of each texture set.
#[test]
fn bboxes() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let proj = Project::open(std::path::Path::new(&p)).unwrap();
    let m = &proj.mesh;
    let bb = |sel: &dyn Fn(usize) -> bool| {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            if !sel(t) {
                continue;
            }
            for &i in tri {
                let v = &m.vertices[i as usize];
                for a in 0..3 {
                    lo[a] = lo[a].min(v.pos[a]);
                    hi[a] = hi[a].max(v.pos[a]);
                }
            }
        }
        (lo, hi)
    };
    let (lo, hi) = bb(&|_| true);
    println!("all: lo {lo:.3?} hi {hi:.3?}");
    for (i, name) in m.set_names.iter().enumerate() {
        let (lo, hi) = bb(&|t| m.tri_mat.get(t).copied() == Some(i as u32));
        println!("set {i} {name}: lo {lo:.3?} hi {hi:.3?}");
    }
}
