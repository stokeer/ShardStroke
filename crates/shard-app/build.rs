//! Windows: the executable's icon (assets/icon.ico, made from docs/brand/icon.svg). Puts the Open Image Denoise
//! libraries (`third_party/oidn/bin`, loaded at run time by `oidn.rs`) next to the executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/app.rc", embed_resource::NONE).manifest_optional().unwrap();
    }

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/oidn/bin");
    println!("cargo:rerun-if-changed={}", src.display());
    let Ok(out) = std::env::var("OUT_DIR") else { return };
    // OUT_DIR is target/<profile>/build/<crate>-<hash>/out
    let Some(bin) = std::path::Path::new(&out).ancestors().nth(3) else { return };
    let Ok(entries) = std::fs::read_dir(&src) else { return };
    for e in entries.flatten() {
        let dst = bin.join(e.file_name());
        let newer = match (e.metadata().and_then(|m| m.modified()), dst.metadata().and_then(|m| m.modified())) {
            (Ok(a), Ok(b)) => a > b,
            _ => true,
        };
        if newer {
            let _ = std::fs::copy(e.path(), &dst);
        }
    }
}
