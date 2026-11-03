//! Diagnostic: procedural usage of a project (set `SHARD_PROBE_SPP`). Skipped otherwise.

use std::collections::BTreeMap;

use shard_model::import::import_project_stacks;
use shard_model::*;

#[derive(Default)]
struct Use {
    count: usize,
    as_content: usize,
    as_mask: usize,
    as_filter: usize,
    tweaks: BTreeMap<String, usize>,
    outputs: BTreeMap<String, usize>,
    inputs: usize,
}

fn add(m: &mut BTreeMap<String, Use>, p: &Procedural, role: u8) {
    if std::env::var("SHARD_DUMP").map(|v| Some(v.as_str()) == p.graph_name()).unwrap_or(false) {
        println!("TWEAKS {:?} outputs {:?}", p.tweaks, p.outputs.iter().map(|o| &o.id).collect::<Vec<_>>());
    }
    let u = m.entry(p.graph_name().unwrap_or("?").to_string()).or_default();
    u.count += 1;
    match role {
        0 => u.as_content += 1,
        1 => u.as_mask += 1,
        _ => u.as_filter += 1,
    }
    for k in p.tweaks.keys() {
        *u.tweaks.entry(k.clone()).or_default() += 1;
    }
    for o in &p.outputs {
        *u.outputs.entry(o.id.clone()).or_default() += 1;
    }
    u.inputs += p.input_sources.len();
}

fn effects(es: &[Effect], role: u8, m: &mut BTreeMap<String, Use>) {
    for e in es {
        match &e.kind {
            EffectKind::Fill(f) => {
                for s in &f.sources {
                    if let Source::Procedural { procedural, .. } = s {
                        add(m, procedural, role);
                    }
                }
            }
            EffectKind::Filter { procedural: Some(p) } => add(m, p, 2),
            EffectKind::Group { children } => effects(children, role, m),
            _ => {}
        }
    }
}

fn walk(ls: &[Layer], m: &mut BTreeMap<String, Use>) {
    for l in ls {
        effects(&l.content, 0, m);
        if let Some(mask) = &l.mask {
            effects(&mask.effects, 1, m);
        }
        walk(&l.children, m);
    }
}

#[test]
fn probe() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut m = BTreeMap::new();
    for st in import_project_stacks(p).unwrap() {
        walk(&st.layers, &mut m);
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by_key(|(_, u)| std::cmp::Reverse(u.count));
    for (name, u) in v.iter().take(40) {
        println!(
            "{:4}  {name}  content={} mask={} filter={} inputs={} outputs={:?}",
            u.count, u.as_content, u.as_mask, u.as_filter, u.inputs, u.outputs
        );
        let mut t: Vec<_> = u.tweaks.iter().collect();
        t.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        println!("      tweaks: {:?}", t.iter().take(8).map(|(k, c)| format!("{k}x{c}")).collect::<Vec<_>>());
    }
    println!("distinct graphs: {}", v.len());
}

fn count_strokes(es: &[Effect], acc: &mut (usize, usize)) {
    for e in es {
        match &e.kind {
            EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => {
                acc.0 += 1;
                acc.1 += a.strokes.len();
            }
            EffectKind::Group { children } => count_strokes(children, acc),
            _ => {}
        }
    }
}

fn layers_with_strokes(ls: &[Layer], name: &str, out: &mut Vec<String>) {
    for l in ls {
        let mut acc = (0, 0);
        count_strokes(&l.content, &mut acc);
        if let Some(m) = &l.mask {
            count_strokes(&m.effects, &mut acc);
        }
        if acc.1 > 0 {
            out.push(format!("{name}: '{}' effects={} strokes={}", l.name, acc.0, acc.1));
        }
        layers_with_strokes(&l.children, name, out);
    }
}

#[test]
fn stroke_usage() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut out = Vec::new();
    let (mut layers, mut strokes) = (0usize, 0usize);
    for st in import_project_stacks(p).unwrap() {
        let mut v = Vec::new();
        layers_with_strokes(&st.layers, &st.name, &mut v);
        for line in &v {
            let n: usize = line.rsplit("strokes=").next().and_then(|x| x.parse().ok()).unwrap_or(0);
            strokes += n;
        }
        layers += v.len();
        out.extend(v);
    }
    for l in out.iter().take(40) {
        println!("{l}");
    }
    println!("layers with painted strokes: {layers}, total strokes: {strokes}");
}

fn collect_paint<'a>(es: &'a [Effect], out: &mut Vec<&'a PaintAction>) {
    for e in es {
        match &e.kind {
            EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => out.push(a),
            EffectKind::Group { children } => collect_paint(children, out),
            _ => {}
        }
    }
}

fn collect_layers<'a>(ls: &'a [Layer], out: &mut Vec<&'a PaintAction>) {
    for l in ls {
        collect_paint(&l.content, out);
        if let Some(m) = &l.mask {
            collect_paint(&m.effects, out);
        }
        collect_layers(&l.children, out);
    }
}

