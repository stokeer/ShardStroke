//! Measurement of the engine's output against Painter's own exports (docs/VALIDATION.md).
//!
//! The reference is what Painter wrote for the same project: either the "Substance" PNG set
//! (`<prefix><set>_BaseColor.png`, `_OcclusionRoughnessMetallic.png`, `_Normal.png`, `_Emissive.png`) or the
//! Arma Reforger BCR / NMO TGA pair. Every channel is compared over the texels covered by the UV islands, at
//! the resolution we composed (the reference is box-filtered down). The numbers are on a 0..255 scale so
//! they read like image differences: MAE 6.7 means 6.7 levels of 255 on average.

use std::path::{Path, PathBuf};

use crate::project::Project;
use crate::raster;

/// A reference image, rows top-first, RGBA8.
pub struct Truth {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

pub fn read_png(path: &Path) -> Result<Truth, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let ch = info.color_type.samples();
    let rgba = buf[..info.buffer_size()]
        .chunks_exact(ch)
        .flat_map(|p| match ch {
            1 => [p[0], p[0], p[0], 255],
            2 => [p[0], p[0], p[0], p[1]],
            3 => [p[0], p[1], p[2], 255],
            _ => [p[0], p[1], p[2], p[3]],
        })
        .collect();
    Ok(Truth { w: info.width, h: info.height, rgba })
}

/// Uncompressed 24/32-bit TGA (what Painter writes), either origin.
pub fn read_tga(path: &Path) -> Result<Truth, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() < 18 {
        return Err("tga: too short".into());
    }
    let (id_len, ty) = (b[0] as usize, b[2]);
    if ty != 2 {
        return Err(format!("tga: unsupported image type {ty} (only uncompressed true-colour)"));
    }
    let (w, h) = (u16::from_le_bytes([b[12], b[13]]) as u32, u16::from_le_bytes([b[14], b[15]]) as u32);
    let depth = b[16] as usize / 8;
    if depth != 3 && depth != 4 {
        return Err(format!("tga: unsupported depth {}", b[16]));
    }
    let top_first = b[17] & 0x20 != 0;
    let start = 18 + id_len;
    let need = (w * h) as usize * depth;
    if b.len() < start + need {
        return Err("tga: truncated".into());
    }
    let mut rgba = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        let src_y = if top_first { y } else { h - 1 - y };
        for x in 0..w {
            let s = start + ((src_y * w + x) as usize) * depth;
            let d = ((y * w + x) * 4) as usize;
            rgba[d] = b[s + 2];
            rgba[d + 1] = b[s + 1];
            rgba[d + 2] = b[s];
            if depth == 4 {
                rgba[d + 3] = b[s + 3];
            }
        }
    }
    Ok(Truth { w, h, rgba })
}

fn read_any(path: &Path) -> Result<Truth, String> {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("tga") => read_tga(path),
        _ => read_png(path),
    }
}

/// One component of `t` resampled to `res` x `res` (box filter when larger, nearest otherwise), 0..255.
fn component(t: &Truth, c: usize, res: u32) -> Vec<f32> {
    let (w, h, r) = (t.w as usize, t.h as usize, res as usize);
    let mut out = vec![0.0f32; r * r];
    for y in 0..r {
        let (y0, y1) = (y * h / r, ((y + 1) * h / r).max(y * h / r + 1).min(h));
        for x in 0..r {
            let (x0, x1) = (x * w / r, ((x + 1) * w / r).max(x * w / r + 1).min(w));
            let mut acc = 0u32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    acc += t.rgba[(yy * w + xx) * 4 + c] as u32;
                }
            }
            out[y * r + x] = acc as f32 / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    out
}

/// Where the reference of every channel lives.
#[derive(Clone, Debug)]
pub enum Layout {
    /// `<prefix><set>_BaseColor.png` etc.
    SubstancePng { prefix: String },
    /// `<dir>/<set>_BCR.tga` and `<dir>/<set>_NMO.tga`.
    BcrNmo { dir: PathBuf },
    /// The project's own texture cache: the stack as Painter composed it when the file was saved (every channel,
    /// height included; always as fresh as the file).
    SppCache,
}

impl Layout {
    pub fn parse(kind: &str, truth: &str) -> Result<Layout, String> {
        match kind {
            "substance_png" | "png" => Ok(Layout::SubstancePng { prefix: truth.to_string() }),
            "bcr_nmo" | "bcr" => Ok(Layout::BcrNmo { dir: PathBuf::from(truth) }),
            "cache" | "spp_cache" => Ok(Layout::SppCache),
            other => Err(format!("unknown layout '{other}' (substance_png | bcr_nmo | cache)")),
        }
    }
}

