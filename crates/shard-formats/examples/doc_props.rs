//! Print the scalar document settings (`paint/document.bin`) of projects.
//!
//! usage: doc_props PROJECT.spp...

use shard_formats::Container;

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(mut c) = Container::open(&path) else { continue };
        let Ok(root) = c.decode("paint/document.bin") else { continue };
        let Some(o) = root.as_object() else { continue };
        let props: Vec<String> = ["normalMapSyscoord", "tangentSpaceMode", "colorManagementWorkflow", "projectWorkflow"]
            .iter()
            .map(|k| format!("{k}={:?}", o.get(*k).and_then(|v| v.as_i64())))
            .collect();
        println!("{path}: {}", props.join(" "));
    }
}
