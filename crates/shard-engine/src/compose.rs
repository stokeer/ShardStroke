//! Headless composition of a texture set: the helpers the app's worker threads and the measurement
//! tools share (mesh maps lookup by name, stroke rasteriser, Options wiring).

use shard_compose::MeshMap;
use shard_model::{DocKind, Document, Effect, EffectKind, Layer};

/// The document of one texture set.
pub fn document(name: &str, layers: &[Layer]) -> Document {
    let mut layers = layers.to_vec();
    // diagnostic: SHARD_RAISE=name,name moves those layers (at any depth) one position up
    fn raise(ls: &mut Vec<Layer>, n: &str) -> bool {
        if let Some(i) = ls.iter().position(|l| l.name == n) {
            if i > 0 {
                ls.swap(i, i - 1);
            }
            return true;
        }
        ls.iter_mut().any(|l| raise(&mut l.children, n))
    }
    // diagnostic: SHARD_ROOT_ORDER=a,b,c puts those root layers on top in that order (top first)
    if let Ok(v) = std::env::var("SHARD_ROOT_ORDER") {
        let mut head: Vec<Layer> = Vec::new();
        for n in v.split(',').filter(|n| !n.is_empty()) {
            if let Some(i) = layers.iter().position(|l| l.name == n) {
                head.push(layers.remove(i));
            }
        }
        head.extend(layers.drain(..));
        layers = head;
    }
    // diagnostic: SHARD_ROOT_ASIS=1 reads the root list of the file as top-first (the model keeps it reversed)
    if std::env::var_os("SHARD_ROOT_ASIS").is_some() {
        layers.reverse();
    }
    if let Ok(v) = std::env::var("SHARD_RAISE") {
        for n in v.split(',') {
            raise(&mut layers, n);
        }
    }
    Document { kind: DocKind::Project, name: name.to_string(), root_class: String::new(), layers }
}

pub fn bake_matches(bake: MeshMap, url: &str) -> bool {
    let n = url.split('?').next().unwrap_or("").to_ascii_lowercase();
    match bake {
        MeshMap::AmbientOcclusion => {
            n.contains("ambient_occlusion") || n.ends_with("_ao_utility - raw") || n.contains("ambient occlusion map")
        }
        MeshMap::Curvature => n.contains("curvature"),
        MeshMap::WorldNormal => {
            n.contains("world_space_normal") || n.contains("worldspacenormal") || n.contains("world space normal")
        }
        MeshMap::NormalBase => {
            n.contains("normal_base") || n.ends_with("_normal_utility - raw") || n.contains("normal map from mesh")
        }
        MeshMap::Position => n.contains("position"),
        MeshMap::UvMask => false,
    }
}

const ASSIGN: char = '\u{1}';

/// Records that tell [`find_bake_url`] which resource a texture set is assigned for each mesh map. They
/// travel inside the url list (the project's `additionalMaps` / `normalMapBitmap`), so a set uses the maps
/// the project says it uses and not the first resource whose name looks right.
pub fn assignment_records(set: &str, assigned: &[(u32, String)]) -> Vec<String> {
    assigned.iter().map(|(u, url)| format!("{ASSIGN}{set}{ASSIGN}{u}{ASSIGN}{url}")).collect()
}

fn usage_of(bake: MeshMap) -> u32 {
    match bake {
        MeshMap::AmbientOcclusion => 1,
        MeshMap::Curvature => 2,
        MeshMap::WorldNormal => 3,
        MeshMap::Position => 4,
        MeshMap::NormalBase => 6,
        MeshMap::UvMask => 0,
    }
}

/// Resource URL of `bake` for the texture set `set_name`: the assignment recorded in the project first,
/// then the `<set>_` prefix, then containment.
pub fn find_bake_url<'a>(urls: &'a [String], set_name: &str, bake: MeshMap) -> Option<&'a str> {
    let want = format!("{ASSIGN}{set_name}{ASSIGN}{}{ASSIGN}", usage_of(bake));
    if let Some(r) = urls.iter().find(|u| u.starts_with(&want)) {
        return Some(&r[want.len()..]);
    }
    let urls: Vec<&String> = urls.iter().filter(|u| !u.starts_with(ASSIGN)).collect();
    let cands: Vec<&&String> = urls.iter().filter(|u| bake_matches(bake, u)).collect();
    let prefix = format!("{set_name}_");
    cands
        .iter()
        .find(|u| u.trim_start_matches('/').starts_with(&prefix))
        .or_else(|| cands.iter().find(|u| u.contains(set_name)))
        .map(|u| u.as_str())
}

/// Mesh maps of one texture set for the compositor.
pub struct SetMaps<'a> {
    pub bitmaps: &'a crate::bitmaps::ProjectBitmaps,
    pub urls: &'a [String],
    pub set: &'a str,
    /// Procedurals are rendered by the external Substance engine (when installed). Off for the quick
    /// low-resolution passes, which use the native approximations so the first picture is instant.
    pub exact: bool,
    /// Mesh and texture-set index. Diagnostic `SHARD_PAD_MESHMAPS=1` extends the baked maps (black outside
    /// the UV islands) outwards before graphs read them; measured worse, so Painter's raw maps are kept.
    pub mesh: Option<&'a crate::mesh::Mesh>,
    pub set_index: usize,
    padded: std::cell::RefCell<std::collections::HashMap<MeshMap, std::sync::Arc<shard_compose::Image>>>,
    surfaces: std::cell::RefCell<std::collections::HashMap<(u32, u32), std::sync::Arc<shard_compose::Surface>>>,
}

