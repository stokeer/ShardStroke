//! `.sbsar` (7z archive of a compiled Substance graph): extracts the interface (`.xml`) that
//! describes the parameters a user can edit (labels, groups, ranges, enums, image inputs).
use std::io::{Cursor, Read};

/// Files of the `assemblies/content/*` folder: `(name, bytes)`.
pub fn extract_content(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    match extract_7z(bytes) {
        Ok(v) if v.iter().any(|(n, _)| n.ends_with(".xml")) => Ok(v),
        // Archives written with an empty "Copy" coder id: members are stored raw.
        _ => scan_stored_xml(bytes).map(|x| vec![("assemblies/content/0000/graph.xml".to_string(), x)]),
    }
}

/// A bitmap stored inside a compiled graph (the grunge and noise sources of the shelf procedurals).
#[derive(Debug, Clone)]
pub struct EmbeddedImage {
    pub w: u32,
    pub h: u32,
    /// Components per pixel: 1 (grayscale) or 3 (RGB).
    pub comps: usize,
    pub data: Vec<u8>,
}

/// The compiled graph (`.sbsasm`, SBAM) of an archive.
pub fn extract_sbsasm(bytes: &[u8]) -> Option<Vec<u8>> {
    if let Ok(v) = extract_content(bytes) {
        if let Some((_, d)) = v.into_iter().find(|(n, _)| n.ends_with(".sbsasm")) {
            return Some(d);
        }
    }
    // stored (Copy coder) archive: SBAM header with the file size at +0x10
    let i = bytes.windows(4).position(|w| w == b"SBAM")?;
    let size = u32::from_le_bytes(bytes.get(i + 0x10..i + 0x14)?.try_into().ok()?) as usize;
    bytes.get(i..i + size.min(bytes.len() - i)).map(|s| s.to_vec())
}

/// Bitmaps embedded in the compiled graph of `sbsar`, in file order. Procedurals such as
/// `grunge_stone_details` or `mg_metal_edge_wear` carry their source textures as plain JPEGs.
pub fn embedded_images(sbsar: &[u8]) -> Vec<EmbeddedImage> {
    let Some(asm) = extract_sbsasm(sbsar) else { return Vec::new() };
    let mut out = Vec::new();
    let mut i = 0;
    while i + 12 < asm.len() {
        let hit = asm[i] == 0xFF
            && asm[i + 1] == 0xD8
            && asm[i + 2] == 0xFF
            && matches!(asm[i + 3], 0xE0 | 0xE1 | 0xDB)
            && (asm[i + 3] != 0xE0 || &asm[i + 6..i + 10] == b"JFIF");
        if !hit {
            i += 1;
            continue;
        }
        let mut dec = jpeg_decoder::Decoder::new(Cursor::new(&asm[i..]));
        match dec.decode() {
            Ok(px) => {
                if let Some(info) = dec.info() {
                    let comps = match info.pixel_format {
                        jpeg_decoder::PixelFormat::L8 => 1,
                        jpeg_decoder::PixelFormat::RGB24 => 3,
                        _ => 0,
                    };
                    if comps != 0 {
                        out.push(EmbeddedImage { w: info.width as u32, h: info.height as u32, comps, data: px });
                    }
                }
                // continue after this image's end-of-image marker
                let end = asm[i + 2..].windows(2).position(|w| w == [0xFF, 0xD9]).map(|p| i + 2 + p + 2);
                i = end.unwrap_or(i + 4);
            }
            Err(_) => i += 4,
        }
    }
    out
}

/// The embedded preview (`icon*.png`) of a compiled graph, if it carries one.
pub fn extract_icon(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = None;
    if let Ok(mut r) = sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty()) {
        let _ = r.for_each_entries(|e, rd| {
            let name = e.name().to_ascii_lowercase();
            if out.is_none() && !e.is_directory() && name.ends_with(".png") && name.contains("icon") {
                let mut v = Vec::new();
                rd.read_to_end(&mut v)?;
                out = Some(v);
            }
            Ok(true)
        });
    }
    out.or_else(|| scan_stored_png(bytes))
}