/// References of a texture set from the project's texture cache (`None` when the set has none).
fn cache_truth(proj: &Project, set: usize, res: u32) -> Option<SetTruth> {
    let uid = proj.sets.get(set)?.cache_uid;
    let mut c = shard_formats::Container::open(&proj.path).ok()?;
    let chans = c.texture_cache_channels(uid);
    if chans.is_empty() {
        return None;
    }
    let mut t = SetTruth { images: Vec::new(), base: None, rough: None, metal: None, ao: None, normal: None, emissive: None, height: None };
    let to_rgba = |img: &shard_formats::texcache::CacheImage, f: &dyn Fn(usize) -> [u8; 4]| Truth {
        w: img.width,
        h: img.height,
        rgba: (0..(img.width * img.height) as usize).flat_map(f).collect(),
    };
    let byte = |img: &shard_formats::texcache::CacheImage, i: usize, k: usize| (img.get(i, k) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    // a project blending in ACEScg caches its colour in ACEScg (sRGB-encoded): brought to sRGB like the export
    let aces = crate::bitmaps::ProjectBitmaps::open(&proj.path, 1024).is_some_and(|b| b.aces_working());
    let colour = |img: &shard_formats::texcache::CacheImage, i: usize| -> [u8; 4] {
        if !aces {
            return [byte(img, i, 0), byte(img, i, 1), byte(img, i, 2), 255];
        }
        let l = [0, 1, 2].map(|k| shard_compose::srgb_to_linear(img.get(i, k).clamp(0.0, 1.0)));
        let m = &shard_compose::ACESCG_TO_SRGB;
        let q = |r: &[f32; 3]| (shard_compose::linear_to_srgb((r[0] * l[0] + r[1] * l[1] + r[2] * l[2]).clamp(0.0, 1.0)) * 255.0 + 0.5) as u8;
        [q(&m[0]), q(&m[1]), q(&m[2]), 255]
    };
    if let Some(b) = c.texture_cache(uid, 0) {
        t.images.push(to_rgba(&b, &|i| colour(&b, i)));
        t.base = Some(t.images.len() - 1);
    }
    let rough = c.texture_cache(uid, 7);
    let metal = c.texture_cache(uid, 13);
    if let Some(img) = rough.as_ref().or(metal.as_ref()) {
        let orm = to_rgba(img, &|i| {
            [255, rough.as_ref().map_or(0, |r| byte(r, i, 0)), metal.as_ref().map_or(0, |m| byte(m, i, 0)), 255]
        });
        t.images.push(orm);
        let k = t.images.len() - 1;
        t.rough = rough.is_some().then_some((k, 1));
        t.metal = metal.is_some().then_some((k, 2));
    }
    if let Some(e) = c.texture_cache(uid, 4) {
        t.images.push(to_rgba(&e, &|i| colour(&e, i)));
        t.emissive = Some(t.images.len() - 1);
    }
    if let Some(h) = c.texture_cache(uid, 1) {
        t.height = Some(h.to_f32(1, res.min(h.width)));
    }
    Some(t)
}

/// Reference images of one texture set, each channel as (image index, component).
struct SetTruth {
    images: Vec<Truth>,
    base: Option<usize>,
    rough: Option<(usize, usize)>,
    metal: Option<(usize, usize)>,
    ao: Option<(usize, usize)>,
    normal: Option<(usize, [usize; 2])>,
    emissive: Option<usize>,
    /// Height values (units of the Height channel) at the measure resolution, rows top-down.
    height: Option<Vec<f32>>,
}

fn load_truth(layout: &Layout, set: &str) -> Option<SetTruth> {
    let mut t = SetTruth { images: Vec::new(), base: None, rough: None, metal: None, ao: None, normal: None, emissive: None, height: None };
    let mut push = |t: &mut SetTruth, path: PathBuf| -> Option<usize> {
        let img = read_any(&path).ok()?;
        t.images.push(img);
        Some(t.images.len() - 1)
    };
    match layout {
        Layout::SubstancePng { prefix } => {
            let p = |s: &str| PathBuf::from(format!("{prefix}{set}_{s}.png"));
            t.base = push(&mut t, p("BaseColor"));
            if let Some(i) = push(&mut t, p("OcclusionRoughnessMetallic")) {
                t.ao = Some((i, 0));
                t.rough = Some((i, 1));
                t.metal = Some((i, 2));
            }
            if let Some(i) = push(&mut t, p("Normal")) {
                t.normal = Some((i, [0, 1]));
            }
            t.emissive = push(&mut t, p("Emissive"));
        }
        Layout::SppCache => return None, // read from the project by `cache_truth`
        Layout::BcrNmo { dir } => {
            if let Some(i) = push(&mut t, dir.join(format!("{set}_BCR.tga"))) {
                t.base = Some(i);
                t.rough = Some((i, 3));
            }
            if let Some(i) = push(&mut t, dir.join(format!("{set}_NMO.tga"))) {
                t.normal = Some((i, [0, 1]));
                t.metal = Some((i, 2));
                t.ao = Some((i, 3));
            }
        }
    }
    (!t.images.is_empty()).then_some(t)
}

/// Difference statistics between two equally long sample lists (0..255).
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub n: usize,
    pub mae: f64,
    pub rmse: f64,
    /// Pearson correlation (1 = same structure, 0 = unrelated).
    pub corr: f64,
    /// Mean of ours minus mean of the reference.
    pub bias: f64,
    /// 95th percentile of the absolute error.
    pub p95: f64,
    pub ours_mean: f64,
    pub truth_mean: f64,
}

pub fn metrics(ours: &[f32], truth: &[f32]) -> Metrics {
    let n = ours.len().min(truth.len());
    if n == 0 {
        return Metrics::default();
    }
    let (mut sa, mut sb, mut sab, mut saa, mut sbb, mut sabs, mut ssq) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    let mut errs: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (ours[i] as f64, truth[i] as f64);
        sa += a;
        sb += b;
        sab += a * b;
        saa += a * a;
        sbb += b * b;
        let d = (a - b).abs();
        sabs += d;
        ssq += d * d;
        errs.push(d as f32);
    }
    let nf = n as f64;
    let (ma, mb) = (sa / nf, sb / nf);
    let (va, vb) = (saa / nf - ma * ma, sbb / nf - mb * mb);
    let corr = if va > 1e-9 && vb > 1e-9 { (sab / nf - ma * mb) / (va.sqrt() * vb.sqrt()) } else { f64::NAN };
    let k = ((n - 1) as f64 * 0.95) as usize;
    let (_, p95, _) = errs.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap());
    Metrics { n, mae: sabs / nf, rmse: (ssq / nf).sqrt(), corr, bias: ma - mb, p95: *p95 as f64, ours_mean: ma, truth_mean: mb }
}

#[derive(Clone, Debug)]
pub struct ChannelReport {
    pub name: String,
    pub m: Metrics,
    /// MAE when the reference is flipped vertically: if it is clearly lower, the orientation is wrong.
    pub flipped_mae: f64,
}

#[derive(Clone, Debug)]
pub struct SetReport {
    pub set: String,
    pub res: u32,
    pub channels: Vec<ChannelReport>,
    pub compose_secs: f64,
    pub covered: usize,
    pub notes: Vec<String>,
}

fn flip_rows(v: &[f32], res: usize) -> Vec<f32> {
    let mut o = Vec::with_capacity(v.len());
    for y in (0..res).rev() {
        o.extend_from_slice(&v[y * res..(y + 1) * res]);
    }
    o
}

fn pick(v: &[f32], cov: &[bool]) -> Vec<f32> {
    v.iter().zip(cov).filter(|(_, c)| **c).map(|(x, _)| *x).collect()
}

/// Our samples of one (possibly multi-component) channel as parallel lists: `comps[c][texel]`.
fn image_comps(img: &shard_compose::Image, res: u32, count: usize, map: impl Fn(f32) -> f32) -> Vec<Vec<f32>> {
    let r = res as usize;
    let (iw, ih) = (img.w as usize, img.h as usize);
    // an image bigger than `res` (composed at the project's resolution, exported smaller) is box-filtered
    let (kx, ky) = ((iw / r).max(1), (ih / r).max(1));
    (0..count)
        .map(|c| {
            let mut out = Vec::with_capacity(r * r);
            for y in 0..r {
                for x in 0..r {
                    if kx == 1 && ky == 1 {
                        let sy = (y * ih / r).min(ih - 1);
                        let sx = (x * iw / r).min(iw - 1);
                        out.push(map(img.pixel(sx as u32, sy as u32)[c.min(img.n - 1)]));
                    } else {
                        let mut acc = 0.0f32;
                        for dy in 0..ky {
                            for dx in 0..kx {
                                let p = img.pixel(((x * kx + dx).min(iw - 1)) as u32, ((y * ky + dy).min(ih - 1)) as u32);
                                acc += map(p[c.min(img.n - 1)]);
                            }
                        }
                        out.push(acc / (kx * ky) as f32);
                    }
                }
            }
            out
        })
        .collect()
}