impl<'a> SetMaps<'a> {
    pub fn new(
        bitmaps: &'a crate::bitmaps::ProjectBitmaps,
        urls: &'a [String],
        set: &'a str,
        exact: bool,
        mesh: Option<&'a crate::mesh::Mesh>,
        set_index: usize,
    ) -> Self {
        Self { bitmaps, urls, set, exact, mesh, set_index, padded: Default::default(), surfaces: Default::default() }
    }
}

/// Whether a pass at `res` should use the exact external renderer (`SHARD_EXACT_ALL=1` forces it).
pub fn exact_for(res: u32) -> bool {
    res >= 1024 || std::env::var_os("SHARD_EXACT_ALL").is_some()
}

/// Render, in parallel, every procedural graph a stack will ask the external renderer for (generators and
/// materials; filters depend on the content below them and render when reached). The results land in the
/// disk cache, so the evaluation that follows only reads pictures instead of waiting for one process each.
pub fn prewarm(layers: &[Layer], maps: &SetMaps, res: u32) {
    if !maps.exact || crate::oracle::get().is_none() {
        return;
    }
    use shard_model::{Procedural, Source};
    fn effects(es: &[Effect], out: &mut Vec<(Procedural, Option<String>)>) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Fill(f) => {
                    for s in &f.sources {
                        if let Source::Procedural { procedural, .. } = s {
                            if procedural.outputs.len() > 1 {
                                for o in &procedural.outputs {
                                    out.push((procedural.clone(), Some(o.id.clone())));
                                }
                            } else {
                                out.push((procedural.clone(), None));
                            }
                        }
                    }
                }
                EffectKind::Group { children } => effects(children, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[Layer], out: &mut Vec<(Procedural, Option<String>)>) {
        for l in ls.iter().filter(|l| l.visible) {
            effects(&l.content, out);
            if let Some(m) = &l.mask {
                effects(&m.effects, out);
            }
            walk(&l.children, out);
        }
    }
    let mut jobs = Vec::new();
    walk(layers, &mut jobs);
    let mut seen = std::collections::HashSet::new();
    jobs.retain(|(p, o)| seen.insert(format!("{:?}|{:?}|{:?}", p.resource, p.tweaks, o)));
    // renders asked for before (by this project, set and size) are not requested again: building the requests of
    // a whole stack takes ~70 ms and an edit rarely adds a graph
    {
        use std::hash::{Hash, Hasher};
        static DONE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u64>>> = std::sync::OnceLock::new();
        let done = DONE.get_or_init(Default::default);
        let token = shard_compose::MeshMaps::cache_token(maps);
        if let (Some(token), Ok(mut done)) = (token, done.lock()) {
            jobs.retain(|(p, o)| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (token, res, format!("{:?}|{:?}|{:?}", p.resource, p.tweaks, o)).hash(&mut h);
                done.insert(h.finish())
            });
        }
    }
    if jobs.is_empty() {
        return;
    }
    let t0 = std::time::Instant::now();
    // the baked mesh maps the generators take as inputs decode in parallel (cached for every later job)
    {
        let urls: Vec<String> = [MeshMap::AmbientOcclusion, MeshMap::Curvature, MeshMap::WorldNormal, MeshMap::Position]
            .into_iter()
            .filter_map(|m| find_bake_url(maps.urls, maps.set, m).map(str::to_string))
            .collect();
        maps.bitmaps.preload(&urls);
    }
    // the requests are built here (the bitmap provider is single-threaded); the renders run in parallel
    let requests: Vec<crate::oracle::Request> = jobs
        .iter()
        .filter_map(|(p, out)| {
            let (rw, rh) = shard_compose::render_size(p, res, res)?;
            maps.build_request(p, out.as_deref(), rw, rh, None)
        })
        .collect();
    let Some(oracle) = crate::oracle::get() else { return };
    if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
        eprintln!("[time] prewarm requests built: {:?}", t0.elapsed());
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(2, 4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(req) = requests.get(i) else { break };
                let _ = oracle.render_shared(req);
            });
        }
    });
    if std::env::var_os("SHARD_TRACE").is_some() {
        eprintln!("[prewarm] {} graph render(s) at {res}: {:?}", requests.len(), t0.elapsed());
    }
}

