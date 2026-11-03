//! Texture sets of a project with their `sceneMaterialUID` and the texture-cache channels found for it.
//!
//! usage: set_uids PROJECT.spp

fn main() {
    let path = std::env::args().nth(1).expect("usage: set_uids PROJECT.spp");
    let stacks = shard_model::import::import_project_stacks(&path).expect("import");
    let mut c = shard_formats::Container::open(&path).expect("open");
    for s in &stacks {
        let ch: Vec<u32> = c.texture_cache_channels(s.uid).into_iter().map(|(k, _)| k).collect();
        println!("{:<28} uid {:<22} cache {:?}", s.name, s.uid, ch);
    }
}
