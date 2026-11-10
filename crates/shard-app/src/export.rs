//! Texture export in the Arma Reforger layout used by the project's Painter export preset:
//!
//! * `<set>_BCR.tga`: base colour (sRGB) in RGB, roughness in A, island borders padded (dilation);
//! * `<set>_NMO.tga`: tangent normal X/Y in R/G, metallic in B, baked ambient occlusion in A.
//!
//! Files are 32-bit uncompressed TGA, bottom-left origin, with the TRUEVISION footer, like the exports
//! of Painter itself (docs/VALIDATION.md).

use std::path::{Path, PathBuf};

use crate::ui::{AppState, Bake};
pub use shard_engine::raster::write_tga;
pub(crate) use shard_engine::raster::{coverage, dilate, dilate_f32, erode, pad_f32};

fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Export one texture set (BCR / NMO, TGA); returns the written files.
pub fn export_set(st: &AppState, set: usize, dir: &Path, res: u32) -> Result<Vec<PathBuf>, String> {
    export_arma(st, set, dir, res, FileType::Tga, BitDepth::B8)
}

/// The Arma Reforger pictures of one set, written as `file_type`.
fn export_arma(st: &AppState, set: usize, dir: &Path, res: u32, file_type: FileType, depth: BitDepth) -> Result<Vec<PathBuf>, String> {
    let name = st.stacks.get(set).map(|s| s.name.clone()).ok_or("no such texture set")?;
    let mesh = &st.scene.mesh;
    let (w, h) = (res, res);
    let comp = st.approx_compose(set, &[0, 7, 13], res);
    let base = &comp.channels[&0];
    let rough = &comp.channels[&7];
    let metal = &comp.channels[&13];
    let overlay = st.scene.texs.get(set).map(|t| t.overlay());
    let cov = coverage(mesh, set, w, h);

    // ---- BCR
    let mut bcr = vec![0u8; (w * h * 4) as usize];
    for i in 0..(w * h) as usize {
        let mut rgb = [
            q(shard_compose::linear_to_srgb(base.data[i * base.n])),
            q(shard_compose::linear_to_srgb(base.data[i * base.n + 1.min(base.n - 1)])),
            q(shard_compose::linear_to_srgb(base.data[i * base.n + 2.min(base.n - 1)])),
        ];
        if let Some((ov, ow, oh)) = overlay {
            let (x, y) = ((i as u32 % w) as u64 * ow as u64 / w as u64, (i as u32 / w) as u64 * oh as u64 / h as u64);
            let j = ((y * ow as u64 + x) * 4) as usize;
            let a = ov[j + 3] as f32 / 255.0;
            if a > 0.0 {
                for c in 0..3 {
                    rgb[c] = (rgb[c] as f32 + (ov[j + c] as f32 - rgb[c] as f32) * a + 0.5) as u8;
                }
            }
        }
        bcr[i * 4..i * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], q(rough.data[i])]);
    }
    dilate(&mut bcr, &cov, w, h, 192);

    // ---- NMO
    let nrm = st.bake_image(set, Bake::NormalBase);
    // baked normal + the stack's Height and Normal channels, when the stack writes them
    let stack_nrm = st.approx_normal(set, res).filter(|n| n.w == w && n.h == h);
    let ao = st.bake_image(set, Bake::AmbientOcclusion);
    let at = |img: &shard_compose::Image, x: u32, y: u32, c: usize| {
        let sx = (x as u64 * img.w as u64 / w as u64) as u32;
        let sy = (y as u64 * img.h as u64 / h as u64) as u32;
        let p = img.pixel(sx.min(img.w - 1), sy.min(img.h - 1));
        p[c.min(img.n - 1)]
    };
    let mut nmo = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !cov[i] {
                continue; // padded below by dilation, like the Painter exports
            }
            let (nx, ny) = match (&stack_nrm, &nrm) {
                (Some(s), _) => (s.data[i * s.n], s.data[i * s.n + 1]),
                (None, Some(n)) => (at(n, x, y, 0), at(n, x, y, 1)),
                (None, None) => (0.5, 0.5),
            };
            let o = ao.as_ref().map(|a| at(a, x, y, 0)).unwrap_or(1.0);
            nmo[i * 4..i * 4 + 4].copy_from_slice(&[q(nx), q(ny), q(metal.data[i]), q(o)]);
        }
    }

    // Painter pads the normal and metallic of the NMO texture all the way, but the baked occlusion only a
    // little (black elsewhere, also in the holes of the islands): measured on the a test vehicle interior
    // export, which has 21% black occlusion texels; a band of resolution / 128 texels reproduces it.
    let ao: Vec<u8> = nmo.chunks_exact(4).map(|p| p[3]).collect();
    dilate(&mut nmo, &cov, w, h, 192);
    if std::env::var_os("SHARD_EXPORT_PAD_AO").is_none() {
        let mut band: Vec<u8> = ao.iter().flat_map(|a| [*a, *a, *a, *a]).collect();
        dilate(&mut band, &cov, w, h, (w / 128).max(2));
        for (p, b) in nmo.chunks_exact_mut(4).zip(band.chunks_exact(4)) {
            p[3] = b[0];
        }
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let as_f32 = |px: &[u8]| px.iter().map(|v| *v as f32 / 255.0).collect::<Vec<f32>>();
    // BCR keeps sRGB colour in RGB (linear in float files); NMO is data
    let f1 = save_image(&dir.join(format!("{name}_BCR")), w, h, &as_f32(&bcr), Layout::Rgba, [true, true, true, false], file_type, depth)?;
    let f2 = save_image(&dir.join(format!("{name}_NMO")), w, h, &as_f32(&nmo), Layout::Rgba, [false; 4], file_type, depth)?;
    Ok(vec![f1, f2])
}

