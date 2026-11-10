use shard_formats::Container;
/// Dump every .sbsar of a project with its member list and file sizes (SHARD_PROBE_SPP, SHARD_OUT_DIR).
#[test]
fn raw_head() {
    let (Ok(p), Ok(out)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_OUT_DIR")) else { return };
    let mut c = Container::open(&p).unwrap();
    let names: Vec<String> = c.members().map(|d| d.path.clone()).filter(|n| n.ends_with(".sbsar")).collect();
    for n in names {
        let b = c.read(&n).unwrap();
        let Ok(files) = shard_formats::sbsar::extract_content(&b) else { continue };
        for (name, data) in files {
            let short = name.rsplit('/').next().unwrap().to_string();
            if short.ends_with(".sbsasm") {
                std::fs::write(std::path::Path::new(&out).join(&short), &data).unwrap();
            }
        }
    }
}