impl shard_compose::MeshMaps for SetMaps<'_> {
    fn mesh_map(&self, map: MeshMap) -> Option<std::sync::Arc<shard_compose::Image>> {
        use shard_compose::BitmapProvider;
        if let Some(p) = self.padded.borrow().get(&map) {
            return Some(p.clone());
        }
        if map == MeshMap::UvMask {
            // coverage of the UV layout, the input of the UV-border generators
            let mesh = self.mesh?;
            let res = 1024u32;
            let cov = crate::raster::coverage(mesh, self.set_index, res, res);
            let data: Vec<f32> = cov.iter().map(|c| if *c { 1.0 } else { 0.0 }).collect();
            let img = std::sync::Arc::new(shard_compose::Image { w: res, h: res, n: 1, data });
            self.padded.borrow_mut().insert(map, img.clone());
            return Some(img);
        }
        let raw = self.bitmaps.bitmap(find_bake_url(self.urls, self.set, map)?)?;
        let Some(mesh) = self.mesh.filter(|_| std::env::var_os("SHARD_PAD_MESHMAPS").is_some()) else {
            return Some(raw);
        };
        let cov = crate::raster::erode(&crate::raster::coverage(mesh, self.set_index, raw.w, raw.h), raw.w, raw.h);
        let mut data = raw.data.clone();
        crate::raster::pad_f32(&mut data, raw.n, &cov, raw.w, raw.h, (raw.w / 32).clamp(8, 64));
        let img = std::sync::Arc::new(shard_compose::Image { w: raw.w, h: raw.h, n: raw.n, data });
        self.padded.borrow_mut().insert(map, img.clone());
        Some(img)
    }

    fn cache_token(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mesh = self.mesh?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.bitmaps.identity().hash(&mut h);
        (self.set, self.set_index, self.exact, crate::oracle::is_enabled(), mesh.fingerprint()).hash(&mut h);
        self.urls.hash(&mut h);
        Some(h.finish())
    }

    fn surface(&self, w: u32, h: u32) -> Option<std::sync::Arc<shard_compose::Surface>> {
        let mesh = self.mesh?;
        if let Some(s) = self.surfaces.borrow().get(&(w, h)) {
            return Some(s.clone());
        }
        let t0 = std::time::Instant::now();
        let s = std::sync::Arc::new(crate::raster::surface(mesh, self.set_index, w, h));
        if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
            eprintln!("[time] surface raster {w}x{h}: {:?}", t0.elapsed());
        }
        self.surfaces.borrow_mut().insert((w, h), s.clone());
        Some(s)
    }

    fn embedded(&self, resource: &str, index: usize) -> Option<std::sync::Arc<shard_compose::Image>> {
        use shard_compose::BitmapProvider;
        self.bitmaps.bitmap(&format!("{resource}#e{index}"))
    }

    fn render_graph(
        &self,
        p: &shard_model::Procedural,
        output: Option<&str>,
        w: u32,
        h: u32,
        content: Option<&shard_compose::Image>,
    ) -> Option<std::sync::Arc<shard_compose::Image>> {
        let oracle = crate::oracle::get()?;
        let req = self.build_request(p, output, w, h, content)?;
        let img = oracle.render_shared(&req);
        // SHARD_DUMP_RENDER=DIR: every rendered output as DIR/<graph>_<output>_<w>.pgm (first component, 0..1)
        if let (Some(dir), Some(i)) = (std::env::var_os("SHARD_DUMP_RENDER"), img.as_ref()) {
            let name = format!("{}_{}_{}.pgm", p.graph_name().unwrap_or("graph"), output.unwrap_or("main"), i.w);
            let mut b = format!("P5\n{} {}\n255\n", i.w, i.h).into_bytes();
            b.extend(i.data.chunks_exact(i.n).map(|px| (px[0].clamp(0.0, 1.0) * 255.0).round() as u8));
            let _ = std::fs::write(std::path::Path::new(&dir).join(name.replace(['/', '\\', ':'], "_")), b);
            if i.n == 4 || i.n == 2 {
                let name = format!("{}_{}_{}_alpha.pgm", p.graph_name().unwrap_or("graph"), output.unwrap_or("main"), i.w);
                let mut b = format!("P5\n{} {}\n255\n", i.w, i.h).into_bytes();
                b.extend(i.data.chunks_exact(i.n).map(|px| (px[i.n - 1].clamp(0.0, 1.0) * 255.0).round() as u8));
                let _ = std::fs::write(std::path::Path::new(&dir).join(name.replace(['/', '\\', ':'], "_")), b);
            }
            eprintln!("[dump render] {} '{}' {}x{} n={}", p.graph_name().unwrap_or("?"), output.unwrap_or("main"), i.w, i.h, i.n);
        }
        img
    }
}