// ---------------------------------------------------------------- export templates (File > Export Textures)

/// Where an output channel takes its value from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Src {
    /// Base colour component (sRGB-encoded) 0..2.
    Base(usize),
    Roughness,
    /// 1 - roughness.
    Smoothness,
    Metallic,
    /// Final tangent normal component 0..2 in the template's convention.
    Normal(usize),
    AmbientOcclusion,
    /// Height, 0.5 = no height (Painter's export: 0.5 + h / 2).
    Height,
    /// Emissive component (sRGB-encoded) 0..2.
    Emissive(usize),
    One,
    Zero,
}

/// One file of a template: `$set` in the name is the texture set; four channels (a grey file repeats R).
#[derive(Clone, Debug)]
pub struct Output {
    pub name: &'static str,
    pub channels: [Src; 4],
    /// Written only when the stack has that content (emissive, height).
    pub optional: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalFormat {
    OpenGl,
    DirectX,
}

/// An output template: the files written for each texture set.
#[derive(Clone, Debug)]
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub normal: NormalFormat,
    pub outputs: Vec<Output>,
}

/// The built-in templates (Painter's presets of the same names, and the Arma Reforger one of the corpus).
pub fn templates() -> Vec<Template> {
    use Src::*;
    let rgb = |c: fn(usize) -> Src, a: Src| [c(0), c(1), c(2), a];
    let grey = |s: Src| [s, s, s, One];
    vec![
        Template {
            name: "Arma Reforger (BCR / NMO)",
            description: "BCR: base colour + roughness in alpha. NMO: normal X/Y, metallic, ambient occlusion. Padded past the UV islands.",
            normal: NormalFormat::DirectX,
            outputs: Vec::new(), // the dedicated path, `export_set`
        },
        Template {
            name: "PBR Metallic Roughness",
            description: "One file per channel: BaseColor, Roughness, Metallic, Normal, Height, AO, Emissive.",
            normal: NormalFormat::OpenGl,
            outputs: vec![
                Output { name: "$set_BaseColor", channels: rgb(Base, One), optional: false },
                Output { name: "$set_Roughness", channels: grey(Roughness), optional: false },
                Output { name: "$set_Metallic", channels: grey(Metallic), optional: false },
                Output { name: "$set_Normal", channels: rgb(Normal, One), optional: false },
                Output { name: "$set_Height", channels: grey(Height), optional: true },
                Output { name: "$set_AO", channels: grey(AmbientOcclusion), optional: false },
                Output { name: "$set_Emissive", channels: rgb(Emissive, One), optional: true },
            ],
        },
        Template {
            name: "Unreal Engine (Packed)",
            description: "BaseColor, OcclusionRoughnessMetallic (R = AO, G = roughness, B = metallic), Normal (DirectX), Emissive.",
            normal: NormalFormat::DirectX,
            outputs: vec![
                Output { name: "$set_BaseColor", channels: rgb(Base, One), optional: false },
                Output { name: "$set_OcclusionRoughnessMetallic", channels: [AmbientOcclusion, Roughness, Metallic, One], optional: false },
                Output { name: "$set_Normal", channels: rgb(Normal, One), optional: false },
                Output { name: "$set_Emissive", channels: rgb(Emissive, One), optional: true },
            ],
        },
        Template {
            name: "Unity HDRP (Metallic Standard)",
            description: "BaseMap, MaskMap (R = metallic, G = AO, B = detail mask, A = smoothness), Normal (OpenGL), Emissive.",
            normal: NormalFormat::OpenGl,
            outputs: vec![
                Output { name: "$set_BaseMap", channels: rgb(Base, One), optional: false },
                Output { name: "$set_MaskMap", channels: [Metallic, AmbientOcclusion, Zero, Smoothness], optional: false },
                Output { name: "$set_Normal", channels: rgb(Normal, One), optional: false },
                Output { name: "$set_Emissive", channels: rgb(Emissive, One), optional: true },
            ],
        },
        Template {
            name: "glTF PBR Metal Roughness",
            description: "baseColor, occlusionRoughnessMetallic (R = AO, G = roughness, B = metallic), normal (OpenGL), emissive.",
            normal: NormalFormat::OpenGl,
            outputs: vec![
                Output { name: "$set_baseColor", channels: rgb(Base, One), optional: false },
                Output { name: "$set_occlusionRoughnessMetallic", channels: [AmbientOcclusion, Roughness, Metallic, One], optional: false },
                Output { name: "$set_normal", channels: rgb(Normal, One), optional: false },
                Output { name: "$set_emissive", channels: rgb(Emissive, One), optional: true },
            ],
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Png,
    Tga,
    Jpeg,
    Tiff,
    Bmp,
    Exr,
}

impl FileType {
    pub const ALL: [FileType; 6] = [FileType::Png, FileType::Tga, FileType::Jpeg, FileType::Tiff, FileType::Bmp, FileType::Exr];

    pub fn label(self) -> &'static str {
        match self {
            FileType::Png => "PNG",
            FileType::Tga => "TGA",
            FileType::Jpeg => "JPEG",
            FileType::Tiff => "TIFF",
            FileType::Bmp => "BMP",
            FileType::Exr => "EXR",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            FileType::Png => "png",
            FileType::Tga => "tga",
            FileType::Jpeg => "jpg",
            FileType::Tiff => "tif",
            FileType::Bmp => "bmp",
            FileType::Exr => "exr",
        }
    }

    /// The bit depths the format can hold (the first is its default).
    pub fn depths(self) -> &'static [BitDepth] {
        match self {
            FileType::Png => &[BitDepth::B8, BitDepth::B16],
            FileType::Tiff => &[BitDepth::B8, BitDepth::B16, BitDepth::F32],
            FileType::Exr => &[BitDepth::F16, BitDepth::F32],
            _ => &[BitDepth::B8],
        }
    }

    /// JPEG has no alpha channel.
    pub fn has_alpha(self) -> bool {
        self != FileType::Jpeg
    }
}

