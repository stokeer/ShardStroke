//! Diagnostic: prints every bitmap source of a project (set `SHARD_PROBE_SPP`). Skipped otherwise.

use shard_model::import::import_project_stacks;
use shard_model::*;

fn walk_effects(es: &[Effect], layer: &str, where_: &str) {
    for e in es {
        match &e.kind {
            EffectKind::Fill(f) => {
                for s in &f.sources {
                    if let Source::Bitmap { resource, channels, opacity } = s {
                        println!(
                            "{layer} [{where_}] proj={:?} filt={} uv={:?} ch={:?} op={opacity} res={resource:?}",
                            f.projection, f.filtering, f.uv, channels
                        );
                    }
                }
            }
            EffectKind::Group { children } => walk_effects(children, layer, where_),
            _ => {}
        }
    }
}

fn walk(ls: &[Layer]) {
    for l in ls {
        walk_effects(&l.content, &l.name, "content");
        if let Some(m) = &l.mask {
            walk_effects(&m.effects, &l.name, "mask");
        }
        walk(&l.children);
    }
}

#[test]
fn probe() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    for st in import_project_stacks(p).unwrap() {
        println!("== {}", st.name);
        walk(&st.layers);
    }
}

fn describe(es: &[Effect]) -> String {
    es.iter()
        .map(|e| {
            let vis = if e.visible { "" } else { "(hidden)" };
            let blend = e.blend.first().map(|b| format!("{}@{}", b.mode.name(), b.opacity)).unwrap_or_default();
            let inner = match &e.kind {
                EffectKind::Fill(f) => format!(
                    "fill(effch={:?})[{}]",
                    e.channels.0,
                    f.sources
                        .iter()
                        .map(|s| match s {
                            Source::Procedural { procedural, .. } => format!("proc:{}", procedural.graph_name().unwrap_or("?")),
                            Source::Uniform { channels, color, opacity } => format!("uniform{:?}={:?}@{}", channels.0, color, opacity),
                            other => other.kind().to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                EffectKind::Group { children } => format!("group{{{}}}", describe(children)),
                EffectKind::Filter { procedural } => format!(
                    "filter:{}",
                    procedural.as_ref().and_then(|p| p.graph_name()).unwrap_or("?")
                ),
                other => e.type_name().to_string() + if matches!(other, EffectKind::Paint(_)) { "(strokes)" } else { "" },
            };
            format!("{inner}{vis}<{blend}>")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn tree(ls: &[Layer], depth: usize) {
    for l in ls {
        let mask = l
            .mask
            .as_ref()
            .map(|m| format!("MASK en={} init={} [{}]", m.enabled, m.initial, describe(&m.effects)))
            .unwrap_or_else(|| "no mask".into());
        let b = l.blend.iter().map(|b| format!("{}@{}:{:?}", b.mode.name(), b.opacity, b.channels)).collect::<Vec<_>>().join(";");
        println!(
            "{}{} {:?} vis={} blend={b} content=[{}] {mask}",
            "  ".repeat(depth),
            l.name,
            l.kind,
            l.visible,
            describe(&l.content)
        );
        tree(&l.children, depth + 1);
    }
}

#[test]
fn tree_of_set() {
    let (Ok(p), Ok(set)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_TREE")) else { return };
    for st in import_project_stacks(p).unwrap() {
        if st.name == set {
            tree(&st.layers, 0);
        }
    }
}

#[test]
fn order_heuristic() {
    use shard_model::import::import_file;
    let root = std::path::PathBuf::from(r"C:\Program Files\Adobe\Adobe Substance 3D Painter\resources\starter_assets");
    if !root.exists() {
        return;
    }
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("spsm")).unwrap_or(false) {
                    out.push(p);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    let (mut base_first, mut base_last, mut mask_first, mut mask_last) = (0, 0, 0, 0);
    let mut examples = Vec::new();
    for f in &files {
        let Ok(d) = import_file(f) else { continue };
        let names: Vec<String> = d.layers.iter().map(|l| l.name.to_ascii_lowercase()).collect();
        if names.len() < 3 {
            continue;
        }
        let is_base = |n: &String| n.contains("base") || n.contains("raw") || n.contains("metal") && !n.contains("dirt");
        let is_top = |n: &String| n.contains("dust") || n.contains("dirt") || n.contains("scratch") || n.contains("edge") || n.contains("wear");
        if is_base(names.first().unwrap()) {
            base_first += 1;
        }
        if is_base(names.last().unwrap()) {
            base_last += 1;
        }
        if is_top(names.first().unwrap()) {
            mask_first += 1;
        }
        if is_top(names.last().unwrap()) {
            mask_last += 1;
        }
        if examples.len() < 6 {
            examples.push(format!("{}: {:?}", f.file_name().unwrap().to_string_lossy(), names));
        }
    }
    println!("files={} base_first={base_first} base_last={base_last} wear_first={mask_first} wear_last={mask_last}", files.len());
    for e in examples {
        println!("{e}");
    }
}