#[test]
fn stroke_stats() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut actions = Vec::new();
    let stacks = import_project_stacks(p).unwrap();
    for st in &stacks {
        collect_layers(&st.layers, &mut actions);
    }
    let (mut n_strokes, mut n_keys) = (0, 0);
    let (mut minp, mut maxp) = ([f32::MAX; 2], [f32::MIN; 2]);
    let mut kinds = BTreeMap::new();
    let mut view_types = BTreeMap::new();
    let mut sizes = BTreeMap::new();
    let mut spaces = BTreeMap::new();
    let mut hard = BTreeMap::new();
    let mut spacing = BTreeMap::new();
    let mut flows = BTreeMap::new();
    let mut projs: BTreeMap<String, usize> = BTreeMap::new();
    let mut classes = BTreeMap::new();
    let mut shapes = BTreeMap::new();
    let mut tmax = 0.0f32;
    let mut pressured = 0;
    for a in &actions {
        let b = &a.brush;
        *classes.entry(b.class.clone()).or_insert(0usize) += 1;
        *sizes.entry(format!("{:.4}", b.size)).or_insert(0usize) += 1;
        *spaces.entry(b.size_space).or_insert(0usize) += 1;
        *hard.entry(format!("{:?}", b.hardness())).or_insert(0usize) += 1;
        *spacing.entry(format!("{:.2}", b.spacing)).or_insert(0usize) += 1;
        *flows.entry(format!("{:.2}", b.flow)).or_insert(0usize) += 1;
        *shapes.entry(b.shape.as_ref().and_then(|s| s.graph_name()).unwrap_or("-").to_string()).or_insert(0usize) += 1;
        for s in &a.strokes {
            n_strokes += 1;
            *view_types.entry(s.view_type).or_insert(0usize) += 1;
            if !s.pressure.is_empty() {
                pressured += 1;
            }
            let p = &s.projection;
            *projs.entry(format!("fx={:.3} fy={:.3} m22={:.4} m32={:.4}", p[0], p[5], p[10], p[14])).or_insert(0) += 1;
            for k in &s.keys {
                n_keys += 1;
                *kinds.entry(k.kind).or_insert(0usize) += 1;
                for i in 0..2 {
                    minp[i] = minp[i].min(k.pos[i]);
                    maxp[i] = maxp[i].max(k.pos[i]);
                }
                tmax = tmax.max(k.time);
            }
        }
    }
    println!("actions={} strokes={n_strokes} keys={n_keys} pressured_strokes={pressured}", actions.len());
    println!("key pos range x[{:.3},{:.3}] y[{:.3},{:.3}] max time {tmax:.1}s", minp[0], maxp[0], minp[1], maxp[1]);
    println!("key kinds {kinds:?}\nview types {view_types:?}");
    println!("brush classes {classes:?}\nshapes {shapes:?}\nsize_space {spaces:?}\nhardness {hard:?}\nspacing {spacing:?}\nflow {flows:?}");
    println!("sizes {sizes:?}");
    println!("projections:");
    for (k, v) in &projs {
        println!("  {v:5}  {k}");
    }
}

