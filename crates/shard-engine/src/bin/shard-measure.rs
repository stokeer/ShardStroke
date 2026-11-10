//! `shard-measure`: measure the engine against Painter's own exports, no window and no GPU.
//!
//! ```text
//! shard-measure                                  # whole corpus (tools/corpus.json), 1024 px
//! shard-measure --only NAME    --res 2048        # one project
//! shard-measure --project X.spp --layout png --truth C:/dir/Prefix_   # ad hoc
//! shard-measure --json out.json                  # also write the numbers
//! shard-measure --only NAME    --dump out/       # ours / truth / 4x difference images per channel
//! shard-measure --only NAME    --layers          # print the layer tree of each set
//! shard-measure --baseline before.json --tol 0.5 # fail when a channel got worse by more than tol (MAE, 0..255)
//! shard-measure --only NAME --ladder EXTRACTION_DIR   # layer by layer vs the extraction plugin's export
//! ```
//!
//! Corpus file: `{"projects":[{"name","project","layout","truth","res"?,"sets"?:[..],"mask_dir"?,"masks"?:[{set,layer,file}]}]}`;
//! `mask_dir` holds masks exported by Painter as `<set>__<layer>.png` (`mask_skip` lists stems to leave out). `layout` is
//! `substance_png` (truth = file prefix) or `bcr_nmo` (truth = directory).

use std::path::PathBuf;
use std::process::ExitCode;

use shard_engine::measure::{self, Layout, MaskTruth, SetReport};
use shard_engine::project::Project;
use serde_json::{json, Value};

struct Entry {
    name: String,
    project: PathBuf,
    layout: Layout,
    res: Option<u32>,
    sets: Option<Vec<String>>,
    masks: Vec<MaskTruth>,
    /// Layer or group names hidden while evaluating (the reference was exported with them hidden).
    hide: Vec<String>,
}

