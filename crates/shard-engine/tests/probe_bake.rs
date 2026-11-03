//! Diagnostic: statistics of a baked mesh map of a project (SHARD_PROBE_SPP + SHARD_BAKE_HASH).
use shard_formats::Container;

#[test]
fn bake_stats() {
    let (Ok(p), Ok(hash)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_BAKE_HASH")) else { return };
    let mut c = Container::open(&p).unwrap();
    let bytes = c.read(&format!("resources/{hash}.image")).or_else(|_| c.read(&format!("resources/{hash}"))).unwrap();
    let img = shard_formats::image::decode(&bytes).unwrap();
    println!("{}x{} format {:?}", img.width, img.height, img.format);
    let f = img.to_rgba_f32().unwrap();
    let n = (img.width * img.height) as usize;
    for (c, name) in ["R", "G", "B", "A"].iter().enumerate() {
        let mut mn = f32::MAX;
        let mut mx = f32::MIN;
        let mut sum = 0f64;
        for i in 0..n {
            let v = f[i * 4 + c];
            mn = mn.min(v);
            mx = mx.max(v);
            sum += v as f64;
        }
        println!("{name}: min {mn:.4} max {mx:.4} mean {:.4}", sum / n as f64);
    }
    // corner / background texel
    println!("texel(0,0) = {:?}", &f[0..4]);
}

/// SHARD_PROBE_SPP + SHARD_BAKE_HASH + SHARD_BAKE_OUT: write the bake as an 8-bit gray PNG (first component).
#[test]
fn bake_png() {
    let (Ok(p), Ok(hash), Ok(out)) =
        (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_BAKE_HASH"), std::env::var("SHARD_BAKE_OUT"))
    else {
        return;
    };
    let mut c = Container::open(&p).unwrap();
    let bytes = c.read(&format!("resources/{hash}.image")).or_else(|_| c.read(&format!("resources/{hash}"))).unwrap();
    let img = shard_formats::image::decode(&bytes).unwrap();
    let f = img.to_rgba_f32().unwrap();
    let (w, h) = (img.width, img.height);
    let gray: Vec<u8> = (0..(w * h) as usize).map(|i| (f[i * 4].clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
    let file = std::fs::File::create(out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&gray).unwrap();
}