/// First PNG of a stored (uncompressed) archive, found by scanning for its signature and walking
/// the chunks up to `IEND`.
fn scan_stored_png(bytes: &[u8]) -> Option<Vec<u8>> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let a = bytes.windows(8).position(|w| w == SIG)?;
    let mut p = a + 8;
    while p + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]) as usize;
        let kind = &bytes[p + 4..p + 8];
        p += 12 + len;
        if kind == b"IEND" {
            return bytes.get(a..p.min(bytes.len())).map(|s| s.to_vec());
        }
    }
    None
}

/// Interface xml of a stored (uncompressed) archive, found by scanning.
fn scan_stored_xml(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let find = |h: &[u8], n: &[u8], from: usize| h[from..].windows(n.len()).position(|w| w == n).map(|p| p + from);
    let a = find(bytes, b"<?xml", 0).ok_or("no stored xml")?;
    let end_tag = b"</sbsdescription>";
    let b = find(bytes, end_tag, a).ok_or("unterminated xml")? + end_tag.len();
    Ok(bytes[a..b].to_vec())
}

fn extract_7z(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut r = sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let res = r.for_each_entries(|e, rd| {
        if !e.is_directory() && e.name().contains("assemblies/content/") {
            let mut v = Vec::new();
            rd.read_to_end(&mut v)?;
            out.push((e.name().to_string(), v));
        }
        Ok(true)
    });
    // Some archives end with a folder whose coder has an empty id (the icon, stored raw): the reader stops
    // there with an error after it already produced the graph and its interface. Keep what it read.
    match res {
        Err(e) if out.is_empty() => Err(e.to_string()),
        _ => Ok(out),
    }
}

/// Editor widget of a parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Widget {
    Slider { min: f32, max: f32, step: f32, clamp: bool },
    Toggle,
    Combo(Vec<(i32, String)>),
    Color,
    /// Image pin (mesh map, anchor, bitmap).
    Image,
}

/// Default value as written in the interface.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    Float(Vec<f32>),
    Int(Vec<i32>),
    None,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub identifier: String,
    pub label: String,
    /// Substance value type (0 float, 1..3 float2..4, 4 int, 5 image, 8 int2).
    pub ty: u32,
    pub default: DefaultValue,
    pub widget: Widget,
    pub group: Option<String>,
    /// `showas="pin"` (input slot) vs. `tweak` (property).
    pub pin: bool,
    pub visible_if: Option<String>,
    /// Number of components of a slider (1 for scalars).
    pub comps: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Interface {
    pub label: String,
    pub category: String,
    pub description: String,
    /// `(identifier, label)` in display order.
    pub groups: Vec<(String, String)>,
    pub params: Vec<Param>,
    /// Numeric defaults of every input, including those without a widget (used by `visibleif`).
    pub values: Vec<(String, Vec<f32>)>,
    /// Identifiers of every image input (picture pins, with or without a widget).
    pub images: Vec<String>,
    /// Identifiers of the graph outputs, in file order.
    pub outputs: Vec<String>,
    /// Usages declared by each output (`baseColor`, `roughness`...): `(identifier, usage names)`.
    pub output_usages: Vec<(String, Vec<String>)>,
    /// The XML of every output element (diagnostics).
    pub output_xml: Vec<(String, String)>,
}

fn floats(s: &str) -> Vec<f32> {
    s.split(',').filter_map(|t| t.trim().parse().ok()).collect()
}

