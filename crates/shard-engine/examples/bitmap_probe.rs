//! Decode the bitmap resources of a project whose name contains a word and print their format and statistics.
//!
//! usage: bitmap_probe PROJECT.spp WORD

use shard_engine::bitmaps::ProjectBitmaps;
use shard_formats::{image, Container};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (path, word) = (&a[0], a.get(1).map(String::as_str).unwrap_or("Curvature"));
    let pb = ProjectBitmaps::open(std::path::Path::new(path), 4096).expect("project");
    let mut c = Container::open(path).expect("open");
    for url in pb.resource_urls() {
        if !url.contains(word) {
            continue;
        }
        let hash = url.rsplit("version=").next().unwrap_or("").trim_end_matches(".image");
        let bytes = c.read(&format!("resources/{hash}.image")).or_else(|_| c.read(&format!("resources/{hash}")));
        let Ok(bytes) = bytes else {
            println!("{url}: member not found");
            continue;
        };
        match image::decode(&bytes) {
            Ok(img) => {
                let f = img.to_rgba_f32().unwrap_or_default();
                let n = (f.len() / 4).max(1);
                let mean: Vec<f32> = (0..4).map(|k| f.iter().skip(k).step_by(4).sum::<f32>() / n as f32).collect();
                println!("{url}: {}x{} {:?} ({} bytes) mean rgba {:?}", img.width, img.height, img.format, bytes.len(), mean);
            }
            Err(e) => println!("{url}: decode failed {e:?}"),
        }
    }
}
