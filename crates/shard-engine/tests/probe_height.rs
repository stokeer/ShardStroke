//! SHARD_PROBE_SPP (+ SHARD_SET, SHARD_RES): statistics of the composed Height and Normal channels.
use shard_engine::project::Project;

#[test]
fn height_stats() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let res: u32 = std::env::var("SHARD_RES").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let only = std::env::var("SHARD_SET").ok();
    let proj = Project::open(std::path::Path::new(&p)).unwrap();
    for (i, s) in proj.sets.iter().enumerate() {
        if only.as_deref().map_or(false, |o| o != s.name) {
            continue;
        }
        let comp = proj.compose(i, &[1, 22], res);
        for (bit, label) in [(1u32, "height"), (22, "normal")] {
            match comp.channels.get(&bit) {
                Some(img) => {
                    let v: Vec<f32> = img.data.iter().step_by(img.n).copied().collect();
                    let n = v.len() as f32;
                    let mean = v.iter().sum::<f32>() / n;
                    let sd = (v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n).sqrt();
                    let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |a, x| (a.0.min(*x), a.1.max(*x)));
                    println!("{} {label:7} min {lo:.4} max {hi:.4} mean {mean:.4} sd {sd:.4}", s.name);
                }
                None => println!("{} {label:7} none", s.name),
            }
        }
    }
}

/// SHARD_PROBE_SPP, SHARD_RECT="u0,v0,u1,v1" (image space, 0..1): statistics of the colour channels in a UV rectangle.
#[test]
fn rect_stats() {
    let (Ok(p), Ok(rect)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_RECT")) else { return };
    let r: Vec<f32> = rect.split(',').filter_map(|v| v.parse().ok()).collect();
    let res: u32 = std::env::var("SHARD_RES").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let proj = Project::open(std::path::Path::new(&p)).unwrap();
    let comp = proj.compose(0, &[0, 7, 13], res);
    for (bit, label) in [(0u32, "base"), (7, "rough"), (13, "metal")] {
        let img = &comp.channels[&bit];
        let (mut lo, mut hi, mut sum, mut n) = (f32::MAX, f32::MIN, 0.0f64, 0u64);
        let mut hist = [0u32; 8];
        for y in (r[1] * res as f32) as u32..(r[3] * res as f32) as u32 {
            for x in (r[0] * res as f32) as u32..(r[2] * res as f32) as u32 {
                let v = img.data[((y * res + x) as usize) * img.n];
                lo = lo.min(v);
                hi = hi.max(v);
                sum += v as f64;
                n += 1;
                hist[((v.clamp(0.0, 0.9999)) * 8.0) as usize] += 1;
            }
        }
        println!("{label:6} min {lo:.4} max {hi:.4} mean {:.4}  hist(8) {:?}", sum / n as f64, hist);
    }
}
