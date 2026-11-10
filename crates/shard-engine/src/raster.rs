//! Raster operations on texture-space images: UV coverage of a set, island padding (dilation, erosion,
//! diffusion-free edge extension) and TGA output. Shared by the app and the headless tools.

use std::path::Path;

use crate::mesh::Mesh;

/// Per-texel UV coverage of one set at `w x h`.
/// Value of `per_tri` (indexed by triangle) of the triangle that covers each texel of the set (-1 = none).
pub fn tri_map(mesh: &Mesh, set: usize, w: u32, h: u32, per_tri: &[u32]) -> Vec<i64> {
    let mut out = vec![-1i64; (w * h) as usize];
    for t in 0..mesh.indices.len() / 3 {
        if mesh.tri_mat.get(t).copied() != Some(set as u32) {
            continue;
        }
        let i = &mesh.indices[3 * t..3 * t + 3];
        let uv: Vec<[f32; 2]> = i
            .iter()
            .map(|&k| {
                let v = &mesh.vertices[k as usize];
                [v.uv[0] * w as f32, (1.0 - v.uv[1]) * h as f32]
            })
            .collect();
        let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
        if det.abs() < 1e-9 {
            continue;
        }
        let x0 = uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
        let x1 = uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil().min(w as f32 - 1.0) as u32;
        let y0 = uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
        let y1 = uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil().min(h as f32 - 1.0) as u32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                let l2 = 1.0 - l0 - l1;
                if l0 >= 0.0 && l1 >= 0.0 && l2 >= 0.0 {
                    out[(y * w + x) as usize] = per_tri[t] as i64;
                }
            }
        }
    }
    out
}

pub fn coverage(mesh: &Mesh, set: usize, w: u32, h: u32) -> Vec<bool> {
    let mut cov = vec![false; (w * h) as usize];
    for t in 0..mesh.indices.len() / 3 {
        if mesh.tri_mat.get(t).copied() != Some(set as u32) {
            continue;
        }
        let i = &mesh.indices[3 * t..3 * t + 3];
        let uv: Vec<[f32; 2]> = i
            .iter()
            .map(|&k| {
                let v = &mesh.vertices[k as usize];
                [v.uv[0] * w as f32, (1.0 - v.uv[1]) * h as f32]
            })
            .collect();
        let (x0, x1) = (uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max));
        let (y0, y1) = (uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min), uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max));
        let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
        if det.abs() < 1e-9 {
            continue;
        }
        let (xa, xb) = (x0.floor().max(0.0) as u32, x1.ceil().min(w as f32 - 1.0) as u32);
        let (ya, yb) = (y0.floor().max(0.0) as u32, y1.ceil().min(h as f32 - 1.0) as u32);
        // conservative: triangles smaller than a texel still cover the texels holding their vertices
        for q in &uv {
            let (cx, cy) = (q[0].floor(), q[1].floor());
            if cx >= 0.0 && cy >= 0.0 && cx < w as f32 && cy < h as f32 {
                cov[(cy as u32 * w + cx as u32) as usize] = true;
            }
        }
        for y in ya..=yb {
            for x in xa..=xb {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                let l2 = 1.0 - l0 - l1;
                if l0 >= -0.02 && l1 >= -0.02 && l2 >= -0.02 {
                    cov[(y * w + x) as usize] = true;
                }
            }
        }
    }
    cov
}

/// Object-space position and unit normal of the surface point behind every texel of set `set`, by
/// rasterising its triangles in UV space (barycentric interpolation of the vertex data).
pub fn surface(mesh: &Mesh, set: usize, w: u32, h: u32) -> shard_compose::Surface {
    let n = (w * h) as usize;
    let mut pos = vec![[0.0f32; 3]; n];
    let mut nrm = vec![[0.0f32, 0.0, 1.0]; n];
    let mut cov = vec![false; n];
    // Where surfaces share a texel (mirrored / copied parts on one UV island) the first one in the scene keeps it, as
    // in Painter: a test vehicle's tri-planar `Fill layer 2` against the exports goes from corr 0.29 -> 0.99,
    // 0.23 -> 0.89 and 0.69 -> 0.95 on two other hull sets; only the interior set prefers the last (0.87 vs 0.79).
    // SHARD_SURFACE_LAST=1: the last one keeps it.
    let first_wins = std::env::var_os("SHARD_SURFACE_LAST").is_none();
    for t in 0..mesh.indices.len() / 3 {
        if mesh.tri_mat.get(t).copied() != Some(set as u32) {
            continue;
        }
        let ids = &mesh.indices[3 * t..3 * t + 3];
        let vs = [&mesh.vertices[ids[0] as usize], &mesh.vertices[ids[1] as usize], &mesh.vertices[ids[2] as usize]];
        let uv: Vec<[f32; 2]> = vs.iter().map(|v| [v.uv[0] * w as f32, (1.0 - v.uv[1]) * h as f32]).collect();
        let det = (uv[1][1] - uv[2][1]) * (uv[0][0] - uv[2][0]) + (uv[2][0] - uv[1][0]) * (uv[0][1] - uv[2][1]);
        if det.abs() < 1e-9 {
            continue;
        }
        let x0 = uv.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
        let x1 = uv.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil().min(w as f32 - 1.0) as u32;
        let y0 = uv.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
        let y1 = uv.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil().min(h as f32 - 1.0) as u32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let l0 = ((uv[1][1] - uv[2][1]) * (px - uv[2][0]) + (uv[2][0] - uv[1][0]) * (py - uv[2][1])) / det;
                let l1 = ((uv[2][1] - uv[0][1]) * (px - uv[2][0]) + (uv[0][0] - uv[2][0]) * (py - uv[2][1])) / det;
                let l2 = 1.0 - l0 - l1;
                if l0 >= -0.01 && l1 >= -0.01 && l2 >= -0.01 {
                    let i = (y * w + x) as usize;
                    if first_wins && cov[i] {
                        continue;
                    }
                    let mut p = [0.0f32; 3];
                    let mut q = [0.0f32; 3];
                    for (k, l) in [l0, l1, l2].into_iter().enumerate() {
                        for c in 0..3 {
                            p[c] += l * vs[k].pos[c];
                            q[c] += l * vs[k].normal[c];
                        }
                    }
                    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt().max(1e-9);
                    pos[i] = p;
                    nrm[i] = [q[0] / len, q[1] / len, q[2] / len];
                    cov[i] = true;
                }
            }
        }
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &mesh.vertices {
        for c in 0..3 {
            lo[c] = lo[c].min(v.pos[c]);
            hi[c] = hi[c].max(v.pos[c]);
        }
    }
    shard_compose::Surface { w, h, pos, nrm, covered: cov, lo, hi }
}