impl SetMaps<'_> {
    /// The picture a source plugged into a graph input gives: another graph rendered at the same size, a bitmap
    /// of the project, or a flat colour. `None` for what only the compositor knows (anchor references).
    fn input_image(&self, s: &shard_model::Source, w: u32, h: u32) -> Option<std::sync::Arc<shard_compose::Image>> {
        use shard_compose::{BitmapProvider, MeshMaps};
        match s {
            shard_model::Source::Procedural { procedural, .. } => self.render_graph(procedural, None, w, h, None),
            shard_model::Source::Bitmap { resource: Some(r), .. } => self.bitmaps.bitmap(r),
            shard_model::Source::Uniform { color: Some(c), .. } => {
                Some(std::sync::Arc::new(shard_compose::Image::filled(w.min(64), h.min(64), &[c[0], c[1], c[2]])))
            }
            _ => None,
        }
    }

    /// Everything the external renderer needs for one graph: archive, numeric inputs and image inputs.
    pub fn build_request(
        &self,
        p: &shard_model::Procedural,
        output: Option<&str>,
        w: u32,
        h: u32,
        content: Option<&shard_compose::Image>,
    ) -> Option<crate::oracle::Request> {
        use shard_compose::BitmapProvider;
        if !self.exact {
            return None;
        }
        // diagnostics: SHARD_ORACLE_ONLY=a,b renders only these graphs, SHARD_ORACLE_SKIP=a,b never does
        let gname = p.graph_name().unwrap_or("");
        if let Ok(v) = std::env::var("SHARD_ORACLE_ONLY") {
            if !v.split(',').any(|x| x == gname) {
                return None;
            }
        }
        if let Ok(v) = std::env::var("SHARD_ORACLE_SKIP") {
            if v.split(',').any(|x| x == gname) {
                return None;
            }
        }
        let trace = std::env::var_os("SHARD_TRACE_RENDER").is_some();
        let resource = p.resource.as_deref()?;
        let t_a = std::time::Instant::now();
        let Some(bytes) = self.bitmaps.sbsar_bytes(resource) else {
            if trace {
                eprintln!("[request] {gname}: archive not found for {resource}");
            }
            return None;
        };
        let t_b = std::time::Instant::now();
        let iface = match shard_formats::sbsar::Interface::from_sbsar(&bytes) {
            Ok(i) => i,
            Err(e) => {
                if trace {
                    eprintln!("[request] {gname}: interface of the archive ({} bytes) not readable: {e:?}", bytes.len());
                }
                return None;
            }
        };
        if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
            eprintln!("[time]   request {gname}: sbsar_bytes {:?}, interface {:?} ({} bytes)", t_b - t_a, t_b.elapsed(), bytes.len());
        }
        if std::env::var_os("SHARD_TRACE_PARAMS").is_some() {
            for q in &iface.params {
                eprintln!("[params] {gname}: {} ty={} comps={} default={:?} widget={:?}", q.identifier, q.ty, q.comps, q.default, q.widget);
            }
            eprintln!("[params] {gname}: image inputs {:?}, outputs {:?}", iface.images, iface.outputs);
        }
        if std::env::var_os("SHARD_TRACE_OUTPUTS").is_some() {
            eprintln!("[outputs] {gname}: usages {:?}", iface.output_usages);
            for (id, xml) in &iface.output_xml {
                eprintln!("[outputs]   {id}: {}", xml.chars().take(600).collect::<String>());
            }
        }
        // numeric inputs: the stored tweaks that exist in the interface (the size is ours)
        let known: std::collections::HashSet<&str> = iface.params.iter().map(|q| q.identifier.as_str()).collect();
        let mut values: Vec<(String, String)> = Vec::new();
        for (k, t) in &p.tweaks {
            if k == "$outputsize" || !(known.contains(k.as_str()) || k == "$randomseed") {
                continue;
            }
            use shard_model::Tweak;
            let v = match t {
                Tweak::Float(f) => format!("{f}"),
                Tweak::Int(i) => format!("{i}"),
                Tweak::Bool(b) => (*b as i32).to_string(),
                Tweak::Int2([a, b]) => format!("{a},{b}"),
                Tweak::Float2(v) => format!("{},{}", v[0], v[1]),
                Tweak::Float3(v) => format!("{},{},{}", v[0], v[1], v[2]),
                Tweak::Float4(v) => format!("{},{},{},{}", v[0], v[1], v[2], v[3]),
                Tweak::Other => continue,
            };
            values.push((k.clone(), v));
        }
        // experiments: SHARD_ORACLE_SET="graph:key=value;graph:key=value" overrides numeric inputs
        if let Ok(spec) = std::env::var("SHARD_ORACLE_SET") {
            for item in spec.split(';') {
                if let Some((g, kv)) = item.split_once(':') {
                    if g == p.graph_name().unwrap_or("") {
                        if let Some((k, v)) = kv.split_once('=') {
                            values.retain(|(x, _)| x != k);
                            values.push((k.to_string(), v.to_string()));
                        }
                    }
                }
            }
        }
        values.push(("$outputsize".into(), format!("{},{}", w.trailing_zeros(), h.trailing_zeros())));
        // image inputs: baked mesh maps by name, and the content a filter processes
        let mut inputs: Vec<(String, std::sync::Arc<shard_compose::Image>)> = Vec::new();
        let mut content_given = false;
        // which image input receives the content of a filter: the one named like the output
        // (`roughness` -> `roughness`), else `input` / `source`, else the first free colour input
        let mesh_map_of = |id: &str| -> Option<MeshMap> {
            if id.contains("curvature") {
                Some(MeshMap::Curvature)
            } else if id.contains("ambient_occlusion") || id == "ao" || id.contains("occlusion") {
                Some(MeshMap::AmbientOcclusion)
            } else if id.contains("world_space_normal") {
                Some(MeshMap::WorldNormal)
            } else if id == "mesh_uv_mask" || id.contains("uv_mask") {
                Some(MeshMap::UvMask)
            } else if id.contains("position") {
                Some(MeshMap::Position)
            } else if id == "normal_base"
                || id == "mesh_normal"
                || (id == "micro_normal" && std::env::var_os("SHARD_FEED_MICRO_NORMAL").is_some())
            {
                // experiment: the "micro details" input of the curvature / edge generators is the normal
                // channel of the stack below; feeding the plain baked normal measured worse, so it stays off
                Some(MeshMap::NormalBase)
            } else {
                None
            }
        };
        let images: &Vec<String> = &iface.images;
        let content_input: Option<String> = content.and_then(|_| {
            let want = output.map(|o| o.to_ascii_lowercase());
            images
                .iter()
                .find(|q| want.as_deref() == Some(q.to_ascii_lowercase().as_str()))
                .or_else(|| images.iter().find(|q| matches!(q.to_ascii_lowercase().as_str(), "input" | "source" | "input_1")))
                .or_else(|| {
                    images.iter().find(|q| {
                        let id = q.to_ascii_lowercase();
                        mesh_map_of(&id).is_none() && !id.ends_with("_alpha") && !id.contains("grunge") && !id.contains("micro")
                    })
                })
                .cloned()
        });
        for qid in images.iter() {
            let q = qid.as_str();
            let id = q.to_ascii_lowercase();
            // an input the layer feeds from a source of its own (the mask editor's `texture`: a grunge procedural):
            // rendered at the same size and bound. Left empty, a test tank's "dirt sharp" mask came out 0.78 against
            // Painter's ~0.36. SHARD_NO_INPUT_SOURCES=1 leaves them unconnected.
            if let Some(k) = p.input_ids.iter().position(|x| x == q) {
                if std::env::var_os("SHARD_NO_INPUT_SOURCES").is_none() {
                    if let Some(img) = p.input_sources.get(k).and_then(|s| self.input_image(s, w, h)) {
                        if trace {
                            eprintln!("[request] {gname}: input '{q}' fed from its source ({}x{}x{})", img.w, img.h, img.n);
                        }
                        inputs.push((q.to_string(), img));
                        continue;
                    }
                }
            }
            if content_input.as_deref() == Some(q) {
                if let Some(c) = content {
                    inputs.push((q.to_string(), std::sync::Arc::new(c.clone())));
                    content_given = true;
                }
                continue;
            }
            if let Some(c) = content {
                // a filter's alpha pin: fully opaque (an unconnected alpha makes the result black)
                if id.ends_with("_alpha") {
                    let white = shard_compose::Image { w: c.w, h: c.h, n: 1, data: vec![1.0; (c.w * c.h) as usize] };
                    inputs.push((q.to_string(), std::sync::Arc::new(white)));
                    continue;
                }
            }
            if let Some(m) = mesh_map_of(&id) {
                let t_m = std::time::Instant::now();
                if let Some(img) = shard_compose::MeshMaps::mesh_map(self, m) {
                    inputs.push((q.to_string(), img));
                }
                if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
                    eprintln!("[time]   mesh map {m:?} for {gname}: {:?}", t_m.elapsed());
                }
            }
        }
        if content.is_some() && !content_given {
            return None; // a filter we cannot feed
        }
        // several outputs (`output` + `output_Alpha`): ask for the colour one by name
        let output = output
            .map(str::to_string)
            .or_else(|| iface.outputs.iter().find(|o| o.eq_ignore_ascii_case("output")).cloned());
        Some(crate::oracle::Request {
            sbsar: bytes,
            output,
            values,
            inputs,
            size_log2: w.trailing_zeros(),
            label: p.graph_name().unwrap_or("?").to_string(),
        })
    }
}

