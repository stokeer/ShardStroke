//! Offline fit of a non-UV projection against a mask exported by Painter.
//! SHARD_FIT_SPP, SHARD_FIT_SET, SHARD_FIT_LAYER, SHARD_FIT_MASK.
use shard_compose::MeshMaps;
use shard_engine::{bitmaps::ProjectBitmaps, compose::SetMaps, measure, raster};
use shard_model::{Effect, EffectKind, Layer, Source};

fn find_fill<'a>(es: &'a [Effect]) -> Option<(&'a shard_model::Fill, &'a shard_model::Procedural)> {
    for e in es {
        match &e.kind {
            EffectKind::Fill(f) => {
                for s in &f.sources {
                    if let Source::Procedural { procedural, .. } = s {
                        return Some((f, procedural));
                    }
                }
            }
            EffectKind::Group { children } => {
                if let Some(r) = find_fill(children) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

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
    let path = std::path::Path::new(&spp);
    let proj = shard_engine::project::Project::open(path).unwrap();
    let si = proj.sets.iter().position(|s| s.name == set).expect("set");
    let l = find_layer(&proj.sets[si].layers, &layer).expect("layer");
    let (fill, p) = find_fill(&l.mask.as_ref().expect("mask").effects).expect("fill with a graph");
    println!("graph {:?} projection {:?} projector {:?} uv {:?}", p.graph_name(), fill.projection, fill.projector, fill.uv);

    let res = 1024u32;
    let bitmaps = ProjectBitmaps::open(path, 1024).unwrap();
    let maps = SetMaps::new(&bitmaps, &proj.bake_urls, &set, true, Some(&proj.mesh), si);
    let (rw, rh) = shard_compose::render_size(p, res, res).expect("size");
    let img = maps.render_graph(p, None, rw, rh, None).expect("render");
    let surf = maps.surface(res, res).expect("surface");
    let cov = raster::erode(&raster::coverage(&proj.mesh, si, res, res), res, res);

    // truth, 2x2 averaged to the compose size
    let t = measure::read_png(std::path::Path::new(&mask)).unwrap();
    let f = (t.w / res) as usize;
    let truth: Vec<f32> = (0..(res * res) as usize)
        .map(|i| {
            let (x, y) = (i % res as usize, i / res as usize);
            let mut a = 0.0;
            for dy in 0..f {
                for dx in 0..f {
                    a += t.rgba[((y * f + dy) * t.w as usize + x * f + dx) * 4] as f32;
                }
            }
            a / (f * f) as f32 / 255.0
        })
        .collect();

    let nearest = |u: f32, v: f32| -> f32 {
        let (u, v) = (u - u.floor(), v - v.floor());
        let x = ((u * img.w as f32) as usize).min(img.w as usize - 1);
        let y = ((v * img.h as f32) as usize).min(img.h as usize - 1);
        img.data[(y * img.w as usize + x) * img.n]
    };
    let corr = |a: &[f32], b: &[f32]| -> f32 {
        let n = a.len() as f32;
        let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
        let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            sab += (x - ma) * (y - mb);
            saa += (x - ma) * (x - ma);
            sbb += (y - mb) * (y - mb);
        }
        sab / (saa.sqrt() * sbb.sqrt() + 1e-9)
    };

    if std::env::var_os("SHARD_FIT_GRAD").is_some() {
        // Gradient layer: truth = ramp(world) * clouds(UV). Bin truth / clouds by world y (and z).
        let effs = &l.mask.as_ref().unwrap().effects;
        let mut fills: Vec<(&shard_model::Fill, &shard_model::Procedural)> = Vec::new();
        fn collect<'a>(es: &'a [Effect], out: &mut Vec<(&'a shard_model::Fill, &'a shard_model::Procedural)>) {
            for e in es {
                match &e.kind {
                    EffectKind::Fill(f) => {
                        for s in &f.sources {
                            if let Source::Procedural { procedural, .. } = s {
                                out.push((f, procedural));
                            }
                        }
                    }
                    EffectKind::Group { children } => collect(children, out),
                    _ => {}
                }
            }
        }
        collect(effs, &mut fills);
        let (cf, cp) = fills.iter().find(|(_, p)| p.graph_name() == Some("clouds_2")).expect("clouds");
        let (cw, ch) = shard_compose::render_size(cp, res, res).unwrap();
        let cimg = maps.render_graph(cp, None, cw, ch, None).unwrap();
        let sc = cf.uv.scale.unwrap_or([1.0, 1.0]);
        let clouds = |x: usize, y: usize| -> f32 {
            let (u, v) = ((x as f32 + 0.5) / res as f32, (y as f32 + 0.5) / res as f32);
            let (u, v) = ((u - 0.5) * sc[0] + 0.5, (v - 0.5) * sc[1] + 0.5);
            let (u, v) = (u - u.floor(), v - v.floor());
            let xi = ((u * cimg.w as f32) as usize).min(cimg.w as usize - 1);
            let yi = ((v * cimg.h as f32) as usize).min(cimg.h as usize - 1);
            cimg.data[(yi * cimg.w as usize + xi) * cimg.n]
        };
        // our planar ramp against the observed ratio
        {
            let (gf, gp) = fills.iter().find(|(_, p)| p.graph_name() == Some("gradient_linear_1")).expect("gradient");
            let (gw, gh) = shard_compose::render_size(gp, res, res).unwrap();
            let gimg = maps.render_graph(gp, None, gw, gh, None).unwrap();
            let spec = shard_compose::ProjSpec::of(gf);
            let xf = shard_compose::UvXform::new(&gf.uv);
            let mut bins = vec![(0.0f64, 0usize); 10];
            let mut worst: Vec<(f32, usize)> = Vec::new();
            for i in 0..(res * res) as usize {
                if !cov[i] || !surf.covered[i] {
                    continue;
                }
                let c = clouds(i % res as usize, i / res as usize);
                if c < 0.5 {
                    continue;
                }
                let r = shard_compose::project_sample(&gimg, 0, 1, &surf, i, &xf, &spec).unwrap_or(0.0);
                let b = ((r * 10.0) as usize).min(9);
                bins[b].0 += (truth[i] / c) as f64;
                bins[b].1 += 1;
                if r > 0.38 && r < 0.62 && truth[i] / c < 0.1 {
                    worst.push((surf.pos[i][1], i));
                }
            }
            println!("ratio truth/clouds by our planar ramp value:");
            for (b, (sum, n)) in bins.iter().enumerate() {
                if *n > 0 {
                    println!("   ramp {:.1}  truth/clouds {:.2} (n {})", b as f32 / 10.0, sum / *n as f64, n);
                }
            }
            let n = worst.len();
            if n > 0 {
                let ys: Vec<f32> = worst.iter().map(|w| w.0).collect();
                let (lo, hi) = ys.iter().fold((f32::MAX, f32::MIN), |a, y| (a.0.min(*y), a.1.max(*y)));
                println!("texels where our ramp > 0.8 but truth ~ 0: {n}, world y in [{lo:.2}, {hi:.2}]");
                let sample: Vec<String> = worst.iter().step_by((n / 8).max(1)).take(8).map(|(_, i)| {
                    let p = surf.pos[*i];
                    format!("({:.2},{:.2},{:.2}) n=({:.2},{:.2},{:.2})", p[0], p[1], p[2], surf.nrm[*i][0], surf.nrm[*i][1], surf.nrm[*i][2])
                }).collect();
                println!("   samples: {}", sample.join("  "));
            }
        }
        for axis in [1usize, 2] {
            let mut bins = vec![(0.0f64, 0usize); 24];
            let (lo, hi) = (surf.lo[axis], surf.hi[axis]);
            for i in 0..(res * res) as usize {
                if !cov[i] || !surf.covered[i] {
                    continue;
                }
                let c = clouds(i % res as usize, i / res as usize);
                if c < 0.5 {
                    continue;
                }
                let b = (((surf.pos[i][axis] - lo) / (hi - lo)) * 24.0).clamp(0.0, 23.0) as usize;
                bins[b].0 += (truth[i] / c) as f64;
                bins[b].1 += 1;
            }
            println!("ratio truth/clouds by world {} (lo {lo:.2} hi {hi:.2}):", ["x", "y", "z"][axis]);
            for (b, (sum, n)) in bins.iter().enumerate() {
                if *n > 50 {
                    println!("   {:+.2}  ratio {:.2} (n {})", lo + (hi - lo) * (b as f32 + 0.5) / 24.0, sum / *n as f64, n);
                }
            }
        }
        return;
    }
    if std::env::var_os("SHARD_FIT_HALF").is_some() {
        // S = 2 and offset 0.5 are known: fit swap and signs for each half axis
        for axis in 0..3usize {
            for sign in [1.0f32, -1.0] {
                let mut sel: Vec<usize> = (0..(res * res) as usize)
                    .filter(|&i| cov[i] && surf.covered[i] && surf.nrm[i][axis] * sign > 0.95)
                    .collect();
                if sel.len() < 500 {
                    println!("half axis {axis}{} : {} texels", if sign > 0.0 { "+" } else { "-" }, sel.len());
                    continue;
                }
                let step = (sel.len() / 15000).max(1);
                sel = sel.into_iter().step_by(step).collect();
                let tv: Vec<f32> = sel.iter().map(|&i| truth[i]).collect();
                let (c1, c2) = match axis {
                    0 => (1, 2),
                    1 => (0, 2),
                    _ => (0, 1),
                };
                let mut rows: Vec<(f32, String)> = Vec::new();
                for swap in 0..2 {
                    for (s1, s2) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                        let pred: Vec<f32> = sel
                            .iter()
                            .map(|&i| {
                                let p = surf.pos[i];
                                let (a, b) = (p[c1], p[c2]);
                                let (a, b) = if swap == 1 { (b, a) } else { (a, b) };
                                nearest(s1 * a / 2.0 + 0.5, s2 * b / 2.0 + 0.5)
                            })
                            .collect();
                        rows.push((corr(&pred, &tv), format!("swap={swap} signs=({s1},{s2})")));
                    }
                }
                rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                println!(
                    "half axis {axis}{} ({} texels): best {:.3} {} | next {:.3} {}",
                    if sign > 0.0 { "+" } else { "-" },
                    sel.len(),
                    rows[0].0,
                    rows[0].1,
                    rows[1].0,
                    rows[1].1
                );
            }
        }
        return;
    }
    for axis in 0..3usize {
        // texels facing along `axis`, surface coordinates in the other two
        let mut sel: Vec<usize> = (0..(res * res) as usize)
            .filter(|&i| cov[i] && surf.covered[i] && surf.nrm[i][axis].abs() > 0.95 && truth[i] >= 0.0)
            .collect();
        if sel.len() < 2000 {
            println!("axis {axis}: only {} texels", sel.len());
            continue;
        }
        let step = (sel.len() / 12000).max(1);
        sel = sel.into_iter().step_by(step).collect();
        let tv: Vec<f32> = sel.iter().map(|&i| truth[i]).collect();
        let (c1, c2) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let mut best: Vec<(f32, String)> = Vec::new();
        for &s in &[0.5f32, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0] {
            for swap in 0..2 {
                for (s1, s2) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                    for oxi in 0..24 {
                        for oyi in 0..24 {
                            let (ox, oy) = (oxi as f32 / 24.0, oyi as f32 / 24.0);
                            let pred: Vec<f32> = sel
                                .iter()
                                .map(|&i| {
                                    let p = surf.pos[i];
                                    let (a, b) = (p[c1], p[c2]);
                                    let (a, b) = if swap == 1 { (b, a) } else { (a, b) };
                                    nearest(s1 * a / s + ox, s2 * b / s + oy)
                                })
                                .collect();
                            let c = corr(&pred, &tv);
                            if best.len() < 6 || c > best[best.len() - 1].0 {
                                best.push((c, format!("S={s} swap={swap} signs=({s1},{s2}) off=({ox:.3},{oy:.3})")));
                                best.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                                best.truncate(6);
                            }
                        }
                    }
                }
            }
        }
        println!("axis {axis} ({} texels):", sel.len());
        for (c, d) in best {
            println!("   corr {c:.3}  {d}");
        }
    }
}
