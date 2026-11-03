//! Build the ShardStroke model from Painter data files (`.spsm/.spmsk/.sppr/.spt/.spp`).

use std::collections::BTreeMap;
use std::path::Path;

use shard_formats::alg::{Object, Value};
use shard_formats::Container;

use crate::*;

#[derive(Debug)]
pub struct ImportError(pub String);
impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "import: {}", self.0)
    }
}
impl std::error::Error for ImportError {}

fn e<T>(s: impl Into<String>) -> Result<T, ImportError> {
    Err(ImportError(s.into()))
}

const MEMBERS: [&str; 4] = ["preset.bin", "paint/document.bin", "preset_2.bin", "preset_1.bin"];

pub fn import_file(path: impl AsRef<Path>) -> Result<Document, ImportError> {
    let path = path.as_ref();
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    let kind = match ext.as_str() {
        "spsm" => DocKind::SmartMaterial,
        "spmsk" => DocKind::SmartMask,
        "sppr" => DocKind::Preset,
        "spt" => DocKind::Template,
        "spp" => DocKind::Project,
        x => return e(format!("unsupported extension .{x}")),
    };
    let mut c = Container::open(path).map_err(|x| ImportError(x.to_string()))?;
    let have: Vec<String> = c.members().map(|d| d.path.clone()).collect();
    let mut root = None;
    for m in MEMBERS {
        if have.iter().any(|h| h == m) {
            if let Ok(v) = c.decode(m) {
                root = Some(v);
                break;
            }
        }
    }
    let root = match root {
        Some(r) => r,
        None => return e("no decodable preset/document member"),
    };
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let layers = from_root(&root)?;
    let root_class = root.as_object().map(|o| o.class.clone()).unwrap_or_default();
    Ok(Document { kind, name, root_class, layers })
}

/// Layer stack of one material (texture set) of a project.
#[derive(Debug, Clone)]
pub struct MaterialStack {
    /// `sceneMaterialName` (matches the material names stored in the project scene).
    pub name: String,
    pub layers: Vec<Layer>,
    /// Texture resolution of the material (width, height) in texels (`resolutionLog2`).
    pub resolution: (u32, u32),
    /// Mesh maps assigned to the texture set (`additionalMaps`): `(usage, resource url)` with usage
    /// 1 ambient occlusion, 2 curvature, 3 world space normal, 4 position. The base normal map
    /// (`normalMapBitmap`) is reported with usage 6.
    pub mesh_maps: Vec<(u32, String)>,
    /// `sceneMaterialUID`: the key of the set's channels in Painter's texture cache
    /// (`texture-cache/<uid>/...`, see `shard_formats::texcache`).
    pub uid: u64,
    /// `height2NormalMethod` of the texture set (how Painter turns the Height channel into normals).
    pub height_to_normal_method: i64,
}