fn load_corpus(path: &str) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    // `$SHARD_CORPUS` in the paths: the folder the corpus projects live under (the corpus is not distributed)
    let text = match std::env::var("SHARD_CORPUS") {
        Ok(root) => text.replace("$SHARD_CORPUS", &root.replace('\\', "/")),
        Err(_) if text.contains("$SHARD_CORPUS") => return Err(format!("{path}: set SHARD_CORPUS to the folder of the corpus projects")),
        Err(_) => text,
    };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let mut out = Vec::new();
    for p in v["projects"].as_array().ok_or("corpus: missing 'projects'")? {
        let s = |k: &str| p[k].as_str().ok_or_else(|| format!("corpus entry missing '{k}'"));
        out.push(Entry {
            name: s("name")?.to_string(),
            project: PathBuf::from(s("project")?),
            layout: Layout::parse(s("layout")?, s("truth")?)?,
            res: p["res"].as_u64().map(|r| r as u32),
            sets: p["sets"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            hide: p["hide"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            masks: mask_dir(p["mask_dir"].as_str())
                .into_iter()
                // `mask_skip`: file stems of the folder that an explicit entry of `masks` re-maps (two groups
                // can share a name, and the export does not say which one it is)
                .filter(|m| {
                    let stem = m.file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    !p["mask_skip"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(stem)))
                })
                .chain(p["masks"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| {
                            Some(MaskTruth {
                                set: m["set"].as_str()?.to_string(),
                                layer: m["layer"].as_str()?.to_string(),
                                file: PathBuf::from(m["file"].as_str()?),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default())
                .collect(),
        });
    }
    Ok(out)
}

/// Masks exported by Painter into a folder: `<Texture set>__<Layer>.png` (`~` stands for `/` in `Group~Layer`).
fn mask_dir(dir: Option<&str>) -> Vec<MaskTruth> {
    let Some(dir) = dir else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<MaskTruth> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("png")) != Some(true) {
                return None;
            }
            let stem = p.file_stem()?.to_str()?.to_string();
            let (set, layer) = stem.split_once("__")?;
            Some(MaskTruth { set: set.to_string(), layer: layer.replace('~', "/"), file: p })
        })
        .collect();
    out.sort_by(|a, b| (&a.set, &a.layer).cmp(&(&b.set, &b.layer)));
    out
}

fn run(entry: &Entry, res: u32, only_set: Option<&str>, dump: Option<&std::path::Path>) -> Result<Vec<SetReport>, String> {
    let t0 = std::time::Instant::now();
    // the reference of this entry was exported with some layers hidden
    // (SHARD_EXTRA_HIDE=name,name hides more layers for an experiment)
    let mut hide = entry.hide.clone();
    if let Ok(x) = std::env::var("SHARD_EXTRA_HIDE") {
        hide.extend(x.split(',').filter(|s| !s.is_empty()).map(str::to_string));
    }
    if hide.is_empty() {
        std::env::remove_var("SHARD_HIDE_LAYER");
    } else {
        std::env::set_var("SHARD_HIDE_LAYER", hide.join(","));
    }
    let proj = Project::open(&entry.project)?;
    eprintln!("[{}] opened in {:.1}s ({} sets, {} triangles)", entry.name, t0.elapsed().as_secs_f32(), proj.sets.len(), proj.mesh.triangle_count());
    if std::env::var_os("SHARD_TRACE_MESH").is_some() {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &proj.mesh.vertices {
            for a in 0..3 {
                lo[a] = lo[a].min(v.pos[a]);
                hi[a] = hi[a].max(v.pos[a]);
            }
        }
        eprintln!("[mesh] bounds ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3}) size ({:.3},{:.3},{:.3})", lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
        // where could the centre Painter normalised the scene around be? (mesh units, after our own normalisation)
        {
            let m = &proj.mesh;
            let n = m.vertices.len().max(1) as f64;
            let mut cen = [0.0f64; 3];
            for v in &m.vertices {
                for a in 0..3 {
                    cen[a] += v.pos[a] as f64 / n;
                }
            }
            eprintln!("[mesh] vertex centroid ({:.4},{:.4},{:.4})", cen[0], cen[1], cen[2]);
            // area weighted centroid of the triangles
            let (mut acen, mut atot) = ([0.0f64; 3], 0.0f64);
            for t in m.indices.chunks_exact(3) {
                let p: Vec<[f32; 3]> = t.iter().map(|&i| m.vertices[i as usize].pos).collect();
                let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let area = 0.5 * ((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]) as f64).sqrt();
                for a in 0..3 {
                    acen[a] += area * ((p[0][a] + p[1][a] + p[2][a]) as f64 / 3.0);
                }
                atot += area;
            }
            eprintln!("[mesh] area centroid ({:.4},{:.4},{:.4})", acen[0] / atot, acen[1] / atot, acen[2] / atot);
            // per texture set AABB centre
            let nsets = m.set_names.len().max(1);
            for s in 0..nsets {
                let (mut slo, mut shi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for (t, tri) in m.indices.chunks_exact(3).enumerate() {
                    if m.tri_mat.get(t).copied() != Some(s as u32) {
                        continue;
                    }
                    for &i in tri {
                        for a in 0..3 {
                            slo[a] = slo[a].min(m.vertices[i as usize].pos[a]);
                            shi[a] = shi[a].max(m.vertices[i as usize].pos[a]);
                        }
                    }
                }
                eprintln!(
                    "[mesh] set {s} '{}' AABB ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3}) centre ({:.4},{:.4},{:.4})",
                    m.set_names.get(s).map(String::as_str).unwrap_or("?"),
                    slo[0], slo[1], slo[2], shi[0], shi[1], shi[2],
                    (slo[0] + shi[0]) * 0.5, (slo[1] + shi[1]) * 0.5, (slo[2] + shi[2]) * 0.5
                );
            }
        }
    }
    let mut reports = Vec::new();
    for set in 0..proj.sets.len() {
        let name = &proj.sets[set].name;
        if let Some(list) = &entry.sets {
            if !list.iter().any(|s| s == name) {
                continue;
            }
        }
        if only_set.is_some_and(|s| s != name) {
            continue;
        }
        match measure::measure_set(&proj, set, &entry.layout, res, dump) {
            Some(rep) => {
                measure::print_set(&entry.name, &rep);
                reports.push(rep);
            }
            None => eprintln!("[{}] {name}: no reference files, skipped", entry.name),
        }
    }
    // masks exported by Painter, one layer at a time
    let mut by_set: std::collections::BTreeMap<String, Vec<measure::ChannelReport>> = Default::default();
    for mt in &entry.masks {
        if only_set.is_some_and(|s| s != mt.set) {
            continue;
        }
        match measure::measure_mask(&proj, mt, res, dump) {
            Ok((rep, notes)) => {
                println!(
                    "== {} / mask '{}' of {}:  MAE {:.1}  RMSE {:.1}  p95 {:.1}  corr {:.3}  bias {:+.1}  ours {:.1} | truth {:.1}",
                    entry.name, mt.layer, mt.set, rep.m.mae, rep.m.rmse, rep.m.p95, rep.m.corr, rep.m.bias, rep.m.ours_mean, rep.m.truth_mean
                );
                for n in notes.iter().take(3) {
                    println!("  note: {n}");
                }
                by_set.entry(mt.set.clone()).or_default().push(rep);
            }
            Err(e) => eprintln!("[{}] mask '{}': {e}", entry.name, mt.layer),
        }
    }
    for (set, channels) in by_set {
        reports.push(SetReport { set: format!("masks/{set}"), res, channels, compose_secs: 0.0, covered: 0, notes: Vec::new() });
    }
    Ok(reports)
}

fn to_json(all: &[(String, Vec<SetReport>)]) -> Value {
    json!({
        "projects": all.iter().map(|(name, sets)| json!({
            "name": name,
            "sets": sets.iter().map(|s| json!({
                "set": s.set, "res": s.res, "compose_secs": s.compose_secs,
                "channels": s.channels.iter().map(|c| json!({
                    "name": c.name, "mae": c.m.mae, "rmse": c.m.rmse, "p95": c.m.p95,
                    "corr": if c.m.corr.is_nan() { Value::Null } else { json!(c.m.corr) },
                    "bias": c.m.bias,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Compare with a previous run; returns the number of regressions.
fn compare(base: &Value, now: &Value, tol: f64) -> usize {
    let mut bad = 0;
    for p in now["projects"].as_array().into_iter().flatten() {
        for s in p["sets"].as_array().into_iter().flatten() {
            let find = base["projects"].as_array().into_iter().flatten()
                .filter(|b| b["name"] == p["name"])
                .flat_map(|b| b["sets"].as_array().into_iter().flatten())
                .find(|b| b["set"] == s["set"]);
            let Some(prev) = find else { continue };
            for c in s["channels"].as_array().into_iter().flatten() {
                let Some(pc) = prev["channels"].as_array().into_iter().flatten().find(|x| x["name"] == c["name"]) else { continue };
                let (a, b) = (pc["mae"].as_f64().unwrap_or(0.0), c["mae"].as_f64().unwrap_or(0.0));
                let d = b - a;
                if d.abs() >= 0.05 {
                    let mark = if d > tol { bad += 1; "WORSE " } else if d < -tol { "better" } else { "      " };
                    println!(
                        "  {mark} {} / {} / {:<18} MAE {:>6.2} -> {:>6.2} ({:+.2})",
                        p["name"].as_str().unwrap_or("?"), s["set"].as_str().unwrap_or("?"),
                        c["name"].as_str().unwrap_or("?"), a, b, d
                    );
                }
            }
        }
    }
    bad
}

fn main() -> ExitCode {
    let mut corpus = "tools/corpus.json".to_string();
    let (mut only, mut res, mut json_out, mut baseline, mut tol) = (None::<String>, None::<u32>, None::<String>, None::<String>, 0.5f64);
    let (mut project, mut layout, mut truth, mut set) = (None::<String>, "substance_png".to_string(), None::<String>, None::<String>);
    let mut dump: Option<String> = None;
    let mut list_layers = false;
    let mut ladder: Option<String> = None;
    let mut filter_test: Option<String> = None;
    let mut compose_res: Option<u32> = None;
    let (mut ft_layer, mut ft_from, mut ft_to) = (0u64, 0usize, 0usize);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_default();
        match a.as_str() {
            "--corpus" => corpus = val(),
            "--only" => only = Some(val()),
            "--res" => res = val().parse().ok(),
            "--json" => json_out = Some(val()),
            "--baseline" => baseline = Some(val()),
            "--tol" => tol = val().parse().unwrap_or(0.5),
            "--project" => project = Some(val()),
            "--layout" => layout = val(),
            "--truth" => truth = Some(val()),
            "--set" => set = Some(val()),
            "--dump" => dump = Some(val()),
            "--layers" => list_layers = true,
            "--ladder" => ladder = Some(val()),
            // --filter-test DIR --layer UID --from STEP --to STEP: Painter's picture before / after one filter layer
            "--filter-test" => filter_test = Some(val()),
            "--layer" => ft_layer = val().parse().unwrap_or(0),
            // --compose-res 2048: compose at the project's resolution and box-filter to --res (Painter exports downscaled)
            "--compose-res" => compose_res = val().parse().ok(),
            "--from" => ft_from = val().parse().unwrap_or(0),
            "--to" => ft_to = val().parse().unwrap_or(0),
            "-h" | "--help" => {
                println!("{}", include_str!("shard-measure.rs").lines().take(14).map(|l| l.trim_start_matches("//! ").trim_start_matches("//!")).collect::<Vec<_>>().join("\n"));
                return ExitCode::SUCCESS;
            }
            other => eprintln!("ignored argument: {other}"),
        }
    }
    let entries = match (project, truth) {
        (Some(p), Some(t)) => match Layout::parse(&layout, &t) {
            Ok(l) => vec![Entry {
                name: PathBuf::from(&p).file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string(),
                project: PathBuf::from(p),
                layout: l,
                res: None,
                sets: None,
                masks: Vec::new(),
                hide: Vec::new(),
            }],
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        },
        _ => match load_corpus(&corpus) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        },
    };
    if let Some(dir) = &filter_test {
        for e in &entries {
            if only.as_ref().is_some_and(|o| !e.name.eq_ignore_ascii_case(o)) {
                continue;
            }
            let Ok(proj) = Project::open(&e.project) else { continue };
            let r = res.or(e.res).unwrap_or(1024);
            for (i, s) in proj.sets.iter().enumerate() {
                if set.as_deref().map_or(true, |w| w != s.name) {
                    continue;
                }
                match measure::filter_isolation(&proj, i, std::path::Path::new(dir), ft_from, ft_to, ft_layer, r) {
                    Ok(vs) => {
                        println!("== {} / {}: layer {} from step {} to step {}", e.name, s.name, ft_layer, ft_from, ft_to);
                        for v in vs {
                            println!("  {:<48} MAE {:6.2}  bias {:+6.2}  (vs the input {:5.2})", v.label, v.mae, v.bias, v.vs_input);
                        }
                    }
                    Err(err) => eprintln!("[{}] {}: {err}", e.name, s.name),
                }
            }
        }
        return ExitCode::SUCCESS;
    }
    if let Some(dir) = &ladder {
        let mut any = false;
        for e in &entries {
            if only.as_ref().is_some_and(|o| !e.name.eq_ignore_ascii_case(o)) {
                continue;
            }
            let Ok(proj) = Project::open(&e.project) else {
                eprintln!("[{}] cannot open {}", e.name, e.project.display());
                continue;
            };
            let r = res.or(e.res).unwrap_or(1024);
            if let Some(why) = measure::extraction_stale(std::path::Path::new(dir), &e.project) {
                eprintln!("[{}] WARNING: {why}", e.name);
            }
            for (i, s) in proj.sets.iter().enumerate() {
                if set.as_deref().is_some_and(|w| w != s.name) {
                    continue;
                }
                // composed at the set's own resolution by default (Painter's filters, like `Sharpen`, work at the
                // project's resolution; composing at 1024 made the last step of an interior set look +2.4 wrong)
                let cr = compose_res.unwrap_or_else(|| s.resolution.0.max(r));
                match measure::measure_ladder(&proj, i, std::path::Path::new(dir), r, cr) {
                    Some(rows) => {
                        any = true;
                        measure::print_ladder(&e.name, &s.name, &rows);
                    }
                    None => eprintln!("[{}] {}: no ladder in {dir}", e.name, s.name),
                }
            }
        }
        return if any { ExitCode::SUCCESS } else { ExitCode::from(1) };
    }
    if list_layers {
        for e in &entries {
            if only.as_ref().is_some_and(|o| !e.name.eq_ignore_ascii_case(o)) {
                continue;
            }
            let Ok(proj) = Project::open(&e.project) else { continue };
            for (i, s) in proj.sets.iter().enumerate() {
                if set.as_deref().is_some_and(|w| w != s.name) {
                    continue;
                }
                println!("== {} / {} (set {i})", e.name, s.name);
                fn walk(ls: &[shard_model::Layer], depth: usize) {
                    for l in ls {
                        let kind = if l.kind == shard_model::LayerKind::Group { "group" } else { "layer" };
                        let uid = if std::env::var_os("SHARD_LIST_UIDS").is_some() { format!(" uid={}", l.uid) } else { String::new() };
                        println!("{}{} '{}'{}{}", "  ".repeat(depth), kind, l.name, if l.visible { "" } else { " (hidden)" }, uid);
                        // SHARD_LIST_ANCHORS=1: anchor points a layer defines and references it reads
                        if std::env::var_os("SHARD_LIST_ANCHORS").is_some() {
                            fn scan(es: &[shard_model::Effect], where_: &str, out: &mut Vec<String>) {
                                for e in es {
                                    match &e.kind {
                                        shard_model::EffectKind::Anchor { .. } => out.push(format!("{where_} anchor uid={}", e.uid)),
                                        shard_model::EffectKind::Fill(f) => {
                                            for s in &f.sources {
                                                if let shard_model::Source::Reference { target, .. } = s {
                                                    out.push(format!("{where_} reference -> {target} (effect {})", e.uid));
                                                }
                                            }
                                        }
                                        shard_model::EffectKind::Group { children } => scan(children, where_, out),
                                        _ => {}
                                    }
                                }
                            }
                            let mut out = Vec::new();
                            scan(&l.content, "content", &mut out);
                            if let Some(m) = &l.mask {
                                scan(&m.effects, "mask", &mut out);
                            }
                            for o in out {
                                println!("{}    - {o}", "  ".repeat(depth));
                            }
                        }
                        // SHARD_SUMMARY=1: one line per effect of the mask and content (kind, blend, counts)
                        if std::env::var_os("SHARD_SUMMARY").is_some() {
                            fn sum(es: &[shard_model::Effect], ind: usize, out: &mut Vec<String>) {
                                for e in es {
                                    let b = e.blend.first().map(|b| format!("{} {:.2}", b.mode.name(), b.opacity)).unwrap_or_default();
                                    let what = match &e.kind {
                                        shard_model::EffectKind::Decal(h) => {
                                            let w = h.iter().filter(|x| x.color[0] > 0.5).count();
                                            let center = h.iter().filter(|x| (x.position[0] - 0.5).abs() < 1e-3 && (x.position[1] - 0.5).abs() < 1e-3).count();
                                            let outside = h.iter().filter(|x| !(0.0..=1.0).contains(&x.position[0]) || !(0.0..=1.0).contains(&x.position[1])).count();
                                            let mut kinds: std::collections::BTreeMap<(i32, i32, i32), usize> = Default::default();
                                            for x in h.iter() {
                                                *kinds.entry((x.granularity, x.paint_through, x.view_type)).or_default() += 1;
                                            }
                                            if std::env::var_os("SHARD_LIST_HITS").is_some() {
                                                for x in h.iter() {
                                                    eprintln!(
                                                        "      hit uid={} pos=({:.4},{:.4}) {} gran={} m=[{:.3} {:.3} {:.3} {:.3} | {:.3} {:.3}]",
                                                        x.uid, x.position[0], x.position[1], if x.color[0] > 0.5 { "W" } else { "B" }, x.granularity,
                                                        x.proj_view_inv[0], x.proj_view_inv[5], x.proj_view_inv[10], x.proj_view_inv[15], x.proj_view_inv[12], x.proj_view_inv[13]
                                                    );
                                                }
                                            }
                                            let mut colours: std::collections::BTreeMap<String, usize> = Default::default();
                                            for x in h.iter() {
                                                *colours.entry(format!("{:.3},{:.3},{:.3}", x.color[0], x.color[1], x.color[2])).or_default() += 1;
                                            }
                                            format!(
                                                "decal: {} hits ({} white, {} black; {} at the centre, {} outside the view; (granularity, through, view) {:?}; colours {:?})",
                                                h.len(), w, h.len() - w, center, outside, kinds, colours
                                            )
                                        }
                                        shard_model::EffectKind::Paint(a) | shard_model::EffectKind::Eraser(a) | shard_model::EffectKind::Smudge(a) => {
                                            let kind = match &e.kind {
                                                shard_model::EffectKind::Paint(_) => "paint",
                                                shard_model::EffectKind::Eraser(_) => "erase",
                                                _ => "smudge",
                                            };
                                            let colour = a.color.iter().find_map(|s| match s {
                                                shard_model::Source::Uniform { color: Some(c), .. } => Some(c[0]),
                                                _ => None,
                                            });
                                            let keys: usize = a.strokes.iter().map(|s| s.keys.len()).sum();
                                            format!(
                                                "{kind}: {} strokes, {keys} keys, size {:.3}, opacity {:.2}, colour {:?}, view_type {:?}",
                                                a.strokes.len(),
                                                a.brush.size,
                                                a.opacity,
                                                colour,
                                                a.strokes.iter().map(|s| s.view_type).collect::<Vec<_>>()
                                            )
                                        }
                                        shard_model::EffectKind::Fill(f) => {
                                            let src: Vec<String> = f.sources.iter().map(|s| s.kind().to_string()).collect();
                                            format!("fill [{}] {:?}", src.join(","), f.projection)
                                        }
                                        shard_model::EffectKind::Filter { procedural } => format!(
                                            "filter {}",
                                            procedural.as_ref().and_then(|p| p.graph_name()).unwrap_or("?")
                                        ),
                                        shard_model::EffectKind::Levels(_) => "levels".into(),
                                        shard_model::EffectKind::Anchor { .. } => format!("anchor {}", e.uid),
                                        shard_model::EffectKind::Group { children } => format!("group ({} children)", children.len()),
                                        shard_model::EffectKind::Other(_) => "other".into(),
                                    };
                                    // runs of paint effects collapse into one line
                                    if std::env::var_os("SHARD_LIST_STROKES").is_none() {
                                    if let (shard_model::EffectKind::Paint(a) | shard_model::EffectKind::Eraser(a) | shard_model::EffectKind::Smudge(a), Some(last)) = (&e.kind, out.last_mut()) {
                                        if let Some(rest) = last.trim_start().strip_prefix("paint/erase runs: ") {
                                            let mut it = rest.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty());
                                            let (n, k): (usize, usize) = (it.next().and_then(|v| v.parse().ok()).unwrap_or(0), it.next().and_then(|v| v.parse().ok()).unwrap_or(0));
                                            *last = format!("{}paint/erase runs: {} effects, {} strokes", " ".repeat(ind), n + 1, k + a.strokes.len());
                                            continue;
                                        }
                                        let _ = last;
                                    }
                                    }
                                    let what = if std::env::var_os("SHARD_LIST_STROKES").is_some() {
                                        what
                                    } else if let shard_model::EffectKind::Paint(a) | shard_model::EffectKind::Eraser(a) | shard_model::EffectKind::Smudge(a) = &e.kind {
                                        format!("paint/erase runs: 1 effects, {} strokes", a.strokes.len())
                                    } else {
                                        what
                                    };
                                    out.push(format!("{}{} {}{}", " ".repeat(ind), what, b, if e.visible { "" } else { " (hidden)" }));
                                    if let shard_model::EffectKind::Group { children } = &e.kind {
                                        sum(children, ind + 2, out);
                                    }
                                }
                            }
                            if !l.blend.is_empty() {
                                let bl: Vec<String> = l
                                    .blend
                                    .iter()
                                    .map(|b| {
                                        let ch: Vec<String> = (0..128u32).filter(|i| b.channels.contains(*i)).map(|i| i.to_string()).collect();
                                        let ch = if ch.len() > 12 { "all".to_string() } else { ch.join(",") };
                                        format!("{} {:.2} [{ch}]", b.mode.name(), b.opacity)
                                    })
                                    .collect();
                                println!("{}    blend: {}", "  ".repeat(depth), bl.join("; "));
                            }
                            let mut out = Vec::new();
                            sum(&l.content, 0, &mut out);
                            for o in out {
                                println!("{}    c: {o}", "  ".repeat(depth));
                            }
                            if let Some(m) = &l.mask {
                                println!("{}    mask: enabled={} initial={}", "  ".repeat(depth), m.enabled, m.initial);
                                let mut out = Vec::new();
                                sum(&m.effects, 0, &mut out);
                                for o in out {
                                    println!("{}      m: {o}", "  ".repeat(depth));
                                }
                            }
                        }
                        // SHARD_DUMP_LAYER=name prints the content and mask of that layer (children left out)
                        if std::env::var("SHARD_DUMP_LAYER").is_ok_and(|n| n == l.name) {
                            let mut c = l.clone();
                            c.children.clear();
                            println!("{:#?}", c);
                        }
                        walk(&l.children, depth + 1);
                    }
                }
                walk(&s.layers, 0);
            }
        }
        return ExitCode::SUCCESS;
    }
    let mut all = Vec::new();
    for e in &entries {
        if only.as_ref().is_some_and(|o| !e.name.eq_ignore_ascii_case(o)) {
            continue;
        }
        match run(e, res.or(e.res).unwrap_or(1024), set.as_deref(), dump.as_deref().map(std::path::Path::new)) {
            Ok(r) => all.push((e.name.clone(), r)),
            Err(err) => eprintln!("[{}] failed: {err}", e.name),
        }
    }
    let now = to_json(&all);
    if let Some(p) = json_out {
        if let Err(e) = std::fs::write(&p, serde_json::to_string_pretty(&now).unwrap()) {
            eprintln!("{p}: {e}");
        }
    }
    if let Some(b) = baseline {
        match std::fs::read_to_string(&b).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(prev) => {
                println!("== versus {b} (tolerance {tol} MAE)");
                let bad = compare(&prev, &now, tol);
                if bad > 0 {
                    println!("{bad} regression(s)");
                    return ExitCode::from(2);
                }
                println!("no regressions");
            }
            None => eprintln!("cannot read baseline {b}"),
        }
    }
    ExitCode::SUCCESS
}
