use std::path::Path;

use shard_formats::image::{decode, ImageError, PixelFormat};
use shard_formats::Container;

#[test]
fn project_images_decode() {
    let p = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("vehicle.spp")).unwrap_or_default();
    if !p.exists() {
        return;
    }
    let mut c = Container::open(p).unwrap();
    let names: Vec<String> = c
        .members()
        .filter(|d| d.path.starts_with("resources/") && d.path.ends_with(".image") && !d.path.contains(".alg_meta"))
        .map(|d| d.path.clone())
        .collect();
    assert!(names.len() >= 60, "found {} images", names.len());
    let (mut ok, mut unsupported) = (0, 0);
    let mut fmts = std::collections::BTreeMap::<String, usize>::new();
    for n in names {
        let b = c.read(&n).unwrap();
        match decode(&b) {
            Ok(img) => {
                assert!(img.width > 0 && img.height > 0);
                let rgba = img.to_rgba8().unwrap();
                assert_eq!(rgba.len(), (img.width * img.height * 4) as usize);
                *fmts.entry(format!("{:?}", img.format)).or_default() += 1;
                ok += 1;
            }
            Err(ImageError::Unsupported(_)) => unsupported += 1,
            Err(e) => panic!("{n}: {e}"),
        }
    }
    eprintln!("images ok={ok} unsupported={unsupported} formats={fmts:?}");
    assert_eq!(ok, 67); // 66 lz4 + the single jp2
    assert_eq!(unsupported, 0); // the single jp2 (JPEG 2000) image decodes since the jp2 tile reader
    assert!(fmts.contains_key(&format!("{:?}", PixelFormat::Rgba16F)));
}
