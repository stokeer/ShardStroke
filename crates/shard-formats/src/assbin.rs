//! Reader for Painter's `paint/scene.assbin` (an Assimp binary-dump variant).
//!
//! Only what is needed to recover meshes is decoded. The layout was derived from data files
//! (see `tools/assbin.py` and `docs/FORMATS.md`):
//!
//! ```text
//! file   : 512-byte header ("ASSIMP.binary-dump.<date>\n", versions, 256 filename, options)
//! scene  : node tree ... then, per mesh:
//! mesh   : u32 chunk_len, u32 0, u32 name_len, name, u32 primitive_types (4 = triangles),
//!          u32 ?, u32 flags, u32 num_vertices,
//!          num_vertices x vec3 positions,
//!          then, each preceded by a u32 count (== num_vertices):
//!            [flag 0x02] normals, [flag 0x04] tangents and bitangents,
//!          [flag 0x100] u32 numComponents, u32 count, then num_vertices x vec3 texcoords (u, v, 0),
//!          u32 num_faces, u32 1, num_faces x { u32 3, i0, i1, i2, u32 flag }
//! ```

/// Meshes plus the scene's material names (index = `SceneMesh::material`).
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub meshes: Vec<SceneMesh>,
    pub materials: Vec<String>,
}

/// Parse meshes and material names.
pub fn parse_scene(file: &[u8]) -> Scene {
    let b: &[u8] = if file.starts_with(b"ASSIMP.binary-dump") && file.len() > 0x200 { &file[0x200..] } else { file };
    Scene { meshes: parse_meshes(file), materials: material_names(b) }
}

