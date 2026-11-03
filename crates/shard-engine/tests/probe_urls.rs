//! Diagnostic: list the resource urls of a project containing SHARD_URL_FILTER (SHARD_PROBE_SPP).
#[test]
fn list_urls() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let filter = std::env::var("SHARD_URL_FILTER").unwrap_or_default().to_ascii_lowercase();
    let b = shard_engine::bitmaps::ProjectBitmaps::open(std::path::Path::new(&p), 64).unwrap();
    for u in b.resource_urls() {
        if u.to_ascii_lowercase().contains(&filter) {
            println!("{u}");
        }
    }
}
