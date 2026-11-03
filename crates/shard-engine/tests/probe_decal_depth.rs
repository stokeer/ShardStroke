//! Which chunks of a texture set does a click ray cross at any depth (behind other geometry)?
//! SHARD_FIT_SPP, SHARD_FIT_SET, SHARD_FIT_LAYER, SHARD_FIT_MASK: compares with the chunks white in a mask.
use glam::{Mat4, Vec3, Vec4};
use shard_engine::{raster, strokes};
use shard_model::{DecalHit, Effect, EffectKind, Layer};
use std::collections::{BTreeMap, HashSet};

fn find_layer<'a>(ls: &'a [Layer], name: &str) -> Option<&'a Layer> {
    for l in ls {
        if l.name == name {
            return Some(l);
        }
        if let Some(r) = find_layer(&l.children, name) {
            return Some(r);
        }
    }
    None
}

fn collect(es: &[Effect], out: &mut Vec<DecalHit>) {
    for e in es {
        match &e.kind {
            EffectKind::Decal(h) => out.extend(h.iter().cloned()),
            EffectKind::Group { children } => collect(children, out),
            _ => {}
        }
    }
}

#[test]
fn depth() {
    let (Ok(spp), Ok(set), Ok(layer), Ok(maskp)) = (
        std::env::var("SHARD_FIT_SPP"),
        std::env::var("SHARD_FIT_SET"),
        std::env::var("SHARD_FIT_LAYER"),
        std::env::var("SHARD_FIT_MASK"),
    ) else {
        return;
    };
    let proj = shard_engine::project::Project::open(std::path::Path::new(&spp)).unwrap();
    let si = proj.sets.iter().position(|s| s.name == set).expect("set");
    let l = find_layer(&proj.sets[si].layers, &layer).expect("layer");
    let mut hits = Vec::new();
    collect(&l.mask.as_ref().unwrap().effects, &mut hits);
    let mesh = &proj.mesh;
    let chunk = strokes::uv_chunks(mesh);

    // chunks of the set that are white in the exported mask
    let res = 1024usize;
    let f = std::fs::File::open(&maskp).unwrap();
    let mut r = png::Decoder::new(std::io::BufReader::new(f)).read_info().unwrap();
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    // chunk id per texel (first triangle covering it)
    let mut ids = vec![u32::MAX; res * res];
    for t in 0..mesh.tri_mat.len() {
        if mesh.tri_mat[t] != si as u32 {
            continue;
        }
        let i = &mesh.indices[3 * t..3 * t + 3];
        let uv: Vec<[f32; 2]> = i.iter().map(|k| {
            let v = &mesh.vertices[*k as usize];
            [v.uv[0] * res as f32, (1.0 - v.uv[1]) * res as f32]
        }).collect();
        let (x0, x1) = (uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max));
        let (y0, y1) = (uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max));
        let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
        if det.abs() < 1e-9 {
            continue;
        }
        for y in (y0.floor().max(0.0) as usize)..=(y1.ceil().min(res as f32 - 1.0) as usize) {
            for x in (x0.floor().max(0.0) as usize)..=(x1.ceil().min(res as f32 - 1.0) as usize) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                if l0 >= -0.02 && l1 >= -0.02 && 1.0 - l0 - l1 >= -0.02 {
                    ids[y * res + x] = chunk[t];
                }
            }
        }
    }
    let mut sum: BTreeMap<u32, (f64, u32)> = BTreeMap::new();
    for y in 0..res {
        for x in 0..res {
            let c = ids[y * res + x];
            if c == u32::MAX {
                continue;
            }
            let v = buf[((y * h / res) * w + x * w / res) * ch] as f64;
            let e = sum.entry(c).or_default();
            e.0 += v;
            e.1 += 1;
        }
    }
    let white: HashSet<u32> = sum.iter().filter(|(_, (s, n))| s / *n as f64 > 128.0 && *n >= 20).map(|(c, _)| *c).collect();
    println!("{} chunks in the set, {} white in the mask", sum.len(), white.len());

    // every crossing of every click ray with this set's triangles
    let mut crossed_white: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (k, hit) in hits.iter().enumerate() {
        let inv = Mat4::from_cols_array(&hit.proj_view_inv);
        let (x, y) = (2.0 * hit.position[0] - 1.0, 2.0 * hit.position[1] - 1.0);
        let n = inv * Vec4::new(x, y, -1.0, 1.0);
        let f = inv * Vec4::new(x, y, 1.0, 1.0);
        let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
        let dir = (f - n).normalize_or_zero();
        if dir == Vec3::ZERO {
            continue;
        }
        let mut o = n;
        let mut depth = 0;
        let mut order: Vec<(u32, u32)> = Vec::new();
        for _ in 0..40 {
            let Some(r) = mesh.raycast(o, dir) else { break };
            let set_of = mesh.tri_mat[r.tri as usize];
            order.push((set_of, chunk[r.tri as usize]));
            o = r.point + dir * 1e-4;
            depth += 1;
        }
        let _ = depth;
        for (d, (s, c)) in order.iter().enumerate() {
            if *s == si as u32 && white.contains(c) {
                crossed_white
                    .entry(*c)
                    .or_default()
                    .push(format!("hit#{k} {} {} depth {d}/{}", if hit.color[0] > 0.5 { "W" } else { "B" }, if (hit.position[0] - 0.5).abs() < 1e-3 && (hit.position[1] - 0.5).abs() < 1e-3 { "centre" } else { "off" }, order.len()));
            }
        }
    }
    // hypothesis: a click selects the chunk with the same rank (in the order of the triangle ids) in every set
    {
        let nsets = mesh.set_names.len();
        let mut chunks_of: Vec<Vec<u32>> = vec![Vec::new(); nsets];
        let mut seen = HashSet::new();
        for t in 0..mesh.tri_mat.len() {
            let c = chunk[t];
            if seen.insert(c) {
                chunks_of[mesh.tri_mat[t] as usize].push(c);
            }
        }
        for v in chunks_of.iter_mut() {
            v.sort();
        }
        println!("chunks per set: {:?}", chunks_of.iter().map(|v| v.len()).collect::<Vec<_>>());
        let mut tested = HashSet::new();
        let (mut good, mut total) = (0, 0);
        let mut by_rank_hit = 0;
        for hit in hits.iter().filter(|h| h.color[0] > 0.5) {
            let inv = Mat4::from_cols_array(&hit.proj_view_inv);
            let (x, y) = (2.0 * hit.position[0] - 1.0, 2.0 * hit.position[1] - 1.0);
            let n = inv * Vec4::new(x, y, -1.0, 1.0);
            let f = inv * Vec4::new(x, y, 1.0, 1.0);
            let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
            let dir = (f - n).normalize_or_zero();
            let Some(r) = mesh.raycast(n, dir) else { continue };
            let s = mesh.tri_mat[r.tri as usize] as usize;
            let c = chunk[r.tri as usize];
            let Some(rank) = chunks_of[s].iter().position(|x| *x == c) else { continue };
            if !tested.insert((s, rank)) {
                continue;
            }
            // the same rank in the set under study (scaled when the sets have different sizes)
            let target = &chunks_of[si];
            let cand = target[rank.min(target.len() - 1)];
            total += 1;
            if white.contains(&cand) {
                good += 1;
            }
            if white.contains(&cand) && s == si {
                by_rank_hit += 1;
            }
        }
        println!("same-rank rule: {good} of {total} distinct (set, rank) clicks land on a white chunk ({} white of {} chunks); own-set {by_rank_hit}", white.len(), chunks_of[si].len());
    }
    println!("white chunks crossed by some ray: {} of {}", crossed_white.len(), white.len());
    for (c, v) in crossed_white.iter().take(30) {
        println!("  chunk {c}: {}", v.iter().take(4).cloned().collect::<Vec<_>>().join("; "));
    }
    let _ = raster::coverage;
}