/// Measure one texture set against the references of `layout`. `None` when no reference file exists.
pub fn measure_set(proj: &Project, set: usize, layout: &Layout, res: u32, dump: Option<&Path>) -> Option<SetReport> {
    let name = proj.sets.get(set)?.name.clone();
    let truth = match layout {
        Layout::SppCache => cache_truth(proj, set, res)?,
        _ => load_truth(layout, &name)?,
    };
    let cov = raster::erode(&raster::coverage(&proj.mesh, set, res, res), res, res);
    let covered = cov.iter().filter(|c| **c).count();
    let r = res as usize;

    let t0 = std::time::Instant::now();
    let mut bits = vec![0u32, 7, 13];
    if truth.emissive.is_some() {
        bits.push(4);
    }
    if truth.height.is_some() {
        bits.push(1);
    }
    // SHARD_ROUGH_FROM_GLOSS=1: take roughness as 1 - Glossiness (channel 6) instead of the Roughness channel
    if std::env::var_os("SHARD_ROUGH_FROM_GLOSS").is_some() {
        bits.push(6);
    }
    // Painter renders at the resolution of the texture set and an export at a smaller size is that picture
    // reduced: composing at the set's own size and box-filtering to `res` reproduces it (a filter such as
    // Sharpen is resolution dependent: at 1024 its jump on an interior set's base colour was 3.1 against 0.6).
    // SHARD_MEASURE_NATIVE=0 composes at `res` as before.
    let native = if std::env::var("SHARD_MEASURE_NATIVE").is_ok_and(|v| v == "0") {
        res
    } else {
        proj.sets[set].resolution.0.min(4096).max(res)
    };
    let comp = proj.compose(set, &bits, native);
    let compose_secs = t0.elapsed().as_secs_f64();
    // SHARD_EDIT_BENCH=1: how long a recomposition takes after the same stack, after changing the opacity of the
    // top layer, and after changing a layer in the middle (masks and checkpoints are kept between them)
    if std::env::var_os("SHARD_EDIT_BENCH").is_some() {
        use shard_model::{BlendMode, ChannelBlend, ChannelSet, Layer};
        fn count(ls: &[Layer]) -> usize {
            ls.iter().map(|l| 1 + count(&l.children)).sum()
        }
        // lower the opacity of the k-th layer in depth-first order (top first)
        fn tweak(ls: &mut [Layer], k: &mut usize) -> bool {
            for l in ls.iter_mut() {
                if *k == 0 {
                    // SHARD_EDIT_BENCH=hide: hide the layer instead (the eye toggle)
                    if std::env::var("SHARD_EDIT_BENCH").is_ok_and(|v| v == "hide") {
                        l.visible = !l.visible;
                        return true;
                    }
                    match l.blend.first_mut() {
                        Some(b) => b.opacity = (b.opacity - 0.1).max(0.0),
                        None => l.blend.push(ChannelBlend { mode: BlendMode::NORMAL, channels: ChannelSet(u128::MAX), opacity: 0.9 }),
                    }
                    return true;
                }
                *k -= 1;
                if tweak(&mut l.children, k) {
                    return true;
                }
            }
            false
        }
        let layers = &proj.sets[set].layers;
        let total = count(layers);
        let timed = |label: &str, l: &[Layer]| {
            let t = std::time::Instant::now();
            let _ = proj.compose_layers(set, l, &bits, res);
            println!("  edit bench: {label}: {:.2}s", t.elapsed().as_secs_f64());
        };
        timed("same stack", layers);
        for (label, at) in [("first layer", 0), ("a third down", total / 3), ("two thirds down", 2 * total / 3), ("last layer", total.saturating_sub(1))] {
            let mut l = layers.clone();
            let mut k = at;
            tweak(&mut l, &mut k);
            timed(&format!("{label} (#{at} of {total}) opacity -10%"), &l);
        }
    }

    let mut channels = Vec::new();
    let mut notes = Vec::new();
    let set_name = name.clone();
    let mut add = |channels: &mut Vec<ChannelReport>, label: &str, ours: &[f32], truth: &[f32]| {
        if let Some(dir) = dump {
            dump_channel(dir, &set_name, label, res, ours, truth, &cov);
        }
        let (a, b) = (pick(ours, &cov), pick(truth, &cov));
        let m = metrics(&a, &b);
        let flipped = pick(&flip_rows(truth, r), &cov);
        channels.push(ChannelReport { name: label.to_string(), m, flipped_mae: metrics(&a, &flipped).mae });
    };

    // base colour: sRGB-encoded, three components
    if let (Some(bi), Some(img)) = (truth.base, comp.channels.get(&0)) {
        let ours = image_comps(img, res, 3, |v| shard_compose::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0);
        for (c, l) in ["base_color.r", "base_color.g", "base_color.b"].into_iter().enumerate() {
            add(&mut channels, l, &ours[c], &component(&truth.images[bi], c, res));
        }
    }
    // SHARD_CHUNK_COLOR=1: base colour (red) per UV chunk: texels, MAE, mean ours / truth
    if let (Some(bi), Some(img), true) = (truth.base, comp.channels.get(&0), std::env::var_os("SHARD_CHUNK_COLOR").is_some()) {
        let chunks = crate::strokes::uv_chunks(&proj.mesh);
        let ids = raster::tri_map(&proj.mesh, set, res, res, &chunks);
        let ours = image_comps(img, res, 1, |v| shard_compose::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).remove(0);
        let t = component(&truth.images[bi], 0, res);
        let mut acc: std::collections::BTreeMap<i64, (usize, f64, f64, f64)> = Default::default();
        for i in 0..(res * res) as usize {
            if ids[i] < 0 || !cov[i] {
                continue;
            }
            let e = acc.entry(ids[i]).or_default();
            e.0 += 1;
            e.1 += (ours[i] - t[i]).abs() as f64;
            e.2 += ours[i] as f64;
            e.3 += t[i] as f64;
        }
        for (c, (n, ae, mo, mt)) in acc {
            if n >= 400 {
                let n = n as f64;
                notes.push(format!("colour chunk {c:5}: {n:6} texels  MAE {:6.1}  ours {:6.1}  truth {:6.1}", ae / n, mo / n, mt / n));
            }
        }
    }
    // SHARD_CHUNK_METAL=1: metallic per UV chunk: texels, MAE, mean ours / truth
    if let (Some((ti, c)), Some(img), true) = (truth.metal, comp.channels.get(&13), std::env::var_os("SHARD_CHUNK_METAL").is_some()) {
        let chunks = crate::strokes::uv_chunks(&proj.mesh);
        let ids = raster::tri_map(&proj.mesh, set, res, res, &chunks);
        let ours = image_comps(img, res, 1, |v| v.clamp(0.0, 1.0) * 255.0).remove(0);
        let t = component(&truth.images[ti], c, res);
        let mut acc: std::collections::BTreeMap<i64, (usize, f64, f64, f64)> = Default::default();
        for i in 0..(res * res) as usize {
            if ids[i] < 0 || !cov[i] {
                continue;
            }
            let e = acc.entry(ids[i]).or_default();
            e.0 += 1;
            e.1 += (ours[i] - t[i]).abs() as f64;
            e.2 += ours[i] as f64;
            e.3 += t[i] as f64;
        }
        for (c, (n, ae, mo, mt)) in acc {
            if n >= 300 {
                let n = n as f64;
                notes.push(format!("metal chunk {c:5}: {n:6} texels  MAE {:6.1}  ours {:6.1}  truth {:6.1}", ae / n, mo / n, mt / n));
            }
        }
    }
    let scalar = |bit: u32| comp.channels.get(&bit).map(|i| image_comps(i, res, 1, |v| v.clamp(0.0, 1.0) * 255.0).remove(0));
    let rough_ours = if std::env::var_os("SHARD_ROUGH_FROM_GLOSS").is_some() {
        scalar(6).map(|g| g.into_iter().map(|v| 255.0 - v).collect::<Vec<f32>>())
    } else {
        scalar(7)
    };
    if let (Some((ti, c)), Some(ours)) = (truth.rough, rough_ours) {
        add(&mut channels, "roughness", &ours, &component(&truth.images[ti], c, res));
    }
    if let (Some((ti, c)), Some(ours)) = (truth.metal, scalar(13)) {
        add(&mut channels, "metallic", &ours, &component(&truth.images[ti], c, res));
    }
    if let (Some(ei), Some(img)) = (truth.emissive, comp.channels.get(&4)) {
        let ours = image_comps(img, res, 3, |v| shard_compose::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0);
        for (c, l) in ["emissive.r", "emissive.g", "emissive.b"].into_iter().enumerate() {
            add(&mut channels, l, &ours[c], &component(&truth.images[ei], c, res));
        }
    }
    // height, in units of the channel times 255 (a step of 0.1 reads 25.5), around 128 so the dumped pictures keep
    // the engravings (negative heights)
    if let Some(th) = truth.height.as_ref() {
        let ours = match comp.channels.get(&1) {
            Some(img) => image_comps(img, res, 1, |v| v * 255.0 + 128.0).remove(0),
            None => vec![128.0; (res * res) as usize],
        };
        if th.len() == ours.len() {
            let tv: Vec<f32> = th.iter().map(|v| v * 255.0 + 128.0).collect();
            add(&mut channels, "height (x255+128)", &ours, &tv);
        } else {
            notes.push(format!("height reference is {} texels, not {}", th.len(), ours.len()));
        }
    }
    // baked mesh maps: ambient occlusion and the base normal
    if let Some((ti, c)) = truth.ao {
        match proj.mesh_map(set, shard_compose::MeshMap::AmbientOcclusion, 2048) {
            Some(img) => {
                let ours = image_comps(&img, res, 1, |v| v.clamp(0.0, 1.0) * 255.0).remove(0);
                add(&mut channels, "ao (bake)", &ours, &component(&truth.images[ti], c, res));
            }
            None => notes.push("no ambient occlusion bake found".into()),
        }
    }
    if let Some((ti, comps)) = truth.normal {
        match proj.mesh_map(set, shard_compose::MeshMap::NormalBase, 2048) {
            Some(img) => {
                let ours = image_comps(&img, res, 2, |v| v.clamp(0.0, 1.0) * 255.0);
                add(&mut channels, "normal.x (bake)", &ours[0], &component(&truth.images[ti], comps[0], res));
                add(&mut channels, "normal.y (bake)", &ours[1], &component(&truth.images[ti], comps[1], res));
            }
            None => notes.push("no normal bake found".into()),
        }
    }
    if let Some((ti, comps)) = truth.normal {
        let nm = proj.normal(set, res);
        let ours = image_comps(&nm, res, 2, |v| v.clamp(0.0, 1.0) * 255.0);
        add(&mut channels, "normal.x (stack)", &ours[0], &component(&truth.images[ti], comps[0], res));
        add(&mut channels, "normal.y (stack)", &ours[1], &component(&truth.images[ti], comps[1], res));
    }
    // SHARD_CHUNK_FIT=1: per UV chunk, mean of (Painter normal.x - baked normal.x) and of ours (stack - baked)
    if let (Some((ti, comps)), true) = (truth.normal, std::env::var_os("SHARD_CHUNK_FIT").is_some()) {
        let chunks = crate::strokes::uv_chunks(&proj.mesh);
        let ids = raster::tri_map(&proj.mesh, set, res, res, &chunks);
        let mesh_img = proj.mesh_map(set, shard_compose::MeshMap::NormalBase, (res * 2).clamp(1024, 4096));
        let stack = proj.normal(set, res);
        if let Some(mi) = mesh_img {
            let bake = image_comps(&mi, res, 1, |v| v.clamp(0.0, 1.0) * 255.0).remove(0);
            let ours = image_comps(&stack, res, 1, |v| v.clamp(0.0, 1.0) * 255.0).remove(0);
            let t = component(&truth.images[ti], comps[0], res);
            let mut acc: std::collections::BTreeMap<i64, (usize, f64, f64)> = Default::default();
            for i in 0..(res * res) as usize {
                if ids[i] < 0 || !cov[i] {
                    continue;
                }
                let e = acc.entry(ids[i]).or_default();
                e.0 += 1;
                e.1 += (t[i] - bake[i]) as f64;
                e.2 += (ours[i] - bake[i]) as f64;
            }
            for (c, (n, dt, do_)) in acc {
                if n >= 400 {
                    notes.push(format!("chunk {c:5}: {n:6} texels  truth-bake {:+6.2}  ours-bake {:+6.2}", dt / n as f64, do_ / n as f64));
                }
            }
        }
    }
    // SHARD_FIT_H2N=1: least-squares slope between (truth normal - mesh normal) and the slope of our Height
    if let (Some((ti, comps)), true) = (truth.normal, std::env::var_os("SHARD_FIT_H2N").is_some()) {
        let comp_h = proj.compose(set, &[1], res);
        let mesh = proj.mesh_map(set, shard_compose::MeshMap::NormalBase, (res * 2).clamp(1024, 4096));
        if let (Some(hi), Some(mi)) = (comp_h.channels.get(&1), mesh.as_ref()) {
            let mesh_s = image_comps(mi, res, 2, |v| v.clamp(0.0, 1.0) * 2.0 - 1.0);
            for (axis, name) in [(0usize, "x"), (1, "y")] {
                let t = component(&truth.images[ti], comps[axis], res);
                let rr = res as i32;
                let hv = |x: i32, y: i32| hi.data[((y.clamp(0, rr - 1) * rr + x.clamp(0, rr - 1)) as usize) * hi.n];
                let (mut sff, mut srf, mut srr, mut cnt) = (0.0f64, 0.0f64, 0.0f64, 0u64);
                for y in 1..rr - 1 {
                    for x in 1..rr - 1 {
                        let i = (y * rr + x) as usize;
                        if !cov[i] {
                            continue;
                        }
                        let g = if axis == 0 {
                            hv(x + 1, y - 1) + 2.0 * hv(x + 1, y) + hv(x + 1, y + 1) - hv(x - 1, y - 1) - 2.0 * hv(x - 1, y) - hv(x - 1, y + 1)
                        } else {
                            hv(x - 1, y + 1) + 2.0 * hv(x, y + 1) + hv(x + 1, y + 1) - hv(x - 1, y - 1) - 2.0 * hv(x, y - 1) - hv(x + 1, y - 1)
                        } / 8.0;
                        let f = (-g * res as f32) as f64;
                        let r = ((t[i] / 255.0 * 2.0 - 1.0) - mesh_s[axis][i]) as f64;
                        sff += f * f;
                        srf += r * f;
                        srr += r * r;
                        cnt += 1;
                    }
                }
                if sff > 0.0 && srr > 0.0 {
                    notes.push(format!(
                        "h2n fit {name}: slope {:.3e}  corr {:.3}  (n {cnt}, |f| rms {:.4}, |r| rms {:.4})",
                        srf / sff,
                        srf / (sff.sqrt() * srr.sqrt()),
                        (sff / cnt as f64).sqrt(),
                        (srr / cnt as f64).sqrt()
                    ));
                }
            }
        }
    }
    for c in &channels {
        if c.flipped_mae + 3.0 < c.m.mae {
            notes.push(format!("{}: flipped reference matches better ({:.1} vs {:.1}): orientation?", c.name, c.flipped_mae, c.m.mae));
        }
    }
    Some(SetReport { set: name, res, channels, compose_secs, covered, notes })
}