/// Per-material layer stacks of a `.spp` project (`paint/document.bin`).
pub fn import_project_stacks(path: impl AsRef<Path>) -> Result<Vec<MaterialStack>, ImportError> {
    let mut c = Container::open(path.as_ref()).map_err(|x| ImportError(x.to_string()))?;
    let root = c.decode("paint/document.bin").map_err(|x| ImportError(x.to_string()))?;
    let o = root.as_object().ok_or_else(|| ImportError("document root is not an object".into()))?;
    let mut out = Vec::new();
    for m in list(o, "materials") {
        let Some(mo) = m.as_object() else { continue };
        let name = mo.get("sceneMaterialName").and_then(Value::as_str).unwrap_or("").to_string();
        let mut layers = Vec::new();
        if let Some(st) = list(mo, "stacks").first().and_then(Value::as_object) {
            if let Some(stack) = st.get("stack").and_then(Value::as_object) {
                // items are stored bottom-to-top; the model keeps stacks top-first like the UI
                for it in list(stack, "items").iter().rev() {
                    if let Some(l) = it.as_object() {
                        layers.push(layer(l));
                    }
                }
                // The root list is bottom-to-top like the nested ones: the layer-by-layer export of the Painter plugin
                // (an interior set: a filter stack on top of a group) agrees with the plain
                // reading. An older rule (plain layers on top, among the groups the first one of the file on top and the
                // rest bottom-to-top below it) had been fitted to final exports and was hiding other defects;
                // SHARD_ROOT_HEURISTIC=1 brings it back for comparison.
                if layers.len() > 1 && std::env::var_os("SHARD_ROOT_HEURISTIC").is_some() {
                    let (plain, mut groups): (Vec<Layer>, Vec<Layer>) =
                        layers.drain(..).partition(|l| l.kind != LayerKind::Group);
                    if groups.len() > 1 {
                        groups.rotate_right(1);
                    }
                    layers = plain;
                    layers.extend(groups);
                }
            }
        }
        let res = match mo.get("resolutionLog2") {
            Some(Value::Raw(_, b)) if b.len() == 8 => {
                let w = i32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(0, 15) as u32;
                let h = i32::from_le_bytes([b[4], b[5], b[6], b[7]]).clamp(0, 15) as u32;
                (1u32 << w, 1u32 << h)
            }
            _ => (1024, 1024),
        };
        let mut mesh_maps: Vec<(u32, String)> = Vec::new();
        for am in list(mo, "additionalMaps").iter().filter_map(Value::as_object) {
            let usage = am.get("usage").and_then(Value::as_i64).unwrap_or(-1);
            if let (true, Some(url)) = (usage >= 0, am.get("urlToBitmapRes").and_then(Value::as_str)) {
                if !url.is_empty() {
                    mesh_maps.push((usage as u32, url.to_string()));
                }
            }
        }
        if let Some(url) = sub(mo, "normalMapBitmap").and_then(|b| b.get("urlToBitmapRes")).and_then(Value::as_str) {
            if !url.is_empty() {
                mesh_maps.push((6, url.to_string()));
            }
        }
        let uid = raw_u64(mo.get("sceneMaterialUID"));
        let height_to_normal_method = mo.get("height2NormalMethod").and_then(Value::as_i64).unwrap_or(0);
        out.push(MaterialStack { name, layers, resolution: res, mesh_maps, uid, height_to_normal_method });
    }
    Ok(out)
}