/// Bits per channel of the written files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
    B8,
    B16,
    F16,
    F32,
}

impl BitDepth {
    pub fn label(self) -> &'static str {
        match self {
            BitDepth::B8 => "8 bits",
            BitDepth::B16 => "16 bits",
            BitDepth::F16 => "16 bits float",
            BitDepth::F32 => "32 bits float",
        }
    }

    fn float(self) -> bool {
        matches!(self, BitDepth::F16 | BitDepth::F32)
    }
}

/// Channels of a written picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Gray,
    Rgb,
    Rgba,
}

/// Write `data` (RGBA floats 0..1, `w x h`) to `base` + the format's extension, with `layout`'s channels.
/// Channels flagged in `srgb` hold sRGB-encoded colour: kept in integer files, made linear in float ones
/// (Painter's convention for EXR / 32-bit). Returns the path written.
pub fn save_image(
    base: &Path,
    w: u32,
    h: u32,
    data: &[f32],
    layout: Layout,
    srgb: [bool; 4],
    file_type: FileType,
    depth: BitDepth,
) -> Result<PathBuf, String> {
    let path = base.with_extension(file_type.extension());
    let depth = if file_type.depths().contains(&depth) { depth } else { file_type.depths()[0] };
    let layout = if layout == Layout::Rgba && !file_type.has_alpha() { Layout::Rgb } else { layout };
    let comps: &[usize] = match layout {
        Layout::Gray => &[0],
        Layout::Rgb => &[0, 1, 2],
        Layout::Rgba => &[0, 1, 2, 3],
    };
    let n = (w * h) as usize;
    let value = |i: usize, c: usize| -> f32 {
        let v = data[i * 4 + c];
        if depth.float() && srgb[c] {
            shard_compose::srgb_to_linear(v.clamp(0.0, 1.0))
        } else {
            v
        }
    };
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if file_type == FileType::Exr {
        use exr::prelude::*;
        let px = |x: usize, y: usize, c: usize| value(y * w as usize + x, comps[c.min(comps.len() - 1)]);
        let alpha = layout == Layout::Rgba;
        let size = Vec2(w as usize, h as usize);
        let result = if depth == BitDepth::F16 {
            let get = |p: Vec2<usize>| {
                (f16::from_f32(px(p.0, p.1, 0)), f16::from_f32(px(p.0, p.1, 1)), f16::from_f32(px(p.0, p.1, 2)), f16::from_f32(if alpha { px(p.0, p.1, 3) } else { 1.0 }))
            };
            if alpha {
                Image::from_channels(size, SpecificChannels::rgba(get)).write().to_file(&path)
            } else {
                Image::from_channels(size, SpecificChannels::rgb(|p: Vec2<usize>| { let c = get(p); (c.0, c.1, c.2) })).write().to_file(&path)
            }
        } else {
            let get = |p: Vec2<usize>| (px(p.0, p.1, 0), px(p.0, p.1, 1), px(p.0, p.1, 2), if alpha { px(p.0, p.1, 3) } else { 1.0 });
            if alpha {
                Image::from_channels(size, SpecificChannels::rgba(get)).write().to_file(&path)
            } else {
                Image::from_channels(size, SpecificChannels::rgb(|p: Vec2<usize>| { let c = get(p); (c.0, c.1, c.2) })).write().to_file(&path)
            }
        };
        result.map_err(|e| err(&e))?;
        return Ok(path);
    }
    if file_type == FileType::Tga && depth == BitDepth::B8 && layout == Layout::Rgba {
        // our own writer: Painter's exact layout (bottom-left origin, TRUEVISION footer)
        let px: Vec<u8> = (0..n * 4).map(|k| q(value(k / 4, k % 4))).collect();
        write_tga(&path, w, h, &px)?;
        return Ok(path);
    }
    let fmt = match file_type {
        FileType::Png => image::ImageFormat::Png,
        FileType::Tga => image::ImageFormat::Tga,
        FileType::Jpeg => image::ImageFormat::Jpeg,
        FileType::Tiff => image::ImageFormat::Tiff,
        FileType::Bmp => image::ImageFormat::Bmp,
        FileType::Exr => unreachable!(),
    };
    let collect8 = || -> Vec<u8> { (0..n).flat_map(|i| comps.iter().map(move |&c| (i, c))).map(|(i, c)| q(value(i, c))).collect() };
    let collect16 = || -> Vec<u16> {
        (0..n).flat_map(|i| comps.iter().map(move |&c| (i, c))).map(|(i, c)| (value(i, c).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect()
    };
    let collect32 = || -> Vec<f32> { (0..n).flat_map(|i| comps.iter().map(move |&c| (i, c))).map(|(i, c)| value(i, c)).collect() };
    let img: image::DynamicImage = match (depth, layout) {
        (BitDepth::B8, Layout::Gray) => image::GrayImage::from_raw(w, h, collect8()).map(image::DynamicImage::ImageLuma8),
        (BitDepth::B8, Layout::Rgb) => image::RgbImage::from_raw(w, h, collect8()).map(image::DynamicImage::ImageRgb8),
        (BitDepth::B8, Layout::Rgba) => image::RgbaImage::from_raw(w, h, collect8()).map(image::DynamicImage::ImageRgba8),
        (BitDepth::B16, Layout::Gray) => image::ImageBuffer::from_raw(w, h, collect16()).map(image::DynamicImage::ImageLuma16),
        (BitDepth::B16, Layout::Rgb) => image::ImageBuffer::from_raw(w, h, collect16()).map(image::DynamicImage::ImageRgb16),
        (BitDepth::B16, Layout::Rgba) => image::ImageBuffer::from_raw(w, h, collect16()).map(image::DynamicImage::ImageRgba16),
        // 32-bit float TIFF: grey goes as RGB (the encoder has no float grey)
        (_, Layout::Rgba) => image::ImageBuffer::from_raw(w, h, collect32()).map(image::DynamicImage::ImageRgba32F),
        (_, Layout::Gray) => {
            let g = collect32();
            image::ImageBuffer::from_raw(w, h, g.iter().flat_map(|v| [*v, *v, *v]).collect()).map(image::DynamicImage::ImageRgb32F)
        }
        (_, Layout::Rgb) => image::ImageBuffer::from_raw(w, h, collect32()).map(image::DynamicImage::ImageRgb32F),
    }
    .ok_or_else(|| err(&"image size"))?;
    img.save_with_format(&path, fmt).map_err(|e| err(&e))?;
    Ok(path)
}

/// Settings of one export (the Export Textures dialog).
#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub template: usize,
    pub file_type: FileType,
    pub depth: BitDepth,
    /// None: each set at its own resolution.
    pub size: Option<u32>,
    /// Padding past the UV islands in texels; None = to the whole texture (Painter's "Dilation infinite").
    pub padding: Option<u32>,
    /// None: the template's.
    pub normal: Option<NormalFormat>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { template: 0, file_type: FileType::Tga, depth: BitDepth::B8, size: None, padding: None, normal: None }
    }
}