/// Grow the covered texels outwards by up to `iters` pixels (4-neighbourhood average) in an interleaved
/// float image of `n` components. Only the moving border is visited, so large paddings stay cheap.
pub fn dilate_f32(px: &mut [f32], n: usize, cov: &[bool], w: u32, h: u32, iters: u32) {
    shard_compose::dilate(px, n, cov, w, h, iters);
}

/// Coverage with the outermost ring of texels removed: baked maps blend island borders with their black
/// background (anti-aliasing), so those texels cannot be trusted as the source of padding.
pub fn erode(cov: &[bool], w: u32, h: u32) -> Vec<bool> {
    let (wi, hi) = (w as usize, h as usize);
    let mut out = cov.to_vec();
    for y in 0..hi {
        for x in 0..wi {
            let i = y * wi + x;
            if !cov[i] {
                continue;
            }
            let edge = x == 0 || y == 0 || x + 1 == wi || y + 1 == hi || !cov[i - 1] || !cov[i + 1] || !cov[i - wi] || !cov[i + wi];
            if edge {
                out[i] = false;
            }
        }
    }
    out
}

/// Edge padding of a float image: grow the covered texels outwards by `iters` pixels (average of the
/// already filled 4-neighbours), so bilinear / mip sampling at UV seams never reads the background.
pub fn pad_f32(data: &mut [f32], n: usize, cov: &[bool], w: u32, h: u32, iters: u32) {
    let (wi, hi) = (w as i32, h as i32);
    let mut filled = cov.to_vec();
    // frontier: uncovered texels touching a filled one
    let mut frontier: Vec<usize> = Vec::new();
    let touches = |filled: &[bool], x: i32, y: i32| -> bool {
        [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
            let (nx, ny) = (x + dx, y + dy);
            nx >= 0 && ny >= 0 && nx < wi && ny < hi && filled[(ny * wi + nx) as usize]
        })
    };
    for y in 0..hi {
        for x in 0..wi {
            let i = (y * wi + x) as usize;
            if !filled[i] && touches(&filled, x, y) {
                frontier.push(i);
            }
        }
    }
    for _ in 0..iters {
        if frontier.is_empty() {
            break;
        }
        let mut next: Vec<usize> = Vec::new();
        let mut done: Vec<(usize, Vec<f32>)> = Vec::with_capacity(frontier.len());
        for &i in &frontier {
            let (x, y) = ((i as i32) % wi, (i as i32) / wi);
            let mut acc = vec![0f32; n];
            let mut cnt = 0f32;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= wi || ny >= hi {
                    continue;
                }
                let j = (ny * wi + nx) as usize;
                if filled[j] {
                    for c in 0..n {
                        acc[c] += data[j * n + c];
                    }
                    cnt += 1.0;
                }
            }
            if cnt > 0.0 {
                acc.iter_mut().for_each(|v| *v /= cnt);
                done.push((i, acc));
            }
        }
        for (i, v) in &done {
            data[i * n..i * n + n].copy_from_slice(v);
            filled[*i] = true;
        }
        for (i, _) in &done {
            let (x, y) = ((*i as i32) % wi, (*i as i32) / wi);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                    let j = (ny * wi + nx) as usize;
                    if !filled[j] {
                        next.push(j);
                    }
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        frontier = next;
    }
}

/// Grow the covered texels outwards by `iters` pixels (4-neighbourhood average), RGBA8 in place.
pub fn dilate(px: &mut [u8], cov: &[bool], w: u32, h: u32, iters: u32) {
    let mut filled = cov.to_vec();
    for _ in 0..iters {
        let mut next = filled.clone();
        let mut updates: Vec<(usize, [u8; 4])> = Vec::new();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let i = (y as u32 * w + x as u32) as usize;
                if filled[i] {
                    continue;
                }
                let (mut acc, mut n) = ([0u32; 4], 0u32);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = (ny as u32 * w + nx as u32) as usize;
                    if filled[j] {
                        for c in 0..4 {
                            acc[c] += px[j * 4 + c] as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    updates.push((i, [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, (acc[3] / n) as u8]));
                    next[i] = true;
                }
            }
        }
        if updates.is_empty() {
            break;
        }
        for (i, v) in updates {
            px[i * 4..i * 4 + 4].copy_from_slice(&v);
        }
        filled = next;
    }
}

/// Write RGBA8 (rows top-first) as a 32-bit bottom-left TGA with the TRUEVISION footer.
pub fn write_tga(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut out = Vec::with_capacity(18 + (w * h * 4) as usize + 26);
    out.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&[32, 8]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(b"TRUEVISION-XFILE.\0");
    let mut f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(&out).map_err(|e| e.to_string())
}