pub fn from_root(root: &Value) -> Result<Vec<Layer>, ImportError> {
    let o = root.as_object().ok_or_else(|| ImportError("root is not an object".into()))?;
    match o.class.as_str() {
        "DataLayersPreset" => Ok(o
            .get("layerGroup")
            .and_then(Value::as_object)
            .map(|g| vec![layer(g)])
            .unwrap_or_default()),
        "DataDocument" => {
            let mut out = Vec::new();
            for m in list(o, "materials") {
                if let Some(mo) = m.as_object() {
                    for s in list(mo, "stacks") {
                        if let Some(so) = s.as_object() {
                            if let Some(st) = so.get("stack").and_then(Value::as_object) {
                                for it in list(st, "items").iter().rev() {
                                    if let Some(l) = it.as_object() {
                                        out.push(layer(l));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(out)
        }
        // Brush/tool presets etc.: no layer stack. Callers can inspect `Document::root_class`.
        _ => Ok(Vec::new()),
    }
}

fn list<'a>(o: &'a Object, k: &str) -> &'a [Value] {
    o.get(k).and_then(Value::as_list).unwrap_or(&[])
}

fn sub<'a>(o: &'a Object, k: &str) -> Option<&'a Object> {
    o.get(k).and_then(Value::as_object)
}

fn visible(o: &Object) -> bool {
    match o.get("enabled") {
        Some(Value::Bool(b)) => *b,
        Some(v) => v.as_i64().map(|x| x != 0).unwrap_or(true),
        None => true,
    }
}

fn f32_of(v: Option<&Value>) -> Option<f32> {
    v.and_then(Value::as_f32)
}

fn raw_floats<const N: usize>(v: Option<&Value>) -> Option<[f32; N]> {
    if let Some(Value::Raw(_, b)) = v {
        if b.len() == N * 4 {
            let mut out = [0f32; N];
            for (i, c) in b.chunks_exact(4).enumerate() {
                out[i] = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            }
            return Some(out);
        }
    }
    None
}

fn channels(v: Option<&Value>) -> ChannelSet {
    match v {
        Some(Value::Uint(u)) => ChannelSet(*u as u128),
        Some(Value::QUint(u)) => ChannelSet(*u as u128),
        Some(Value::Int(i)) => ChannelSet(*i as u64 as u128),
        Some(Value::Raw(_, b)) => {
            let mut x = 0u128;
            for (i, byte) in b.iter().take(16).enumerate() {
                x |= (*byte as u128) << (8 * i);
            }
            ChannelSet(x)
        }
        _ => ChannelSet(0),
    }
}

fn blends(o: &Object) -> Vec<ChannelBlend> {
    list(o, "perChannelBlending")
        .iter()
        .filter_map(Value::as_object)
        .map(|b| ChannelBlend {
            mode: BlendMode(b.get("blendingMode").and_then(Value::as_i64).unwrap_or(2) as u8),
            channels: channels(b.get("channelTypes")),
            opacity: f32_of(b.get("opacity")).unwrap_or(1.0),
        })
        .collect()
}

fn tweak(o: &Object) -> Tweak {
    let v = o.get("value");
    match o.class.as_str() {
        "DataTweakFloat" => f32_of(v).map(Tweak::Float).unwrap_or(Tweak::Other),
        "DataTweakInt" => match v {
            Some(Value::Raw(_, b)) if b.len() == 4 => Tweak::Int(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Some(x) => x.as_i64().map(|i| Tweak::Int(i as i32)).unwrap_or(Tweak::Other),
            None => Tweak::Other,
        },
        "DataTweakInt2" => match v {
            Some(Value::Raw(_, b)) if b.len() == 8 => Tweak::Int2([
                i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                i32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            ]),
            _ => Tweak::Other,
        },
        "DataTweakFloat2" => raw_floats::<2>(v).map(Tweak::Float2).unwrap_or(Tweak::Other),
        "DataTweakFloat3" => raw_floats::<3>(v).map(Tweak::Float3).unwrap_or(Tweak::Other),
        "DataTweakFloat4" => raw_floats::<4>(v).map(Tweak::Float4).unwrap_or(Tweak::Other),
        "DataTweakBool" => v.and_then(|x| x.as_i64()).map(|i| Tweak::Bool(i != 0)).unwrap_or(Tweak::Other),
        _ => Tweak::Other,
    }
}

fn procedural(p: &Object) -> Procedural {
    let mut tw = BTreeMap::new();
    for t in list(p, "tweaks").iter().filter_map(Value::as_object) {
        if let Some(id) = t.get("identifier").and_then(Value::as_str) {
            let mut value = tweak(t);
            // SHARD_TWEAK_UID="uid=v[,v,v];uid=v": the project as it was when a Painter extraction was made (the
            // extraction's layer json holds the values it had) without touching the file
            if let Ok(spec) = std::env::var("SHARD_TWEAK_UID") {
                let uid = raw_u64(t.get("uid"));
                for item in spec.split(';') {
                    let Some((u, v)) = item.split_once('=') else { continue };
                    if u.trim().parse::<u64>().ok() != Some(uid) {
                        continue;
                    }
                    let f: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    value = match f.len() {
                        1 => Tweak::Float(f[0]),
                        3 => Tweak::Float3([f[0], f[1], f[2]]),
                        4 => Tweak::Float4([f[0], f[1], f[2], f[3]]),
                        _ => value,
                    };
                }
            }
            tw.insert(id.to_string(), value);
        }
    }
    Procedural {
        resource: p.get("urlToSbsRes").and_then(Value::as_str).map(str::to_string),
        outputs: list(p, "outputs")
            .iter()
            .filter_map(Value::as_object)
            .map(|o| ProceduralOutput {
                id: o.get("identifier").and_then(Value::as_str).unwrap_or("").to_string(),
                channels: channels(o.get("channelTypes")),
                color_space: o.get("workflowLegacy").and_then(Value::as_i64).unwrap_or(0) as i32,
            })
            .collect(),
        resolution: Default::default(),
        tweaks: tw,
        inputs: list(p, "inputs")
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|o| o.get("identifier").and_then(Value::as_str).map(str::to_string))
            .collect(),
        input_sources: list(p, "sourcesInputs")
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|s| sub(s, "source"))
            .map(source)
            .collect(),
        input_ids: list(p, "sourcesInputs")
            .iter()
            .filter_map(Value::as_object)
            .filter(|s| sub(s, "source").is_some())
            .map(|s| sub(s, "input").and_then(|i| i.get("identifier")).and_then(Value::as_str).unwrap_or("").to_string())
            .collect(),
    }
}

fn source(s: &Object) -> Source {
    let ch = channels(s.get("channelTypes"));
    let op = f32_of(s.get("opacity")).unwrap_or(1.0);
    match s.class.as_str() {
        "DataSourceUniform" => Source::Uniform { channels: ch, color: raw_floats::<3>(s.get("color")), opacity: op },
        "DataSourceBitmap" => Source::Bitmap {
            channels: ch,
            resource: sub(s, "bitmap")
                .and_then(|b| b.get("urlToBitmapRes"))
                .and_then(Value::as_str)
                .map(str::to_string),
            opacity: op,
        },
        "DataSourceProcedural" => {
            let mut p = sub(s, "procedural").map(procedural).unwrap_or_default();
            if let Some(r) = sub(s, "resolutionOverride") {
                let i32_of = |v: Option<&Value>| match v {
                    Some(Value::Raw(_, b)) if b.len() == 4 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    Some(v) => v.as_i64().unwrap_or(0) as i32,
                    None => 0,
                };
                p.resolution.mode = r.get("mode").and_then(Value::as_i64).unwrap_or(0) as i32;
                p.resolution.log2_offset = i32_of(r.get("log2Offset"));
                if let Some(Value::Raw(_, b)) = r.get("manualValue") {
                    if b.len() == 8 {
                        p.resolution.manual = [
                            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                            u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                        ];
                    }
                }
            }
            Source::Procedural { channels: ch, procedural: p, opacity: op }
        }
        "DataSourceReference" => Source::Reference {
            channels: ch,
            opacity: op,
            target: sub(s, "reference").map(|r| raw_u64(r.get("source"))).unwrap_or(0),
            levels: sub(s, "levels").map(levels).filter(|l| *l != Levels::default()),
        },
        c => Source::Other(c.to_string()),
    }
}

fn decal_hit(h: &Object) -> crate::DecalHit {
    // a click has `position`; a rectangle selection (marquee) has `position0` and `position1`
    let mut rect = raw_floats::<2>(h.get("position0")).zip(raw_floats::<2>(h.get("position1"))).map(|(a, b)| [a, b]);
    let mut position = raw_floats::<2>(h.get("position")).unwrap_or([0.5; 2]);
    // SHARD_RECT_AS_CLICK=1 (experiment): a rectangle is the click at its middle
    if let (Some(r), true) = (rect, std::env::var_os("SHARD_RECT_AS_CLICK").is_some()) {
        position = [(r[0][0] + r[1][0]) * 0.5, (r[0][1] + r[1][1]) * 0.5];
    }
    // a rectangle without width or height (a click that moved a little while the button was down: a test tank's
    // `ExhaustGrid`, 0 x 0.0008) selects nothing as a rectangle: it is the click at its middle
    if let Some(r) = rect.filter(|r| (r[0][0] - r[1][0]).abs() < 1e-6 || (r[0][1] - r[1][1]).abs() < 1e-6) {
        position = [(r[0][0] + r[1][0]) * 0.5, (r[0][1] + r[1][1]) * 0.5];
        rect = None;
    }
    crate::DecalHit {
        uid: raw_u64(h.get("uid")),
        position,
        rect,
        proj_view_inv: raw_floats::<16>(h.get("projViewInvMatrix")).unwrap_or([0.0; 16]),
        color: raw_floats::<3>(h.get("color")).unwrap_or([1.0; 3]),
        granularity: h.get("granularity").and_then(Value::as_i64).unwrap_or(0) as i32,
        paint_through: h.get("paintThrough").and_then(Value::as_i64).unwrap_or(0) as i32,
        view_type: h.get("viewType").and_then(Value::as_i64).unwrap_or(0) as i32,
    }
}

fn levels(o: &Object) -> Levels {
    let d = Levels::default();
    Levels {
        clamp: flag(o.get("clamp")),
        gamma: raw_floats::<3>(o.get("gamma")).unwrap_or(d.gamma),
        in_min: raw_floats::<3>(o.get("inputMinimum")).unwrap_or(d.in_min),
        in_max: raw_floats::<3>(o.get("inputMaximum")).unwrap_or(d.in_max),
        out_min: raw_floats::<3>(o.get("outputMinimum")).unwrap_or(d.out_min),
        out_max: raw_floats::<3>(o.get("outputMaximum")).unwrap_or(d.out_max),
    }
}

fn flag(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(v) => v.as_i64().map(|x| x != 0).unwrap_or(false),
        None => false,
    }
}

fn raw_u64(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Raw(_, b)) if b.len() == 8 => u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        Some(v) => v.as_u64().unwrap_or(0),
        None => 0,
    }
}