pub fn collect_key_u(layers: &[Layer], out: &mut Vec<f32>) {
    fn effects(es: &[shard_model::Effect], out: &mut Vec<f32>) {
        for e in es {
            match &e.kind {
                EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => {
                    for s in a.strokes.iter().filter(|s| s.view_type != 0) {
                        out.extend(s.keys.iter().map(|k| k.pos[0]));
                    }
                }
                EffectKind::Group { children } => effects(children, out),
                _ => {}
            }
        }
    }
    for l in layers {
        effects(&l.content, out);
        if let Some(m) = &l.mask {
            effects(&m.effects, out);
        }
        collect_key_u(&l.children, out);
    }
}

/// `compose_job` that also returns every layer's mask (u8, at most 512 wide) for the mask cache.
pub fn compose_job_masks(
    doc: &Document,
    bitmaps: Option<&crate::bitmaps::ProjectBitmaps>,
    urls: &[String],
    set_name: &str,
    mesh: &crate::mesh::Mesh,
    set: usize,
    bits: &[u32],
    res: u32,
    aspect: f32,
) -> (shard_compose::Composite, Vec<(Vec<usize>, u32, Vec<u8>)>) {
    let none = shard_compose::NoBitmaps;
    let provider: &dyn shard_compose::BitmapProvider = bitmaps.map(|b| b as _).unwrap_or(&none);
    let no_maps = shard_compose::NoMaps;
    let set_maps = bitmaps.map(|b| SetMaps::new(b, urls, set_name, exact_for(res), Some(mesh), set));
    let maps: &dyn shard_compose::MeshMaps = set_maps.as_ref().map(|m| m as _).unwrap_or(&no_maps);
    let raster = crate::strokes::SetStrokes::new(mesh, set, aspect);
    let strokes: Option<&dyn shard_compose::StrokeRaster> =
        if std::env::var_os("SHARD_NO_STROKES").is_some() { None } else { Some(&raster) };
    if let Some(m) = &set_maps {
        prewarm(&doc.layers, m, res);
    }
    let opts = shard_compose::Options { bitmaps: provider, maps, strokes, skip_unsupported: true, allow_approx: true, aces_working: bitmaps.is_some_and(|b| b.aces_working()) };
    let (c, masks) = shard_compose::compose_with_masks(doc, res, res, bits, &opts);
    // The masks kept for the mask view: up to 1024 wide (it was 512, which looked blurry on a 2K set), box
    // filtered down, and halved while the whole set would exceed ~96 MB of 8-bit data.
    let mut out_res = res.min(1024);
    while out_res > 256 && masks.len() * (out_res as usize) * (out_res as usize) > (96 << 20) {
        out_res /= 2;
    }
    let step = (res / out_res).max(1);
    let out_res = res / step;
    let area = (step * step) as f32;
    let packed = masks
        .into_iter()
        .map(|(path, v)| {
            let mut q = Vec::with_capacity((out_res * out_res) as usize);
            for y in 0..out_res {
                for x in 0..out_res {
                    let mut acc = 0.0f32;
                    for dy in 0..step {
                        let row = ((y * step + dy) * res + x * step) as usize;
                        acc += v[row..row + step as usize].iter().map(|g| g.clamp(0.0, 1.0)).sum::<f32>();
                    }
                    q.push((acc / area * 255.0 + 0.5) as u8);
                }
            }
            (path, out_res, q)
        })
        .collect();
    (c, packed)
}