#[test]
fn brush_size_distribution() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut actions = Vec::new();
    let stacks = import_project_stacks(p).unwrap();
    for st in &stacks {
        collect_layers(&st.layers, &mut actions);
    }
    let mut sizes: Vec<f32> = actions.iter().map(|a| a.brush.size).collect();
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |f: f32| sizes[((sizes.len() - 1) as f32 * f) as usize];
    println!("n={} min={:.4} p10={:.4} p50={:.4} p90={:.4} max={:.4}", sizes.len(), q(0.0), q(0.1), q(0.5), q(0.9), q(1.0));
    // size vs stroke extent: bounding box of key positions per action
    let mut rows = Vec::new();
    for a in &actions {
        for s in a.strokes.iter().take(1) {
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for k in &s.keys {
                for i in 0..2 {
                    lo[i] = lo[i].min(k.pos[i]);
                    hi[i] = hi[i].max(k.pos[i]);
                }
            }
            let fy = s.projection[5];
            rows.push((a.brush.size, hi[0] - lo[0], hi[1] - lo[1], fy, a.brush.spacing, s.keys.len()));
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("largest brushes: size, extent_x, extent_y, proj_fy, spacing, keys");
    for r in rows.iter().take(8) {
        println!("  {:.4} {:.3} {:.3} {:.2} {:.2} {}", r.0, r.1, r.2, r.3, r.4, r.5);
    }
    println!("smallest:");
    for r in rows.iter().rev().take(4) {
        println!("  {:.4} {:.3} {:.3} {:.2} {:.2} {}", r.0, r.1, r.2, r.3, r.4, r.5);
    }
}

#[test]
fn layer_brush_dump() {
    let (Ok(p), Ok(name)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_LAYER")) else { return };
    fn find<'a>(ls: &'a [Layer], name: &str, out: &mut Vec<&'a Layer>) {
        for l in ls {
            if l.name == name {
                out.push(l);
            }
            find(&l.children, name, out);
        }
    }
    let stacks = import_project_stacks(p).unwrap();
    for st in &stacks {
        let mut v = Vec::new();
        find(&st.layers, &name, &mut v);
        for l in v {
            let mut acts = Vec::new();
            if let Some(m) = &l.mask {
                collect_paint(&m.effects, &mut acts);
            }
            collect_paint(&l.content, &mut acts);
            for a in acts {
                let b = &a.brush;
                println!(
                    "action: size={} space={} flow={} spacing={} cull={}({}) hardness={:?} opacity={} color={:?} strokes={}",
                    b.size, b.size_space, b.flow, b.spacing, b.culling_enabled, b.culling_angle, b.hardness(), a.opacity,
                    a.color.iter().map(|s| s.kind()).collect::<Vec<_>>(), a.strokes.len()
                );
                for s in a.strokes.iter().take(2) {
                    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                    for k in &s.keys {
                        for i in 0..2 {
                            lo[i] = lo[i].min(k.pos[i]);
                            hi[i] = hi[i].max(k.pos[i]);
                        }
                    }
                    println!("   stroke keys={} x[{:.2},{:.2}] y[{:.2},{:.2}] view_type={}", s.keys.len(), lo[0], hi[0], lo[1], hi[1], s.view_type);
                }
            }
        }
    }
}

#[test]
fn filter_tweaks_of_layer() {
    let (Ok(p), Ok(name)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_LAYER")) else { return };
    fn find<'a>(ls: &'a [Layer], name: &str, out: &mut Vec<&'a Layer>) {
        for l in ls {
            if l.name == name {
                out.push(l);
            }
            find(&l.children, name, out);
        }
    }
    fn dump(es: &[Effect]) {
        for e in es {
            match &e.kind {
                EffectKind::Filter { procedural: Some(p) } => {
                    println!("  filter {:?} tweaks={:?}", p.graph_name(), p.tweaks);
                }
                EffectKind::Group { children } => dump(children),
                _ => {}
            }
        }
    }
    for st in &import_project_stacks(p).unwrap() {
        let mut v = Vec::new();
        find(&st.layers, &name, &mut v);
        for l in v {
            println!("{}: layer '{}'", st.name, l.name);
            dump(&l.content);
        }
    }
}

#[test]
fn mask_effects_of_layer() {
    let (Ok(p), Ok(name)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_LAYER")) else { return };
    fn find<'a>(ls: &'a [Layer], name: &str, out: &mut Vec<&'a Layer>) {
        for l in ls {
            if l.name == name {
                out.push(l);
            }
            find(&l.children, name, out);
        }
    }
    fn dump(es: &[Effect], depth: usize) {
        for (i, e) in es.iter().enumerate() {
            let pad = "  ".repeat(depth + 1);
            let blend: Vec<_> = e.blend.iter().map(|b| (b.mode.0, b.opacity, b.channels.0)).collect();
            println!("{pad}#{i} uid={} visible={} label={:?} blend(mode,op,ch)={:?}", e.uid, e.visible, e.label, blend);
            match &e.kind {
                EffectKind::Fill(f) => {
                    for s in &f.sources {
                        if let shard_model::Source::Procedural { procedural: p, opacity, .. } = s {
                            println!("{pad}   proc {:?} opacity={opacity} inputs={:?}", p.graph_name(), p.inputs);
                            for (k, v) in &p.tweaks {
                                println!("{pad}     {k} = {v:?}");
                            }
                        } else {
                            let ch = match s {
                                shard_model::Source::Uniform { channels, color, opacity } => format!("channels={:#x} color={:?} opacity={opacity}", channels.0, color),
                                shard_model::Source::Bitmap { channels, resource, opacity } => format!("channels={:#x} bitmap={:?} opacity={opacity}", channels.0, resource.as_deref().map(|r| r.split('?').next().unwrap_or(r))),
                                shard_model::Source::Reference { channels, target, opacity, .. } => format!("channels={:#x} reference->{target} opacity={opacity}", channels.0),
                                _ => String::new(),
                            };
                            println!("{pad}   src {} {ch}", s.kind());
                        }
                    }
                }
                EffectKind::Filter { procedural: Some(p) } => {
                    println!("{pad}   filter {:?} tweaks={:?}", p.graph_name(), p.tweaks);
                }
                EffectKind::Group { children } => dump(children, depth + 1),
                EffectKind::Levels(lv) => println!("{pad}   levels {:?}", lv),
                other => println!("{pad}   {:?}", std::mem::discriminant(other)),
            }
        }
    }
    for st in &import_project_stacks(p).unwrap() {
        let mut v = Vec::new();
        find(&st.layers, &name, &mut v);
        for l in v {
            println!("{}: layer '{}' blend={:?}", st.name, l.name, l.blend.iter().map(|b| (b.mode.0, b.opacity)).collect::<Vec<_>>());
            if let Some(m) = &l.mask {
                println!(" mask enabled={} initial={}", m.enabled, m.initial);
                dump(&m.effects, 0);
            }
            if std::env::var_os("SHARD_CONTENT").is_some() {
                println!(" content:");
                dump(&l.content, 0);
            }
        }
    }
}

#[test]
fn stack_tree() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    fn eff(es: &[Effect]) -> String {
        es.iter()
            .map(|e| {
                let b = e.blend.first().map(|b| format!("@{}", b.mode.0)).unwrap_or_default();
                let v = if e.visible { "" } else { "(off)" };
                let k = match &e.kind {
                    EffectKind::Fill(f) => f
                        .sources
                        .iter()
                        .map(|s| match s {
                            shard_model::Source::Procedural { procedural, .. } => procedural.graph_name().unwrap_or("?").to_string(),
                            other => other.kind().to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join("+"),
                    EffectKind::Filter { procedural } => format!("filter:{}", procedural.as_ref().and_then(|p| p.graph_name()).unwrap_or("?")),
                    EffectKind::Group { children } => format!("grp[{}]", eff(children)),
                    EffectKind::Paint(a) => format!("paint{}", a.strokes.len()),
                    EffectKind::Eraser(a) => format!("erase{}", a.strokes.len()),
                    EffectKind::Levels(_) => "levels".into(),
                    EffectKind::Anchor { .. } => "anchor".into(),
                    EffectKind::Decal(_) => "decal".into(),
                    other => format!("{:?}", std::mem::discriminant(other)),
                };
                format!("{k}{b}{v}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
    fn walk(ls: &[Layer], d: usize) {
        for l in ls {
            let pad = "  ".repeat(d);
            let mask = l.mask.as_ref().map(|m| format!(" MASK(en={} init={})[{}]", m.enabled, m.initial, eff(&m.effects))).unwrap_or_default();
            let b: Vec<_> = l.blend.iter().map(|b| (b.mode.0, b.opacity)).collect();
            println!("{pad}{:?} '{}' vis={} blend={:?} content[{}]{}", l.kind, l.name, l.visible, b, eff(&l.content), mask);
            walk(&l.children, d + 1);
        }
    }
    for st in &import_project_stacks(p).unwrap() {
        println!("== {}", st.name);
        walk(&st.layers, 0);
    }
}

#[test]
fn fill_uv_values() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    fn walk_effects(es: &[Effect], layer: &str, out: &mut Vec<String>) {
        for e in es {
            match &e.kind {
                EffectKind::Fill(f) => {
                    let u = &f.uv;
                    let non_default = u.scale.is_some_and(|s| (s[0] - 1.0).abs() > 1e-4 || (s[1] - 1.0).abs() > 1e-4)
                        || u.position.is_some_and(|s| s[0].abs() > 1e-4 || s[1].abs() > 1e-4)
                        || u.rotation.is_some_and(|r| r.abs() > 1e-4)
                        || f.projection != shard_model::Projection::Uv;
                    if non_default {
                        let src: Vec<_> = f
                            .sources
                            .iter()
                            .map(|s| match s {
                                shard_model::Source::Procedural { procedural, .. } => procedural.graph_name().unwrap_or("?").to_string(),
                                o => o.kind().to_string(),
                            })
                            .collect();
                        out.push(format!("{layer}: proj={:?} wrap={} mode={} scale={:?} pos={:?} rot={:?} pivot={:?} src={:?}", f.projection, u.wrap, u.scale_mode, u.scale, u.position, u.rotation, u.pivot, src));
                    }
                }
                EffectKind::Group { children } => walk_effects(children, layer, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[Layer], out: &mut Vec<String>) {
        for l in ls {
            walk_effects(&l.content, &format!("{} (content)", l.name), out);
            if let Some(m) = &l.mask {
                walk_effects(&m.effects, &format!("{} (mask)", l.name), out);
            }
            walk(&l.children, out);
        }
    }
    let mut out = Vec::new();
    for st in &import_project_stacks(p).unwrap() {
        walk(&st.layers, &mut out);
    }
    println!("{} fills with a non-default transform / projection", out.len());
    for l in out.iter().take(60) {
        println!("{l}");
    }
}

#[test]
fn decal_hits_summary() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    fn walk_effects(es: &[Effect], layer: &str) {
        for e in es {
            match &e.kind {
                EffectKind::Decal(h) => {
                    let mut g = std::collections::BTreeMap::new();
                    for x in h {
                        *g.entry((x.granularity, x.paint_through, x.view_type)).or_insert(0) += 1;
                    }
                    let uids: Vec<u64> = h.iter().map(|x| x.uid).collect();
                    println!("{layer}: uids {:?}", uids);
                    let seq: String = h.iter().map(|x| if x.color[0] > 0.5 { 'W' } else { 'B' }).collect();
                    println!("{layer}: color sequence {seq}");
                    let cols: Vec<_> = h.iter().map(|x| x.color).take(3).collect();
                    println!("{layer}: {} hits (granularity, paintThrough, viewType)->count {:?} colors {:?}", h.len(), g, cols);
                }
                EffectKind::Group { children } => walk_effects(children, layer),
                _ => {}
            }
        }
    }
    fn walk(ls: &[Layer]) {
        for l in ls {
            walk_effects(&l.content, &format!("{} (content)", l.name));
            if let Some(m) = &l.mask {
                walk_effects(&m.effects, &format!("{} (mask)", l.name));
            }
            walk(&l.children);
        }
    }
    for st in &import_project_stacks(p).unwrap() {
        walk(&st.layers);
    }
}

#[test]
fn decal_matrix_probe() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let st = &import_project_stacks(p).unwrap()[0];
    fn find(ls: &[Layer], name: &str) -> Option<Layer> {
        for l in ls {
            if l.name == name {
                return Some(l.clone());
            }
            if let Some(x) = find(&l.children, name) {
                return Some(x);
            }
        }
        None
    }
    fn first_decal(es: &[Effect]) -> Option<Vec<shard_model::DecalHit>> {
        for e in es {
            match &e.kind {
                EffectKind::Decal(h) => return Some((**h).clone()),
                EffectKind::Group { children } => {
                    if let Some(x) = first_decal(children) {
                        return Some(x);
                    }
                }
                _ => {}
            }
        }
        None
    }
    let l = find(&st.layers, &std::env::var("SHARD_LAYER").unwrap_or("Polymer".into())).unwrap();
    let hits = first_decal(&l.mask.as_ref().unwrap().effects).unwrap();
    let (mut ulo, mut uhi, mut vlo, mut vhi) = (9.0f32, -9.0f32, 9.0f32, -9.0f32);
    for h in &hits {
        ulo = ulo.min(h.position[0]);
        uhi = uhi.max(h.position[0]);
        vlo = vlo.min(h.position[1]);
        vhi = vhi.max(h.position[1]);
    }
    println!("layer hits u in [{ulo:.3},{uhi:.3}] v in [{vlo:.3},{vhi:.3}]");
    for h in hits.iter().take(3) {
        println!("pos={:?}", h.position);
        for r in 0..4 {
            println!("  col{r}: {:?}", &h.proj_view_inv[r * 4..r * 4 + 4]);
        }
    }
}

/// SHARD_PROBE_SPP + SHARD_LAYER=<name>: print every layer with that name: blend entries, content effects
/// with their sources (resource urls, uniform colours) and mask effects.
#[test]
fn layer_detail() {
    let (Ok(p), Ok(name)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_LAYER")) else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn src(s: &shard_model::Source) -> String {
        match s {
            shard_model::Source::Uniform { channels, color, opacity } => format!("uniform ch={:x} {color:?} op={opacity}", channels.0),
            shard_model::Source::Bitmap { channels, resource, opacity } => format!("bitmap ch={:x} {resource:?} op={opacity}", channels.0),
            shard_model::Source::Procedural { channels, procedural, opacity } => {
                format!("procedural ch={:x} {:?} tweaks={:?} op={opacity}", channels.0, procedural.graph_name(), procedural.tweaks)
            }
            shard_model::Source::Reference { channels, target, opacity, .. } => format!("reference ch={:x} -> {target} op={opacity}", channels.0),
            shard_model::Source::Other(c) => format!("other {c}"),
        }
    }
    fn eff(es: &[Effect], pad: &str) {
        for e in es {
            let blend: Vec<String> = e.blend.iter().map(|b| format!("{}@{:.2}ch={:x}", b.mode.name(), b.opacity, b.channels.0)).collect();
            println!("{pad}effect uid={} vis={} ch={:x} blend=[{}] {}", e.uid, e.visible, e.channels.0, blend.join(","), e.type_name());
            match &e.kind {
                EffectKind::Fill(f) => {
                    println!("{pad}  proj={:?} filt={} uv={:?}", f.projection, f.filtering, f.uv);
                    for s in &f.sources {
                        println!("{pad}  - {}", src(s));
                    }
                }
                EffectKind::Group { children } => eff(children, &format!("{pad}    ")),
                EffectKind::Levels(l) => println!("{pad}  levels {l:?}"),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], name: &str, set: &str) {
        for l in ls {
            if l.name == name {
                let blend: Vec<String> = l.blend.iter().map(|b| format!("{}@{:.2}ch={:x}", b.mode.name(), b.opacity, b.channels.0)).collect();
                println!("== set '{set}' layer '{}' vis={} uid={} blend=[{}]", l.name, l.visible, l.uid, blend.join(","));
                eff(&l.content, "  content ");
                if let Some(m) = &l.mask {
                    println!("  mask enabled={} initial={}", m.enabled, m.initial);
                    eff(&m.effects, "  mask ");
                }
            }
            walk(&l.children, name, set);
        }
    }
    for st in &stacks {
        walk(&st.layers, &name, &st.name);
    }
}

/// SHARD_PROBE_SPP: how many fill effects use each projection (content / mask, per source kind).
#[test]
fn projection_counts() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    fn eff(es: &[Effect], place: &str, counts: &mut std::collections::BTreeMap<String, usize>) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Fill(f) => {
                    for s in &f.sources {
                        let k = format!("{place:7} {:?} {}", f.projection, s.kind());
                        *counts.entry(k).or_default() += 1;
                    }
                }
                EffectKind::Group { children } => eff(children, place, counts),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], counts: &mut std::collections::BTreeMap<String, usize>) {
        for l in ls.iter().filter(|l| l.visible) {
            eff(&l.content, "content", counts);
            if let Some(m) = &l.mask {
                eff(&m.effects, "mask", counts);
            }
            walk(&l.children, counts);
        }
    }
    for s in &stacks {
        walk(&s.layers, &mut counts);
    }
    for (k, v) in counts {
        println!("{v:5}  {k}");
    }
}

/// SHARD_PROBE_SPP + SHARD_SET: layers of the set whose mask or content has a non-UV procedural fill.
#[test]
fn projected_layers() {
    let (Ok(p), Ok(set)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_SET")) else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn eff(es: &[Effect], out: &mut Vec<String>) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Fill(f) if !matches!(f.projection, shard_model::Projection::Uv | shard_model::Projection::Fill) => {
                    for s in &f.sources {
                        if let shard_model::Source::Procedural { procedural, .. } = s {
                            out.push(format!(
                                "{:?} {} scale={:?} off={:?} rot={:?}",
                                f.projection,
                                procedural.graph_name().unwrap_or("?"),
                                f.uv.scale,
                                f.uv.position,
                                f.uv.rotation
                            ));
                        }
                    }
                }
                EffectKind::Group { children } => eff(children, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], trail: &str) {
        for l in ls.iter().filter(|l| l.visible) {
            let mut hits = Vec::new();
            eff(&l.content, &mut hits);
            if let Some(m) = &l.mask {
                eff(&m.effects, &mut hits);
            }
            let path = if trail.is_empty() { l.name.clone() } else { format!("{trail}/{}", l.name) };
            for h in hits {
                println!("{path}: {h}");
            }
            walk(&l.children, &path);
        }
    }
    for s in stacks.iter().filter(|s| s.name == set) {
        walk(&s.layers, "");
    }
}

/// SHARD_PROBE_SPP (+ SHARD_SET): which channel bits the sources of visible fills write, by source kind.
#[test]
fn channel_writers() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let only = std::env::var("SHARD_SET").ok();
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    let mut counts: std::collections::BTreeMap<(u32, &'static str), usize> = Default::default();
    fn eff(es: &[Effect], counts: &mut std::collections::BTreeMap<(u32, &'static str), usize>) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Fill(f) => {
                    for s in &f.sources {
                        let ch = match s {
                            shard_model::Source::Uniform { channels, .. }
                            | shard_model::Source::Bitmap { channels, .. }
                            | shard_model::Source::Procedural { channels, .. }
                            | shard_model::Source::Reference { channels, .. } => *channels,
                            _ => continue,
                        };
                        for b in 0..128u32 {
                            if ch.contains(b) && ch.count() < 20 {
                                *counts.entry((b, s.kind())).or_default() += 1;
                            }
                        }
                    }
                }
                EffectKind::Group { children } => eff(children, counts),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], counts: &mut std::collections::BTreeMap<(u32, &'static str), usize>) {
        for l in ls.iter().filter(|l| l.visible) {
            eff(&l.content, counts);
            walk(&l.children, counts);
        }
    }
    for s in stacks.iter().filter(|s| only.as_deref().map_or(true, |o| s.name == o)) {
        walk(&s.layers, &mut counts);
    }
    for ((bit, kind), n) in counts {
        println!("{n:5}  bit {bit:2} {:<18} {kind}", shard_formats::enums::channel_name(bit).unwrap_or_default());
    }
}

/// SHARD_PROBE_SPP + SHARD_SET (+ SHARD_BIT, default 22): layers with a visible fill source writing that channel.
#[test]
fn bit_writers() {
    let (Ok(p), Ok(set)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_SET")) else { return };
    let bit: u32 = std::env::var("SHARD_BIT").ok().and_then(|v| v.parse().ok()).unwrap_or(22);
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn eff(es: &[Effect], bit: u32, out: &mut Vec<String>) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Fill(f) => {
                    for s in &f.sources {
                        let (ch, what) = match s {
                            shard_model::Source::Uniform { channels, .. } => (*channels, "uniform".to_string()),
                            shard_model::Source::Bitmap { channels, .. } => (*channels, "bitmap".to_string()),
                            shard_model::Source::Procedural { channels, procedural, .. } => {
                                (*channels, format!("procedural {}", procedural.graph_name().unwrap_or("?")))
                            }
                            shard_model::Source::Reference { channels, .. } => (*channels, "reference".to_string()),
                            _ => continue,
                        };
                        if ch.contains(bit) {
                            out.push(format!("{what} (writes {} channels) proj {:?}", ch.count(), f.projection));
                        }
                    }
                }
                EffectKind::Group { children } => eff(children, bit, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], trail: &str, bit: u32) {
        for l in ls.iter().filter(|l| l.visible) {
            let mut hits = Vec::new();
            eff(&l.content, bit, &mut hits);
            let path = if trail.is_empty() { l.name.clone() } else { format!("{trail}/{}", l.name) };
            for h in hits {
                println!("{path}: {h}");
            }
            walk(&l.children, &path, bit);
        }
    }
    for s in stacks.iter().filter(|s| s.name == set) {
        walk(&s.layers, "", bit);
    }
}

/// SHARD_PROBE_SPP + SHARD_SET: visible filter effects (content and masks) with the channels they act on.
#[test]
fn filter_effects() {
    let (Ok(p), Ok(set)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_SET")) else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn eff(es: &[Effect], where_: &str, out: &mut Vec<String>) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Filter { procedural } => out.push(format!(
                    "{where_} filter {} channels={} (empty={})",
                    procedural.as_ref().and_then(|p| p.graph_name()).unwrap_or("?"),
                    e.channels.count(),
                    e.channels.is_empty()
                )),
                EffectKind::Group { children } => eff(children, where_, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], trail: &str) {
        for l in ls.iter().filter(|l| l.visible) {
            let mut hits = Vec::new();
            eff(&l.content, "content", &mut hits);
            if let Some(m) = &l.mask {
                eff(&m.effects, "mask", &mut hits);
            }
            let path = if trail.is_empty() { l.name.clone() } else { format!("{trail}/{}", l.name) };
            for h in hits {
                println!("{path}: {h}");
            }
            walk(&l.children, &path);
        }
    }
    for s in stacks.iter().filter(|s| s.name == set) {
        walk(&s.layers, "");
    }
}

/// SHARD_PROBE_SPP: scalar properties of every DataMaterial (height2NormalMethod, padding, mixing...).
#[test]
fn material_props() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut c = shard_formats::Container::open(std::path::Path::new(&p)).unwrap();
    let root = c.decode("paint/document.bin").unwrap();
    let o = root.as_object().unwrap();
    for (k, v) in &o.props {
        if matches!(v, shard_formats::alg::Value::List(_)) {
            continue;
        }
        println!("doc.{k} = {v:?}");
    }
    if let Some(shard_formats::alg::Value::List(ms)) = o.get("materials") {
        for m in ms {
            let mo = m.as_object().unwrap();
            for (k, v) in &mo.props {
                match v {
                    shard_formats::alg::Value::List(l) => println!("  {k} = [list of {}]", l.len()),
                    shard_formats::alg::Value::Object(_) => println!("  {k} = <object>"),
                    other => println!("  {k} = {other:?}"),
                }
            }
        }
    }
}