fn raw_u32(v: Option<&Value>) -> u32 {
    match v {
        Some(Value::Raw(_, b)) if b.len() == 4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        _ => 0,
    }
}

fn stroke(o: &Object) -> Stroke {
    let keys: Vec<StrokeKey> = list(o, "points")
        .iter()
        .filter_map(Value::as_object)
        .map(|k| StrokeKey {
            pos: raw_floats::<2>(k.get("position")).unwrap_or([0.0; 2]),
            time: f32_of(k.get("time")).unwrap_or(0.0),
            kind: k.get("type").and_then(Value::as_i64).unwrap_or(0) as u8,
        })
        .collect();
    // Pen pressure is a timeline (`DataTimelineKey { x: seconds, value }`), sampled here at every key's time;
    // empty for a mouse stroke.
    let timeline: Vec<(f32, f32)> = list(o, "pressure")
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|k| Some((f32_of(k.get("x"))?, f32_of(k.get("value"))?)))
        .collect();
    let pressure: Vec<f32> = if timeline.is_empty() {
        list(o, "pressure").iter().filter_map(Value::as_f32).collect()
    } else {
        keys.iter()
            .map(|k| {
                let t = k.time;
                match timeline.iter().position(|(x, _)| *x >= t) {
                    Some(0) => timeline[0].1,
                    Some(i) => {
                        let ((x0, v0), (x1, v1)) = (timeline[i - 1], timeline[i]);
                        if x1 > x0 { v0 + (v1 - v0) * (t - x0) / (x1 - x0) } else { v1 }
                    }
                    None => timeline.last().map(|p| p.1).unwrap_or(1.0),
                }
            })
            .collect()
    };
    Stroke {
        keys,
        view: raw_floats::<16>(o.get("viewMatrix")).unwrap_or([0.0; 16]),
        projection: raw_floats::<16>(o.get("projectionMatrix")).unwrap_or([0.0; 16]),
        seed: raw_u32(o.get("randomSeed")),
        view_type: o.get("viewType").and_then(Value::as_i64).unwrap_or(0) as i32,
        lazy_distance: f32_of(o.get("lazyDistance")).unwrap_or(0.0),
        pressure,
        duplicate_count: raw_u32(o.get("duplicateCount")),
        duplicate_matrix: raw_floats::<16>(o.get("duplicateMatrix")),
    }
}