fn gray_png(path: &Path, res: u32, vals: &[f32]) {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, res, res);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut w) = enc.write_header() {
            let bytes: Vec<u8> = vals.iter().map(|v| v.clamp(0.0, 255.0).round() as u8).collect();
            let _ = w.write_image_data(&bytes);
        }
    }
    let _ = std::fs::write(path, out);
}

/// Write `ours`, `truth` and the amplified absolute difference (x4, only inside the islands) of a channel.
fn dump_channel(dir: &Path, set: &str, label: &str, res: u32, ours: &[f32], truth: &[f32], cov: &[bool]) {
    let _ = std::fs::create_dir_all(dir);
    let safe: String = format!("{set}_{label}").chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    gray_png(&dir.join(format!("{safe}_ours.png")), res, ours);
    gray_png(&dir.join(format!("{safe}_truth.png")), res, truth);
    let diff: Vec<f32> =
        ours.iter().zip(truth).zip(cov).map(|((a, b), c)| if *c { ((a - b).abs() * 4.0).min(255.0) } else { 0.0 }).collect();
    gray_png(&dir.join(format!("{safe}_diff.png")), res, &diff);
}

/// A mask exported by Painter ("Export mask to file") to compare layer by layer.
#[derive(Clone, Debug)]
pub struct MaskTruth {
    pub set: String,
    /// Layer name, or `Group/Child` to tell equal names apart.
    pub layer: String,
    pub file: PathBuf,
}

