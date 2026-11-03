//! Offline fit of the polygon-fill click convention against masks exported by Painter.
//! SHARD_FIT_SPP, SHARD_FIT_SET, SHARD_FIT_LAYER (a layer whose mask has Decal effects), SHARD_FIT_MASK (png).
//! Scores every variant by the correlation of the replayed white clicks with the exported mask.
use shard_compose::StrokeRaster;
use shard_engine::{raster, strokes::SetStrokes};
use shard_model::{DecalHit, Effect, EffectKind, Layer};

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

fn corr(a: &[f32], b: &[f32], cov: &[bool]) -> f32 {
    let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for i in 0..a.len() {
        if !cov[i] {
            continue;
        }
        let (x, y) = (a[i] as f64, b[i] as f64);
        n += 1.0;
        sa += x;
        sb += y;
        saa += x * x;
        sbb += y * y;
        sab += x * y;
    }
    let (ma, mb) = (sa / n, sb / n);
    let (va, vb) = (saa / n - ma * ma, sbb / n - mb * mb);
    if va <= 1e-12 || vb <= 1e-12 {
        return f32::NAN;
    }
    ((sab / n - ma * mb) / (va.sqrt() * vb.sqrt())) as f32
}

#[test]
fn fit() {
    let (Ok(spp), Ok(set), Ok(layer), Ok(mask)) = (
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
    collect(&l.mask.as_ref().expect("mask").effects, &mut hits);
    println!("{} hits, {} white", hits.len(), hits.iter().filter(|h| h.color[0] > 0.5).count());

    let res = 1024usize;
    let img = image_gray(&mask, res);
    let cov = raster::coverage(&proj.mesh, si, res as u32, res as u32);
    let truth: Vec<f32> = img.iter().map(|v| if *v > 128.0 { 1.0 } else { 0.0 }).collect();
    let center = |h: &DecalHit| (h.position[0] - 0.5).abs() < 1e-3 && (h.position[1] - 0.5).abs() < 1e-3;
    // variants that start from a full mask and replay every click in order (white and black)
    let in_order: Vec<(&str, Vec<DecalHit>)> = vec![
        ("full start, all hits, no centre", hits.iter().filter(|h| !center(h)).cloned().collect()),
        ("full start, all hits", hits.to_vec()),
        ("full start, black no centre", hits.iter().filter(|h| h.color[0] <= 0.5 && !center(h)).cloned().collect()),
        ("full start, black all", hits.iter().filter(|h| h.color[0] <= 0.5).cloned().collect()),
        ("full start, reversed, no centre", hits.iter().rev().filter(|h| !center(h)).cloned().collect()),
    ];
    // hypothesis: a click at the exact centre of the view acts as an eraser whatever colour it carries
    let centre_erases: Vec<DecalHit> = hits
        .iter()
        .map(|h| {
            let mut h = h.clone();
            if center(&h) {
                h.color = [0.0; 3];
            }
            h
        })
        .collect();
    let centre_erases_keep: Vec<DecalHit> = centre_erases.iter().filter(|h| !center(h) || h.color[0] <= 0.5).cloned().collect();
    let mut in_order = in_order;
    in_order.push(("full start, centre clicks erase", centre_erases));
    in_order.push(("full start, centre clicks erase (kept)", centre_erases_keep));
    let variants: Vec<(&str, Vec<DecalHit>)> = vec![
        ("white centre only", hits.iter().filter(|h| h.color[0] > 0.5 && center(h)).cloned().collect()),
        ("white non-centre only (== no centre)", hits.iter().filter(|h| h.color[0] > 0.5 && !center(h)).cloned().collect()),
        ("all hits in order, start black", hits.to_vec()),
        ("black all", hits.iter().filter(|h| h.color[0] <= 0.5).cloned().collect()),
        ("white, no centre", hits.iter().filter(|h| h.color[0] > 0.5 && !center(h)).cloned().collect()),
        ("white, all", hits.iter().filter(|h| h.color[0] > 0.5).cloned().collect()),
        ("black, no centre", hits.iter().filter(|h| h.color[0] <= 0.5 && !center(h)).cloned().collect()),
        ("black, all", hits.iter().filter(|h| h.color[0] <= 0.5).cloned().collect()),
    ];
    let mut best: Vec<(f32, String)> = Vec::new();
    for aspect in [0.6f32, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.78, 2.0] {
        for (xo, yo) in [(0.0f32, 0.0f32), (0.05, 0.0), (-0.05, 0.0), (0.0, 0.05), (0.0, -0.05)] {
            for ydown in [false, true] {
                std::env::set_var("SHARD_DECAL_ASPECT", format!("{aspect}"));
                std::env::set_var("SHARD_DECAL_XOFF", format!("{xo}"));
                std::env::set_var("SHARD_DECAL_YOFF", format!("{yo}"));
                if ydown {
                    std::env::set_var("SHARD_DECAL_Y_DOWN", "1");
                } else {
                    std::env::remove_var("SHARD_DECAL_Y_DOWN");
                }
                for (name, hs) in &in_order {
                    let raster = SetStrokes::new(&proj.mesh, si, 1.0);
                    let mut buf = vec![1.0f32; res * res];
                    raster.decal(hs, &mut buf, res as u32, res as u32, 1.0);
                    let c = corr(&buf, &truth, &cov);
                    best.push((c, format!("{name}: aspect {aspect} off ({xo},{yo}) ydown {ydown}")));
                }
                for (name, hs) in &variants {
                    let raster = SetStrokes::new(&proj.mesh, si, 1.0);
                    let mut buf = vec![0.0f32; res * res];
                    raster.decal(hs, &mut buf, res as u32, res as u32, 1.0);
                    let c = corr(&buf, &truth, &cov);
                    best.push((c, format!("{name}: aspect {aspect} off ({xo},{yo}) ydown {ydown}")));
                }
            }
        }
    }
    best.retain(|(c, _)| !c.is_nan());
    best.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (c, n) in best.iter().take(12) {
        println!("corr {c:.3}  {n}");
    }
    for (c, n) in best.iter().filter(|(_, n)| n.contains("aspect 1 off (0,0) ydown false")) {
        println!("default-convention corr {c:.3}  {n}");
    }
    let base: Option<&(f32, String)> = None;
    // what each variant selects at the default convention: share of the covered area, and truth under it
    std::env::set_var("SHARD_DECAL_ASPECT", "1.0");
    std::env::set_var("SHARD_DECAL_XOFF", "0");
    std::env::set_var("SHARD_DECAL_YOFF", "0");
    std::env::remove_var("SHARD_DECAL_Y_DOWN");
    let covered = cov.iter().filter(|c| **c).count() as f32;
    for (name, hs) in variants.iter().chain(in_order.iter()) {
        let raster = SetStrokes::new(&proj.mesh, si, 1.0);
        let start = if name.starts_with("full") { 1.0 } else { 0.0 };
        let mut buf = vec![start; res * res];
        raster.decal(hs, &mut buf, res as u32, res as u32, 1.0);
        let (mut sel, mut tsum) = (0f32, 0f32);
        for i in 0..buf.len() {
            let on = if start == 1.0 { buf[i] < 0.5 } else { buf[i] > 0.5 };
            if cov[i] && on {
                sel += 1.0;
                tsum += truth[i];
            }
        }
        println!("{name}: {} hits, selects {:.1}% of the set, truth white under it {:.0}%", hs.len(), 100.0 * sel / covered, 100.0 * tsum / sel.max(1.0));
    }
    println!("default: {base:?}");
}

fn image_gray(path: &str, res: usize) -> Vec<f32> {
    let f = std::fs::File::open(path).expect("mask");
    let dec = png::Decoder::new(std::io::BufReader::new(f));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    let mut out = vec![0f32; res * res];
    for y in 0..res {
        for x in 0..res {
            let (sx, sy) = (x * w / res, y * h / res);
            out[y * res + x] = buf[(sy * w + sx) * ch] as f32;
        }
    }
    out
}