fn brush(b: &Object, shape: Option<&Object>) -> Brush {
    let em = sub(b, "stampsEmitter");
    let ef = |k: &str, d: f32| em.and_then(|e| f32_of(e.get(k))).unwrap_or(d);
    let ei = |k: &str| em.map(|e| flag(e.get(k))).unwrap_or(false);
    Brush {
        class: b.class.clone(),
        size: f32_of(b.get("brushSize")).unwrap_or(0.0),
        size_jitter: f32_of(b.get("brushSizeJitter")).unwrap_or(0.0),
        size_space: b.get("brushSizeSpace").and_then(Value::as_i64).unwrap_or(0) as i32,
        flow: f32_of(b.get("flow")).unwrap_or(1.0),
        flow_jitter: f32_of(b.get("flowJitter")).unwrap_or(0.0),
        culling_angle: f32_of(b.get("cullingAngle")).unwrap_or(0.0),
        culling_enabled: flag(b.get("cullingEnabled")),
        paint_through: flag(b.get("paintThrough")),
        alignment: b.get("alignment").and_then(Value::as_i64).unwrap_or(0) as i32,
        filtering: b.get("filtering").and_then(Value::as_i64).unwrap_or(0) as i32,
        spacing: ef("spacing", 0.2),
        follow_path: ei("followPath"),
        position_jitter: ef("positionJitter", 0.0),
        rotation: ef("rotation", 0.0),
        rotation_jitter: ef("rotationJitter", 0.0),
        pressure_size: ei("pressureSizeEnabled"),
        pressure_size_min: ef("pressureSizeMinimum", 0.0),
        pressure_flow: ei("pressureFlowEnabled"),
        pressure_flow_min: ef("pressureFlowMinimum", 0.0),
        shape: shape.and_then(|s| sub(s, "procedural")).map(procedural),
    }
}