impl Interface {
    /// Parse a graph interface `.xml`.
    pub fn parse(xml: &str) -> Result<Self, String> {
        let doc = roxmltree::Document::parse_with_options(
            xml,
            roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() },
        )
        .map_err(|e| e.to_string())?;
        let graph = doc
            .descendants()
            .find(|n| n.has_tag_name("graph"))
            .ok_or("no <graph>")?;
        let mut it = Interface {
            label: graph.attribute("label").unwrap_or("").into(),
            category: graph.attribute("category").unwrap_or("").into(),
            description: graph.attribute("description").unwrap_or("").into(),
            ..Default::default()
        };
        for g in graph.descendants().filter(|n| n.has_tag_name("guigroup")) {
            it.groups.push((
                g.attribute("identifier").unwrap_or("").into(),
                g.attribute("label").unwrap_or("").into(),
            ));
        }
        if let Some(outs) = graph.children().find(|n| n.has_tag_name("outputs")) {
            for o in outs.children().filter(|n| n.has_tag_name("output")) {
                if let Some(id) = o.attribute("identifier") {
                    it.outputs.push(id.to_string());
                    let usages: Vec<String> = o
                        .descendants()
                        .filter(|n| n.has_tag_name("channelsusage") || n.has_tag_name("usage") || n.has_tag_name("channelusage"))
                        .filter_map(|n| n.attribute("usage").or_else(|| n.attribute("name")).map(str::to_string))
                        .collect();
                    it.output_usages.push((id.to_string(), usages));
                    it.output_xml.push((id.to_string(), o.document().input_text()[o.range()].to_string()));
                }
            }
        }
        let inputs = graph.children().find(|n| n.has_tag_name("inputs"));
        for inp in inputs.iter().flat_map(|n| n.children()).filter(|n| n.has_tag_name("input")) {
            let id = inp.attribute("identifier").unwrap_or("");
            if id.starts_with('$') {
                continue;
            }
            let ty: u32 = inp.attribute("type").and_then(|t| t.parse().ok()).unwrap_or(0);
            if ty == 5 {
                it.images.push(id.to_string());
            }
            if matches!(ty, 0 | 1 | 2 | 3 | 4 | 8) {
                it.values.push((id.to_string(), floats(inp.attribute("default").unwrap_or(""))));
            }
            let Some(gui) = inp.children().find(|n| n.has_tag_name("inputgui")) else { continue };
            let widget = match gui.attribute("widget").unwrap_or("") {
                "slider" => {
                    let s = gui.children().find(|n| n.has_tag_name("guislider"));
                    let f = |a: &str, d: f32| {
                        s.and_then(|s| s.attribute(a)).and_then(|v| floats(v).first().copied()).unwrap_or(d)
                    };
                    Widget::Slider {
                        min: f("min", 0.0),
                        max: f("max", 1.0),
                        step: f("step", 0.01),
                        clamp: s.and_then(|s| s.attribute("clamp")) == Some("yes"),
                    }
                }
                "togglebutton" => Widget::Toggle,
                "combobox" => Widget::Combo(
                    gui.descendants()
                        .filter(|n| n.has_tag_name("guicomboboxitem"))
                        .filter_map(|n| {
                            Some((n.attribute("value")?.parse().ok()?, n.attribute("text")?.to_string()))
                        })
                        .collect(),
                ),
                "color" => Widget::Color,
                "image" => Widget::Image,
                _ => continue,
            };
            let def = inp.attribute("default").unwrap_or("");
            let default = match ty {
                0..=3 => DefaultValue::Float(floats(def)),
                4 | 8 => DefaultValue::Int(def.split(',').filter_map(|t| t.trim().parse().ok()).collect()),
                _ => DefaultValue::None,
            };
            let comps = match &default {
                DefaultValue::Float(v) => v.len().max(1),
                DefaultValue::Int(v) => v.len().max(1),
                DefaultValue::None => 1,
            };
            it.params.push(Param {
                identifier: id.into(),
                label: gui.attribute("label").unwrap_or(id).into(),
                ty,
                default,
                widget,
                group: gui.attribute("group").map(String::from),
                pin: gui.attribute("showas") == Some("pin") || matches!(ty, 5),
                visible_if: gui.attribute("visibleif").map(String::from),
                comps,
            });
        }
        Ok(it)
    }

    /// Interface of the graph inside a `.sbsar`.
    pub fn from_sbsar(bytes: &[u8]) -> Result<Self, String> {
        let files = extract_content(bytes)?;
        let (_, xml) = files.iter().find(|(n, _)| n.ends_with(".xml")).ok_or("no .xml in sbsar")?;
        Self::parse(std::str::from_utf8(xml).map_err(|e| e.to_string())?)
    }
}