fn find_layer(layers: &[shard_model::Layer], want: &[&str], ancestors: &mut Vec<String>, trail: &mut Vec<usize>) -> bool {
    for (i, l) in layers.iter().enumerate() {
        trail.push(i);
        let ok = Some(&l.name.as_str()) == want.last()
            && want[..want.len() - 1].iter().all(|seg| ancestors.iter().any(|a| a == seg));
        if ok {
            return true;
        }
        ancestors.push(l.name.clone());
        let found = find_layer(&l.children, want, ancestors, trail);
        ancestors.pop();
        if found {
            return true;
        }
        trail.pop();
    }
    false
}

/// Compare our evaluation of one layer's mask with the mask Painter exported for it.
pub fn measure_mask(proj: &Project, mt: &MaskTruth, res: u32, dump: Option<&Path>) -> Result<(ChannelReport, Vec<String>), String> {
    let set = proj.sets.iter().position(|s| s.name == mt.set).ok_or_else(|| format!("no texture set '{}'", mt.set))?;
    let parts: Vec<&str> = mt.layer.split('/').collect();
    let mut trail = Vec::new();
    if !find_layer(&proj.sets[set].layers, &parts, &mut Vec::new(), &mut trail) {
        return Err(format!("no layer '{}' in '{}'", mt.layer, mt.set));
    }
    let truth = read_any(&mt.file)?;
    let bitmaps = crate::bitmaps::ProjectBitmaps::open(&proj.path, if res > 1024 { 2048 } else { 1024 });
    let st = &proj.sets[set];
    let doc = crate::compose::document(&st.name, &st.layers);
    let preview = crate::compose::mask_job(
        &doc,
        bitmaps.as_ref(),
        &proj.bake_urls,
        &st.name,
        &proj.mesh,
        set,
        &trail,
        res,
        proj.stroke_aspect,
    )
    .ok_or("layer has no mask")?;
    let mut ours: Vec<f32> = preview.values.iter().map(|v| v.clamp(0.0, 1.0) * 255.0).collect();
    let theirs = component(&truth, 0, res);
    // Painter exports the layer's own mask (verified: Gradient 0.989, Fill layer 10 0.966, Floor 0.973);
    // SHARD_MASK_EFFECTIVE=1 multiplies in the masks of the groups above
    let own = ours.clone();
    if std::env::var_os("SHARD_MASK_EFFECTIVE").is_some() {
        for k in 1..trail.len() {
            if let Some(pm) = crate::compose::mask_job(
                &doc,
                bitmaps.as_ref(),
                &proj.bake_urls,
                &st.name,
                &proj.mesh,
                set,
                &trail[..k],
                res,
                proj.stroke_aspect,
            ) {
                for (o, v) in ours.iter_mut().zip(&pm.values) {
                    *o *= v.clamp(0.0, 1.0);
                }
            }
        }
    }
    let _ = own;
    let cov = raster::erode(&raster::coverage(&proj.mesh, set, res, res), res, res);
    let r = res as usize;
    if let Some(dir) = dump {
        dump_channel(dir, &mt.set, &format!("mask {}", mt.layer), res, &ours, &theirs, &cov);
    }
    let (a, b) = (pick(&ours, &cov), pick(&theirs, &cov));
    let flipped = pick(&flip_rows(&theirs, r), &cov);
    let rep = ChannelReport { name: format!("mask {}", mt.layer), m: metrics(&a, &b), flipped_mae: metrics(&a, &flipped).mae };
    let mut notes = preview.notes;
    // SHARD_MASK_CHUNKS=1: the UV chunks where our mask and the export disagree
    if std::env::var_os("SHARD_MASK_CHUNKS").is_some() {
        let chunks = crate::strokes::uv_chunks(&proj.mesh);
        let ids = raster::tri_map(&proj.mesh, set, res, res, &chunks);
        let mut acc: std::collections::BTreeMap<i64, (usize, f64, f64)> = Default::default();
        for i in 0..(res * res) as usize {
            if ids[i] < 0 || !cov[i] {
                continue;
            }
            let e = acc.entry(ids[i]).or_default();
            e.0 += 1;
            e.1 += ours[i] as f64;
            e.2 += theirs[i] as f64;
        }
        let mut rows: Vec<(i64, usize, f64, f64)> = acc.into_iter().map(|(c, (n, o, t))| (c, n, o / n as f64, t / n as f64)).collect();
        rows.sort_by(|x, y| ((y.2 - y.3).abs() * y.1 as f64).partial_cmp(&((x.2 - x.3).abs() * x.1 as f64)).unwrap());
        for (c, n, o, t) in rows.iter().take(40) {
            if (o - t).abs() > 20.0 {
                println!("  chunk {c:6}: {n:6} texels  ours {o:6.1}  truth {t:6.1}  {}", if o > t { "we select more" } else { "we select less" });
            }
        }
    }
    if !preview.reliable {
        notes.push("our mask is approximated".into());
    }
    Ok((rep, notes))
}