/// Export one set with a template; returns the written files.
pub fn export_template(st: &AppState, set: usize, dir: &Path, opts: &ExportOptions) -> Result<Vec<PathBuf>, String> {
    let all = templates();
    let tpl = all.get(opts.template).ok_or("no such template")?;
    let res = opts.size.unwrap_or_else(|| st.scene.texs.get(set).map(|t| t.w).unwrap_or(1024));
    if tpl.outputs.is_empty() {
        return export_arma(st, set, dir, res, opts.file_type, opts.depth);
    }
    let name = st.stacks.get(set).map(|s| s.name.clone()).ok_or("no such texture set")?;
    let (w, h) = (res, res);
    let n = (w * h) as usize;
    let comp = st.approx_compose(set, &[0, 1, 4, 7, 13], res);
    let chan = |bit: u32| comp.channels.get(&bit).filter(|i| i.w == w && i.h == h);
    let has_content = |bit: u32| chan(bit).is_some_and(|i| i.data.iter().any(|v| v.abs() > 1e-4));
    let nrm_stack = st.approx_normal(set, res).filter(|i| i.w == w && i.h == h);
    let nrm_bake = st.bake_image(set, Bake::NormalBase);
    let ao = st.bake_image(set, Bake::AmbientOcclusion);
    let sample = |img: &shard_compose::Image, i: usize, c: usize| {
        let (x, y) = ((i as u32 % w) as u64, (i as u32 / w) as u64);
        let (sx, sy) = ((x * img.w as u64 / w as u64) as u32, (y * img.h as u64 / h as u64) as u32);
        img.pixel(sx.min(img.w - 1), sy.min(img.h - 1))[c.min(img.n - 1)]
    };
    // the viewport keeps normals in the project's (DirectX) convention
    let flip_green = tpl.normal_or(opts.normal) == NormalFormat::OpenGl;
    let value = |s: Src, i: usize| -> f32 {
        match s {
            Src::Base(c) => chan(0).map(|b| shard_compose::linear_to_srgb(b.data[i * b.n + c.min(b.n - 1)])).unwrap_or(0.5),
            Src::Roughness => chan(7).map(|r| r.data[i * r.n]).unwrap_or(0.5),
            Src::Smoothness => 1.0 - chan(7).map(|r| r.data[i * r.n]).unwrap_or(0.5),
            Src::Metallic => chan(13).map(|m| m.data[i * m.n]).unwrap_or(0.0),
            Src::Normal(c) => {
                let v = match (&nrm_stack, &nrm_bake) {
                    (Some(s), _) => s.data[i * s.n + c.min(s.n - 1)],
                    (None, Some(b)) => sample(b, i, c),
                    (None, None) => [0.5, 0.5, 1.0][c.min(2)],
                };
                if c == 1 && flip_green { 1.0 - v } else { v }
            }
            Src::AmbientOcclusion => ao.as_ref().map(|a| sample(a, i, 0)).unwrap_or(1.0),
            Src::Height => chan(1).map(|hgt| 0.5 + 0.5 * hgt.data[i * hgt.n]).unwrap_or(0.5),
            Src::Emissive(c) => chan(4).map(|e| shard_compose::linear_to_srgb(e.data[i * e.n + c.min(e.n - 1)])).unwrap_or(0.0),
            Src::One => 1.0,
            Src::Zero => 0.0,
        }
    };
    let cov = coverage(&st.scene.mesh, set, w, h);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files = Vec::new();
    for out in &tpl.outputs {
        let needs = |bit: u32| out.channels.iter().any(|c| matches!((c, bit), (Src::Emissive(_), 4) | (Src::Height, 1)));
        if out.optional && ((needs(4) && !has_content(4)) || (needs(1) && !has_content(1))) {
            continue;
        }
        let mut px = vec![0f32; n * 4];
        for i in 0..n {
            for (k, s) in out.channels.iter().enumerate() {
                px[i * 4 + k] = value(*s, i);
            }
        }
        match opts.padding {
            None => dilate_f32(&mut px, 4, &cov, w, h, w.max(h)),
            Some(0) => {}
            Some(p) => dilate_f32(&mut px, 4, &cov, w, h, p),
        }
        // grey when R, G and B come from one source; alpha only when the template fills it
        let c = &out.channels;
        let layout = if c[3] != Src::One {
            Layout::Rgba
        } else if c[0] == c[1] && c[1] == c[2] {
            Layout::Gray
        } else {
            Layout::Rgb
        };
        let srgb = c.map(|s| matches!(s, Src::Base(_) | Src::Emissive(_)));
        let file = save_image(&dir.join(out.name.replace("$set", &name)), w, h, &px, layout, srgb, opts.file_type, opts.depth)?;
        files.push(file);
    }
    Ok(files)
}