/// Final tangent-space normal map of a set at `res`: the baked mesh normal map (decoded at up to twice that
/// size and averaged down) combined with the stack's Height and Normal channels. `None` when the stack writes
/// neither channel (the baked map is shown as it is). Free of `AppState` so a worker thread can run it.
pub fn normal_job(
    doc: &Document,
    bitmaps: Option<&crate::bitmaps::ProjectBitmaps>,
    urls: &[String],
    set_name: &str,
    mesh: &crate::mesh::Mesh,
    set: usize,
    res: u32,
    aspect: f32,
    height_normal: Option<(Option<&shard_compose::Image>, Option<&shard_compose::Image>)>,
) -> Option<shard_compose::Image> {
    let (h, n) = match height_normal {
        Some(hn) => hn,
        None => {
            let bits: Vec<u32> = [1u32, 22].into_iter().filter(|b| shard_compose::writes_channel(doc, *b)).collect();
            if bits.is_empty() {
                return None;
            }
            let c = compose_job(doc, bitmaps, urls, set_name, mesh, set, &bits, res, aspect);
            let (h, n) = (c.channels.get(&1).cloned(), c.channels.get(&22).cloned());
            return assemble_job_normal(bitmaps, urls, set_name, res, h.as_ref(), n.as_ref(), Some((mesh, set)));
        }
    };
    assemble_job_normal(bitmaps, urls, set_name, res, h, n, Some((mesh, set)))
}

/// The assembly half of [`normal_job`], for callers that composed the channels themselves.
pub fn assemble_job_normal(
    bitmaps: Option<&crate::bitmaps::ProjectBitmaps>,
    urls: &[String],
    set_name: &str,
    res: u32,
    height: Option<&shard_compose::Image>,
    normal: Option<&shard_compose::Image>,
    mesh_set: Option<(&crate::mesh::Mesh, usize)>,
) -> Option<shard_compose::Image> {
    if height.is_none() && normal.is_none() {
        return None;
    }
    let mut mesh_normal = find_bake_url(urls, set_name, MeshMap::NormalBase)
        .and_then(|u| bitmaps.and_then(|b| b.bitmap_full(u, (res * 2).clamp(1024, 4096))));
    // The bake has a black background outside the islands (its outermost ring blended with it): extended outwards at
    // its own size before it is reduced and combined, or the reduction mixes the black into the border texels and
    // every UV seam shows a dark line (a test vehicle interior's seat bases). SHARD_NO_BAKE_PAD=1 keeps it raw.
    if let (Some(m), Some((mesh, set))) = (mesh_normal.as_mut(), mesh_set) {
        if m.w == m.h && std::env::var_os("SHARD_NO_BAKE_PAD").is_none() {
            let cov = crate::raster::erode(&crate::raster::coverage(mesh, set, m.w, m.h), m.w, m.h);
            crate::raster::dilate_f32(&mut m.data, m.n, &cov, m.w, m.h, (m.w / 64).clamp(8, 32));
        }
    }
    // The height is extended past the island borders before its slopes are taken: outside an island it is 0, and
    // the step at every UV seam turned into a dark line in the normal (a test vehicle interior). Painter takes the slopes
    // on the surface, where a seam is not an edge. SHARD_NO_HEIGHT_PAD=1 keeps the raw borders.
    let padded = match (height, mesh_set) {
        (Some(h), Some((mesh, set))) if std::env::var_os("SHARD_NO_HEIGHT_PAD").is_none() => {
            let cov = crate::raster::coverage(mesh, set, h.w, h.h);
            let mut img = h.clone();
            crate::raster::dilate_f32(&mut img.data, img.n, &cov, img.w, img.h, (img.w / 64).clamp(4, 32));
            Some(img)
        }
        _ => None,
    };
    let height = padded.as_ref().or(height);
    Some(shard_compose::assemble_normal(mesh_normal.as_ref(), height, normal, res, res, &shard_compose::NormalMix::default()))
}

