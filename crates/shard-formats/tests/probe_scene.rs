//! Diagnostic: mesh bounding boxes of a project (set `SHARD_PROBE_SPP`). Skipped otherwise.
use shard_formats::Container;

#[test]
fn mesh_boxes() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut c = Container::open(&p).unwrap();
    let scene = c.scene().unwrap();
    println!("meshes={} materials={:?}", scene.meshes.len(), scene.materials);
    let (mut glo, mut ghi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for m in &scene.meshes {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &m.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(v[i]);
                hi[i] = hi[i].max(v[i]);
                glo[i] = glo[i].min(v[i]);
                ghi[i] = ghi[i].max(v[i]);
            }
        }
        println!(
            "{:40} mat={} verts={:6} idx={:6} lo=({:.2},{:.2},{:.2}) hi=({:.2},{:.2},{:.2})",
            m.name, m.material, m.positions.len(), m.indices.len(), lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        );
    }
    println!("global lo={glo:?} hi={ghi:?}");
}

#[test]
fn decode_named_resource() {
    let (Ok(p), Ok(m)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_MEMBER")) else { return };
    let mut c = Container::open(&p).unwrap();
    let bytes = c.read(&m).unwrap();
    match shard_formats::image::decode(&bytes) {
        Ok(i) => println!("ok {}x{} {:?} pitch={} data={}", i.width, i.height, i.format, i.pitch, i.data.len()),
        Err(e) => println!("ERR {e}"),
    }
}


#[test]
fn normal_stats() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut c = Container::open(&p).unwrap();
    let scene = c.scene().unwrap();
    for m in &scene.meshes {
        let Some(n) = &m.normals else {
            println!("{}: no normals", m.name);
            continue;
        };
        // distribution of normals on the front face x ~ 0.10 (screen): positions with pos.x between 0.095 and 0.105
        let mut cnt = 0;
        let mut dev = 0.0f32;
        let mut samples = Vec::new();
        for (i, p) in m.positions.iter().enumerate() {
            if (p[0] - 0.10).abs() < 0.006 {
                cnt += 1;
                dev = dev.max((1.0 - n[i][0]).abs());
                if samples.len() < 8 {
                    samples.push((*p, n[i]));
                }
            }
        }
        println!("{}: {} verts near the screen plane, max |1-nx| = {dev}", m.name, cnt);
        for s in samples {
            println!("   pos {:?} normal {:?}", s.0, s.1);
        }
    }
}


#[test]
fn screen_normals() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut c = Container::open(&p).unwrap();
    let scene = c.scene().unwrap();
    let m = &scene.meshes[0];
    let n = m.normals.as_ref().unwrap();
    // triangles whose three vertices lie on x ~ 0.10 (the screen plane) and |z| < 0.75
    let mut seen = std::collections::BTreeSet::new();
    let mut tris = 0;
    for t in m.indices.chunks_exact(3) {
        let on = t.iter().all(|&i| (m.positions[i as usize][0] - 0.10).abs() < 0.004 && m.positions[i as usize][2].abs() < 0.75);
        if !on {
            continue;
        }
        tris += 1;
        for &i in t {
            let (p, nn) = (m.positions[i as usize], n[i as usize]);
            seen.insert(((p[1] * 100.0).round() as i32, (p[2] * 100.0).round() as i32, (nn[0] * 1000.0).round() as i32, (nn[1] * 1000.0).round() as i32, (nn[2] * 1000.0).round() as i32));
        }
    }
    println!("{tris} screen-plane triangles, {} distinct (y,z,n) vertices", seen.len());
    for (y, z, a, b, c) in seen.iter().take(60) {
        println!("  y={:.2} z={:.2} n=({:.3},{:.3},{:.3})", *y as f32 / 100.0, *z as f32 / 100.0, *a as f32 / 1000.0, *b as f32 / 1000.0, *c as f32 / 1000.0);
    }
}


#[test]
fn tangent_stats() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut c = Container::open(&p).unwrap();
    let scene = c.scene().unwrap();
    for m in &scene.meshes {
        let (Some(n), Some(t), Some(b)) = (&m.normals, &m.tangents, &m.bitangents) else {
            println!("{}: normals={} tangents={} bitangents={}", m.name, m.normals.is_some(), m.tangents.is_some(), m.bitangents.is_some());
            continue;
        };
        let (mut dn, mut db, mut right, mut left) = (0.0f32, 0.0f32, 0, 0);
        for i in 0..n.len() {
            let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            dn = dn.max(dot(n[i], t[i]).abs());
            db = db.max(dot(t[i], b[i]).abs());
            let cr = [n[i][1] * t[i][2] - n[i][2] * t[i][1], n[i][2] * t[i][0] - n[i][0] * t[i][2], n[i][0] * t[i][1] - n[i][1] * t[i][0]];
            if dot(cr, b[i]) >= 0.0 { right += 1 } else { left += 1 }
        }
        println!("{}: verts={} max|n.t|={dn:.3} max|t.b|={db:.3} cross(n,t).b>0: {right}, <0: {left}", m.name, n.len());
        println!("  sample n={:?} t={:?} b={:?}", n[100], t[100], b[100]);
    }
}