impl Template {
    fn normal_or(&self, over: Option<NormalFormat>) -> NormalFormat {
        over.unwrap_or(self.normal)
    }
}

/// Export every texture set (or only `only`).
pub fn export_all(st: &AppState, dir: &Path, res: Option<u32>, only: Option<usize>) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for set in 0..st.stacks.len() {
        if only.is_some_and(|o| o != set) {
            continue;
        }
        let r = res.unwrap_or_else(|| st.scene.texs.get(set).map(|t| t.w).unwrap_or(1024));
        files.extend(export_set(st, set, dir, r)?);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every format, depth and channel layout writes a file that reads back with the right size and values.
    #[test]
    fn every_format_round_trips() {
        let dir = std::env::temp_dir().join(format!("shard_export_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (8u32, 4u32);
        // a gradient in R, constant G / B, alpha 0.5
        let data: Vec<f32> = (0..w * h).flat_map(|i| [(i % w) as f32 / (w - 1) as f32, 0.25, 0.75, 0.5]).collect();
        for ft in FileType::ALL {
            for &depth in ft.depths() {
                for layout in [Layout::Gray, Layout::Rgb, Layout::Rgba] {
                    let base = dir.join(format!("t_{}_{:?}_{:?}", ft.label(), depth, layout));
                    let path = save_image(&base, w, h, &data, layout, [false; 4], ft, depth).unwrap_or_else(|e| panic!("{e}"));
                    let tol = if ft == FileType::Jpeg { 0.08 } else { 0.01 };
                    let (rw, rh, px): (u32, u32, Vec<[f32; 4]>) = if ft == FileType::Exr {
                        let img = exr::prelude::read_first_rgba_layer_from_file(
                            &path,
                            |res, _| (res.width(), vec![[0f32; 4]; res.width() * res.height()]),
                            |(wd, v): &mut (usize, Vec<[f32; 4]>), p, (r, g, b, a): (f32, f32, f32, f32)| v[p.y() * *wd + p.x()] = [r, g, b, a],
                        )
                        .unwrap();
                        let size = img.layer_data.size;
                        (size.0 as u32, size.1 as u32, img.layer_data.channel_data.pixels.1)
                    } else {
                        let img = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).to_rgba32f();
                        (img.width(), img.height(), img.pixels().map(|p| p.0).collect())
                    };
                    assert_eq!((rw, rh), (w, h), "{}", path.display());
                    for (i, p) in px.iter().enumerate() {
                        let want = data[i * 4];
                        assert!((p[0] - want).abs() < tol, "{} texel {i}: {} vs {want}", path.display(), p[0]);
                        if layout != Layout::Gray {
                            assert!((p[2] - 0.75).abs() < tol, "{} blue", path.display());
                        }
                        if layout == Layout::Rgba && ft.has_alpha() {
                            assert!((p[3] - 0.5).abs() < tol, "{} alpha {}", path.display(), p[3]);
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