pub fn layer_name_at(layers: &[Layer], path: &[usize]) -> String {
    let mut ls = layers;
    for (k, &i) in path.iter().enumerate() {
        match ls.get(i) {
            Some(l) if k + 1 == path.len() => return l.name.clone(),
            Some(l) => ls = &l.children,
            None => break,
        }
    }
    "?".into()
}

/// Mask of one layer, evaluated like `compose_job` (free of `AppState` for worker threads).
pub fn mask_job(
    doc: &Document,
    bitmaps: Option<&crate::bitmaps::ProjectBitmaps>,
    urls: &[String],
    set_name: &str,
    mesh: &crate::mesh::Mesh,
    set: usize,
    path: &[usize],
    res: u32,
    aspect: f32,
) -> Option<shard_compose::MaskPreview> {
    let none = shard_compose::NoBitmaps;
    let provider: &dyn shard_compose::BitmapProvider = bitmaps.map(|b| b as _).unwrap_or(&none);
    let no_maps = shard_compose::NoMaps;
    let set_maps = bitmaps.map(|b| SetMaps::new(b, urls, set_name, exact_for(res), Some(mesh), set));
    let maps: &dyn shard_compose::MeshMaps = set_maps.as_ref().map(|m| m as _).unwrap_or(&no_maps);
    // the stroke rasteriser is costly to build: only when the stack actually has strokes to replay
    fn has_strokes(layers: &[Layer]) -> bool {
        fn eff(es: &[Effect]) -> bool {
            es.iter().any(|e| match &e.kind {
                EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => !a.strokes.is_empty(),
                EffectKind::Group { children } => eff(children),
                _ => false,
            })
        }
        layers.iter().any(|l| eff(&l.content) || l.mask.as_ref().is_some_and(|m| eff(&m.effects)) || has_strokes(&l.children))
    }
    let t0 = std::time::Instant::now();
    let raster = has_strokes(&doc.layers).then(|| crate::strokes::SetStrokes::new(mesh, set, aspect));
    let strokes: Option<&dyn shard_compose::StrokeRaster> = raster.as_ref().map(|r| r as _);
    if std::env::var_os("SHARD_TRACE").is_some() {
        eprintln!("[mask job] stroke raster: {:?} (built: {})", t0.elapsed(), raster.is_some());
    }
    let opts = shard_compose::Options { bitmaps: provider, maps, strokes, skip_unsupported: true, allow_approx: true, aces_working: bitmaps.is_some_and(|b| b.aces_working()) };
    shard_compose::layer_mask(doc, path, res, res, &opts)
}

/// One thumbnail of the layers panel: the content of a layer on the shown channel, or its mask.
pub struct LayerThumb {
    /// Stable identity of the layer.
    pub uid: u64,
    /// The layer's mask (true) or its content (false).
    pub mask: bool,
    /// Side of the square picture in pixels.
    pub size: u32,
    /// RGBA8 (the content is sRGB-encoded for colour channels).
    pub rgba: Vec<u8>,
}

fn uid_at(layers: &[Layer], path: &[usize]) -> Option<u64> {
    let mut ls = layers;
    for (k, &i) in path.iter().enumerate() {
        let l = ls.get(i)?;
        if k + 1 == path.len() {
            return Some(l.uid);
        }
        ls = &l.children;
    }
    None
}