/// SHARD_PROBE_SPP + SHARD_SET: layers with decal / polygon-fill effects: where they are (content / mask)
/// and the range of hit uids with their colours.
#[test]
fn decal_owners() {
    let (Ok(p), Ok(set)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_SET")) else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn eff(es: &[Effect], where_: &str, out: &mut Vec<String>) {
        for e in es {
            match &e.kind {
                EffectKind::Decal(hits) => {
                    let (lo, hi) = hits.iter().fold((u64::MAX, 0u64), |a, h| (a.0.min(h.uid), a.1.max(h.uid)));
                    let w = hits.iter().filter(|h| h.color[0] > 0.5).count();
                    out.push(format!(
                        "{where_} decal effect uid={} vis={} hits={} (white {w}, black {}) hit uids {lo}..{hi}",
                        e.uid,
                        e.visible,
                        hits.len(),
                        hits.len() - w
                    ));
                    if std::env::var_os("SHARD_LIST_HITS").is_some() {
                        let l: Vec<String> =
                            hits.iter().map(|h| format!("{}{}", h.uid, if h.color[0] > 0.5 { "W" } else { "B" })).collect();
                        out.push(format!("    hits in order: {}", l.join(" ")));
                    }
                }
                EffectKind::Group { children } => eff(children, where_, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], trail: &str) {
        for l in ls {
            let mut hits = Vec::new();
            eff(&l.content, "content", &mut hits);
            if let Some(m) = &l.mask {
                eff(&m.effects, &format!("mask(initial={},enabled={})", m.initial, m.enabled), &mut hits);
            }
            let path = if trail.is_empty() { l.name.clone() } else { format!("{trail}/{}", l.name) };
            for h in hits {
                println!("{path} [{:?} vis={}]: {h}", l.kind, l.visible);
            }
            walk(&l.children, &path);
        }
    }
    for s in stacks.iter().filter(|s| s.name == set) {
        walk(&s.layers, "");
    }
}

/// SHARD_PROBE_SPP + SHARD_LAYER: paint / eraser / smudge actions of a layer (content and mask): brush,
/// shape graph, colour sources and a summary of every stroke.
#[test]
fn paint_detail() {
    let (Ok(p), Ok(name)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_LAYER")) else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn act(e: &Effect, a: &shard_model::PaintAction, kind: &str, pad: &str) {
        let b = &a.brush;
        println!(
            "{pad}{kind} uid={} vis={} ch={:x} opacity={:.2} brush[size={:.4} space={} flow={:.2} spacing={:.3} cull={}@{:.2} thru={} align={} filt={} rot={:.1} follow={}]",
            e.uid, e.visible, e.channels.0, a.opacity, b.size, b.size_space, b.flow, b.spacing, b.culling_enabled,
            b.culling_angle, b.paint_through, b.alignment, b.filtering, b.rotation, b.follow_path
        );
        if let Some(sh) = &b.shape {
            println!("{pad}  shape graph {:?} resource {:?} tweaks {:?} inputs {:?}", sh.graph_name(), sh.resource, sh.tweaks, sh.inputs);
        }
        for c in &a.color {
            let d = match c {
                shard_model::Source::Uniform { channels, color, opacity } => format!("uniform ch={:x} {color:?} op={opacity}", channels.0),
                shard_model::Source::Bitmap { channels, resource, opacity } => format!("bitmap ch={:x} {resource:?} op={opacity}", channels.0),
                shard_model::Source::Procedural { channels, procedural, opacity } => {
                    format!("procedural ch={:x} {:?} res={:?} tweaks={:?} op={opacity}", channels.0, procedural.graph_name(), procedural.resource, procedural.tweaks)
                }
                shard_model::Source::Reference { channels, target, opacity, .. } => format!("reference ch={:x} -> {target} op={opacity}", channels.0),
                shard_model::Source::Other(c) => format!("other {c}"),
            };
            println!("{pad}  colour source: {d}");
        }
        for s in a.strokes.iter().take(40) {
            let (lo, hi) = s.keys.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |m, k| {
                ([m.0[0].min(k.pos[0]), m.0[1].min(k.pos[1])], [m.1[0].max(k.pos[0]), m.1[1].max(k.pos[1])])
            });
            println!(
                "{pad}  stroke keys={} view_type={} seed={} lazy={:.3} pos x[{:.3},{:.3}] y[{:.3},{:.3}] pressure={} view0={:.2} proj0={:.2}",
                s.keys.len(), s.view_type, s.seed, s.lazy_distance, lo[0], hi[0], lo[1], hi[1], s.pressure.len(), s.view[0], s.projection[0]
            );
        }
    }
    fn eff(es: &[Effect], pad: &str) {
        for e in es {
            match &e.kind {
                EffectKind::Paint(a) => act(e, a, "paint", pad),
                EffectKind::Eraser(a) => act(e, a, "eraser", pad),
                EffectKind::Smudge(a) => act(e, a, "smudge", pad),
                EffectKind::Group { children } => eff(children, &format!("{pad}    ")),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], name: &str) {
        for l in ls {
            if l.name == name {
                println!("== layer '{}' uid={}", l.name, l.uid);
                eff(&l.content, "  content ");
                if let Some(m) = &l.mask {
                    println!("  mask initial={} enabled={}", m.initial, m.enabled);
                    eff(&m.effects, "  mask ");
                }
            }
            walk(&l.children, name);
        }
    }
    for st in &stacks {
        walk(&st.layers, &name);
    }
}

/// SHARD_PROBE_SPP + SHARD_UID: raw properties of the document object with that `uid` (depth SHARD_DEPTH, default 3).
#[test]
fn raw_object() {
    let (Ok(p), Ok(uid)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_UID")) else { return };
    let uid: u64 = uid.parse().unwrap();
    let depth: usize = std::env::var("SHARD_DEPTH").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let mut c = shard_formats::Container::open(std::path::Path::new(&p)).unwrap();
    let root = c.decode("paint/document.bin").unwrap();
    use shard_formats::alg::Value;
    fn show(v: &Value, pad: &str, depth: usize) {
        match v {
            Value::Object(o) => {
                println!("{pad}<{}>", o.class);
                if depth == 0 {
                    return;
                }
                for (k, x) in &o.props {
                    match x {
                        Value::List(l) => {
                            println!("{pad}  {k}: list[{}]", l.len());
                            if depth > 1 {
                                for it in l.iter().take(3) {
                                    show(it, &format!("{pad}    "), depth - 1);
                                }
                            }
                        }
                        Value::Object(_) => {
                            println!("{pad}  {k}:");
                            show(x, &format!("{pad}    "), depth - 1);
                        }
                        Value::Raw(t, b) => println!("{pad}  {k}: raw({t}) {:?}", &b[..b.len().min(64)]),
                        Value::Bytes(b) => println!("{pad}  {k}: bytes[{}] {:?}", b.len(), &b[..b.len().min(48)]),
                        other => println!("{pad}  {k}: {other:?}"),
                    }
                }
            }
            other => println!("{pad}{other:?}"),
        }
    }
    fn find<'a>(v: &'a Value, uid: u64) -> Option<&'a Value> {
        match v {
            Value::Object(o) => {
                if matches!(o.get("uid"), Some(Value::Uint(u)) if *u == uid) {
                    return Some(v);
                }
                o.props.iter().find_map(|(_, x)| find(x, uid))
            }
            Value::List(l) => l.iter().find_map(|x| find(x, uid)),
            _ => None,
        }
    }
    match find(&root, uid) {
        Some(v) => show(v, "", depth),
        None => println!("uid {uid} not found"),
    }
}

/// SHARD_PROBE_SPP: paint actions with a stencil, by kind and text; with the projection matrix.
#[test]
fn stencils() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn act(layer: &str, a: &shard_model::PaintAction, kind: &str) {
        if let Some(st) = &a.stencil {
            let d = match st {
                shard_model::Stencil::Text(t) => format!("text {:?} font {} size {} auto {} pad {:?} off {:?} align {}/{}", t.text, t.font.rsplit('/').next().unwrap_or(""), t.size, t.auto_size, t.padding, t.offset, t.h_align, t.v_align),
                shard_model::Stencil::Procedural(p) => format!("procedural {:?} tweaks {:?}", p.graph_name(), p.tweaks),
                other => format!("{other:?}"),
            };
            println!("{layer}: {kind} strokes={} wrap={} proj={:?} {d}", a.strokes.len(), a.proj_wrap, a.proj.map(|m| m.map(|v| (v * 1000.0).round() / 1000.0)));
        }
    }
    fn eff(layer: &str, es: &[Effect]) {
        for e in es {
            match &e.kind {
                EffectKind::Paint(a) => act(layer, a, "paint"),
                EffectKind::Eraser(a) => act(layer, a, "eraser"),
                EffectKind::Group { children } => eff(layer, children),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer]) {
        for l in ls {
            eff(&l.name, &l.content);
            if let Some(m) = &l.mask {
                eff(&l.name, &m.effects);
            }
            walk(&l.children);
        }
    }
    for st in &stacks {
        walk(&st.layers);
    }
}

/// SHARD_PROBE_SPP: root layers of every set in the order the model keeps them (top first) with uids.
#[test]
fn root_order() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    for st in &stacks {
        println!("set {}", st.name);
        for (i, l) in st.layers.iter().enumerate() {
            println!("  [{i}] uid={} {:?} '{}' vis={} blend={:?}", l.uid, l.kind, l.name, l.visible, l.blend.iter().map(|b| (b.mode.name(), b.channels.0, b.opacity)).collect::<Vec<_>>());
        }
    }
}

