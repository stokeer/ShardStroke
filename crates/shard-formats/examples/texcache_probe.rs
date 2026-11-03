//! Decode every texture-cache member of a project and print its statistics.
//!
//! usage: texcache_probe PROJECT.spp

use shard_formats::{texcache, Container};

fn main() {
    let path = std::env::args().nth(1).expect("usage: texcache_probe PROJECT.spp");
    let mut c = Container::open(&path).expect("open");
    let members: Vec<String> = c
        .members()
        .filter(|d| d.path.starts_with("texture-cache/") && !d.path.ends_with("/list"))
        .map(|d| d.path.clone())
        .collect();
    for m in members {
        let t0 = std::time::Instant::now();
        let Ok(b) = c.read(&m) else { continue };
        match texcache::decode(&b) {
            Some(img) => {
                let n = img.format.comps();
                let px = (img.width * img.height) as usize;
                let stats: Vec<String> = (0..n)
                    .map(|k| {
                        let (mut lo, mut hi, mut sum) = (f32::MAX, f32::MIN, 0.0f64);
                        for i in 0..px {
                            let v = img.get(i, k);
                            lo = lo.min(v);
                            hi = hi.max(v);
                            sum += v as f64;
                        }
                        format!("{lo:.4}/{:.4}/{hi:.4}", sum / px as f64)
                    })
                    .collect();
                println!("{m} {:?} {}x{} {:?} in {:?}", img.format, img.width, img.height, stats, t0.elapsed());
            }
            None => println!("{m}: not decoded ({:?})", texcache::parse_header(&b)),
        }
    }
}