/// Thumbnails of every layer of a set on channel `bit`, evaluated at `res` x `res` and reduced to `res / 2`
/// (a box filter hides the aliasing of sampling big textures that small). Free of `AppState`.
pub fn thumbs_job(
    doc: &Document,
    bitmaps: Option<&crate::bitmaps::ProjectBitmaps>,
    urls: &[String],
    set_name: &str,
    mesh: &crate::mesh::Mesh,
    set: usize,
    bit: u32,
    res: u32,
    aspect: f32,
    exact: bool,
) -> Vec<LayerThumb> {
    let none = shard_compose::NoBitmaps;
    let provider: &dyn shard_compose::BitmapProvider = bitmaps.map(|b| b as _).unwrap_or(&none);
    let no_maps = shard_compose::NoMaps;
    let set_maps = bitmaps.map(|b| SetMaps::new(b, urls, set_name, exact, Some(mesh), set));
    let maps: &dyn shard_compose::MeshMaps = set_maps.as_ref().map(|m| m as _).unwrap_or(&no_maps);
    fn has_strokes(layers: &[Layer]) -> bool {
        fn eff(es: &[Effect]) -> bool {
            es.iter().any(|e| match &e.kind {
                EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => !a.strokes.is_empty(),
                EffectKind::Decal(_) => true,
                EffectKind::Group { children } => eff(children),
                _ => false,
            })
        }
        layers.iter().any(|l| eff(&l.content) || l.mask.as_ref().is_some_and(|m| eff(&m.effects)) || has_strokes(&l.children))
    }
    let raster = has_strokes(&doc.layers).then(|| crate::strokes::SetStrokes::new(mesh, set, aspect));
    let strokes: Option<&dyn shard_compose::StrokeRaster> = raster.as_ref().map(|r| r as _);
    if exact {
        if let Some(m) = &set_maps {
            prewarm(&doc.layers, m, res);
        }
    }
    let opts = shard_compose::Options { bitmaps: provider, maps, strokes, skip_unsupported: true, allow_approx: true, aces_working: bitmaps.is_some_and(|b| b.aces_working()) };
    let t = shard_compose::thumbnails(doc, res, res, bit, &opts);
    let out_size = (res / 2).max(1);
    // colour channels are stored linear; the picture shows them sRGB-encoded
    let encode = matches!(bit, 0 | 4);
    let q = |v: f32| ((if encode { shard_compose::linear_to_srgb(v.clamp(0.0, 1.0)) } else { v.clamp(0.0, 1.0) }) * 255.0 + 0.5) as u8;
    // average of the 2x2 block under each output pixel, per component
    let reduce = |img: &shard_compose::Image| -> Vec<u8> {
        let (w, h, n) = (img.w as usize, img.h as usize, img.n);
        let o = out_size as usize;
        let mut rgba = Vec::with_capacity(o * o * 4);
        for y in 0..o {
            for x in 0..o {
                let mut acc = [0.0f32; 3];
                let mut cnt = 0.0;
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                    let p = &img.data[(sy * w + sx) * n..(sy * w + sx) * n + n];
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += p[c.min(n - 1)];
                    }
                    cnt += 1.0;
                }
                rgba.extend_from_slice(&[q(acc[0] / cnt), q(acc[1] / cnt), q(acc[2] / cnt), 255]);
            }
        }
        rgba
    };
    let mut out = Vec::new();
    for (path, img) in &t.contents {
        if let Some(uid) = uid_at(&doc.layers, path) {
            out.push(LayerThumb { uid, mask: false, size: out_size, rgba: reduce(img) });
        }
    }
    for (path, values) in &t.masks {
        if let Some(uid) = uid_at(&doc.layers, path) {
            let img = shard_compose::Image { w: res, h: res, n: 1, data: values.clone() };
            // masks are data: no sRGB encoding
            let (w, h) = (img.w as usize, img.h as usize);
            let o = out_size as usize;
            let mut rgba = Vec::with_capacity(o * o * 4);
            for y in 0..o {
                for x in 0..o {
                    let mut s = 0.0;
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        s += img.data[((y * 2 + dy).min(h - 1)) * w + (x * 2 + dx).min(w - 1)];
                    }
                    let v = ((s / 4.0).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    rgba.extend_from_slice(&[v, v, v, 255]);
                }
            }
            out.push(LayerThumb { uid, mask: true, size: out_size, rgba });
        }
    }
    out
}

/// Approximate evaluation of a set's stack. Free of `AppState` so a worker thread can run it.
pub fn compose_job(
    doc: &Document,
    bitmaps: Option<&crate::bitmaps::ProjectBitmaps>,
    urls: &[String],
    set_name: &str,
    mesh: &crate::mesh::Mesh,
    set: usize,
    bits: &[u32],
    res: u32,
    aspect: f32,
) -> shard_compose::Composite {
    let none = shard_compose::NoBitmaps;
    let provider: &dyn shard_compose::BitmapProvider = bitmaps.map(|b| b as _).unwrap_or(&none);
    let no_maps = shard_compose::NoMaps;
    let set_maps = bitmaps.map(|b| SetMaps::new(b, urls, set_name, exact_for(res), Some(mesh), set));
    let maps: &dyn shard_compose::MeshMaps = set_maps.as_ref().map(|m| m as _).unwrap_or(&no_maps);
    let t_setup = std::time::Instant::now();
    let raster = crate::strokes::SetStrokes::new(mesh, set, aspect);
    let strokes: Option<&dyn shard_compose::StrokeRaster> =
        if std::env::var_os("SHARD_NO_STROKES").is_some() { None } else { Some(&raster) };
    if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
        eprintln!("[time] stroke raster setup: {:?}", t_setup.elapsed());
    }
    if let Some(m) = &set_maps {
        prewarm(&doc.layers, m, res);
    }
    if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
        eprintln!("[time] setup + prewarm: {:?}", t_setup.elapsed());
    }
    let opts = shard_compose::Options { bitmaps: provider, maps, strokes, skip_unsupported: true, allow_approx: true, aces_working: bitmaps.is_some_and(|b| b.aces_working()) };
    shard_compose::compose_with(doc, res, res, bits, &opts)
}