/// Material names in file order: each material stores a `mat.name` property
/// (`u32 size, u32 len, name, 0`).
fn material_names(b: &[u8]) -> Vec<String> {
    let key = b"mat.name";
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + key.len() < b.len() {
        if &b[i..i + key.len()] != key {
            i += 1;
            continue;
        }
        let start = i + key.len();
        let mut found = None;
        let mut q = start;
        while q + 8 < b.len() && q < start + 48 {
            if let (Some(size), Some(len)) = (u32_at(b, q), u32_at(b, q + 4)) {
                let len = len as usize;
                if len > 0 && len < 128 && size as usize == len + 5 {
                    if let Some(s) = b.get(q + 8..q + 8 + len) {
                        if s.iter().all(|c| (32..127).contains(c)) {
                            found = Some(String::from_utf8_lossy(s).into_owned());
                            break;
                        }
                    }
                }
            }
            q += 1;
        }
        out.push(found.unwrap_or_else(|| format!("material_{}", out.len())));
        i = start;
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct SceneMesh {
    pub name: String,
    /// Index into [`Scene::materials`] (texture set).
    pub material: u32,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// Tangents / bitangents stored by Painter (the frame the normal bakes were made in).
    pub tangents: Option<Vec<[f32; 3]>>,
    pub bitangents: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Triangle list (3 indices per face).
    pub indices: Vec<u32>,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn vec3s(b: &[u8], o: usize, n: usize) -> Option<Vec<[f32; 3]>> {
    let end = o.checked_add(n.checked_mul(12)?)?;
    if end > b.len() {
        return None;
    }
    Some((0..n).map(|i| [f32_at(b, o + 12 * i), f32_at(b, o + 12 * i + 4), f32_at(b, o + 12 * i + 8)]).collect())
}

fn unit_count(a: &[[f32; 3]]) -> usize {
    a.iter()
        .filter(|v| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            (l - 1.0).abs() < 0.05
        })
        .count()
}

/// Parse all triangle meshes from an assbin blob.
pub fn parse_meshes(file: &[u8]) -> Vec<SceneMesh> {
    let b: &[u8] = if file.starts_with(b"ASSIMP.binary-dump") && file.len() > 0x200 { &file[0x200..] } else { file };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 16 < b.len() {
        match try_mesh(b, i) {
            Some((m, end)) => {
                out.push(m);
                i = end;
            }
            // records are not 4-byte aligned (odd-length names), so scan byte by byte
            None => i += 1,
        }
    }
    out
}

/// Diagnostic: every record that looks like a mesh header (name, triangles, positions) and whether it parsed:
/// `(offset, name, flags, vertices, parsed)`.
pub fn mesh_candidates(file: &[u8]) -> Vec<(usize, String, u32, usize, bool)> {
    let b: &[u8] = if file.starts_with(b"ASSIMP.binary-dump") && file.len() > 0x200 { &file[0x200..] } else { file };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 16 < b.len() {
        if let Some((m, end)) = try_mesh(b, i) {
            out.push((i, m.name, 0, m.positions.len(), true));
            i = end;
            continue;
        }
        let ok = (|| {
            if u32_at(b, i + 4)? != 0 {
                return None;
            }
            let nlen = u32_at(b, i + 8)? as usize;
            if nlen == 0 || nlen > 256 {
                return None;
            }
            let raw = b.get(i + 12..i + 12 + nlen)?;
            if !raw.iter().all(|c| (32..127).contains(c)) {
                return None;
            }
            let p = i + 12 + nlen;
            let (prim, flags, nv) = (u32_at(b, p)?, u32_at(b, p + 8)?, u32_at(b, p + 12)? as usize);
            (prim == 4 && nv > 0 && nv < 5_000_000 && flags & 1 != 0).then(|| (String::from_utf8_lossy(raw).into_owned(), flags, nv))
        })();
        if let Some((name, flags, nv)) = ok {
            out.push((i, name, flags, nv, false));
        }
        i += 1;
    }
    out
}

fn try_mesh(b: &[u8], i: usize) -> Option<(SceneMesh, usize)> {
    if u32_at(b, i + 4)? != 0 {
        return None;
    }
    let nlen = u32_at(b, i + 8)? as usize;
    if nlen == 0 || nlen > 256 {
        return None;
    }
    let raw = b.get(i + 12..i + 12 + nlen)?;
    if !raw.iter().all(|c| (32..127).contains(c)) {
        return None;
    }
    let name = String::from_utf8_lossy(raw).into_owned();
    let mut p = i + 12 + nlen;
    let prim = u32_at(b, p)?;
    let material = u32_at(b, p + 4)?;
    let flags = u32_at(b, p + 8)?;
    let nv = u32_at(b, p + 12)? as usize;
    p += 16;
    if prim != 4 || nv == 0 || nv > 5_000_000 || flags & 1 == 0 {
        return None;
    }
    // positions come right after `nv`; each following array is preceded by a u32 count (== nv)
    let positions = vec3s(b, p, nv)?;
    p += 12 * nv;
    let mut normals = None;
    let mut tangents = None;
    let mut bitangents = None;
    let extra = usize::from(flags & 2 != 0) + if flags & 4 != 0 { 2 } else { 0 };
    for k in 0..extra {
        if u32_at(b, p)? as usize != nv {
            return None;
        }
        p += 4;
        let a = vec3s(b, p, nv)?;
        p += 12 * nv;
        // order: normals, tangents, bitangents
        let has_normals = flags & 2 != 0;
        let which = if has_normals { k } else { k + 1 };
        if which == 0 {
            if unit_count(&a) * 2 >= nv {
                normals = Some(a);
            }
        } else if which == 1 {
            if unit_count(&a) * 2 >= nv {
                tangents = Some(a);
            }
        } else if which == 2 && unit_count(&a) * 2 >= nv {
            bitangents = Some(a);
        }
    }
    // vertex colour channels (flag 0x10000 << k): u32 count, then count x rgba f32 -- skipped
    for k in 0..8 {
        if flags & (0x10000 << k) != 0 {
            if u32_at(b, p)? as usize != nv {
                return None;
            }
            p += 4 + 16 * nv;
        }
    }
    // texture coordinate channels (flag 0x100 << k): u32 numComponents, u32 count, then vec3 (u, v, w). Painter
    // paints through the first; the others are read past (62 of a test vehicle's meshes carry a second channel and
    // were dropped by the reader, leaving holes in the model and in the interior's UV layout).
    let mut uvs = None;
    for k in 0..8 {
        if flags & (0x100 << k) == 0 {
            continue;
        }
        if u32_at(b, p + 4)? as usize != nv {
            return None;
        }
        p += 8;
        let a = vec3s(b, p, nv)?;
        p += 12 * nv;
        if uvs.is_none() {
            uvs = Some(a.into_iter().map(|v| [v[0], v[1]]).collect::<Vec<[f32; 2]>>());
        }
    }
    // numFaces + 1 + first face marker (3)
    let mut q = p;
    let limit = (p + 64).min(b.len().saturating_sub(12));
    let mut found = None;
    while q <= limit {
        if let (Some(nf), Some(one), Some(three)) = (u32_at(b, q), u32_at(b, q + 4), u32_at(b, q + 8)) {
            if one == 1 && three == 3 && nf > 0 && nf < 10_000_000 {
                found = Some((q, nf as usize));
                break;
            }
        }
        q += 4;
    }
    let (q, nf) = found?;
    let mut indices = Vec::with_capacity(nf * 3);
    let mut r = q + 8;
    for _ in 0..nf {
        let n = u32_at(b, r)?;
        let (a, c, d) = (u32_at(b, r + 4)?, u32_at(b, r + 8)?, u32_at(b, r + 12)?);
        if n != 3 || a as usize >= nv || c as usize >= nv || d as usize >= nv {
            return None;
        }
        indices.extend_from_slice(&[a, c, d]);
        r += 20;
    }
    Some((SceneMesh { name, material, positions, normals, tangents, bitangents, uvs, indices }, r))
}