/// First object of a property that is stored either as an object or as a list of them.
fn first_obj<'a>(o: &'a Object, k: &str) -> Option<&'a Object> {
    match o.get(k)? {
        Value::List(l) => l.iter().find_map(Value::as_object),
        v => v.as_object(),
    }
}

fn stencil(s: &Object) -> Stencil {
    match s.class.as_str() {
        "DataSourceText" => Stencil::Text(TextStencil {
            // a text with a line break is stored as raw bytes
            text: match s.get("text") {
                Some(Value::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                Some(v) => v.as_str().unwrap_or("").to_string(),
                None => String::new(),
            },
            font: s.get("urlToFontRes").and_then(Value::as_str).unwrap_or("").to_string(),
            size: f32_of(s.get("size")).unwrap_or(0.5),
            auto_size: flag(s.get("autoSize")),
            padding: raw_floats::<2>(s.get("autoPadding")).unwrap_or([0.0; 2]),
            offset: raw_floats::<2>(s.get("offset")).unwrap_or([0.0; 2]),
            h_align: s.get("horizontalAlignment").and_then(Value::as_i64).unwrap_or(0) as i32,
            v_align: s.get("verticalAlignment").and_then(Value::as_i64).unwrap_or(0) as i32,
            letter_spacing: f32_of(s.get("letterSpacing")).unwrap_or(0.0),
            line_spacing: f32_of(s.get("lineSpacing")).unwrap_or(1.0),
            foreground: raw_floats::<3>(s.get("foregroundColor")).unwrap_or([1.0; 3]),
            background: raw_floats::<3>(s.get("backgroundColor")).unwrap_or([0.0; 3]),
            background_opacity: f32_of(s.get("backgroundOpacity")).unwrap_or(0.0),
        }),
        "DataSourceBitmap" => Stencil::Bitmap {
            resource: sub(s, "bitmap").and_then(|b| b.get("urlToBitmapRes")).and_then(Value::as_str).map(str::to_string),
        },
        "DataSourceProcedural" => Stencil::Procedural(sub(s, "procedural").map(procedural).unwrap_or_default()),
        c => Stencil::Other(c.to_string()),
    }
}

fn paint_action(a: &Object) -> PaintAction {
    PaintAction {
        brush: sub(a, "brush").map(|b| brush(b, first_obj(a, "sourceTransparent"))).unwrap_or_default(),
        opacity: f32_of(a.get("opacity")).unwrap_or(1.0),
        color: list(a, "sourceColor").iter().filter_map(Value::as_object).map(source).collect(),
        strokes: list(a, "strokes").iter().filter_map(Value::as_object).map(stroke).collect::<Vec<_>>().into(),
        stencil: first_obj(a, "sourceStencil").map(stencil),
        proj: raw_floats::<9>(a.get("projMatrix")),
        proj_wrap: a.get("projSamplingWrap").and_then(Value::as_i64).unwrap_or(0) as i32,
    }
}

/// `DataActionPaintProj` (the projection tool): an image placed in the view -- a text, a bitmap -- that the strokes
/// reveal where they pass. That is a paint action whose stencil is the projected source and whose colour is the
/// source's own (the foreground of a text). A test pistol's engraved texts and logos are made that way.
fn paint_proj_action(a: &Object) -> PaintAction {
    let mut p = paint_action(a);
    let src = first_obj(a, "sourceColor");
    p.stencil = src.map(stencil);
    p.color = src
        .filter(|s| s.class == "DataSourceText")
        .map(|s| {
            vec![Source::Uniform {
                channels: channels(s.get("channelTypes")),
                color: Some(raw_floats::<3>(s.get("foregroundColor")).unwrap_or([1.0; 3])),
                opacity: f32_of(s.get("opacity")).unwrap_or(1.0),
            }]
        })
        .unwrap_or_default();
    p
}

fn effect(a: &Object) -> Effect {
    let label = a.get("label").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let kind = match a.class.as_str() {
        "DataActionFill" => EffectKind::Fill(Fill {
            projection: Projection::from_value(a.get("projection").and_then(Value::as_i64).unwrap_or(0)),
            filtering: a.get("filtering").and_then(Value::as_i64).unwrap_or(0) as i32,
            uv: {
                let t = sub(a, "uvTransformation");
                UvTransform {
                    scale: t.and_then(|t| raw_floats::<2>(t.get("scaleFactors"))),
                    position: t.and_then(|t| raw_floats::<2>(t.get("translation"))),
                    rotation: t.and_then(|t| f32_of(t.get("rotation"))),
                    pivot: t.and_then(|t| raw_floats::<2>(t.get("GUIPivot"))),
                    scale_mode: t.and_then(|t| t.get("scaleMode")).and_then(Value::as_i64).unwrap_or(0) as i32,
                    wrap: a.get("uvSamplingWrap").and_then(Value::as_i64).unwrap_or(0) as i32,
                }
            },
            sources: list(a, "sources").iter().filter_map(Value::as_object).map(source).collect(),
            projector: raw_floats::<3>(a.get("dddPosition")).map(|position| Projector {
                position,
                euler: raw_floats::<3>(a.get("dddEulerAngles")).unwrap_or([0.0; 3]),
                size: raw_floats::<3>(a.get("dddSize")).unwrap_or([1.0; 3]),
                wrap: a.get("dddWrap").and_then(Value::as_i64).unwrap_or(0) as i32,
            }),
        }),
        "DataActionPaint" => EffectKind::Paint(paint_action(a)),
        "DataActionPaintProj" if std::env::var_os("SHARD_NO_PAINT_PROJ").is_none() => EffectKind::Paint(paint_proj_action(a)),
        "DataActionEraser" => EffectKind::Eraser(paint_action(a)),
        "DataActionSmudge" => EffectKind::Smudge(paint_action(a)),
        "DataActionGroup" => EffectKind::Group {
            children: sub(a, "subStack")
                .map(|s| list(s, "items").iter().filter_map(Value::as_object).map(effect).collect())
                .unwrap_or_default(),
        },
        "DataActionFilterProcedural" => EffectKind::Filter { procedural: sub(a, "procedural").map(procedural) },
        "DataActionFilterLevels" => EffectKind::Levels(sub(a, "levels").map(levels).unwrap_or_default()),
        "DataActionAnchor" => EffectKind::Anchor {
            name: a.get("label").and_then(Value::as_str).map(str::to_string),
        },
        "DataActionDecal" => EffectKind::Decal(
            list(a, "hits")
                .iter()
                .filter_map(Value::as_object)
                .map(|h| decal_hit(h))
                .collect::<Vec<_>>()
                .into(),
        ),
        c => EffectKind::Other(c.trim_start_matches("DataAction").to_ascii_lowercase()),
    };
    Effect {
        uid: raw_u64(a.get("uid")),
        visible: visible(a),
        channels: channels(a.get("channelTypes")),
        label,
        blend: blends(a),
        kind,
    }
}

fn layer(o: &Object) -> Layer {
    if std::env::var_os("SHARD_DEBUG_MASK").is_some() {
        let kind = |k: &str| match o.get(k) {
            None => "absent".to_string(),
            Some(Value::Null) => "null".to_string(),
            Some(v) => format!("{:?}", v).chars().take(60).collect(),
        };
        eprintln!(
            "[mask-field] uid={} '{}' maskActions={} maskEnabled={} maskInitial={}",
            o.get("uid").map(|v| raw_u64(Some(v))).unwrap_or(0),
            o.get("label").and_then(Value::as_str).unwrap_or(""),
            kind("maskActions"),
            kind("maskEnabled"),
            kind("maskInitial")
        );
    }
    // A layer has a mask when `maskActions` holds a stack of actions (even an empty one). `maskEnabled` and
    // `maskInitial` are written for every layer: a layer without a mask can carry `maskInitial = 0`, which
    // made it vanish (a test vehicle `highpass`: the Painter API says it has no mask; SHARD_MASK_FROM_FLAGS=1 restores
    // the old reading).
    let mask_present = o.get("maskActions").map(|v| !matches!(v, Value::Null)).unwrap_or(false)
        || (std::env::var_os("SHARD_MASK_FROM_FLAGS").is_some() && o.get("maskEnabled").is_some());
    let mask = if mask_present {
        Some(Mask {
            explicit: false,
            enabled: match o.get("maskEnabled") {
                Some(Value::Bool(b)) => *b,
                Some(v) => v.as_i64().map(|x| x != 0).unwrap_or(true),
                None => true,
            },
            initial: o.get("maskInitial").and_then(Value::as_i64).unwrap_or(0),
            effects: sub(o, "maskActions")
                .map(|m| list(m, "items").iter().filter_map(Value::as_object).map(effect).collect())
                .unwrap_or_default(),
        })
    } else {
        None
    };
    Layer {
        uid: o.get("uid").map(|v| raw_u64(Some(v))).filter(|u| *u != 0).unwrap_or_else(crate::next_uid),
        kind: if o.class == "DataLayerGroup" { LayerKind::Group } else { LayerKind::Layer },
        name: o.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
        visible: visible(o),
        blend: blends(o),
        content: sub(o, "actions")
            .map(|a| list(a, "items").iter().filter_map(Value::as_object).map(effect).collect())
            .unwrap_or_default(),
        mask,
        children: sub(o, "subStack")
            .map(|s| list(s, "items").iter().rev().filter_map(Value::as_object).map(layer).collect())
            .unwrap_or_default(),
    }
}