/// Pretty table of one set.
pub fn print_set(project: &str, rep: &SetReport) {
    println!(
        "== {project} / {}  {}x{}  ({} texels, composed in {:.1}s)",
        rep.set, rep.res, rep.res, rep.covered, rep.compose_secs
    );
    println!("  {:<18} {:>6} {:>6} {:>6} {:>7} {:>6}  {:>13}", "channel", "MAE", "RMSE", "p95", "corr", "bias", "ours | truth");
    for c in &rep.channels {
        println!(
            "  {:<18} {:>6.1} {:>6.1} {:>6.1} {:>7.3} {:>+6.1}  {:>6.1} | {:<6.1}",
            c.name, c.m.mae, c.m.rmse, c.m.p95, c.m.corr, c.m.bias, c.m.ours_mean, c.m.truth_mean
        );
    }
    for n in &rep.notes {
        println!("  note: {n}");
    }
}

// ---------------------------------------------------------------------------------------------------------
// Layer by layer: the ladder exported by the Painter plugin (tools/painter_plugin/shard_extract.py)

/// One step of the ladder: the stack with every layer up to `step` (bottom to top) visible.
#[derive(Clone, Debug)]
pub struct LadderRow {
    pub step: usize,
    pub name: String,
    pub has_mask: bool,
    /// Our leaf at this step has the same name as the plugin's.
    pub name_matches: bool,
    /// Mean absolute error (0..255) of base colour (mean of R, G, B), roughness and metallic.
    pub base: Option<f64>,
    pub rough: Option<f64>,
    pub metal: Option<f64>,
    /// Mean of ours - mean of the reference, for the sign of the error.
    pub base_bias: Option<f64>,
    pub rough_bias: Option<f64>,
    pub metal_bias: Option<f64>,
}

/// File-name form of a name, like the plugin's `slug`.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut last_sep = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
            last_sep = false;
        } else if !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    let t = out.trim_matches('_');
    let t: String = t.chars().take(40).collect();
    if t.is_empty() {
        "unnamed".into()
    } else {
        t
    }
}

struct Leaf {
    path: Vec<usize>,
    name: String,
    /// Visible in the project: the layer and every group around it.
    visible: bool,
}