/// SHARD_PROBE_SPP: raw order of the root items of the first material stack, with every scalar property.
#[test]
fn raw_root_items() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let mut c = shard_formats::Container::open(std::path::Path::new(&p)).unwrap();
    let root = c.decode("paint/document.bin").unwrap();
    use shard_formats::alg::Value;
    let o = root.as_object().unwrap();
    let Some(Value::List(ms)) = o.get("materials") else { return };
    for m in ms {
        let mo = m.as_object().unwrap();
        let Some(Value::List(sts)) = mo.get("stacks") else { continue };
        for st in sts {
            let so = st.as_object().unwrap();
            println!("stack props:");
            for (k, v) in &so.props {
                match v {
                    Value::List(l) => println!("  {k}: list[{}]", l.len()),
                    Value::Object(_) => println!("  {k}: <object>"),
                    other => println!("  {k}: {other:?}"),
                }
            }
            if let Some(Value::Object(stack)) = so.get("stack") {
                println!("DataStackLayers props:");
                for (k, v) in &stack.props {
                    match v {
                        Value::List(l) => println!("  {k}: list[{}]", l.len()),
                        Value::Object(_) => println!("  {k}: <object>"),
                        other => println!("  {k}: {other:?}"),
                    }
                }
                if let Some(Value::List(items)) = stack.get("items") {
                    for (i, it) in items.iter().enumerate() {
                        let io = it.as_object().unwrap();
                        let scal: Vec<String> = io
                            .props
                            .iter()
                            .filter(|(_, v)| !matches!(v, Value::List(_) | Value::Object(_)))
                            .map(|(k, v)| format!("{k}={v:?}"))
                            .collect();
                        println!("  item[{i}] <{}> {}", io.class, scal.join(" "));
                    }
                }
            }
        }
    }
}

/// SHARD_PROBE_SPP: uids of every group's children in model order (top first), to compare with creation order.
#[test]
fn uid_order() {
    let Ok(p) = std::env::var("SHARD_PROBE_SPP") else { return };
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    fn walk(ls: &[shard_model::Layer], path: &str) {
        let uids: Vec<String> = ls.iter().map(|l| format!("{}:{}", l.name, l.uid)).collect();
        println!("{path}: {}", uids.join("  "));
        for l in ls {
            if !l.children.is_empty() {
                walk(&l.children, &format!("{path}/{}", l.name));
            }
        }
    }
    let all = std::env::var_os("SHARD_ALL_SETS").is_some();
    for s in stacks.iter().take(if all { usize::MAX } else { 1 }) {
        if all {
            let uids: Vec<String> = s.layers.iter().map(|l| format!("{}:{}", l.name, l.uid)).collect();
            println!("ROOT {}: {}", s.name, uids.join("  "));
        } else {
            walk(&s.layers, "<root>");
        }
    }
}