/// Non-group layers bottom to top (depth first), like the plugin's ladder order.
fn leaves_bottom_up(layers: &[shard_model::Layer]) -> Vec<Leaf> {
    fn walk(ls: &[shard_model::Layer], path: &mut Vec<usize>, parent_visible: bool, out: &mut Vec<Leaf>) {
        for (i, l) in ls.iter().enumerate().rev() {
            path.push(i);
            let vis = parent_visible && l.visible;
            if l.kind == shard_model::LayerKind::Group {
                walk(&l.children, path, vis, out);
            } else {
                out.push(Leaf { path: path.clone(), name: l.name.clone(), visible: vis });
            }
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(layers, &mut Vec::new(), true, &mut out);
    out
}

/// The stack with only the leaves in `keep` (and the groups that hold them) visible.
fn only_leaves(layers: &[shard_model::Layer], keep: &std::collections::HashSet<Vec<usize>>) -> Vec<shard_model::Layer> {
    fn walk(ls: &mut [shard_model::Layer], path: &mut Vec<usize>, keep: &std::collections::HashSet<Vec<usize>>) -> bool {
        let mut any = false;
        for (i, l) in ls.iter_mut().enumerate() {
            path.push(i);
            if l.kind == shard_model::LayerKind::Group {
                let inner = walk(&mut l.children, path, keep);
                l.visible = l.visible && inner;
            } else {
                l.visible = l.visible && keep.contains(path);
            }
            any |= l.visible;
            path.pop();
        }
        any
    }
    let mut out = layers.to_vec();
    walk(&mut out, &mut Vec::new(), keep);
    out
}

/// Why an extraction may not describe the project file as it is on disk (None when it does): the `.spp` was saved
/// after the extraction ran, or Painter held unsaved edits then. A test screen's "regression" of 2026-10-12 was this:
/// the project had been edited and saved after its exports were made.
pub fn extraction_stale(extract: &Path, project: &Path) -> Option<String> {
    let stamp = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let (spp, done) = (stamp(project)?, stamp(&extract.join("log.json")).or_else(|| stamp(&extract.join("project.json")))?);
    if let Ok(ahead) = spp.duration_since(done) {
        if ahead.as_secs() > 2 {
            return Some(format!(
                "{} was saved {} min after this extraction: layers, parameters or masks may have changed; extract again",
                project.display(),
                ahead.as_secs() / 60
            ));
        }
    }
    let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(extract.join("project.json")).ok()?).ok()?;
    (info["needs_saving"].as_bool() == Some(true))
        .then(|| "Painter had unsaved edits when this was extracted (the .spp on disk differs from the exports)".to_string())
}

/// Compare our stack, built layer by layer, with the exports of the plugin for texture set `set`.
/// `extract` is the plugin's `<out>/<project>` folder. Returns `None` when it holds no ladder for the set.
pub fn measure_ladder(proj: &Project, set: usize, extract: &Path, res: u32, compose_res: u32) -> Option<Vec<LadderRow>> {
    let name = proj.sets.get(set)?.name.clone();
    let tag = slug(&name);
    let tree: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(extract.join("layers").join(format!("{tag}.json"))).ok()?).ok()?;
    let steps: Vec<(usize, String, bool)> = tree["ladder"]
        .as_array()?
        .iter()
        .filter_map(|s| Some((s["step"].as_u64()? as usize, s["name"].as_str()?.to_string(), s["has_mask"].as_bool().unwrap_or(false))))
        .collect();
    let ladder_dir = extract.join("ladder").join(&tag);
    // step index -> folder
    let mut dirs: std::collections::BTreeMap<usize, PathBuf> = Default::default();
    for e in std::fs::read_dir(&ladder_dir).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if let Some(i) = n.split('_').next().and_then(|d| d.parse::<usize>().ok()) {
            dirs.insert(i, e.path());
        }
    }
    // SHARD_LADDER_STEPS=0,15,30: only these steps (a quick look while changing a rule)
    if let Ok(list) = std::env::var("SHARD_LADDER_STEPS") {
        let keep: Vec<usize> = list.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        dirs.retain(|k, _| keep.contains(k));
    }
    if dirs.is_empty() {
        return None;
    }
    let leaves = leaves_bottom_up(&proj.sets[set].layers);
    let cov = raster::erode(&raster::coverage(&proj.mesh, set, res, res), res, res);
    let bits = [0u32, 7, 13];
    let mut rows = Vec::new();
    for (idx, dir) in &dirs {
        let Some((step, lname, has_mask)) = steps.iter().find(|s| s.0 == *idx).cloned() else { continue };
        let Some(leaf) = leaves.get(*idx) else { continue };
        let keep: std::collections::HashSet<Vec<usize>> =
            leaves[..=*idx].iter().filter(|l| l.visible).map(|l| l.path.clone()).collect();
        let layers = only_leaves(&proj.sets[set].layers, &keep);
        let comp = proj.compose_layers(set, &layers, &bits, compose_res.max(res));
        let mut row = LadderRow {
            step,
            name: lname.clone(),
            has_mask,
            name_matches: leaf.name == lname,
            base: None,
            rough: None,
            metal: None,
            base_bias: None,
            rough_bias: None,
            metal_bias: None,
        };
        let read = |file: &str| read_png(&dir.join(file)).ok();
        // SHARD_LADDER_DUMP=dir: our picture of every step (sRGB 8-bit base colour, gray roughness / metallic)
        if let Ok(out) = std::env::var("SHARD_LADDER_DUMP") {
            let out = Path::new(&out).join(&tag);
            let _ = std::fs::create_dir_all(&out);
            gray_png(&out.join("coverage.png"), res, &cov.iter().map(|c| if *c { 255.0 } else { 0.0 }).collect::<Vec<f32>>());
            let q = |v: f32, srgb: bool| ((if srgb { shard_compose::linear_to_srgb(v.clamp(0.0, 1.0)) } else { v.clamp(0.0, 1.0) }) * 255.0 + 0.5) as u8;
            for (bit, file, colour) in [(0u32, "BaseColor.png", true), (7, "Roughness.png", false), (13, "Metallic.png", false)] {
                if let Some(img) = comp.channels.get(&bit) {
                    // the picture is written at the size it was composed (the project's resolution)
                    let mut rgba = Vec::with_capacity((img.w * img.h * 4) as usize);
                    for p in img.data.chunks_exact(img.n) {
                        let c = |k: usize| q(p[k.min(img.n - 1)], colour);
                        rgba.extend_from_slice(&[c(0), c(1), c(2), 255]);
                    }
                    let path = out.join(format!("{:03}_{file}", step));
                    if let Ok(f) = std::fs::File::create(&path) {
                        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.w, img.h);
                        enc.set_color(png::ColorType::Rgba);
                        enc.set_depth(png::BitDepth::Eight);
                        if let Ok(mut w) = enc.write_header() {
                            let _ = w.write_image_data(&rgba);
                        }
                    }
                }
            }
        }
        if let (Some(t), Some(img)) = (read("BaseColor.png"), comp.channels.get(&0)) {
            let ours = image_comps(img, res, 3, |v| shard_compose::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0);
            let (mut mae, mut bias) = (0.0, 0.0);
            for c in 0..3 {
                let m = metrics(&pick(&ours[c], &cov), &pick(&component(&t, c, res), &cov));
                mae += m.mae / 3.0;
                bias += m.bias / 3.0;
            }
            row.base = Some(mae);
            row.base_bias = Some(bias);
        }
        // OpenPBR projects name the channels SpecularRoughness / BaseMetalness
        for (files, bit, slot) in [(["Roughness.png", "SpecularRoughness.png"], 7u32, 0), (["Metallic.png", "BaseMetalness.png"], 13, 1)] {
            if let (Some(t), Some(img)) = (files.iter().find_map(|f| read(f)), comp.channels.get(&bit)) {
                let ours = image_comps(img, res, 1, |v| v.clamp(0.0, 1.0) * 255.0).remove(0);
                let m = metrics(&pick(&ours, &cov), &pick(&component(&t, 0, res), &cov));
                if slot == 0 {
                    row.rough = Some(m.mae);
                    row.rough_bias = Some(m.bias);
                } else {
                    row.metal = Some(m.mae);
                    row.metal_bias = Some(m.bias);
                }
            }
        }
        rows.push(row);
    }
    Some(rows)
}

/// Print the ladder and mark where the error jumps (the layer that is not reproduced yet).
pub fn print_ladder(project: &str, set: &str, rows: &[LadderRow]) {
    println!("== {project} / {set}: layer by layer (MAE 0..255; jump = error added by that layer, << = more than 1.5)");
    println!("  step  layer                                   base   (jump)  bias    rough  (jump)  bias    metal  (jump)  bias");
    let fmt = |v: Option<f64>| v.map(|x| format!("{x:6.2}")).unwrap_or_else(|| "     -".into());
    let (mut pb, mut pr, mut pm) = (0.0f64, 0.0f64, 0.0f64);
    for r in rows {
        let jump = |now: Option<f64>, prev: &mut f64| -> (String, bool) {
            let Some(n) = now else { return ("      ".into(), false) };
            let d = n - *prev;
            *prev = n;
            (format!("{d:+6.2}"), d > 1.5)
        };
        let (jb, bad_b) = jump(r.base, &mut pb);
        let (jr, bad_r) = jump(r.rough, &mut pr);
        let (jm, bad_m) = jump(r.metal, &mut pm);
        let flag = if bad_b || bad_r || bad_m { "  <<" } else { "" };
        let label: String = r.name.chars().take(36).collect();
        let sb = |v: Option<f64>| v.map(|x| format!("{x:+6.1}")).unwrap_or_else(|| "      ".into());
        println!(
            "  {:>4}  {:<36}{} {}  {} {}  {}  {} {}  {}  {} {}{}{}",
            r.step,
            label,
            if r.name_matches { " " } else { "?" },
            fmt(r.base),
            jb,
            sb(r.base_bias),
            fmt(r.rough),
            jr,
            sb(r.rough_bias),
            fmt(r.metal),
            jm,
            sb(r.metal_bias),
            if r.has_mask { "  [mask]" } else { "" },
            flag
        );
    }
    let last = rows.last();
    let show = |v: Option<f64>| v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "-".into());
    println!(
        "  final step: base {} rough {} metal {}",
        show(last.and_then(|r| r.base)),
        show(last.and_then(|r| r.rough)),
        show(last.and_then(|r| r.metal))
    );
}

// ---------------------------------------------------------------------------------------------------------
// A filter on its own: Painter's picture before the layer fed to our filter, against Painter's picture after it

/// Result of one way of feeding a filter.
pub struct FilterVariant {
    pub label: &'static str,
    pub mae: f64,
    pub bias: f64,
    /// Difference of our result from the picture that was fed in (0 = the filter did nothing).
    pub vs_input: f64,
}

fn find_layer_by_uid<'a>(ls: &'a [shard_model::Layer], uid: u64) -> Option<&'a shard_model::Layer> {
    for l in ls {
        if l.uid == uid {
            return Some(l);
        }
        if let Some(f) = find_layer_by_uid(&l.children, uid) {
            return Some(f);
        }
    }
    None
}

/// Take Painter's base colour at ladder step `from`, run the first filter of layer `uid` over it with our
/// engine in several colour-space conventions and compare with Painter's picture at step `to`.
pub fn filter_isolation(
    proj: &Project,
    set: usize,
    extract: &Path,
    from: usize,
    to: usize,
    uid: u64,
    res: u32,
) -> Result<Vec<FilterVariant>, String> {
    use shard_compose::MeshMaps;
    let tag = slug(&proj.sets[set].name);
    let dir_of = |step: usize| -> Result<PathBuf, String> {
        let base = extract.join("ladder").join(&tag);
        for e in std::fs::read_dir(&base).map_err(|e| format!("{}: {e}", base.display()))?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.split('_').next().and_then(|d| d.parse::<usize>().ok()) == Some(step) {
                return Ok(e.path());
            }
        }
        Err(format!("no ladder folder for step {step}"))
    };
    let (a, b) = (read_png(&dir_of(from)?.join("BaseColor.png"))?, read_png(&dir_of(to)?.join("BaseColor.png"))?);
    let layer = find_layer_by_uid(&proj.sets[set].layers, uid).ok_or("layer uid not found")?;
    let procedural = layer
        .content
        .iter()
        .find_map(|e| match &e.kind {
            shard_model::EffectKind::Filter { procedural: Some(p) } => Some(p.clone()),
            _ => None,
        })
        .ok_or("the layer has no filter effect")?;
    let bitmaps = crate::bitmaps::ProjectBitmaps::open(&proj.path, 1024).ok_or("cannot open the project bitmaps")?;
    let maps = crate::compose::SetMaps::new(&bitmaps, &proj.bake_urls, &proj.sets[set].name, true, Some(&proj.mesh), set);
    let cov = raster::erode(&raster::coverage(&proj.mesh, set, res, res), res, res);
    let comps = |t: &Truth| -> Vec<Vec<f32>> { (0..3).map(|c| component(t, c, res)).collect() };
    let (ca, cb) = (comps(&a), comps(&b));
    let n = (res * res) as usize;
    let to_image = |srgb_in: bool| {
        let mut data = Vec::with_capacity(n * 3);
        for i in 0..n {
            for c in 0..3 {
                let v = ca[c][i] / 255.0;
                data.push(if srgb_in { v } else { shard_compose::srgb_to_linear(v) });
            }
        }
        shard_compose::Image { w: res, h: res, n: 3, data }
    };
    let mut out = Vec::new();
    for (label, srgb_in, decode_out) in [
        ("linear in, as is out (current)", false, false),
        ("sRGB in, decoded out", true, true),
        ("sRGB in, raw out (then it is an sRGB picture)", true, false),
        ("linear in, decoded out", false, true),
    ] {
        let img = to_image(srgb_in);
        let Some(r) = maps.render_graph(&procedural, None, res, res, Some(&img)) else {
            return Err("the graph could not be rendered (Substance engine missing?)".into());
        };
        // express the result as sRGB 0..255 for the comparison
        let ours: Vec<Vec<f32>> = (0..3)
            .map(|c| {
                (0..n)
                    .map(|i| {
                        let v = r.data[i * r.n + c.min(r.n - 1)].clamp(0.0, 1.0);
                        let srgb = match (srgb_in, decode_out) {
                            // the graph worked on linear numbers: encode for display
                            (false, false) => shard_compose::linear_to_srgb(v),
                            // sRGB numbers came out as sRGB numbers
                            (true, false) => v,
                            // decoded to linear, then shown encoded: the same picture as the raw sRGB result
                            (true, true) => v,
                            // linear in, the graph output taken as sRGB: shown as is
                            (false, true) => v,
                        };
                        srgb * 255.0
                    })
                    .collect()
            })
            .collect();
        let (mut mae, mut bias, mut vs_input) = (0.0, 0.0, 0.0);
        for c in 0..3 {
            let m = metrics(&pick(&ours[c], &cov), &pick(&cb[c], &cov));
            mae += m.mae / 3.0;
            bias += m.bias / 3.0;
            vs_input += metrics(&pick(&ours[c], &cov), &pick(&ca[c], &cov)).mae / 3.0;
        }
        out.push(FilterVariant { label, mae, bias, vs_input });
    }
    // no filter at all: how far apart the two pictures are to begin with
    let mut base = 0.0;
    for c in 0..3 {
        base += metrics(&pick(&ca[c], &cov), &pick(&cb[c], &cov)).mae / 3.0;
    }
    out.push(FilterVariant { label: "no filter (the two Painter pictures)", mae: base, bias: 0.0, vs_input: 0.0 });
    Ok(out)
}
