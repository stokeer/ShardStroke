//! CPU reference compositor (v0).
//!
//! Evaluates the layer stack of a [`shard_model::Document`] per channel, bottom to top
//! (the model lists layers top-first). Purpose: a simple, testable ground truth to validate the
//! GPU compositor against.
//!
//! Supported: uniform-colour fills, UV-projected bitmap fills (via a [`BitmapProvider`]), per-channel
//! blend modes (see [`blend`]), layer/source opacity, uniform/bitmap masks, nested groups
//! (Passthrough and flattened). Not yet: procedural sources, other mask effects, filters, levels,
//! paint strokes, non-UV projections. Everything skipped is reported in [`Composite::unsupported`];
//! with [`Options::skip_unsupported`] a layer that hits an unsupported construct is left out entirely.

use std::collections::BTreeMap;
use std::sync::Arc;

mod procedural;
pub use procedural::{MeshMap, MeshMaps, NoMaps, render_size, Surface, ProjSpec, UvXform, project_sample};

use shard_model::*;

/// Float image with `n` interleaved components per pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub n: usize,
    pub data: Vec<f32>,
}

impl Image {
    pub fn filled(w: u32, h: u32, v: &[f32]) -> Self {
        let n = v.len();
        let mut data = Vec::with_capacity((w * h) as usize * n);
        for _ in 0..(w * h) {
            data.extend_from_slice(v);
        }
        Self { w, h, n, data }
    }
    pub fn pixel(&self, x: u32, y: u32) -> &[f32] {
        let i = ((y * self.w + x) as usize) * self.n;
        &self.data[i..i + self.n]
    }
}

#[derive(Debug, Default)]
pub struct Composite {
    /// Result per channel bit index.
    pub channels: BTreeMap<u32, Image>,
    /// Human readable notes about skipped constructs.
    pub unsupported: Vec<String>,
    /// Procedurals/filters evaluated by an approximation (only with [`Options::allow_approx`]).
    pub approximated: Vec<String>,
}

/// Channel bits with a 3-component representation.
fn components(bit: u32) -> usize {
    match bit {
        0 | 22 | 4 | 45 | 41 | 30 | 47 | 48 => 3, // BaseColor, Normal, Emissive, Scattering/Coat/Sheen/Specular colours
        _ => 1,
    }
}

fn default_value(bit: u32) -> Vec<f32> {
    match bit {
        22 => vec![0.5, 0.5, 1.0],
        7 => vec![0.3], // Painter's default roughness when no layer writes the channel
        // Painter's default base colour: mid grey, linear 0.5 (sRGB 188), what a texel no layer writes shows. Measured on
        // a test screen's ladder (steps 0-14, everything outside the polymer keys: 179 -> 0.6 of error).
        0 => vec![0.5; 3],
        b => vec![0.0; components(b)],
    }
}

/// Blend a single component: `b` = backdrop, `s` = source. Returns the blended value
/// (before opacity interpolation). Implemented: the common separable modes.
pub fn blend(mode: BlendMode, b: f32, s: f32) -> f32 {
    match mode.name() {
        "Normal" | "Replace" | "Passthrough" => s,
        "Multiply" => b * s,
        "Divide" => {
            if s <= 1e-6 {
                1.0
            } else {
                (b / s).min(1.0)
            }
        }
        "InverseDivide" => {
            if b <= 1e-6 {
                1.0
            } else {
                (s / b).min(1.0)
            }
        }
        "Darken" => b.min(s),
        "Lighten" => b.max(s),
        "LinearDodge" => (b + s).min(1.0),
        "Subtract" => (b - s).max(0.0),
        "InverseSubtract" => (s - b).max(0.0),
        "Difference" => (b - s).abs(),
        "Exclusion" => b + s - 2.0 * b * s,
        "Screen" => 1.0 - (1.0 - b) * (1.0 - s),
        "Overlay" => {
            if b < 0.5 {
                2.0 * b * s
            } else {
                1.0 - 2.0 * (1.0 - b) * (1.0 - s)
            }
        }
        "HardLight" => {
            if s < 0.5 {
                2.0 * b * s
            } else {
                1.0 - 2.0 * (1.0 - b) * (1.0 - s)
            }
        }
        // Painter's Soft Light is Photoshop's: sqrt(b) for the whole range. The W3C cubic below 0.25 (the usual
        // formula) made the `highpass` layer of a test vehicle interior too weak: fitted on the ladder export,
        // linear space + this formula reproduces it (MAE 2.6 against 3.7), SHARD_SOFTLIGHT_W3C=1 brings it back.
        "SoftLight" => {
            if s < 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                static W3C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                let d = if b <= 0.25 && *W3C.get_or_init(|| std::env::var_os("SHARD_SOFTLIGHT_W3C").is_some()) {
                    ((16.0 * b - 12.0) * b + 4.0) * b
                } else {
                    b.sqrt()
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        "ColorBurn" => {
            if s <= 1e-6 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }
        "ColorDodge" => {
            if s >= 1.0 - 1e-6 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }
        "LinearBurn" => (b + s - 1.0).max(0.0),
        "LinearLight" => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        "VividLight" => {
            if s < 0.5 {
                let s2 = 2.0 * s;
                if s2 <= 1e-6 { 0.0 } else { 1.0 - ((1.0 - b) / s2).min(1.0) }
            } else {
                let s2 = 2.0 * (s - 0.5);
                if s2 >= 1.0 - 1e-6 { 1.0 } else { (b / (1.0 - s2)).min(1.0) }
            }
        }
        "PinLight" => {
            if s < 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * (s - 0.5))
            }
        }
        _ => s, // unknown/not-yet-implemented: treated as Normal
    }
}

/// Is the mode implemented (not falling back to Normal)?
pub fn is_blend_implemented(mode: BlendMode) -> bool {
    !matches!(
        mode.name(),
        "Tint" | "Saturation" | "Color" | "Value" | "NormalMapCombine" | "NormalMapDetail"
            | "NormalMapInverseDetail" | "SignedAddition" | "Disable"
    )
}

/// Colours in Painter documents (uniform fills) are linear; display/8-bit sRGB textures need encoding.
pub fn linear_to_srgb(v: f32) -> f32 {
    lut_lookup(&LINEAR_TO_SRGB, v)
}

pub fn srgb_to_linear(v: f32) -> f32 {
    lut_lookup(&SRGB_TO_LINEAR, v)
}

fn linear_to_srgb_exact(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear_exact(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The transfer curves go through a 4096-cell table (linear interpolation, error below 2e-5): they run for
/// every texel and component of a colour layer and `powf` dominated the blend of a stack.
const LUT_CELLS: usize = 4096;
static SRGB_TO_LINEAR: std::sync::LazyLock<Vec<f32>> =
    std::sync::LazyLock::new(|| (0..=LUT_CELLS + 1).map(|i| srgb_to_linear_exact(i as f32 / LUT_CELLS as f32)).collect());
static LINEAR_TO_SRGB: std::sync::LazyLock<Vec<f32>> =
    std::sync::LazyLock::new(|| (0..=LUT_CELLS + 1).map(|i| linear_to_srgb_exact(i as f32 / LUT_CELLS as f32)).collect());

#[inline]
fn lut_lookup(table: &[f32], v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * LUT_CELLS as f32;
    // NaN casts to 0
    let i = (x as usize).min(LUT_CELLS);
    let t = x - i as f32;
    table[i] + (table[i + 1] - table[i]) * t
}

/// Pseudo channel bit under which mask-stack anchors are stored.
const MASK_ANCHOR: u32 = 1000;

/// Side (px) a textured brush's alpha graph is rendered at: the stamp is resampled from it.
pub const BRUSH_SHAPE_SIDE: u32 = 512;

/// A brush whose alpha is a graph of its own (`knife_01`, `alpha_dirt_03`, `shape`...), not the round `shape_v2`
/// whose hardness profile is evaluated analytically.
pub fn brush_shape_is_textured(p: &Procedural) -> bool {
    p.resource.is_some() && p.graph_name() != Some("shape_v2")
}

/// Replays recorded paint strokes of a texture set into a single-channel texture-space buffer.
pub trait StrokeRaster {
    /// Blend every stroke of `action` into `buf` (`w * h`): towards `value` for painting, or towards
    /// `revert` when erasing. `stencil` is the rendered image of a procedural stencil (a symbol, a noise):
    /// its first channel limits where the strokes show. `shape` is the rendered alpha of the brush when it is not
    /// the plain round one (`knife_01`, `alpha_dirt_03`...): a square image spanning the brush diameter.
    fn paint(
        &self,
        action: &shard_model::PaintAction,
        buf: &mut [f32],
        w: u32,
        h: u32,
        value: f32,
        revert: Option<f32>,
        stencil: Option<&Image>,
        shape: Option<&Image>,
    );

    /// Set `buf` to `value` over the geometry selected by the decal / polygon-fill `hits`.
    fn decal(&self, _hits: &[shard_model::DecalHit], _buf: &mut [f32], _w: u32, _h: u32, _value: f32) -> bool {
        false
    }

    /// Identity of the mesh and settings the replay depends on (see [`MeshMaps::cache_token`]).
    fn cache_token(&self) -> Option<u64> {
        None
    }

}

/// Supplies decoded bitmap resources by the URL stored in the document.
pub trait BitmapProvider {
    fn bitmap(&self, resource: &str) -> Option<Arc<Image>>;

    /// Are the values of this bitmap sRGB-encoded when it fills a colour channel? (Known after `bitmap`.)
    fn colour_encoded(&self, _resource: &str) -> bool {
        false
    }
}

pub struct NoBitmaps;

impl BitmapProvider for NoBitmaps {
    fn bitmap(&self, _: &str) -> Option<Arc<Image>> {
        None
    }
}

pub struct Options<'a> {
    pub bitmaps: &'a dyn BitmapProvider,
    /// Baked mesh maps of the texture set (inputs of curvature/occlusion generators).
    pub maps: &'a dyn MeshMaps,
    /// Stroke replay (needs the mesh); without it painted strokes are ignored.
    pub strokes: Option<&'a dyn StrokeRaster>,
    /// Leave out any layer that contains an unsupported construct (instead of applying it partially).
    pub skip_unsupported: bool,
    /// Evaluate known procedurals/filters with approximations instead of reporting them unsupported.
    pub allow_approx: bool,
    /// The project works in ACEScg (`colorManagementWorkflow` OCIO with the ACES config): uniform colours are
    /// ACEScg, sRGB inputs (bitmaps, graph outputs) are brought into it, the stack is blended there and the colour
    /// channels leave in linear sRGB like the rest of ShardStroke (a test tank: +26 base colour MAE read as linear sRGB).
    pub aces_working: bool,
}

/// Linear sRGB (D65) -> ACEScg (AP1, D60), Bradford adaptation (the OCIO ACES 1.2 config).
pub const SRGB_TO_ACESCG: [[f32; 3]; 3] =
    [[0.613_097_4, 0.339_523_1, 0.047_379_5], [0.070_193_7, 0.916_353_9, 0.013_452_4], [0.020_615_6, 0.109_569_8, 0.869_814_6]];
/// ACEScg -> linear sRGB.
pub const ACESCG_TO_SRGB: [[f32; 3]; 3] =
    [[1.705_051, -0.621_792_1, -0.083_258_9], [-0.130_256_4, 1.140_804_8, -0.010_548_3], [-0.024_003_4, -0.128_968_9, 1.152_972_3]];

/// Apply a colour matrix to the first three components of every pixel of `img` (n >= 3).
pub fn convert_colour(img: &mut Image, m: &[[f32; 3]; 3]) {
    if img.n < 3 {
        return;
    }
    let n = img.n;
    for px in img.data.chunks_exact_mut(n) {
        let (r, g, b) = (px[0], px[1], px[2]);
        for (k, row) in m.iter().enumerate() {
            px[k] = row[0] * r + row[1] * g + row[2] * b;
        }
    }
}

/// Grow the covered texels of an interleaved float image (`n` components, `w` x `h`) outwards by up to `iters`
/// texels: every newly reached texel takes the average of its already filled 4-neighbours. Only the moving border
/// is visited, so padding the whole texture costs one visit per texel.
pub fn dilate(px: &mut [f32], n: usize, cov: &[bool], w: u32, h: u32, iters: u32) {
    let (wi, hi) = (w as usize, h as usize);
    let mut filled = cov.to_vec();
    let nb = |i: usize| -> [Option<usize>; 4] {
        let (x, y) = (i % wi, i / wi);
        [(x + 1 < wi).then(|| i + 1), (x > 0).then(|| i - 1), (y + 1 < hi).then(|| i + wi), (y > 0).then(|| i - wi)]
    };
    let mut frontier: Vec<usize> = (0..filled.len()).filter(|&i| filled[i] && nb(i).iter().flatten().any(|&j| !filled[j])).collect();
    let mut mark = vec![false; filled.len()];
    for _ in 0..iters {
        let mut cand: Vec<usize> = Vec::new();
        for &i in &frontier {
            for j in nb(i).into_iter().flatten() {
                if !filled[j] && !mark[j] {
                    mark[j] = true;
                    cand.push(j);
                }
            }
        }
        if cand.is_empty() {
            break;
        }
        let mut vals = vec![0f32; cand.len() * n];
        for (k, &c) in cand.iter().enumerate() {
            let mut cnt = 0f32;
            for j in nb(c).into_iter().flatten() {
                if filled[j] {
                    for ch in 0..n {
                        vals[k * n + ch] += px[j * n + ch];
                    }
                    cnt += 1.0;
                }
            }
            for ch in 0..n {
                vals[k * n + ch] /= cnt.max(1.0);
            }
        }
        for (k, &c) in cand.iter().enumerate() {
            px[c * n..c * n + n].copy_from_slice(&vals[k * n..k * n + n]);
            filled[c] = true;
            mark[c] = false;
        }
        frontier = cand;
    }
}

impl Default for Options<'static> {
    fn default() -> Self {
        Options { bitmaps: &NoBitmaps, maps: &NoMaps, strokes: None, skip_unsupported: false, allow_approx: false, aces_working: false }
    }
}

struct Ctx<'a> {
    w: u32,
    h: u32,
    notes: &'a mut Vec<String>,
    approx: &'a mut Vec<String>,
    opts: &'a Options<'a>,
    /// Evaluated masks by layer address, with the notes/approximations they produced (masks do not
    /// depend on the channel, so several channels of one document share them).
    masks: &'a mut std::collections::HashMap<usize, (Scalar, Vec<String>, Vec<String>, bool)>,
    /// Anchor point snapshots by `(anchor uid, channel bit)`; mask values use [`MASK_ANCHOR`].
    anchors: &'a mut std::collections::HashMap<(u64, u32), Arc<Image>>,
    /// Keys of the anchor points written by this pass, in order (a checkpoint carries those of its layers).
    anchor_log: Vec<(u64, u32)>,
    /// Coverage (0..1 per texel) of the isolated group whose children are being composed: what its layers
    /// blend against where nothing was written yet is empty, not the backdrop below the group.
    alpha: Option<Vec<f32>>,
    /// Set while evaluating a mask that contains something the evaluator does not reproduce.
    unreliable: bool,
    /// Replay the polygon-fill clicks of a group whose mask is made only of them (otherwise such a group is
    /// shown everywhere). Set for the channels where the group blends as an isolated group.
    replay_group_decals: bool,
    /// Document name (for diagnostics).
    name: &'a str,
    /// Checkpoint bookkeeping (see [`checkpoints`]): hashes of every layer of the stack by address, when the
    /// stack qualifies.
    ck: Option<&'a std::collections::HashMap<usize, u64>>,
    /// Key of the accumulator before the layer being applied.
    ck_cur: u64,
    /// Base key and "is the top level" flag handed to the next `apply_layers` call (a group's children).
    ck_next: Option<(u64, bool)>,
}

/// Publish an anchor point (and remember that this pass wrote it).
fn set_anchor(ctx: &mut Ctx, key: (u64, u32), img: Arc<Image>) {
    ctx.anchors.insert(key, img);
    ctx.anchor_log.push(key);
}

pub fn compose(doc: &Document, w: u32, h: u32, channel_bits: &[u32]) -> Composite {
    compose_with(doc, w, h, channel_bits, &Options::default())
}

pub fn compose_with(doc: &Document, w: u32, h: u32, channel_bits: &[u32], opts: &Options) -> Composite {
    let mut notes = Vec::new();
    let mut approx = Vec::new();
    let mut masks = std::collections::HashMap::new();
    let mut anchors = std::collections::HashMap::new();
    let mut out = Composite::default();
    let layer_hashes = layer_hash_map(&doc.layers, opts);
    for &bit in channel_bits {
        let mut acc = Image::filled(w, h, &default_value(bit));
        let mut ctx = Ctx { w, h, notes: &mut notes, approx: &mut approx, opts, masks: &mut masks, anchors: &mut anchors, unreliable: false, replay_group_decals: false, name: &doc.name, ck: layer_hashes.as_ref(), ck_cur: 0, ck_next: None, anchor_log: Vec::new(), alpha: None };
        let t_bit = std::time::Instant::now();
        ctx.ck_next = top_level_base(&ctx, bit, &acc);
        apply_layers(&doc.layers, bit, &mut acc, &mut ctx);
        if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
            eprintln!("[time] channel {bit}: {:?}", t_bit.elapsed());
        }
        leave_working_space(&mut acc, bit, opts);
        out.channels.insert(bit, acc);
    }
    notes.sort();
    notes.dedup();
    approx.sort();
    approx.dedup();
    out.unsupported = notes;
    out.approximated = approx;
    out
}

/// A colour channel composed in ACEScg goes back to linear sRGB.
fn leave_working_space(acc: &mut Image, bit: u32, opts: &Options) {
    if opts.aces_working && matches!(bit, 0 | 4) {
        convert_colour(acc, &ACESCG_TO_SRGB);
    }
}

/// Does any visible layer of `doc` write channel `bit` (a fill source or paint strokes aimed at it)? A stack
/// that does not has a flat result there, so the channel need not be evaluated.
pub fn writes_channel(doc: &Document, bit: u32) -> bool {
    fn eff(es: &[Effect], bit: u32) -> bool {
        es.iter().filter(|e| e.visible).any(|e| match &e.kind {
            EffectKind::Fill(f) => f.sources.iter().any(|s| match s {
                Source::Uniform { channels, .. }
                | Source::Bitmap { channels, .. }
                | Source::Procedural { channels, .. }
                | Source::Reference { channels, .. } => channels.contains(bit),
                _ => false,
            }),
            EffectKind::Paint(a) => !a.strokes.is_empty() && (e.channels.is_empty() || e.channels.contains(bit)),
            EffectKind::Group { children } => eff(children, bit),
            _ => false,
        })
    }
    fn layers(ls: &[Layer], bit: u32) -> bool {
        ls.iter().filter(|l| l.visible).any(|l| {
            let (mode, op) = blend_for(&l.blend, bit, l.kind == LayerKind::Group);
            (mode.name() != "Disable" && op > 0.0 && eff(&l.content, bit)) || layers(&l.children, bit)
        })
    }
    layers(&doc.layers, bit)
}

/// How the final tangent-space normal map of a texture set is put together (Painter's "Normal mixing").

#[derive(Clone, Copy, Debug)]
pub struct NormalMix {
    /// Slope per UV unit for a height of 1.0 (the height channel is turned into a normal with a Sobel
    /// filter and combined into the normal channel).
    pub height_strength: f32,
    /// Invert the green component of the height-derived normal.
    pub flip_y: bool,
    /// Combine with reoriented normal mapping instead of adding slopes.
    pub reoriented: bool,
}

impl Default for NormalMix {
    fn default() -> Self {
        let f = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        NormalMix {
            // Painter's shader API (`lib-normal.glsl`, normalFromHeight): slope = height_force / 100 per UV unit of
            // height gradient (Sobel weights 4, offsets of one texel, z = HEIGHT_FACTOR 400) -> 1e-2 at the default
            // force. The least-squares fits on a test vehicle exports gave 4.5e-3 .. 5.9e-3 with low correlation (which
            // biases a fitted slope down); a test pistol's relief looked half as strong as Painter's at 5e-3.
            height_strength: f("SHARD_H2N", 1.0e-2),
            flip_y: std::env::var_os("SHARD_H2N_FLIPY").is_some(),
            reoriented: std::env::var_os("SHARD_N_RNM").is_some(),
        }
    }
}

/// Tangent-space normal (0..1 encoded RGB) of a height field, 3x3 Sobel with wrapping borders.
pub fn normal_from_height(h: &Image, strength: f32, flip_y: bool) -> Image {
    let (w, hh) = (h.w as i32, h.h as i32);
    // slope per UV unit: height difference per texel times the number of texels across
    let k = strength * h.w as f32;
    let at = |x: i32, y: i32| h.data[(y.rem_euclid(hh) * w + x.rem_euclid(w)) as usize * h.n];
    let mut data = Vec::with_capacity((h.w * h.h) as usize * 3);
    for y in 0..hh {
        for x in 0..w {
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1))
                / 8.0;
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1))
                / 8.0;
            let (nx, mut ny) = (-gx * k, -gy * k);
            if flip_y {
                ny = -ny;
            }
            let l = (nx * nx + ny * ny + 1.0).sqrt();
            data.extend_from_slice(&[nx / l * 0.5 + 0.5, ny / l * 0.5 + 0.5, 1.0 / l * 0.5 + 0.5]);
        }
    }
    Image { w: h.w, h: h.h, n: 3, data }
}

/// Combine two tangent-space normal maps (0..1 encoded): `detail` over `base`.
pub fn combine_normals(base: &Image, detail: &Image, reoriented: bool) -> Image {
    let n = (base.w * base.h) as usize;
    let mut data = Vec::with_capacity(n * 3);
    for i in 0..n {
        let b = [base.data[i * base.n] * 2.0 - 1.0, base.data[i * base.n + 1] * 2.0 - 1.0, base.data[i * base.n + 2] * 2.0 - 1.0];
        let d = [detail.data[i * 3] * 2.0 - 1.0, detail.data[i * 3 + 1] * 2.0 - 1.0, detail.data[i * 3 + 2] * 2.0 - 1.0];
        let r = if reoriented {
            // reoriented normal mapping (Barre-Brisebois / Hill)
            let t = [b[0], b[1], b[2] + 1.0];
            let u = [-d[0], -d[1], d[2]];
            let dot = t[0] * u[0] + t[1] * u[1] + t[2] * u[2];
            let tz = if t[2].abs() < 1e-6 { 1e-6 } else { t[2] };
            [t[0] * dot / tz - u[0], t[1] * dot / tz - u[1], t[2] * dot / tz - u[2]]
        } else {
            // slopes add (whiteout): xy summed, z multiplied
            [b[0] + d[0], b[1] + d[1], b[2] * d[2]]
        };
        // keep the length of the base vector: a map that was averaged down (shorter than 1) stays as short as
        // the export of the full resolution map averaged down would be, instead of being stretched back to 1
        let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt().clamp(1e-6, 1.0);
        let l = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt().max(1e-6) / lb;
        data.extend_from_slice(&[r[0] / l * 0.5 + 0.5, r[1] / l * 0.5 + 0.5, r[2] / l * 0.5 + 0.5]);
    }
    Image { w: base.w, h: base.h, n: 3, data }
}

/// Final normal map of a set: the mesh normal map with the stack's Height (as a normal) and Normal channels
/// combined over it. `mesh` may be absent (flat), `height` / `stack` come from [`compose`].
pub fn assemble_normal(mesh: Option<&Image>, height: Option<&Image>, stack: Option<&Image>, w: u32, h: u32, mix: &NormalMix) -> Image {
    let flat = Image::filled(w, h, &[0.5, 0.5, 1.0]);
    // stack normal = (height-derived) over (normal channel)
    let mut cur = match stack {
        Some(s) if s.w == w && s.h == h => s.clone(),
        _ => flat.clone(),
    };
    if let Some(hf) = height {
        if hf.w == w && hf.h == h && hf.data.iter().any(|v| *v != hf.data[0]) {
            let nh = normal_from_height(hf, mix.height_strength, mix.flip_y);
            cur = combine_normals(&cur, &nh, mix.reoriented);
        }
    }
    match mesh {
        Some(m) if m.w == w && m.h == h => combine_normals(m, &cur, mix.reoriented),
        Some(m) => {
            // resample the mesh normal map to the compose size (box average when shrinking, like the
            // export of the full resolution map averaged down)
            let chans: Vec<Vec<f32>> = (0..3)
                .map(|c| {
                    let src: Vec<f32> = (0..(m.w * m.h) as usize).map(|i| m.data[i * m.n + c.min(m.n - 1)] * 2.0 - 1.0).collect();
                    procedural::resample_map(&src, m.w as usize, m.h as usize, w as usize, h as usize)
                })
                .collect();
            let mut d = Vec::with_capacity((w * h * 3) as usize);
            for i in 0..(w * h) as usize {
                for ch in &chans {
                    d.push(ch[i] * 0.5 + 0.5);
                }
            }
            combine_normals(&Image { w, h, n: 3, data: d }, &cur, mix.reoriented)
        }
        None => cur,
    }
}

/// Blend mode/opacity of a layer for channel `bit`. Stored entries are overrides: a channel without
/// an entry uses the default of the layer kind (Passthrough for groups, Normal for layers).
/// A layer's mask as evaluated by this compositor, for inspection (Alt+click on a mask thumbnail).
pub struct MaskPreview {
    pub values: Vec<f32>,
    /// False when the mask contains something that is approximated or not reproduced.
    pub reliable: bool,
    /// Why the mask is not exact (unsupported constructs, approximations, replayed strokes).
    pub notes: Vec<String>,
}

/// Anchor uids read by the effects (`Reference` sources, also as procedural inputs).
fn effect_refs(effects: &[Effect], out: &mut Vec<u64>) {
    fn src(s: &Source, out: &mut Vec<u64>) {
        match s {
            Source::Reference { target, .. } => out.push(*target),
            Source::Procedural { procedural, .. } => procedural.input_sources.iter().for_each(|x| src(x, out)),
            _ => {}
        }
    }
    for e in effects {
        match &e.kind {
            EffectKind::Fill(f) => f.sources.iter().for_each(|s| src(s, out)),
            EffectKind::Filter { procedural } => {
                if let Some(p) = procedural {
                    p.input_sources.iter().for_each(|s| src(s, out));
                }
            }
            EffectKind::Group { children } => effect_refs(children, out),
            _ => {}
        }
    }
}

/// Does the effect list define anchor `uid` (also inside groups)?
fn defines_anchor(effects: &[Effect], uid: u64) -> bool {
    effects.iter().any(|e| match &e.kind {
        EffectKind::Anchor { .. } => e.uid == uid,
        EffectKind::Group { children } => defines_anchor(children, uid),
        _ => false,
    })
}

/// Evaluate (in dependency order) only the layer masks that define the anchors `target`'s mask reads.
/// Returns false when an anchor cannot be produced that way (defined in layer content, or missing).
fn eval_anchor_holders(doc: &Document, target: &Layer, ctx: &mut Ctx) -> bool {
    fn flat<'a>(ls: &'a [Layer], out: &mut Vec<&'a Layer>) {
        for l in ls {
            out.push(l);
            flat(&l.children, out);
        }
    }
    let mut all = Vec::new();
    flat(&doc.layers, &mut all);
    let mut needed: Vec<u64> = Vec::new();
    if let Some(m) = &target.mask {
        effect_refs(&m.effects, &mut needed);
    }
    let mut holders: Vec<&Layer> = Vec::new();
    let mut k = 0;
    while k < needed.len() {
        let uid = needed[k];
        k += 1;
        if uid == 0 {
            continue; // unset reference: nothing to evaluate
        }
        if all.iter().any(|l| defines_anchor(&l.content, uid)) {
            if std::env::var_os("SHARD_TRACE").is_some() {
                eprintln!("[anchors] uid {uid} is defined in layer content -> full walk");
            }
            return false;
        }
        let Some(h) = all.iter().find(|l| l.mask.as_ref().is_some_and(|m| defines_anchor(&m.effects, uid))) else {
            if std::env::var_os("SHARD_TRACE").is_some() {
                eprintln!("[anchors] uid {uid} not defined anywhere -> full walk");
            }
            return false;
        };
        if !holders.iter().any(|x| std::ptr::eq(*x, *h)) {
            holders.push(h);
            if let Some(m) = &h.mask {
                effect_refs(&m.effects, &mut needed);
            }
        }
    }
    // lower layers first: their anchors feed the ones above
    let pos = |l: &Layer| all.iter().position(|x| std::ptr::eq(*x, l)).unwrap_or(0);
    holders.sort_by_key(|l| std::cmp::Reverse(pos(l)));
    for h in holders {
        let _ = mask_value(h, ctx);
    }
    true
}

/// Does any effect read an anchor point (a `Reference` source, also as a procedural input)?
fn effects_need_anchors(effects: &[Effect]) -> bool {
    fn src(s: &Source) -> bool {
        match s {
            Source::Reference { .. } => true,
            Source::Procedural { procedural, .. } => procedural.input_sources.iter().any(src),
            _ => false,
        }
    }
    effects.iter().any(|e| match &e.kind {
        EffectKind::Fill(f) => f.sources.iter().any(src),
        EffectKind::Filter { procedural } => procedural.as_ref().is_some_and(|p| p.input_sources.iter().any(src)),
        EffectKind::Group { children } => effects_need_anchors(children),
        EffectKind::Anchor { .. } => false,
        _ => false,
    })
}

/// Evaluate the mask of the layer at `path` (top-first indices through the groups). The whole stack
/// is walked first so anchors referenced by the mask exist; the mask itself comes from that pass.
pub fn layer_mask(doc: &Document, path: &[usize], w: u32, h: u32, opts: &Options) -> Option<MaskPreview> {
    let mut layers = &doc.layers;
    let mut target = None;
    for (k, &i) in path.iter().enumerate() {
        let l = layers.get(i)?;
        if k + 1 == path.len() {
            target = Some(l);
        } else {
            layers = &l.children;
        }
    }
    let target = target?;
    let (mut notes, mut approx) = (Vec::new(), Vec::new());
    let mut masks = std::collections::HashMap::new();
    let mut anchors = std::collections::HashMap::new();
    let mut ctx = Ctx { w, h, notes: &mut notes, approx: &mut approx, opts, masks: &mut masks, anchors: &mut anchors, unreliable: false, replay_group_decals: false, name: &doc.name, ck: None, ck_cur: 0, ck_next: None, anchor_log: Vec::new(), alpha: None };
    // Anchors only exist once the layers that define them were evaluated. Evaluate just those
    // (transitively), not the whole stack; fall back to the full walk when an anchor lives in layer
    // content. This keeps interactive previews fast.
    if target.mask.as_ref().is_some_and(|m| effects_need_anchors(&m.effects)) {
        if !eval_anchor_holders(doc, target, &mut ctx) {
            let mut acc = Image::filled(w, h, &default_value(0));
            apply_layers(&doc.layers, 0, &mut acc, &mut ctx);
        }
    }
    // showing a mask is showing what its clicks select, also for a group (whose clicks are otherwise left out
    // on the channels where it passes through)
    ctx.replay_group_decals = target.kind == LayerKind::Group;
    let (m, unreliable) = mask_value(target, &mut ctx);
    ctx.replay_group_decals = false;
    let npx = (w * h) as usize;
    let values = match m {
        Scalar::Const(c) => vec![c; npx],
        Scalar::Map(v) => v,
    };
    let mut why: Vec<String> = Vec::new();
    // notes/approximations that mention this layer
    let key = format!("'{}'", target.name);
    for n in ctx.notes.iter().chain(ctx.approx.iter()) {
        if n.contains(&key) && !why.contains(n) {
            why.push(n.clone());
        }
    }
    Some(MaskPreview { values, reliable: !unreliable && why.is_empty(), notes: why })
}

/// Like [`compose_with`], and also returns the evaluated mask of every layer that has one
/// (`path`, values), harvested from the pass so showing a mask afterwards costs nothing.
pub fn compose_with_masks(
    doc: &Document,
    w: u32,
    h: u32,
    channel_bits: &[u32],
    opts: &Options,
) -> (Composite, Vec<(Vec<usize>, Vec<f32>)>) {
    let mut notes = Vec::new();
    let mut approx = Vec::new();
    let mut masks = std::collections::HashMap::new();
    let mut anchors = std::collections::HashMap::new();
    let mut out = Composite::default();
    let mut harvested = Vec::new();
    let layer_hashes = layer_hash_map(&doc.layers, opts);
    for (k, &bit) in channel_bits.iter().enumerate() {
        let mut acc = Image::filled(w, h, &default_value(bit));
        let mut ctx = Ctx { w, h, notes: &mut notes, approx: &mut approx, opts, masks: &mut masks, anchors: &mut anchors, unreliable: false, replay_group_decals: false, name: &doc.name, ck: layer_hashes.as_ref(), ck_cur: 0, ck_next: None, anchor_log: Vec::new(), alpha: None };
        ctx.ck_next = top_level_base(&ctx, bit, &acc);
        apply_layers(&doc.layers, bit, &mut acc, &mut ctx);
        if k == 0 {
            fn walk(ls: &[Layer], path: &mut Vec<usize>, ctx: &mut Ctx, out: &mut Vec<(Vec<usize>, Vec<f32>)>) {
                for (i, l) in ls.iter().enumerate() {
                    path.push(i);
                    if l.mask.as_ref().is_some_and(|m| m.enabled && !m.effects.is_empty()) {
                        ctx.replay_group_decals = l.kind == LayerKind::Group;
                        let (m, _) = mask_value(l, ctx);
                        ctx.replay_group_decals = false;
                        let npx = (ctx.w * ctx.h) as usize;
                        out.push((
                            path.clone(),
                            match m {
                                Scalar::Const(c) => vec![c; npx],
                                Scalar::Map(v) => v,
                            },
                        ));
                    }
                    walk(&l.children, path, ctx, out);
                    path.pop();
                }
            }
            walk(&doc.layers, &mut Vec::new(), &mut ctx, &mut harvested);
        }
        leave_working_space(&mut acc, bit, opts);
        out.channels.insert(bit, acc);
    }
    notes.sort();
    notes.dedup();
    approx.sort();
    approx.dedup();
    out.unsupported = notes;
    out.approximated = approx;
    (out, harvested)
}

/// Every mask of the stack (one pass): `(path, layer name, preview)` in top-first tree order.
pub fn all_masks(doc: &Document, w: u32, h: u32, opts: &Options) -> Vec<(Vec<usize>, String, MaskPreview)> {
    let (mut notes, mut approx) = (Vec::new(), Vec::new());
    let mut masks = std::collections::HashMap::new();
    let mut anchors = std::collections::HashMap::new();
    let mut ctx = Ctx { w, h, notes: &mut notes, approx: &mut approx, opts, masks: &mut masks, anchors: &mut anchors, unreliable: false, replay_group_decals: false, name: &doc.name, ck: None, ck_cur: 0, ck_next: None, anchor_log: Vec::new(), alpha: None };
    let mut acc = Image::filled(w, h, &default_value(0));
    apply_layers(&doc.layers, 0, &mut acc, &mut ctx);
    let mut out = Vec::new();
    fn walk(
        ls: &[Layer],
        path: &mut Vec<usize>,
        ctx: &mut Ctx,
        out: &mut Vec<(Vec<usize>, String, MaskPreview)>,
    ) {
        for (i, l) in ls.iter().enumerate() {
            path.push(i);
            if l.mask.as_ref().is_some_and(|m| m.enabled && !m.effects.is_empty()) {
                let n0 = ctx.notes.len();
                let (m, unreliable) = mask_value(l, ctx);
                let _ = n0;
                let npx = (ctx.w * ctx.h) as usize;
                let values = match m {
                    Scalar::Const(c) => vec![c; npx],
                    Scalar::Map(v) => v,
                };
                let key = format!("'{}'", l.name);
                let mut why: Vec<String> = Vec::new();
                for n in ctx.notes.iter().chain(ctx.approx.iter()) {
                    if n.contains(&key) && !why.contains(n) {
                        why.push(n.clone());
                    }
                }
                out.push((path.clone(), l.name.clone(), MaskPreview { values, reliable: !unreliable && why.is_empty(), notes: why }));
            }
            walk(&l.children, path, ctx, out);
            path.pop();
        }
    }
    walk(&doc.layers, &mut Vec::new(), &mut ctx, &mut out);
    out
}

/// What the thumbnails of a layer stack show on one channel (see [`thumbnails`]).
#[derive(Default)]
pub struct Thumbnails {
    /// Content of each layer on its own: `(path, image)`. Groups, layers made only of filters or of paint
    /// strokes, and layers that do not write the channel have none.
    pub contents: Vec<(Vec<usize>, Image)>,
    /// Evaluated mask of each layer that has generators / clicks / strokes in it: `(path, values)`.
    pub masks: Vec<(Vec<usize>, Vec<f32>)>,
}

fn effects_have_fill(es: &[Effect]) -> bool {
    es.iter().any(|e| match &e.kind {
        EffectKind::Fill(_) => true,
        EffectKind::Group { children } => effects_have_fill(children),
        _ => false,
    })
}

/// Contents and masks of every layer for the thumbnails of the layers panel: one pass that first composes the
/// stack (so anchor points and masks exist) and then asks each layer for what it writes on `bit`, alone.
pub fn thumbnails(doc: &Document, w: u32, h: u32, bit: u32, opts: &Options) -> Thumbnails {
    let (mut notes, mut approx) = (Vec::new(), Vec::new());
    let mut masks = std::collections::HashMap::new();
    let mut anchors = std::collections::HashMap::new();
    let mut ctx = Ctx { w, h, notes: &mut notes, approx: &mut approx, opts, masks: &mut masks, anchors: &mut anchors, unreliable: false, replay_group_decals: false, name: &doc.name, ck: None, ck_cur: 0, ck_next: None, anchor_log: Vec::new(), alpha: None };
    let mut acc = Image::filled(w, h, &default_value(bit));
    apply_layers(&doc.layers, bit, &mut acc, &mut ctx);
    let base = Image::filled(w, h, &default_value(bit));
    let n = base.n;
    let npx = (w * h) as usize;
    fn walk(ls: &[Layer], path: &mut Vec<usize>, base: &Image, n: usize, bit: u32, npx: usize, ctx: &mut Ctx, out: &mut Thumbnails) {
        for (i, l) in ls.iter().enumerate() {
            path.push(i);
            if l.kind != LayerKind::Group && effects_have_fill(&l.content) {
                if let Some((content, _)) = content_value(l, bit, n, base, ctx) {
                    let img = match content {
                        Content::Map(img) => img,
                        Content::Const(v) => {
                            let mut data = Vec::with_capacity(npx * v.len());
                            for _ in 0..npx {
                                data.extend_from_slice(&v);
                            }
                            Image { w: ctx.w, h: ctx.h, n: v.len(), data }
                        }
                    };
                    out.contents.push((path.clone(), img));
                }
            }
            if l.mask.as_ref().is_some_and(|m| m.enabled && !m.effects.is_empty()) {
                ctx.replay_group_decals = l.kind == LayerKind::Group;
                let (m, _) = mask_value(l, ctx);
                ctx.replay_group_decals = false;
                let values = match m {
                    Scalar::Const(c) => vec![c; npx],
                    Scalar::Map(v) => v,
                };
                out.masks.push((path.clone(), values));
            }
            walk(&l.children, path, base, n, bit, npx, ctx, out);
            path.pop();
        }
    }
    let mut out = Thumbnails::default();
    walk(&doc.layers, &mut Vec::new(), &base, n, bit, npx, &mut ctx, &mut out);
    out
}

fn blend_for(layer_blend: &[ChannelBlend], bit: u32, is_group: bool) -> (BlendMode, f32) {
    for b in layer_blend {
        if b.channels.contains(bit) {
            return (b.mode, b.opacity);
        }
    }
    // A group channel without an entry composes isolated with a Normal blend, not as Passthrough: in a test vehicle
    // interior the `Overlay` of `Fill layer 5` over Roughness behaves as a plain replace, which is what Overlay
    // over an empty group does (interior set roughness MAE 25.2 -> 11.0), and the filter stack group, which
    // does want Passthrough, lists it explicitly for every channel. SHARD_GROUP_DEFAULT=passthrough restores the
    // old default.
    // Height adds by default (the Painter API reports LinearDodge on the Height of every layer and group that lists
    // nothing for it): read as Normal, a test pistol's grip fill -- a fill of every channel with height 0 -- wiped
    // the engravings of the grip and slide under it. SHARD_HEIGHT_DEFAULT_NORMAL=1 restores the old reading.
    if bit == 1 && std::env::var_os("SHARD_HEIGHT_DEFAULT_NORMAL").is_none() {
        return (BlendMode(10), 1.0); // LinearDodge
    }
    let mode_knob = std::env::var("SHARD_GROUP_DEFAULT").ok();
    let group_default = if is_group
        && match mode_knob.as_deref() {
            Some("passthrough") => false,
            // experiments: only the groups that list some channels / only scalar channels
            Some("listed_all_channels") => !layer_blend.is_empty(),
            Some("scalar_if_listed") => !layer_blend.is_empty() && !matches!(bit, 1 | 22),
            _ => true,
        }
    {
        BlendMode::NORMAL
    } else if is_group {
        BlendMode::PASSTHROUGH
    } else {
        BlendMode::NORMAL
    };
    (group_default, 1.0)
}

/// Run `f(texel index, components, coverage)` over every texel of `acc` (and of `alpha`, when there is one),
/// rows in parallel.
fn for_texels(acc: &mut Image, alpha: Option<&mut Vec<f32>>, f: impl Fn(usize, &mut [f32], Option<&mut f32>) + Sync) {
    use rayon::prelude::*;
    let (w, n) = (acc.w as usize, acc.n);
    match alpha {
        Some(al) => {
            acc.data.par_chunks_mut(w * n).zip(al.par_chunks_mut(w)).enumerate().for_each(|(y, (prow, arow))| {
                for x in 0..w {
                    f(y * w + x, &mut prow[x * n..(x + 1) * n], Some(&mut arow[x]));
                }
            });
        }
        None => {
            acc.data.par_chunks_mut(w * n).enumerate().for_each(|(y, prow)| {
                for x in 0..w {
                    f(y * w + x, &mut prow[x * n..(x + 1) * n], None);
                }
            });
        }
    }
}

/// One component of a source `s` with blend `mode` and strength `a` over the colour `d` of a buffer that is
/// `pa` covered: the standard compositing of a layer onto a group that may be partly empty. Returns the new
/// colour and coverage.
#[inline]
fn composite_px(mode: BlendMode, d: f32, s: f32, a: f32, pa: f32) -> (f32, f32) {
    if pa >= 1.0 {
        return (d + (blend(mode, d, s) - d) * a, 1.0);
    }
    let ar = a + pa * (1.0 - a);
    if ar <= 0.0 {
        return (d, pa);
    }
    let mixed = (1.0 - pa) * s + pa * blend(mode, d, s);
    (d + (mixed - d) * (a / ar), ar)
}

/// Accumulators of the top-level pass kept between compositions ("checkpoints"): after an edit, the layers
/// below the edited one are not applied again; the pass resumes from the picture the unchanged prefix of the
/// stack produced. A checkpoint is found by a hash chain over the layers from the bottom, so any change to a
/// layer (or one below it) changes every key above it and nothing stale can match.
mod checkpoints {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    use crate::Image;

    /// Budget in bytes of float data (`SHARD_CKPT_MB`, default 1024). Anchor images are shared between
    /// checkpoints and counted once: counted per checkpoint, a test pistol's ~30 mask anchors filled the old 384 MB
    /// with the five channels of one composition and evicted the stack before the next edit could resume from it
    /// (hiding the top group recomposed everything).
    fn budget() -> usize {
        static B: OnceLock<usize> = OnceLock::new();
        *B.get_or_init(|| std::env::var("SHARD_CKPT_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1024) << 20)
    }

    pub type Anchors = Vec<((u64, u32), Arc<Image>)>;

    pub struct Entry {
        pub image: Arc<Image>,
        /// Notes / approximations the layers up to this point produced on this channel.
        pub notes: Vec<String>,
        pub approx: Vec<String>,
        /// Anchor points the layers of the level wrote up to this point.
        pub anchors: Anchors,
        /// Coverage of the isolated group being composed (None outside one).
        pub alpha: Option<Arc<Vec<f32>>>,
        tick: u64,
    }

    #[derive(Default)]
    struct Store {
        map: HashMap<u64, Entry>,
        bytes: usize,
        tick: u64,
        /// Anchor images held by the entries: address -> (entries holding it, bytes).
        anchor_refs: HashMap<usize, (usize, usize)>,
    }

    impl Store {
        fn add(&mut self, e: &Entry) {
            self.bytes += own_size(e);
            for (_, a) in &e.anchors {
                let r = self.anchor_refs.entry(Arc::as_ptr(a) as usize).or_insert((0, a.data.len() * 4));
                if r.0 == 0 {
                    self.bytes += r.1;
                }
                r.0 += 1;
            }
        }

        fn remove(&mut self, e: &Entry) {
            self.bytes -= own_size(e);
            for (_, a) in &e.anchors {
                let k = Arc::as_ptr(a) as usize;
                if let Some(r) = self.anchor_refs.get_mut(&k) {
                    r.0 -= 1;
                    if r.0 == 0 {
                        self.bytes -= r.1;
                        self.anchor_refs.remove(&k);
                    }
                }
            }
        }
    }

    fn store() -> &'static Mutex<Store> {
        static S: OnceLock<Mutex<Store>> = OnceLock::new();
        S.get_or_init(Default::default)
    }

    pub type Hit = (Arc<Image>, Vec<String>, Vec<String>, Anchors, Option<Arc<Vec<f32>>>);

    pub fn get(key: u64) -> Option<Hit> {
        let mut s = store().lock().ok()?;
        s.tick += 1;
        let t = s.tick;
        let e = s.map.get_mut(&key)?;
        e.tick = t;
        Some((e.image.clone(), e.notes.clone(), e.approx.clone(), e.anchors.clone(), e.alpha.clone()))
    }

    /// Bytes of an entry without its anchors (those are counted once in the store).
    fn own_size(e: &Entry) -> usize {
        e.image.data.len() * 4 + e.alpha.as_ref().map_or(0, |a| a.len() * 4)
    }

    pub fn put(
        key: u64,
        image: Arc<Image>,
        notes: Vec<String>,
        approx: Vec<String>,
        anchors: Anchors,
        alpha: Option<Arc<Vec<f32>>>,
    ) {
        // pictures past 2048 x 2048 are not kept: a 4K one (and its 4K anchors) evicted the 1024 checkpoints the
        // next edit resumes from (SHARD_CKPT_MAX_PX raises the cap)
        static MAX_PX: OnceLock<usize> = OnceLock::new();
        let max_px = *MAX_PX.get_or_init(|| std::env::var("SHARD_CKPT_MAX_PX").ok().and_then(|v| v.parse().ok()).unwrap_or(2048 * 2048));
        if image.data.len() * 4 > budget() / 3 || (image.w as usize) * (image.h as usize) > max_px {
            return;
        }
        let Ok(mut s) = store().lock() else { return };
        s.tick += 1;
        let tick = s.tick;
        let entry = Entry { image, notes, approx, anchors, alpha, tick };
        s.add(&entry);
        if let Some(old) = s.map.insert(key, entry) {
            s.remove(&old);
        }
        while s.bytes > budget() {
            let Some((&oldest, _)) = s.map.iter().min_by_key(|(_, e)| e.tick) else { break };
            if let Some(e) = s.map.remove(&oldest) {
                s.remove(&e);
            }
        }
    }

    pub fn clear() {
        if let Ok(mut s) = store().lock() {
            s.map.clear();
            s.bytes = 0;
        }
    }
}

/// Forget the checkpoints kept between compositions.
pub fn clear_checkpoints() {
    checkpoints::clear();
}

/// Hash of every layer of the tree by address, or `None` when checkpoints do not apply to this stack (no
/// identity for the mesh maps, or `SHARD_NO_CHECKPOINT`).
fn layer_hash_map(layers: &[Layer], opts: &Options) -> Option<std::collections::HashMap<usize, u64>> {
    if std::env::var_os("SHARD_NO_CHECKPOINT").is_some()
        || opts.maps.cache_token().is_none()
        || opts.strokes.is_some_and(|s| s.cache_token().is_none())
    {
        return None;
    }
    fn walk(ls: &[Layer], out: &mut std::collections::HashMap<usize, u64>) {
        for l in ls {
            // the hash covers the layer and everything below it (children, content, mask)
            out.insert(l as *const Layer as usize, debug_hash(l));
            walk(&l.children, out);
        }
    }
    let mut map = std::collections::HashMap::new();
    walk(layers, &mut map);
    Some(map)
}

/// Key of the empty accumulator of channel `bit`: everything the result depends on besides the layers.
fn top_level_base(ctx: &Ctx, bit: u32, acc: &Image) -> Option<(u64, bool)> {
    use std::hash::{Hash, Hasher};
    ctx.ck?;
    let maps = ctx.opts.maps.cache_token()?;
    let strokes = ctx.opts.strokes.and_then(|s| s.cache_token()).unwrap_or(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (0xC4EC_0002u32, maps, strokes, ctx.w, ctx.h, bit, ctx.opts.allow_approx, ctx.opts.skip_unsupported, acc.n).hash(&mut h);
    // what changes the result besides the layers: the working space, the diagnostic that hides layers by name
    (ctx.opts.aces_working, std::env::var("SHARD_HIDE_LAYER").unwrap_or_default()).hash(&mut h);
    Some((h.finish(), true))
}

/// The children of a group continue from the accumulator at the group (either straight into it or into a copy
/// that is blended afterwards): their chain starts from the key of the accumulator before the group.
fn group_children_base(ctx: &Ctx, from_default: bool) -> Option<(u64, bool)> {
    use std::hash::{Hash, Hasher};
    ctx.ck?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (ctx.ck_cur, 0x6772_6F75u32 + from_default as u32).hash(&mut h);
    Some((h.finish(), false))
}

/// A list of layers on `acc`, resuming from the highest checkpoint whose layers are all unchanged and leaving
/// new ones behind.
fn apply_layers_chained(layers: &[Layer], bit: u32, acc: &mut Image, ctx: &mut Ctx, base: u64, top: bool) {
    use std::hash::{Hash, Hasher};
    let info = ctx.ck.unwrap();
    let order: Vec<&Layer> = layers.iter().rev().collect();
    let mut chain = Vec::with_capacity(order.len() + 1);
    chain.push(base);
    for l in &order {
        let lh = info.get(&(*l as *const Layer as usize)).copied().unwrap_or_else(|| debug_hash(*l));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (chain[chain.len() - 1], lh).hash(&mut h);
        chain.push(h.finish());
    }
    let (notes0, approx0, log0) = (ctx.notes.len(), ctx.approx.len(), ctx.anchor_log.len());
    let mut start = 0;
    for i in (1..chain.len()).rev() {
        if let Some((img, n, a, anchors, alpha)) = checkpoints::get(chain[i]) {
            if img.w == acc.w && img.h == acc.h && img.n == acc.n && alpha.is_some() == ctx.alpha.is_some() {
                *acc = (*img).clone();
                if let Some(al) = alpha {
                    ctx.alpha = Some((*al).clone());
                }
                ctx.notes.extend(n);
                ctx.approx.extend(a);
                // the anchor points the skipped layers wrote
                for (key, img) in anchors {
                    set_anchor(ctx, key, img);
                }
                start = i;
                break;
            }
        }
    }
    let timing = std::env::var_os("SHARD_TRACE_TIMING").is_some();
    if timing && (top || start > 0) {
        eprintln!("[time] bit {bit:2} {} resumes after {start} of {} layers", if top { "stack" } else { "group" }, order.len());
    }
    // SHARD_CKPT_MS: work (ms) between two intermediate checkpoints of a 4 MB picture (bigger ones: longer)
    let unit: f64 = std::env::var("SHARD_CKPT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(25.0);
    let gap = unit * (acc.data.len() as f64 * 4.0 / (4.0 * 1024.0 * 1024.0)).max(1.0);
    let mut last_store = std::time::Instant::now();
    for i in start..order.len() {
        ctx.ck_cur = chain[i];
        apply_layer(order[i], bit, acc, ctx);
        probe_layer(order[i], bit, acc);
        let last = i + 1 == order.len();
        // the finished stack, and the pictures where about `gap` ms of work piled up since the previous one
        if (last && top) || last_store.elapsed().as_secs_f64() * 1000.0 >= gap {
            // the last value of every anchor point this level wrote so far
            let mut seen = std::collections::HashSet::new();
            let anchors: checkpoints::Anchors = ctx.anchor_log[log0..]
                .iter()
                .rev()
                .filter(|k| seen.insert(**k))
                .filter_map(|k| ctx.anchors.get(k).map(|a| (*k, a.clone())))
                .collect();
            checkpoints::put(
                chain[i + 1],
                std::sync::Arc::new(acc.clone()),
                ctx.notes[notes0..].to_vec(),
                ctx.approx[approx0..].to_vec(),
                anchors,
                ctx.alpha.as_ref().map(|a| std::sync::Arc::new(a.clone())),
            );
            last_store = std::time::Instant::now();
        }
    }
}

/// Layers are stored top-first: apply bottom (last) to top (first).
/// Diagnostic, SHARD_PROBE_LAYERS="u,v": the value of the stack at that texel (fraction of the texture, y down) after
/// every layer, base colour only -- which layer puts a mark somewhere. (A group prints its own children first, on the
/// picture of the group.)
/// A bitmap as a square grey stencil (alpha times luminance), stretched over the stencil's unit square.
fn stencil_square(b: &Image) -> Image {
    let side = b.w.max(b.h).clamp(16, 1024);
    let mut data = Vec::with_capacity((side * side) as usize);
    for y in 0..side {
        for x in 0..side {
            let (u, v) = ((x as f32 + 0.5) / side as f32, (y as f32 + 0.5) / side as f32);
            let c = |k: usize| sample(b, u, v, k.min(b.n - 1), b.n);
            let lum = if b.n >= 3 { 0.2126 * c(0) + 0.7152 * c(1) + 0.0722 * c(2) } else { c(0) };
            let alpha = if b.n == 4 || b.n == 2 { c(b.n - 1) } else { 1.0 };
            data.push((lum * alpha).clamp(0.0, 1.0));
        }
    }
    Image { w: side, h: side, n: 1, data }
}

/// Coverage (0 / 1 per texel) of a non-repeating planar projector; `None` when the surface is unknown (the
/// fill then covers everything, as before).
fn planar_footprint(f: &shard_model::Fill, ctx: &Ctx) -> Option<Vec<f32>> {
    let spec = procedural::ProjSpec::of(f);
    let prep = procedural::ProjPrep::new(&spec)?;
    let s = ctx.opts.maps.surface(ctx.w, ctx.h)?;
    let fp: Vec<f32> = (0..(ctx.w * ctx.h) as usize).map(|i| if s.covered[i] && prep.inside(&s, i) { 1.0 } else { 0.0 }).collect();
    if std::env::var_os("SHARD_TRACE_ANCHOR").is_some() {
        eprintln!("[planar] footprint {:.4} of the texture", fp.iter().sum::<f32>() / fp.len().max(1) as f32);
    }
    Some(fp)
}

fn probe_layer(l: &Layer, bit: u32, acc: &Image) {
    static P: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
    let p = P.get_or_init(|| {
        let v = std::env::var("SHARD_PROBE_LAYERS").ok()?;
        let c: Vec<f32> = v.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        (c.len() == 2).then(|| (c[0], c[1]))
    });
    let Some((u, v)) = *p else { return };
    // SHARD_PROBE_BIT=n: probe that channel instead of the base colour (1 = height)
    static PB: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let want = *PB.get_or_init(|| std::env::var("SHARD_PROBE_BIT").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
    if bit != want || acc.w == 0 {
        return;
    }
    if want != 0 {
        let (x, y) = (((u * acc.w as f32) as u32).min(acc.w - 1), ((v * acc.h as f32) as u32).min(acc.h - 1));
        let i = ((y * acc.w + x) as usize) * acc.n;
        let maxabs = acc.data.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let mean = acc.data.iter().sum::<f32>() / acc.data.len().max(1) as f32;
        eprintln!("[probe-layer] bit {want} {}x{} {:<28} -> {:.4}  (mean {mean:.4}, max |x| {maxabs:.4})", acc.w, acc.h, l.name, acc.data[i]);
        return;
    }
    let (x, y) = (((u * acc.w as f32) as u32).min(acc.w - 1), ((v * acc.h as f32) as u32).min(acc.h - 1));
    let i = ((y * acc.w + x) as usize) * acc.n;
    let s = |c: usize| (linear_to_srgb(acc.data[i + c.min(acc.n - 1)].clamp(0.0, 1.0)) * 255.0).round();
    eprintln!("[probe-layer] {}x{} {:<28} -> ({:.0}, {:.0}, {:.0})", acc.w, acc.h, l.name, s(0), s(1), s(2));
}

fn apply_layers(layers: &[Layer], bit: u32, acc: &mut Image, ctx: &mut Ctx) {
    if let Some((base, top)) = ctx.ck_next.take() {
        if ctx.ck.is_some() && !layers.is_empty() {
            return apply_layers_chained(layers, bit, acc, ctx, base, top);
        }
    }
    let timing = std::env::var_os("SHARD_TRACE_TIMING").is_some();
    for l in layers.iter().rev() {
        let t0 = std::time::Instant::now();
        apply_layer(l, bit, acc, ctx);
        probe_layer(l, bit, acc);
        if timing {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if ms >= 20.0 {
                eprintln!("[time] bit {bit:2} {:>8.0} ms  {}{}", ms, if l.kind == LayerKind::Group { "group " } else { "" }, l.name);
            }
        }
    }
}

/// Per-pixel scalar (mask) or constant.
#[derive(Clone)]
enum Scalar {
    Const(f32),
    Map(Vec<f32>),
}

impl Scalar {
    fn at(&self, i: usize) -> f32 {
        match self {
            Scalar::Const(v) => *v,
            Scalar::Map(m) => m[i],
        }
    }
}

/// Blend the content of a fill effect (`new`) over the content built by the effects below it (`prev`), with
/// the effect's own blend mode and opacity for this channel, like the effect stack of a Painter layer.
fn stack_effects(prev: (Content, f32), new: (Content, f32), e: &Effect, bit: u32, n: usize) -> (Content, f32) {
    let (mode, eop) = blend_for(&e.blend, bit, false);
    let plain = mode == BlendMode::NORMAL || mode.name() == "Replace";
    if plain && eop >= 1.0 {
        return new;
    }
    let op = (eop * new.1).clamp(0.0, 1.0);
    let (pop, content) = (prev.1, (prev.0, new.0));
    let mix = |b: f32, s: f32| -> f32 {
        let r = if plain { s } else { blend(mode, b, s) };
        b + (r - b) * op
    };
    let out = match content {
        (Content::Const(b), Content::Const(s)) => {
            Content::Const(b.iter().enumerate().map(|(c, bv)| mix(*bv, s[c.min(s.len() - 1)])).collect())
        }
        (b, s) => {
            let get = |c: &Content, w: usize, h: usize| -> Image {
                match c {
                    Content::Map(i) => i.clone(),
                    Content::Const(v) => {
                        let mut d = Vec::with_capacity(w * h * v.len());
                        for _ in 0..w * h {
                            d.extend_from_slice(v);
                        }
                        Image { w: w as u32, h: h as u32, n: v.len(), data: d }
                    }
                }
            };
            let (w, h) = match (&b, &s) {
                (Content::Map(i), _) | (_, Content::Map(i)) => (i.w as usize, i.h as usize),
                _ => (1, 1),
            };
            let (bi, si) = (get(&b, w, h), get(&s, w, h));
            let nn = n.max(1);
            let mut data = Vec::with_capacity(w * h * nn);
            for i in 0..w * h {
                for c in 0..nn {
                    let bv = bi.data[i * bi.n + c.min(bi.n - 1)];
                    let sv = si.data[i * si.n + c.min(si.n - 1)];
                    data.push(mix(bv, sv));
                }
            }
            Content::Map(Image { w: w as u32, h: h as u32, n: nn, data })
        }
    };
    (out, pop)
}

/// Source pixels for a layer: one constant value per component, or a full image.
enum Content {
    Const(Vec<f32>),
    Map(Image),
}

/// Bilinear sample of `img` at normalised `(u, v)` (v = 0 at the top row), wrapping, component `c`
/// of `n` requested components (1-component bitmaps replicate; RGB(A) -> 1 takes the mean).
pub(crate) fn sample(img: &Image, u: f32, v: f32, c: usize, n: usize) -> f32 {
    let fx = u.rem_euclid(1.0) * img.w as f32 - 0.5;
    let fy = v.rem_euclid(1.0) * img.h as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let wrap = |v: f32, m: u32| (v as i32).rem_euclid(m as i32) as u32;
    let (xa, xb) = (wrap(x0, img.w), wrap(x0 + 1.0, img.w));
    let (ya, yb) = (wrap(y0, img.h), wrap(y0 + 1.0, img.h));
    let comp = |x: u32, y: u32| -> f32 {
        let p = img.pixel(x, y);
        if n == 1 {
            if img.n >= 3 {
                (p[0] + p[1] + p[2]) / 3.0
            } else {
                p[0]
            }
        } else if img.n == 1 {
            p[0]
        } else {
            p[c.min(img.n - 1)]
        }
    };
    let top = comp(xa, ya) * (1.0 - tx) + comp(xb, ya) * tx;
    let bot = comp(xa, yb) * (1.0 - tx) + comp(xb, yb) * tx;
    top * (1.0 - ty) + bot * ty
}

/// Bitmap resampled to the compose resolution through the fill's UV transform (UV projection only).
fn bitmap_map(f: &Fill, resource: &str, n: usize, name: &str, ctx: &mut Ctx) -> Option<Image> {
    if f.projection != Projection::Uv {
        ctx.notes.push(format!("bitmap with {:?} projection on '{name}'", f.projection));
        return None;
    }
    let Some(img) = ctx.opts.bitmaps.bitmap(resource) else {
        ctx.notes.push(format!("bitmap resource not available on '{name}'"));
        return None;
    };
    let xf = procedural::UvXform::new(&f.uv);
    let (w, h) = (ctx.w, ctx.h);
    let mut data = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let (su, sv) = xf.map(u, v);
            for c in 0..n {
                data.push(sample(&img, su, sv, c, n));
            }
        }
    }
    let mut out = Image { w, h, n, data };
    // a bitmap filling a colour channel whose values are sRGB-encoded (see `colour_encoded`) is decoded to linear
    // (SHARD_BITMAP_COLOUR_SRGB=all|none forces it for every bitmap / none)
    let decode = match std::env::var("SHARD_BITMAP_COLOUR_SRGB").ok().as_deref() {
        Some("all") => true,
        Some("none") => false,
        _ => ctx.opts.bitmaps.colour_encoded(resource),
    };
    if n >= 3 && decode {
        for v in out.data.iter_mut() {
            *v = srgb_to_linear(v.clamp(0.0, 1.0));
        }
    }
    if ctx.opts.aces_working {
        convert_colour(&mut out, &SRGB_TO_ACESCG);
    }
    Some(out)
}

/// Blend `src` over `dst` with `mode`, scaled by `op`.
fn combine(dst: Scalar, src: &Scalar, mode: BlendMode, op: f32, npx: usize) -> Scalar {
    if let (Scalar::Const(d), Scalar::Const(s)) = (&dst, src) {
        return Scalar::Const(d + (blend(mode, *d, *s) - d) * op);
    }
    let mut out = vec![0.0f32; npx];
    // rows of 1024 when the image is a multiple of that (one serial row otherwise)
    let width = if npx % 1024 == 0 { 1024 } else { npx.max(1) };
    procedural::par_rows(&mut out, width, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * width + x;
            let d = dst.at(i);
            *o = d + (blend(mode, d, src.at(i)) - d) * op;
        }
    });
    Scalar::Map(out)
}

/// Evaluated masks kept between compositions. Editing a colour or a slider leaves every mask as it was, and
/// evaluating them (generators, strokes, clicks, blurs) is most of what a recomposition costs. An entry is
/// found by a hash of the mask definition and of everything its evaluation reads (see [`persist_key`]);
/// values are kept as 16-bit levels.
mod mask_store {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// Budget in bytes of mask data.
    const BUDGET: usize = 384 << 20;

    pub enum Value {
        Const(f32),
        Map(std::sync::Arc<Vec<u16>>),
    }

    pub struct Entry {
        pub value: Value,
        pub notes: Vec<String>,
        pub approx: Vec<String>,
        pub unreliable: bool,
        /// Anchor points the mask writes (uid, values 0..1 in 16 bits).
        pub anchors: Vec<(u64, std::sync::Arc<Vec<u16>>)>,
        tick: u64,
    }

    #[derive(Default)]
    struct Store {
        map: HashMap<u64, Entry>,
        bytes: usize,
        tick: u64,
    }

    fn store() -> &'static Mutex<Store> {
        static S: OnceLock<Mutex<Store>> = OnceLock::new();
        S.get_or_init(Default::default)
    }

    fn size(v: &Value) -> usize {
        match v {
            Value::Const(_) => 8,
            Value::Map(m) => m.len() * 2,
        }
    }

    fn entry_size(e: &Entry) -> usize {
        size(&e.value) + e.anchors.iter().map(|(_, a)| a.len() * 2).sum::<usize>()
    }

    pub type Hit = (Value, Vec<String>, Vec<String>, bool, Vec<(u64, std::sync::Arc<Vec<u16>>)>);

    pub fn get(key: u64) -> Option<Hit> {
        let mut s = store().lock().ok()?;
        s.tick += 1;
        let t = s.tick;
        let e = s.map.get_mut(&key)?;
        e.tick = t;
        let v = match &e.value {
            Value::Const(c) => Value::Const(*c),
            Value::Map(m) => Value::Map(m.clone()),
        };
        Some((v, e.notes.clone(), e.approx.clone(), e.unreliable, e.anchors.clone()))
    }

    pub fn put(
        key: u64,
        value: Value,
        notes: Vec<String>,
        approx: Vec<String>,
        unreliable: bool,
        anchors: Vec<(u64, std::sync::Arc<Vec<u16>>)>,
    ) {
        let Ok(mut s) = store().lock() else { return };
        s.tick += 1;
        let tick = s.tick;
        let entry = Entry { value, notes, approx, unreliable, anchors, tick };
        let add = entry_size(&entry);
        if let Some(old) = s.map.insert(key, entry) {
            s.bytes -= entry_size(&old);
        }
        s.bytes += add;
        while s.bytes > BUDGET {
            let Some((&oldest, _)) = s.map.iter().min_by_key(|(_, e)| e.tick) else { break };
            if let Some(e) = s.map.remove(&oldest) {
                s.bytes -= entry_size(&e);
            }
        }
    }

    /// Drop everything (a project was closed, or after a settings change that alters results).
    pub fn clear() {
        if let Ok(mut s) = store().lock() {
            s.map.clear();
            s.bytes = 0;
        }
    }
}

/// Forget the masks kept between compositions.
pub fn clear_mask_cache() {
    mask_store::clear();
}

/// Hash of a value's `Debug` text without building the string.
fn debug_hash<T: std::fmt::Debug>(v: &T) -> u64 {
    use std::fmt::Write;
    use std::hash::Hasher;
    struct HashWriter(std::collections::hash_map::DefaultHasher);
    impl Write for HashWriter {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.write(s.as_bytes());
            Ok(())
        }
    }
    let mut w = HashWriter(std::collections::hash_map::DefaultHasher::new());
    let _ = write!(w, "{v:?}");
    w.0.finish()
}

/// Anchor points a mask writes and anchor points its references read, in effect order.
fn mask_context(es: &[Effect], writes: &mut Vec<u64>, reads: &mut Vec<u64>) {
    for e in es {
        match &e.kind {
            EffectKind::Anchor { .. } if e.uid != 0 => writes.push(e.uid),
            EffectKind::Fill(f) => {
                for s in &f.sources {
                    if let Source::Reference { target, .. } = s {
                        reads.push(*target);
                    }
                }
            }
            EffectKind::Group { children } => mask_context(children, writes, reads),
            _ => {}
        }
    }
}

/// Hash of every value of an image (anchor points feed the masks that reference them).
fn image_fingerprint(img: &Image) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ ((img.w as u64) << 32 | img.h as u64) ^ img.n as u64;
    for v in &img.data {
        h = (h ^ v.to_bits() as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Key under which the mask of `l` is kept between compositions, with the anchor points it writes; `None` when
/// it cannot be kept (a provider without identity cannot be matched). A mask that references anchor points is
/// keyed by their values too, so it follows the layers that feed them.
fn persist_key(l: &Layer, ctx: &Ctx) -> Option<(u64, Vec<u64>)> {
    use std::hash::{Hash, Hasher};
    let m = l.mask.as_ref().filter(|m| m.enabled && !m.effects.is_empty())?;
    let maps = ctx.opts.maps.cache_token()?;
    let strokes = match ctx.opts.strokes {
        Some(s) => s.cache_token()?,
        None => 0,
    };
    let (mut writes, mut reads) = (Vec::new(), Vec::new());
    mask_context(&m.effects, &mut writes, &mut reads);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (maps, strokes, ctx.w, ctx.h, ctx.opts.allow_approx, ctx.opts.skip_unsupported, ctx.replay_group_decals).hash(&mut h);
    (l.kind == LayerKind::Group).hash(&mut h);
    l.name.hash(&mut h);
    debug_hash(m).hash(&mut h);
    for t in &reads {
        // the lookup the evaluation itself does: the mask-stack anchor, else the content anchor of colour
        let found = ctx.anchors.get(&(*t, MASK_ANCHOR)).or_else(|| ctx.anchors.get(&(*t, 0)));
        (*t, found.map(|a| image_fingerprint(a))).hash(&mut h);
    }
    Some((h.finish(), writes))
}

fn scalar_from_store(v: mask_store::Value) -> Scalar {
    match v {
        mask_store::Value::Const(c) => Scalar::Const(c),
        mask_store::Value::Map(m) => Scalar::Map(m.iter().map(|q| *q as f32 / 65535.0).collect()),
    }
}

fn scalar_to_store(s: &Scalar) -> mask_store::Value {
    match s {
        Scalar::Const(c) => mask_store::Value::Const(*c),
        Scalar::Map(m) => mask_store::Value::Map(std::sync::Arc::new(
            m.iter().map(|x| (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect(),
        )),
    }
}

/// Mask of a layer and whether it could be evaluated faithfully.
fn mask_value(l: &Layer, ctx: &mut Ctx) -> (Scalar, bool) {
    // the replayed variant of a group mask is cached apart from the plain one (addresses are even)
    let key = l as *const Layer as usize + ctx.replay_group_decals as usize;
    if let Some((s, n, a, u)) = ctx.masks.get(&key) {
        let (s, n, a, u) = (s.clone(), n.clone(), a.clone(), *u);
        ctx.notes.extend(n);
        ctx.approx.extend(a);
        return (s, u);
    }
    let persist = if std::env::var_os("SHARD_NO_MASK_CACHE").is_some() { None } else { persist_key(l, ctx) };
    if let Some((pk, _)) = &persist {
        if let Some((value, n, a, u, anchors)) = mask_store::get(*pk) {
            let s = scalar_from_store(value);
            // the anchor points the mask writes are published as its evaluation would have done
            for (uid, vals) in anchors {
                let data: Vec<f32> = vals.iter().map(|q| *q as f32 / 65535.0).collect();
                let img = Arc::new(Image { w: ctx.w, h: ctx.h, n: 1, data });
                set_anchor(ctx, (uid, MASK_ANCHOR), img);
            }
            ctx.notes.extend(n.iter().cloned());
            ctx.approx.extend(a.iter().cloned());
            ctx.masks.insert(key, (s.clone(), n, a, u));
            return (s, u);
        }
    }
    let (n0, a0) = (ctx.notes.len(), ctx.approx.len());
    let outer = std::mem::replace(&mut ctx.unreliable, false);
    let v = mask_value_uncached(l, ctx);
    let unreliable = std::mem::replace(&mut ctx.unreliable, outer);
    let (n, a) = (ctx.notes[n0..].to_vec(), ctx.approx[a0..].to_vec());
    if let Some((pk, writes)) = persist {
        let anchors: Vec<(u64, std::sync::Arc<Vec<u16>>)> = writes
            .iter()
            .filter_map(|uid| {
                let img = ctx.anchors.get(&(*uid, MASK_ANCHOR))?;
                let q = img.data.iter().map(|x| (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect();
                Some((*uid, std::sync::Arc::new(q)))
            })
            .collect();
        mask_store::put(pk, scalar_to_store(&v), n.clone(), a.clone(), unreliable, anchors);
    }
    ctx.masks.insert(key, (v.clone(), n, a, unreliable));
    (v, unreliable)
}

fn mask_value_uncached(l: &Layer, ctx: &mut Ctx) -> Scalar {
    let Some(m) = &l.mask else { return Scalar::Const(1.0) };
    if !m.enabled {
        return Scalar::Const(1.0);
    }
    let npx = (ctx.w * ctx.h) as usize;
    let base = if m.initial != 0 { 1.0 } else { 0.0 };
    mask_effects(&m.effects, Scalar::Const(base), l, ctx, npx)
}

/// Diagnostic (`SHARD_TRACE_TIMING`): prints how long one mask effect took when it is dropped.
struct EffectTimer {
    start: std::time::Instant,
    name: String,
}

impl Drop for EffectTimer {
    fn drop(&mut self) {
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        if ms >= 4.0 {
            eprintln!("[time]       mask effect {:>7.1} ms  {}", ms, self.name);
        }
    }
}

/// Evaluate mask effects in order on top of `v`; groups are evaluated in place (their children in order).
fn mask_effects(effects: &[Effect], mut v: Scalar, l: &Layer, ctx: &mut Ctx, npx: usize) -> Scalar {
    let timing = std::env::var_os("SHARD_TRACE_TIMING").is_some();
    for e in effects {
        if !e.visible {
            continue;
        }
        let t_eff = std::time::Instant::now();
        let _guard = timing.then(|| EffectTimer { start: t_eff, name: format!("{} / {}", l.name, effect_name(&e.kind)) });
        let (mode, eop) = e.blend.first().map(|b| (b.mode, b.opacity)).unwrap_or((BlendMode::NORMAL, 1.0));
        match &e.kind {
            EffectKind::Fill(f) => {
                // the last source of the fill wins, like the content of a fill layer
                let mut src: Option<(Scalar, f32)> = None;
                for s in &f.sources {
                    match s {
                        Source::Uniform { color: Some(c), opacity, .. } => {
                            // experiment SHARD_MASK_UNIFORM_SRGB=1: the stored value read as linear and encoded
                            let v = if std::env::var_os("SHARD_MASK_UNIFORM_SRGB").is_some() { linear_to_srgb(c[0]) } else { c[0] };
                            src = Some((Scalar::Const(v), *opacity))
                        }
                        Source::Bitmap { resource: Some(r), opacity, .. } => {
                            if let Some(img) = bitmap_map(f, r, 1, &l.name, ctx) {
                                src = Some((Scalar::Map(img.data), *opacity));
                            }
                        }
                        Source::Reference { target, opacity, levels: ref_levels, .. } => {
                            let found = ctx
                                .anchors
                                .get(&(*target, MASK_ANCHOR))
                                .or_else(|| ctx.anchors.get(&(*target, 0)))
                                .cloned();
                            if std::env::var_os("SHARD_TRACE_ANCHOR").is_some() {
                                eprintln!(
                                    "[anchor] '{}' reads {target}: {}",
                                    l.name,
                                    match &found {
                                        Some(i) => format!("mean {:.3}", i.data.iter().sum::<f32>() / i.data.len().max(1) as f32),
                                        None => "missing".into(),
                                    }
                                );
                            }
                            match found {
                                Some(img) => {
                                    let lv = ref_levels.as_ref().filter(|_| std::env::var_os("SHARD_NO_REF_LEVELS").is_none());
                                    let vals: Vec<f32> = img
                                        .data
                                        .chunks_exact(img.n)
                                        .map(|p| p.iter().sum::<f32>() / img.n as f32)
                                        .map(|x| lv.map_or(x, |l| l.apply(x, 0)))
                                        .collect();
                                    src = Some((Scalar::Map(vals), *opacity));
                                }
                                None if *target == 0 && std::env::var("SHARD_REF_ZERO").is_ok() => {
                                    // a reference with no anchor chosen (experiment: white / black)
                                    let v = if std::env::var("SHARD_REF_ZERO").as_deref() == Ok("black") { 0.0 } else { 1.0 };
                                    src = Some((Scalar::Const(v), *opacity));
                                }
                                None => {
                                    ctx.unreliable = true;
                                    ctx.notes.push(format!("reference to an unknown anchor on '{}'", l.name));
                                }
                            }
                        }
                        Source::Procedural { procedural, opacity, .. } => {
                            let t0 = std::time::Instant::now();
                            let gen = if ctx.opts.allow_approx {
                                procedural::generate_scaled(procedural, ctx.w, ctx.h, ctx.opts.maps, &f.uv, &procedural::ProjSpec::of(f))
                            } else {
                                None
                            };
                            if std::env::var_os("SHARD_TRACE").is_some() {
                                eprintln!("[mask gen] {} {}x{}: {:?}", procedural.graph_name().unwrap_or("?"), ctx.w, ctx.h, t0.elapsed());
                            }
                            match gen {
                                Some(g) => {
                                    ctx.approx.push(g.approx);
                                    src = Some((Scalar::Map(g.values), *opacity));
                                }
                                None => {
                                    ctx.unreliable = true;
                                    ctx.notes.push(format!(
                                        "procedural {} on '{}'",
                                        procedural.graph_name().unwrap_or("?"),
                                        l.name
                                    ))
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // A planar projection that does not repeat covers only its projector's box: outside it the fill
                // leaves the mask alone (a test pistol's serial-number mask is a white planar fill the size of the
                // engraving; read as white everywhere it lit the whole metal group through a Lighten reference).
                let footprint = match (&src, f.projection, f.projector) {
                    (Some(_), Projection::Planar, Some(pr)) if pr.wrap == 0 && std::env::var_os("SHARD_PLANAR_EVERYWHERE").is_none() => {
                        planar_footprint(f, ctx)
                    }
                    _ => None,
                };
                match src {
                    Some((s, sop)) if footprint.is_some() => {
                        let fp = footprint.unwrap();
                        let before = v.clone();
                        let after = combine(v, &s, mode, (eop * sop).clamp(0.0, 1.0), npx);
                        v = Scalar::Map((0..npx).map(|i| before.at(i) + (after.at(i) - before.at(i)) * fp[i]).collect());
                    }
                    Some((s, sop)) => v = combine(v, &s, mode, (eop * sop).clamp(0.0, 1.0), npx),
                    None => {
                        ctx.unreliable = true;
                        if !f.sources.iter().any(|s| matches!(s, Source::Procedural { .. })) {
                            ctx.notes.push(format!("mask fill with non-uniform source on '{}'", l.name));
                        }
                    }
                }
            }
            EffectKind::Filter { procedural: Some(p) } => {
                let mut buf = match v {
                    Scalar::Const(c) => vec![c; npx],
                    Scalar::Map(m) => m,
                };
                // A filter effect blends its result over the mask with its own mode and opacity (a test pistol's
                // `Cutline` bevel is in Screen: read as a replacement, the bevel of a line under its threshold
                // erased the line). Replace, or Normal at full opacity, is the plain result.
                // SHARD_MASK_FILTER_REPLACE=1: the old reading.
                let blended = !(mode.name() == "Replace" || (mode == BlendMode::NORMAL && eop >= 1.0))
                    && std::env::var_os("SHARD_MASK_FILTER_REPLACE").is_none();
                let before = blended.then(|| buf.clone());
                let t0 = std::time::Instant::now();
                let mut done = if ctx.opts.allow_approx { procedural::filter(p, &mut buf, ctx.w, ctx.h) } else { None };
                // A filter without a native version (bevel_smooth, blur_slope...) runs in the Substance engine on the
                // mask, at the largest size it renders, resampled back (a test pistol's `Cutline` bevel was dropped and
                // the frame lost its raised parting line). SHARD_NO_MASK_ORACLE=1 keeps ignoring them.
                if done.is_none()
                    && ctx.opts.allow_approx
                    && std::env::var_os("SHARD_NO_ORACLE").is_none()
                    && std::env::var_os("SHARD_NO_MASK_ORACLE").is_none()
                {
                    if let Some((rw, rh)) = procedural::render_size(p, ctx.w, ctx.h) {
                        let (w, h) = (ctx.w as usize, ctx.h as usize);
                        let input = if (rw, rh) == (ctx.w, ctx.h) {
                            buf.clone()
                        } else {
                            procedural::resample_map(&buf, w, h, rw as usize, rh as usize)
                        };
                        let fed = Image { w: rw, h: rh, n: 1, data: input };
                        if let Some(r) = ctx.opts.maps.render_graph(p, None, rw, rh, Some(&fed)) {
                            let grey: Vec<f32> = r.data.chunks_exact(r.n).map(|px| px[0]).collect();
                            buf = if (r.w, r.h) == (ctx.w, ctx.h) {
                                grey
                            } else {
                                procedural::resample_map(&grey, r.w as usize, r.h as usize, w, h)
                            };
                            done = Some(format!("mask filter {}: Substance engine at {rw}x{rh}", p.graph_name().unwrap_or("?")));
                        }
                    }
                }
                if std::env::var_os("SHARD_TRACE").is_some() {
                    eprintln!("[mask filter] {} {}x{}: {:?} ({})", p.graph_name().unwrap_or("?"), ctx.w, ctx.h, t0.elapsed(), done.as_deref().unwrap_or("not evaluated"));
                }
                let evaluated = done.is_some();
                match done {
                    Some(d) => ctx.approx.push(d),
                    None => {
                        ctx.unreliable = true;
                        let msg = format!("mask filter {} on '{}'", p.graph_name().unwrap_or("?"), l.name);
                        if ctx.opts.allow_approx {
                            ctx.approx.push(format!("ignored {msg}"));
                        } else {
                            ctx.notes.push(msg);
                        }
                    }
                }
                v = match before {
                    Some(b) if evaluated => combine(Scalar::Map(b), &Scalar::Map(buf), mode, eop.clamp(0.0, 1.0), npx),
                    _ => Scalar::Map(buf),
                };
            }
            EffectKind::Group { children } => {
                // A group blended with a mode of its own is a canvas of its own: its content (the clicks and
                // strokes of a polygon-fill selection) works on the neutral value of the mode and the result is
                // blended over what is below. A `Multiply` group restricts a generator to what the clicks keep
                // and carves it where they are black; white clicks leave it as it is (Hull `Dirt 1`: the clicks
                // read as writes of 1 made it +9.5 base colour of error).
                let neutral = match mode.name() {
                    "Multiply" => Some(1.0),
                    "LinearDodge" | "Subtract" | "Screen" => Some(0.0),
                    _ => None,
                };
                match neutral {
                    Some(n) if !legacy_clicks() => {
                        let canvas = mask_effects(children, Scalar::Const(n), l, ctx, npx);
                        v = combine(v, &canvas, mode, eop.clamp(0.0, 1.0), npx);
                    }
                    _ => v = mask_effects(children, v, l, ctx, npx),
                }
            }
            EffectKind::Anchor { .. } => {
                if e.uid != 0 {
                    let data: Vec<f32> = match &v {
                        Scalar::Const(c) => vec![*c; npx],
                        Scalar::Map(m) => m.clone(),
                    };
                    if std::env::var_os("SHARD_TRACE_ANCHOR").is_some() {
                        let mean = data.iter().sum::<f32>() / data.len().max(1) as f32;
                        eprintln!("[anchor] mask anchor {} on '{}': mean {mean:.3}", e.uid, l.name);
                        // SHARD_TRACE_ANCHOR=DIR: also the anchor as DIR/anchor_<uid>.pgm
                        let dir = std::path::PathBuf::from(std::env::var_os("SHARD_TRACE_ANCHOR").unwrap());
                        if dir.is_dir() {
                            let mut b = format!("P5\n{} {}\n255\n", ctx.w, ctx.h).into_bytes();
                            b.extend(data.iter().map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8));
                            let _ = std::fs::write(dir.join(format!("anchor_{}.pgm", e.uid)), b);
                        }
                    }
                    let img = Arc::new(Image { w: ctx.w, h: ctx.h, n: 1, data });
                    set_anchor(ctx, (e.uid, MASK_ANCHOR), img);
                }
            }
            EffectKind::Levels(lv) => {
                v = match v {
                    Scalar::Const(c) => Scalar::Const(lv.apply(c, 0)),
                    Scalar::Map(mut m) => {
                        m.iter_mut().for_each(|x| *x = lv.apply(*x, 0));
                        Scalar::Map(m)
                    }
                };
            }
            EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) if a.strokes.is_empty() => {}
            EffectKind::Paint(a) | EffectKind::Eraser(a) if ctx.opts.allow_approx && ctx.opts.strokes.is_some() => {
                if !a.strokes.is_empty() {
                    let raster = ctx.opts.strokes.unwrap();
                    let mut buf = match v {
                        Scalar::Const(c) => vec![c; npx],
                        Scalar::Map(m) => m,
                    };
                    let value = a
                        .color
                        .iter()
                        .find_map(|s| match s {
                            Source::Uniform { color: Some(c), .. } => Some(c[0]),
                            _ => None,
                        })
                        .unwrap_or(1.0);
                    let revert = match e.kind {
                        EffectKind::Eraser(_) if std::env::var_os("SHARD_ERASER_BLACK").is_some() => Some(0.0),
                        EffectKind::Eraser(_) => {
                            Some(l.mask.as_ref().map(|m| if m.initial != 0 { 1.0 } else { 0.0 }).unwrap_or(0.0))
                        }
                        _ => None,
                    };
                    if std::env::var_os("SHARD_TRACE_STROKES").is_some() {
                        eprintln!("[paint] layer '{}' mask action: {} strokes, value {:.2}, opacity {:.2}", l.name, a.strokes.len(), value, a.opacity);
                    }
                    let stencil_img = match &a.stencil {
                        Some(Stencil::Procedural(p)) => ctx.opts.maps.render_graph(p, None, 512, 512, None),
                        Some(Stencil::Bitmap { resource: Some(r) }) => ctx.opts.bitmaps.bitmap(r).map(|b| Arc::new(stencil_square(&b))),
                        _ => None,
                    };
                    // the alpha of a textured brush (its graph rendered once; the plain round brush is analytic)
                    let shape_img = match a.brush.shape.as_ref() {
                        Some(p) if brush_shape_is_textured(p) && std::env::var_os("SHARD_BRUSH_ANALYTIC").is_none() => {
                            ctx.opts.maps.render_graph(p, None, BRUSH_SHAPE_SIDE, BRUSH_SHAPE_SIDE, None)
                        }
                        _ => None,
                    };
                    if std::env::var_os("SHARD_TRACE_STROKES").is_some() {
                        eprintln!(
                            "[paint] brush graph {:?} size {:.4} spacing {:.2} flow {:.2}: alpha image {}",
                            a.brush.shape.as_ref().and_then(|p| p.graph_name()),
                            a.brush.size,
                            a.brush.spacing,
                            a.brush.flow,
                            shape_img.as_ref().map(|i| format!("{}x{}x{}", i.w, i.h, i.n)).unwrap_or_else(|| "none".into())
                        );
                    }
                    raster.paint(a, &mut buf, ctx.w, ctx.h, value, revert, stencil_img.as_deref(), shape_img.as_deref());
                    ctx.approx.push(format!("{} painted strokes replayed on '{}'", a.strokes.len(), l.name));
                    v = Scalar::Map(buf);
                }
            }
            // SHARD_GROUP_DECALS=visible: the old behaviour, a group whose whole mask is polygon-fill clicks is
            // shown everywhere. By default the clicks are replayed (see `apply_layer` for the channels where a
            // passthrough group ignores them).
            EffectKind::Decal(_)
                if l.kind == LayerKind::Group
                    && (!ctx.replay_group_decals || std::env::var("SHARD_GROUP_DECALS").ok().as_deref() == Some("visible"))
                    && l.mask.as_ref().is_some_and(|m| !has_generators(&m.effects) && legacy_clicks()) =>
            {
                ctx.approx.push(format!("decal-only mask of group '{}' treated as fully visible", l.name));
                if matches!(&v, Scalar::Const(c) if *c == 0.0) {
                    v = Scalar::Const(1.0);
                }
            }
            // experiment: SHARD_NO_DECALS=1 skips every polygon-fill effect of masks
            EffectKind::Decal(_) if std::env::var_os("SHARD_NO_DECALS").is_some() => {}
            EffectKind::Decal(hits) if ctx.opts.allow_approx && ctx.opts.strokes.is_some() => {
                if !hits.is_empty() {
                    let raster = ctx.opts.strokes.unwrap();
                    let mut buf = match v {
                        Scalar::Const(c) => vec![c; npx],
                        Scalar::Map(m) => m,
                    };
                    // A group whose whole mask is selections starts fully shown: its clicks carve parts
                    // out (black) rather than build it up -- the frame of a test screen is never clicked
                    // yet shows the group in Painter.
                    // (a group whose clicks are all white is a selection: it starts black and shows only
                    // where clicked, like a test screen's polymer layer over its keys)
                    let mixed = hits.iter().any(|h| h.color[0] <= 0.5);
                    let value = hits[0].color[0];
                    // SHARD_DECAL_BLACK_HITS=ignore|<n>: diagnostic variants of the black clicks
                    let kept: Vec<shard_model::DecalHit>;
                    // Clicks at exactly (0.5, 0.5) are not selections the user made (a test screen's `Metal` group
                    // has 26 of them; ignoring them is what makes the screen and the keys match the export);
                    // SHARD_DECAL_BLACK_HITS=all replays them.
                    // A click at the exact centre of the view (0.5, 0.5) is not a selection of white: in the Painter
                    // export of an interior set the chunks those clicks reach are black whatever colour the click
                    // carries (corr 0.15 -> 0.67 on the `Vehicle_Interior` mask), so they act as an eraser.
                    // SHARD_CENTER_CLICKS=ignore|asis selects the old behaviours (a test screen's `Metal` group was
                    // tuned with the centre clicks left out).
                    let centred = |h: &shard_model::DecalHit| {
                        (legacy_clicks() || h.rect.is_none())
                            && (h.position[0] - 0.5).abs() < 1e-3
                            && (h.position[1] - 0.5).abs() < 1e-3
                    };
                    let erased: Vec<shard_model::DecalHit>;
                    let hits: &[shard_model::DecalHit] = match std::env::var("SHARD_CENTER_CLICKS").ok().as_deref() {
                        Some("asis") => hits,
                        Some("ignore") => {
                            erased = hits.iter().filter(|h| !centred(h)).cloned().collect();
                            &erased
                        }
                        _ => {
                            erased = hits
                                .iter()
                                .map(|h| {
                                    let mut h = h.clone();
                                    if centred(&h) {
                                        h.color = [0.0; 3];
                                    }
                                    h
                                })
                                .collect();
                            &erased
                        }
                    };
                    // SHARD_DECAL_BLACK_HITS=ignore|inview|nocenter|both|whiteonly_inview|all: diagnostic filters
                    let hits: &[shard_model::DecalHit] = match std::env::var("SHARD_DECAL_BLACK_HITS").ok().as_deref() {
                        Some("all") => hits,
                        Some("ignore") => {
                            kept = hits.iter().filter(|h| h.color[0] > 0.5).cloned().collect();
                            &kept
                        }
                        Some(mode @ ("inview" | "nocenter" | "both" | "whiteonly_inview")) => {
                            let inview = |h: &shard_model::DecalHit| (0.0..=1.0).contains(&h.position[0]) && (0.0..=1.0).contains(&h.position[1]);
                            let center = |h: &shard_model::DecalHit| (h.position[0] - 0.5).abs() < 1e-3 && (h.position[1] - 0.5).abs() < 1e-3;
                            kept = hits
                                .iter()
                                .filter(|h| match mode {
                                    "inview" => inview(h),
                                    "nocenter" => !center(h),
                                    "both" => inview(h) && !center(h),
                                    _ => inview(h) || h.color[0] > 0.5,
                                })
                                .cloned()
                                .collect();
                            &kept
                        }
                        _ => hits,
                    };
                    // a group of mixed clicks starts fully shown (see above)
                    if l.kind == LayerKind::Group
                        && mixed
                        && legacy_clicks()
                        && std::env::var_os("SHARD_GROUP_DECAL_BLACK").is_none()
                    {
                        let untouched = matches!(&buf[..], b if b.iter().all(|x| *x == 0.0));
                        if untouched {
                            buf.iter_mut().for_each(|x| *x = 1.0);
                        }
                    }
                    if raster.decal(hits, &mut buf, ctx.w, ctx.h, value) {
                        ctx.approx.push(format!("{} decal / polygon-fill hits replayed on '{}'", hits.len(), l.name));
                    } else {
                        ctx.unreliable = true;
                    }
                    v = Scalar::Map(buf);
                }
            }
            other => {
                // hand-painted strokes, decals, levels... are not evaluated; approximate mode ignores
                // the effect instead of dropping the whole layer
                ctx.unreliable = true;
                let msg = format!("mask effect {:?} on '{}'", effect_name(other), l.name);
                if ctx.opts.allow_approx {
                    ctx.approx.push(format!("ignored {msg}"));
                } else {
                    ctx.notes.push(msg);
                }
            }
        }
        // SHARD_TRACE_MASK=<layer name>: the mask of that layer after every effect (mean, max)
        if std::env::var("SHARD_TRACE_MASK").is_ok_and(|n| n == l.name) {
            let (mean, max) = match &v {
                Scalar::Const(c) => (*c, *c),
                Scalar::Map(m) => (m.iter().sum::<f32>() / m.len().max(1) as f32, m.iter().fold(f32::MIN, |a, b| a.max(*b))),
            };
            eprintln!("[mask trace] '{}' after {} {}: mean {mean:.4} max {max:.3}", l.name, effect_name(&e.kind), e.uid);
            // SHARD_TRACE_MASK_DIR=DIR: the mask after each effect as DIR/mask.pgm (the last one stays)
            if let Some(dir) = std::env::var_os("SHARD_TRACE_MASK_DIR") {
                let mut b = format!("P5\n{} {}\n255\n", ctx.w, ctx.h).into_bytes();
                b.extend((0..npx).map(|i| (v.at(i).clamp(0.0, 1.0) * 255.0).round() as u8));
                let _ = std::fs::write(std::path::Path::new(&dir).join("mask.pgm"), b);
            }
        }
    }
    v
}

/// Does a mask contain something that generates coverage on its own (fills, filters)? Decals,
/// strokes, levels, groups and anchors do not.
fn has_generators(es: &[Effect]) -> bool {
    es.iter().any(|e| match &e.kind {
        EffectKind::Fill(_) | EffectKind::Filter { .. } => true,
        EffectKind::Group { children } => has_generators(children),
        _ => false,
    })
}

/// The click masks of groups follow Painter: the mask starts at `maskInitial`, every click (a point or a
/// `DataDecalHitRect` marquee) selects its chunk (white) or erases it (black), on every channel the group blends.
/// A scope glass group whose mask was 12 clicks tinted the whole turret blue under the older approximations
/// (a mixed group started shown, its clicks were not replayed on the channels it blends by default, marquees
/// were read as clicks at the centre of the view); Test screen ladder, base colour of the whole set 2.0 -> 1.5.
/// `SHARD_GROUP_CLICKS_LEGACY=1` brings those approximations back.
fn legacy_clicks() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("SHARD_GROUP_CLICKS_LEGACY").is_some())
}

/// `(has white clicks, has black clicks)` of the polygon-fill effects in `es`.
fn decal_colors(es: &[Effect]) -> (bool, bool) {
    let mut r = (false, false);
    for e in es {
        match &e.kind {
            EffectKind::Decal(h) => {
                r.0 |= h.iter().any(|x| x.color[0] > 0.5);
                r.1 |= h.iter().any(|x| x.color[0] <= 0.5);
            }
            EffectKind::Group { children } => {
                let c = decal_colors(children);
                r = (r.0 || c.0, r.1 || c.1);
            }
            _ => {}
        }
    }
    r
}

fn effect_name(k: &EffectKind) -> &'static str {
    match k {
        EffectKind::Fill(_) => "fill",
        EffectKind::Paint(_) => "paint",
        EffectKind::Eraser(_) => "eraser",
        EffectKind::Smudge(_) => "smudge",
        EffectKind::Group { .. } => "group",
        EffectKind::Filter { .. } => "filter",
        EffectKind::Levels(_) => "levels",
        EffectKind::Anchor { .. } => "anchor",
        EffectKind::Decal(_) => "decal",
        EffectKind::Other(_) => "other",
    }
}

/// Channel `bit` of a material graph evaluated by the external renderer, through the fill's UV transform.
/// Colour channels come out sRGB-encoded and are linearised like the app's own colour pipeline.
fn rendered_content(
    p: &shard_model::Procedural,
    bit: u32,
    n: usize,
    ctx: &Ctx,
    uv: &shard_model::UvTransform,
    spec: &procedural::ProjSpec,
) -> Option<Image> {
    let trace = std::env::var_os("SHARD_TRACE_RENDER").is_some();
    let Some(output) = procedural_output_for(p, bit) else {
        if trace {
            eprintln!("[render] {}: no output for channel {bit}", p.graph_name().unwrap_or("?"));
        }
        return None;
    };
    let Some((rw, rh)) = procedural::render_size(p, ctx.w, ctx.h) else {
        if trace {
            eprintln!("[render] {}: size {:?} not renderable ({}x{})", p.graph_name().unwrap_or("?"), p.resolution, ctx.w, ctx.h);
        }
        return None;
    };
    let t_render = std::time::Instant::now();
    let Some(img) = ctx.opts.maps.render_graph(p, Some(output), rw, rh, None) else {
        if trace {
            eprintln!("[render] {}: render of '{output}' at {rw}x{rh} failed or unavailable", p.graph_name().unwrap_or("?"));
        }
        return None;
    };
    if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
        eprintln!("[time]     render_graph {} {:?}", p.graph_name().unwrap_or("?"), t_render.elapsed());
    }
    if trace {
        let vals: Vec<f32> = img.data.iter().step_by(img.n).copied().collect();
        let mean = vals.iter().sum::<f32>() / vals.len().max(1) as f32;
        let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        eprintln!(
            "[render] {} '{output}' bit {bit}: {}x{} mean {:.3} range {:.3}..{:.3}",
            p.graph_name().unwrap_or("?"),
            img.w,
            img.h,
            mean,
            lo,
            hi
        );
    }
    let xf = procedural::UvXform::new(uv);
    // Every graph output a colour channel takes is sRGB-encoded, the pattern ones too (`bnw_spots_2` overlaid on the
    // An interior set's `Dirt`: the darkening matches Painter's to 0.3 levels, base colour 2.4 -> 2.1 and another test project
    // 16.1 -> 15.9; the earlier "pattern outputs are linear" rule was measured while other faults hid this).
    // SHARD_PROC_SRGB_ALL=0: only outputs named like colour are decoded; SHARD_PROC_LINEAR=name,name|* forces linear.
    let colour_output = {
        let id = output.to_ascii_lowercase();
        ["basecolor", "base_color", "base color", "diffuse", "albedo", "color", "emissive", "emission"]
            .iter()
            .any(|k| id.contains(k))
    };
    let linear_graph = std::env::var("SHARD_PROC_LINEAR")
        .is_ok_and(|v| v.split(',').any(|n| n == "*" || Some(n) == p.graph_name()))
        || (!colour_output && std::env::var("SHARD_PROC_SRGB_ALL").is_ok_and(|v| v == "0"));
    let colour = matches!(bit, 0 | 4) && !linear_graph;
    let surface = if (spec.projector.is_some()
        && matches!(spec.kind, Projection::Planar | Projection::Triplanar)
        && std::env::var_os("SHARD_NO_PROJECTOR").is_none())
        || (spec.kind == Projection::Triplanar && std::env::var_os("SHARD_TRIPLANAR").is_some())
    {
        ctx.opts.maps.surface(ctx.w, ctx.h)
    } else {
        None
    };
    // a prepared projector turns every surface texel into its texture taps once; all components share them
    let prep = surface.as_ref().and_then(|_| procedural::ProjPrep::new(spec));
    // the UV lookups read the mip level matching their footprint (see `procedural::mip_for`)
    let uv_img = procedural::mip_for(img.clone(), &xf, ctx.w);
    let (w, h) = (ctx.w as usize, ctx.h as usize);
    let mut data = vec![0.0f32; w * h * n];
    procedural::par_rows(&mut data, w * n, |y, row| {
        for x in 0..w {
            let ti = y * w + x;
            let out = &mut row[x * n..(x + 1) * n];
            match &surface {
                Some(sf) if sf.covered[ti] => match &prep {
                    Some(pr) => {
                        let taps = pr.taps(sf, ti, &xf, img.w, img.h);
                        for (c, o) in out.iter_mut().enumerate() {
                            *o = taps.sample(&img, c, n);
                        }
                    }
                    None => {
                        for (c, o) in out.iter_mut().enumerate() {
                            *o = procedural::triplanar_sample(&img, c, n, sf, ti, &xf);
                        }
                    }
                },
                _ => {
                    let (u, v) = xf.map((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    for (c, o) in out.iter_mut().enumerate() {
                        *o = sample(&uv_img, u, v, c, n);
                    }
                }
            }
            if colour {
                for o in out.iter_mut() {
                    *o = srgb_to_linear(o.clamp(0.0, 1.0));
                }
            }
        }
    });
    let mut img = Image { w: ctx.w, h: ctx.h, n, data };
    if colour && ctx.opts.aces_working {
        convert_colour(&mut img, &SRGB_TO_ACESCG);
    }
    Some(img)
}

/// Identifier of the graph output that feeds channel `bit` (a material graph has one per channel).
fn procedural_output_for(p: &shard_model::Procedural, bit: u32) -> Option<&str> {
    p.outputs.iter().find(|o| o.channels.contains(bit)).map(|o| o.id.as_str())
}

/// What the layer content writes to channel `bit` (None = channel untouched) and its opacity.
fn content_value(l: &Layer, bit: u32, n: usize, acc: &Image, ctx: &mut Ctx) -> Option<(Content, f32)> {
    let mut result: Option<(Content, f32)> = None;
    fn walk(
        effects: &[Effect],
        bit: u32,
        n: usize,
        name: &str,
        acc: &Image,
        ctx: &mut Ctx,
        filter_layer: bool,
        result: &mut Option<(Content, f32)>,
    ) {
        for e in effects {
            if !e.visible {
                continue;
            }
            match &e.kind {
                EffectKind::Fill(f) => {
                    // the content built below this fill; the fill writes its own and is blended over it
                    let below = result.take();
                    for s in &f.sources {
                        match s {
                            Source::Uniform { channels, color, opacity } if channels.contains(bit) => {
                                if let Some(c) = color {
                                    let v: Vec<f32> = if n == 3 { c.to_vec() } else { vec![c[0]] };
                                    *result = Some((Content::Const(v), *opacity));
                                }
                            }
                            Source::Bitmap { channels, resource: Some(r), opacity } if channels.contains(bit) => {
                                if let Some(img) = bitmap_map(f, r, n, name, ctx) {
                                    *result = Some((Content::Map(img), *opacity));
                                }
                            }
                            Source::Reference { channels, opacity, target, levels } if channels.contains(bit) => {
                                let lv = levels.as_ref().filter(|_| std::env::var_os("SHARD_NO_REF_LEVELS").is_none());
                                let leveled = |mut im: Image| {
                                    if let Some(l) = lv {
                                        let n = im.n;
                                        for (i, x) in im.data.iter_mut().enumerate() {
                                            *x = l.apply(*x, i % n);
                                        }
                                    }
                                    im
                                };
                                match ctx.anchors.get(&(*target, bit)).cloned() {
                                    Some(img) if img.n == n => *result = Some((Content::Map(leveled((*img).clone())), *opacity)),
                                    Some(img) => {
                                        // component count differs (e.g. grey anchor into colour): replicate / average
                                        let mut data = Vec::with_capacity((img.w * img.h) as usize * n);
                                        for px in img.data.chunks_exact(img.n) {
                                            let m = px.iter().sum::<f32>() / img.n as f32;
                                            for c in 0..n {
                                                data.push(if img.n >= n { px[c] } else { m });
                                            }
                                        }
                                        *result = Some((Content::Map(leveled(Image { w: img.w, h: img.h, n, data })), *opacity));
                                    }
                                    None => ctx.notes.push(format!("reference to an unknown anchor on '{}'", name)),
                                }
                            }
                            Source::Procedural { channels, procedural, opacity } if channels.contains(bit) => {
                                // exact render by the Substance engine, else the graph's constant output, else a
                                // native approximation (each step runs once: the render is the expensive part)
                                let mut done = false;
                                if ctx.opts.allow_approx && std::env::var_os("SHARD_NO_ORACLE").is_none() {
                                    let spec = procedural::ProjSpec::of(f);
                                    if let Some(img) = rendered_content(procedural, bit, n, ctx, &f.uv, &spec) {
                                        ctx.approx.push(format!(
                                            "content {}: Substance engine (exact)",
                                            procedural.graph_name().unwrap_or("?")
                                        ));
                                        *result = Some((Content::Map(img), *opacity));
                                        done = true;
                                    }
                                }
                                if !done && ctx.opts.allow_approx {
                                    let g = procedural_output_for(procedural, bit)
                                        .and_then(|o| procedural::generate_output(procedural, o, n));
                                    if let Some(g) = g {
                                        ctx.approx.push(format!("content {}", g.note));
                                        *result = Some((Content::Const(g.values), *opacity));
                                        done = true;
                                    }
                                }
                                if !done && ctx.opts.allow_approx {
                                    match procedural::generate_scaled(procedural, ctx.w, ctx.h, ctx.opts.maps, &f.uv, &procedural::ProjSpec::of(f)) {
                                        Some(g) => {
                                            ctx.approx.push(format!("content {}", g.approx));
                                            let mut data = Vec::with_capacity(g.values.len() * n);
                                            for v in &g.values {
                                                for _ in 0..n {
                                                    data.push(*v);
                                                }
                                            }
                                            *result = Some((Content::Map(Image { w: ctx.w, h: ctx.h, n, data }), *opacity));
                                        }
                                        None => ctx.notes.push(format!("{} source on '{}'", s.kind(), name)),
                                    }
                                    done = true;
                                }
                                if !done {
                                    ctx.notes.push(format!("{} source on '{}'", s.kind(), name));
                                }
                            }
                            Source::Bitmap { channels, .. } | Source::Procedural { channels, .. } | Source::Reference { channels, .. }
                                if channels.contains(bit) =>
                            {
                                ctx.notes.push(format!("{} source on '{}'", s.kind(), name));
                            }
                            _ => {}
                        }
                    }
                    // blend what the fill produced over the content below, with the effect's blend mode
                    *result = match (below, result.take()) {
                        (Some(prev), Some(new)) => Some(stack_effects(prev, new, e, bit, n)),
                        (Some(prev), None) => Some(prev),
                        (None, new) => new,
                    };
                }
                EffectKind::Group { children } => walk(children, bit, n, name, acc, ctx, filter_layer, result),
                EffectKind::Anchor { .. } if e.uid != 0 => {
                    // remember the content at this point for `reference` sources elsewhere
                    if let Some((c, _)) = result.as_ref() {
                        let img = match c {
                            Content::Map(i) => i.clone(),
                            Content::Const(v) => {
                                let mut d = Vec::with_capacity((ctx.w * ctx.h) as usize * v.len());
                                for _ in 0..ctx.w * ctx.h {
                                    d.extend_from_slice(v);
                                }
                                Image { w: ctx.w, h: ctx.h, n: v.len(), data: d }
                            }
                        };
                        set_anchor(ctx, (e.uid, bit), Arc::new(img));
                    }
                }
                // filters, levels and strokes only matter when they act on this channel
                EffectKind::Levels(lv) if e.channels.is_empty() || e.channels.contains(bit) => {
                    match result.take() {
                        Some((Content::Const(v), op)) => {
                            let v = v.iter().enumerate().map(|(c, x)| lv.apply(*x, c)).collect();
                            *result = Some((Content::Const(v), op));
                        }
                        Some((Content::Map(mut img), op)) => {
                            let nn = img.n;
                            for px in img.data.chunks_exact_mut(nn) {
                                for (c, x) in px.iter_mut().enumerate() {
                                    *x = lv.apply(*x, c);
                                }
                            }
                            *result = Some((Content::Map(img), op));
                        }
                        None => ctx.notes.push(format!("levels without content on '{}'", name)),
                    }
                }
                EffectKind::Filter { procedural: Some(p) }
                    if ctx.opts.allow_approx && (e.channels.is_empty() || e.channels.contains(bit)) =>
                {
                    // a filter processes the content built so far; a layer made only of filters (a
                    // "filter layer") processes everything already composited below. A filter inside a
                    // layer with fills only acts on the channels that layer writes.
                    if result.is_none() && !filter_layer {
                        continue;
                    }
                    let (base, op) = match result.take() {
                        Some((Content::Map(i), op)) => (i, op),
                        Some((Content::Const(v), op)) => {
                            let mut d = Vec::with_capacity((ctx.w * ctx.h) as usize * v.len());
                            for _ in 0..ctx.w * ctx.h {
                                d.extend_from_slice(&v);
                            }
                            (Image { w: ctx.w, h: ctx.h, n: v.len(), data: d }, op)
                        }
                        None => (acc.clone(), 1.0),
                    };
                    let mut img = base;
                    // Painter's stack is continuous past the UV islands, so a spatial filter (sharpen, highpass,
                    // blur) at an island border reads more of the same surface. Here the uncovered texels hold
                    // whatever the masks gave them: a test vehicle's interior Sharpen on top of the stack turned
                    // 95% of the island border ring black (black lines along every UV seam in the viewport). The
                    // input is padded from the covered texels first; SHARD_NO_FILTER_PAD=1 feeds it as is.
                    if std::env::var_os("SHARD_NO_FILTER_PAD").is_none() {
                        if let Some(s) = ctx.opts.maps.surface(img.w, img.h).filter(|s| s.covered.len() == (img.w * img.h) as usize) {
                            dilate(&mut img.data, img.n, &s.covered, img.w, img.h, img.w + img.h);
                        }
                    }
                    let nn = img.n;
                    if std::env::var_os("SHARD_NO_ORACLE").is_none() {
                        // a filter writes the graph output that belongs to this channel (finish_rough has
                        // normal / roughness / glossiness outputs); one that has none does not touch it
                        let out_id = procedural_output_for(p, bit);
                        let skip = out_id.is_none() && p.outputs.len() > 1;
                        if std::env::var_os("SHARD_TRACE_RENDER").is_some() && bit == 0 {
                            eprintln!(
                                "[filter] {} on '{}': outputs {:?} -> picked {:?}",
                                p.graph_name().unwrap_or("?"),
                                name,
                                p.outputs.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                                out_id
                            );
                            eprintln!("[filter]   tweaks {:?}", p.tweaks);
                        }
                        if let (false, Some((rw, rh))) = (skip, procedural::render_size(p, img.w, img.h)) {
                            if rw == img.w && rh == img.h {
                                // experiment (SHARD_FILTER_SRGB=1): a colour channel is handed to the graph sRGB-encoded
                                // and its result decoded, as an artist's filter sees it
                                let graph = p.graph_name().unwrap_or("");
                                let listed = |var: &str| {
                                    std::env::var(var).is_ok_and(|v| v.split(',').any(|n| n == "*" || n == graph))
                                };
                                let colour_ch = matches!(bit, 0 | 4);
                                let srgb = colour_ch && (std::env::var_os("SHARD_FILTER_SRGB").is_some() || listed("SHARD_FILTER_SRGB_IN"));
                                let decode = colour_ch && (std::env::var_os("SHARD_FILTER_SRGB").is_some() || listed("SHARD_FILTER_SRGB_OUT"));
                                let fed = if srgb {
                                    let mut e = img.clone();
                                    for v in e.data.iter_mut() {
                                        *v = linear_to_srgb(v.clamp(0.0, 1.0));
                                    }
                                    e
                                } else {
                                    img.clone()
                                };
                                if let Some(mut r) = ctx.opts.maps.render_graph(p, out_id, rw, rh, Some(&fed)) {
                                    // A filter graph whose output is declared grayscale (the Painter filters sharpen,
                                    // highpass...: the engine converts a colour input to luminance) is run by Painter on
                                    // the components of a colour channel, so the colour survives: once per component
                                    // (the hull of a test vehicle turned grey under a Sharpen on top of the stack).
                                    // SHARD_FILTER_GRAY=1 keeps the old single (luminance) pass.
                                    if r.n == 1 && fed.n >= 3 && std::env::var_os("SHARD_FILTER_GRAY").is_none() {
                                        let mut planes: Vec<Option<Arc<Image>>> = Vec::new();
                                        let mut ok = true;
                                        for c in 0..3usize {
                                            let one = Image {
                                                w: fed.w,
                                                h: fed.h,
                                                n: 1,
                                                data: fed.data.iter().skip(c).step_by(fed.n).copied().collect(),
                                            };
                                            match ctx.opts.maps.render_graph(p, out_id, rw, rh, Some(&one)) {
                                                Some(q) => planes.push(Some(q)),
                                                None => {
                                                    ok = false;
                                                    break;
                                                }
                                            }
                                        }
                                        if ok {
                                            let npx = (img.w * img.h) as usize;
                                            let mut data = Vec::with_capacity(npx * 3);
                                            for i in 0..npx {
                                                for q in planes.iter().flatten() {
                                                    data.push(q.data[i * q.n]);
                                                }
                                            }
                                            r = Arc::new(Image { w: img.w, h: img.h, n: 3, data });
                                        }
                                    }
                                    let mut data = Vec::with_capacity((img.w * img.h) as usize * nn);
                                    for px in r.data.chunks_exact(r.n) {
                                        for c in 0..nn {
                                            let v = px[c.min(r.n - 1)];
                                            data.push(if decode { srgb_to_linear(v.clamp(0.0, 1.0)) } else { v });
                                        }
                                    }
                                    ctx.approx.push(format!(
                                        "filter {}: Substance engine (exact)",
                                        p.graph_name().unwrap_or("?")
                                    ));
                                    *result = Some((Content::Map(Image { w: img.w, h: img.h, n: nn, data }), op));
                                    continue;
                                }
                            }
                        }
                    }
                    let mut applied = procedural::filter_color(p, &mut img.data, nn);
                    let color_done = applied.is_some();
                    for c in 0..nn {
                        if color_done {
                            break;
                        }
                        let mut ch: Vec<f32> = img.data.iter().skip(c).step_by(nn).copied().collect();
                        match procedural::filter(p, &mut ch, ctx.w, ctx.h) {
                            Some(d) => applied = Some(d),
                            None => {
                                applied = None;
                                break;
                            }
                        }
                        for (i, v) in ch.into_iter().enumerate() {
                            img.data[i * nn + c] = v;
                        }
                    }
                    match applied {
                        Some(d) => ctx.approx.push(format!("filter layer: {d}")),
                        // unknown filter: leave the content as it is rather than dropping the layer
                        None => ctx.approx.push(format!(
                            "ignored filter {} on '{}'",
                            p.graph_name().unwrap_or("?"),
                            name
                        )),
                    }
                    *result = Some((Content::Map(img), op));
                }
                EffectKind::Filter { .. } | EffectKind::Levels(_) => {
                    if std::env::var_os("SHARD_TRACE_RENDER").is_some() && bit == 0 {
                        eprintln!("[filter-skip] '{}' {:?} channels {:?} allow_approx {}", name, effect_name(&e.kind), e.channels, ctx.opts.allow_approx);
                    }
                    if e.channels.is_empty() || e.channels.contains(bit) {
                        ctx.notes.push(format!("{} effect on '{}'", effect_name(&e.kind), name));
                    }
                }
                EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => {
                    if !a.strokes.is_empty() && (e.channels.is_empty() || e.channels.contains(bit)) {
                        ctx.notes.push(format!("paint strokes on '{}'", name));
                    }
                }
                _ => {}
            }
        }
    }
    fn has_fill(es: &[Effect]) -> bool {
        es.iter().any(|e| match &e.kind {
            EffectKind::Fill(_) => true,
            EffectKind::Group { children } => has_fill(children),
            _ => false,
        })
    }
    let filter_layer = std::env::var_os("SHARD_FILTER_ON_ACC").is_some() || !has_fill(&l.content);
    walk(&l.content, bit, n, &l.name, acc, ctx, filter_layer, &mut result);
    result
}

/// Does the layer (or a layer below it in a group) hold an anchor point in its content?
fn has_content_anchor(l: &Layer) -> bool {
    fn eff(es: &[Effect]) -> bool {
        es.iter().any(|e| match &e.kind {
            EffectKind::Anchor { .. } => e.uid != 0,
            EffectKind::Group { children } => eff(children),
            _ => false,
        })
    }
    eff(&l.content) || l.children.iter().any(has_content_anchor)
}

/// Levels in the content of a group adjust the group's flattened result for the channels they list (a test vehicle
/// interior's `Vehicle_Interior` stretches Roughness with one). Filters in a group's content are not applied.
fn apply_group_levels(l: &Layer, bit: u32, img: &mut Image) {
    fn walk(es: &[Effect], bit: u32, img: &mut Image) {
        for e in es.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Levels(lv) if e.channels.contains(bit) => {
                    let n = img.n;
                    for px in img.data.chunks_exact_mut(n) {
                        for (c, x) in px.iter_mut().enumerate() {
                            *x = lv.apply(*x, c);
                        }
                    }
                }
                EffectKind::Group { children } => walk(children, bit, img),
                _ => {}
            }
        }
    }
    walk(&l.content, bit, img);
}

fn apply_layer(l: &Layer, bit: u32, acc: &mut Image, ctx: &mut Ctx) {
    if !l.visible {
        // a hidden layer does not blend, but its anchor points stay available to the layers that
        // reference them (the usual way to keep a filtered copy of the stack as a source)
        if has_content_anchor(l) && std::env::var_os("SHARD_NO_HIDDEN_ANCHORS").is_none() {
            let keep = ctx.notes.len();
            let n = acc.n;
            let _ = content_value(l, bit, n, acc, ctx);
            if l.children.iter().any(has_content_anchor) {
                let mut scratch = acc.clone();
                let saved = ctx.alpha.take();
                apply_layers(&l.children, bit, &mut scratch, ctx);
                ctx.alpha = saved;
            }
            ctx.notes.truncate(keep);
        }
        return;
    }
    // diagnostic: SHARD_HIDE_LAYER=name1,name2 skips layers/groups by name
    if let Ok(v) = std::env::var("SHARD_HIDE_LAYER") {
        if v.split(',').any(|n| n.trim() == l.name) {
            return;
        }
    }
    let (mut mode, mut op) = blend_for(&l.blend, bit, l.kind == LayerKind::Group);
    // diagnostic: SHARD_BLEND_OVERRIDE="Layer name|bit|Mode[|opacity];..." replaces one channel blend of a layer
    if let Ok(spec) = std::env::var("SHARD_BLEND_OVERRIDE") {
        for item in spec.split(';') {
            let f: Vec<&str> = item.split('|').collect();
            if f.len() >= 3 && f[0] == l.name && f[1].parse::<u32>().ok() == Some(bit) {
                if let Some(i) = (0..32u8).find(|i| BlendMode(*i).name().eq_ignore_ascii_case(f[2])) {
                    mode = BlendMode(i);
                }
                if let Some(o) = f.get(3).and_then(|v| v.parse::<f32>().ok()) {
                    op = o;
                }
            }
        }
    }
    if mode.name() == "Disable" {
        return;
    }
    let notes_before = ctx.notes.len();
    if !is_blend_implemented(mode) {
        ctx.notes.push(format!("blend mode {} on '{}'", mode.name(), l.name));
    }
    let notes_mask_start = ctx.notes.len();
    // A group whose mask is only polygon-fill clicks follows them on the channels where it blends as an
    // isolated group (a test project, `Metal` group: Height and Normal have an explicit Normal blend and show
    // the black clicks on the screen and the keys -- flat Normal there in the export). On the channels it
    // passes through it is shown everywhere: colour, roughness and metallic keep its steel layers over the
    // bolts, back plate and end wall that carry black clicks.
    let click_mask = l.kind == LayerKind::Group && l.mask.as_ref().is_some_and(|k| !has_generators(&k.effects) && k.enabled);
    // diagnostic: SHARD_GROUP_DECALS_ONLY=a,b replays the clicks of those groups on every channel and shows the
    // other click-mask groups everywhere
    let only = std::env::var("SHARD_GROUP_DECALS_ONLY").ok();
    // a group whose clicks are all white is a selection and is honoured on every channel
    let white_only = click_mask && l.mask.as_ref().is_some_and(|k| decal_colors(&k.effects) == (true, false));
    // a mode that only came from the default of an unlisted channel does not bring the group's clicks with it
    let default_mode = l.kind == LayerKind::Group
        && !l.blend.iter().any(|b| b.channels.contains(bit))
        && match std::env::var("SHARD_GROUP_DEFAULT").ok().as_deref() {
            Some("passthrough") => false,
            Some("listed_all_channels") => !l.blend.is_empty(),
            Some("scalar_if_listed") => !l.blend.is_empty() && !matches!(bit, 1 | 22),
            _ => true,
        };
    ctx.replay_group_decals = click_mask
        && (!legacy_clicks()
            || match &only {
                Some(v) => v.split(',').any(|n| n == l.name),
                None => {
                    (mode != BlendMode::PASSTHROUGH && !default_mode)
                        || white_only
                        || std::env::var_os("SHARD_GROUP_DECALS_ALL_CHANNELS").is_some()
                }
            });
    let t_mask = std::time::Instant::now();
    let (mut m, unreliable) = mask_value(l, ctx);
    if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
        let ms = t_mask.elapsed().as_secs_f64() * 1000.0;
        if ms >= 20.0 {
            eprintln!("[time]   mask  bit {bit:2} {ms:>8.0} ms  {}", l.name);
        }
    }
    ctx.replay_group_decals = false;
    // with approximations, a group whose mask cannot be reproduced is shown (mask ignored) rather
    // than dropped; the notes of its mask do not count as unsupported content
    let notes_before = if l.kind == LayerKind::Group && ctx.opts.allow_approx {
        notes_before.max(notes_mask_start.max(ctx.notes.len()))
    } else {
        notes_before
    };
    if unreliable && l.kind == LayerKind::Group && ctx.opts.allow_approx && std::env::var_os("SHARD_TRUST_MASKS").is_none() {
        // a group is an organiser: when its mask cannot be reproduced, show its content instead of
        // nothing -- unless the mask is made only of decals / hand-painted strokes (no generator at
        // all): then its initial value is the best estimate (a decal-only mask starts empty)
        let initial = l.mask.as_ref().map(|k| if k.initial != 0 { 1.0 } else { 0.0 }).unwrap_or(1.0);
        let only_marks = l.mask.as_ref().is_some_and(|k| !has_generators(&k.effects));
        // SHARD_DECAL_MASK_EMPTY=1: treat a decal-only mask as empty (default: shown, decals are not decoded)
        let empty_ok = only_marks && std::env::var_os("SHARD_DECAL_MASK_EMPTY").is_some();
        m = Scalar::Const(if empty_ok { initial } else { 1.0 });
        ctx.approx.push(format!("mask of group '{}' not reproducible: {}", l.name, if empty_ok { "initial value used" } else { "ignored" }));
    }
    let npx = (acc.w * acc.h) as usize;
    if l.kind == LayerKind::Group {
        if std::env::var_os("SHARD_TRACE_LAYERS").is_some() && bit == 0 {
            let mean = (0..npx).map(|i| m.at(i)).sum::<f32>() / npx.max(1) as f32;
            eprintln!("  [{}px] group '{}' mode={} op={} mask_mean={:.3} replay_clicks={}", ctx.w, l.name, mode.name(), op, mean, ctx.replay_group_decals);
        }
        if ctx.opts.skip_unsupported && ctx.notes.len() > notes_before {
            if std::env::var_os("SHARD_TRACE_LAYERS").is_some() && bit == 0 {
                eprintln!("    xx [{}] SKIPPED group '{}': {:?}", ctx.name, l.name, &ctx.notes[notes_before..]);
            }
            return;
        }
        let unmasked = matches!(m, Scalar::Const(c) if c >= 1.0);
        if mode == BlendMode::PASSTHROUGH && unmasked {
            // Children composite straight into the accumulator (folder acts only as organiser).
            ctx.ck_next = group_children_base(ctx, false);
            apply_layers(&l.children, bit, acc, ctx);
            apply_group_levels(l, bit, acc);
        } else if mode == BlendMode::PASSTHROUGH {
            // Passthrough with a mask: the children's result is faded in by the group mask.
            let coverage_before = ctx.alpha.clone();
            let mut inner = acc.clone();
            ctx.ck_next = group_children_base(ctx, false);
            apply_layers(&l.children, bit, &mut inner, ctx);
            apply_group_levels(l, bit, &mut inner);
            let n = acc.n;
            let inner = &inner;
            let w = acc.w as usize;
            procedural::par_rows(&mut acc.data, w * n, |y, row| {
                for (x, px) in row.chunks_exact_mut(n).enumerate() {
                    let i = y * w + x;
                    let a = (op * m.at(i)).clamp(0.0, 1.0);
                    for (c, d) in px.iter_mut().enumerate() {
                        let (dst, src) = (*d, inner.data[i * n + c]);
                        *d = dst + (src - dst) * a;
                    }
                }
            });
            // inside an isolated group the coverage written by the children shows only where the mask does
            if let (Some(before), Some(after)) = (coverage_before, ctx.alpha.as_mut()) {
                for (i, (b, a)) in before.iter().zip(after.iter_mut()).enumerate() {
                    *a = b + (*a - b) * (op * m.at(i)).clamp(0.0, 1.0);
                }
            }
        } else {
            // An isolated group: its children are composed on their own (empty where nothing is written), then
            // the result is blended onto what is below with the group's mode, opacity and mask. `Replace` also
            // replaces with emptiness: a channel its children do not write comes out as the default value (a
            // `Replace` group over Normal flattens the normals below it).
            let parent_alpha = ctx.alpha.take();
            ctx.alpha = Some(vec![0.0; npx]);
            let mut inner = acc.clone();
            ctx.ck_next = group_children_base(ctx, true);
            apply_layers(&l.children, bit, &mut inner, ctx);
            apply_group_levels(l, bit, &mut inner);
            let inner_alpha = ctx.alpha.take().unwrap_or_else(|| vec![1.0; npx]);
            ctx.alpha = parent_alpha;
            let replace = mode.name() == "Replace";
            let default = default_value(bit);
            let n = acc.n;
            let (inner, inner_alpha, m) = (&inner, &inner_alpha, &m);
            for_texels(acc, ctx.alpha.as_mut(), |i, px, pa| {
                let a_g = (op * m.at(i)).clamp(0.0, 1.0);
                if a_g <= 0.0 {
                    return;
                }
                let cover = inner_alpha[i];
                if replace {
                    for (c, d) in px.iter_mut().enumerate() {
                        let s = cover * inner.data[i * n + c] + (1.0 - cover) * default[c.min(default.len() - 1)];
                        *d += (s - *d) * a_g;
                    }
                    // a replaced pixel is opaque: the default value stands where the children wrote nothing
                    if let Some(pa) = pa {
                        *pa += (1.0 - *pa) * a_g;
                    }
                    return;
                }
                let a = a_g * cover;
                if a <= 0.0 {
                    return;
                }
                let pa_v = pa.as_ref().map(|p| **p).unwrap_or(1.0);
                let mut ar = pa_v;
                for (c, d) in px.iter_mut().enumerate() {
                    let (nd, na) = composite_px(mode, *d, inner.data[i * n + c], a, pa_v);
                    *d = nd;
                    ar = na;
                }
                if let Some(pa) = pa {
                    *pa = ar;
                }
            });
        }
        return;
    }
    let n = acc.n;
    let t_content = std::time::Instant::now();
    let content = content_value(l, bit, n, acc, ctx);
    if std::env::var_os("SHARD_TRACE_TIMING").is_some() {
        let ms = t_content.elapsed().as_secs_f64() * 1000.0;
        if ms >= 20.0 {
            eprintln!("[time]   content bit {bit:2} {ms:>8.0} ms  {}", l.name);
        }
    }
    if ctx.opts.skip_unsupported && ctx.notes.len() > notes_before {
        if std::env::var_os("SHARD_TRACE_LAYERS").is_some() && bit == 0 {
            eprintln!("    xx [{}] SKIPPED '{}': {:?}", ctx.name, l.name, &ctx.notes[notes_before..]);
        }
        return;
    }
    // SHARD_TRACE_CONTENT=<bit>: statistics of what every layer writes to that channel
    if std::env::var("SHARD_TRACE_CONTENT").ok().and_then(|v| v.parse::<u32>().ok()) == Some(bit) {
        let mean_mask = (0..npx).map(|i| m.at(i)).sum::<f32>() / npx.max(1) as f32;
        match &content {
            Some((Content::Const(v), sop)) => {
                eprintln!("  [content bit {bit}] '{}' {} op {:.2} mask mean {:.3}: const {:.4} @ {:.2}", l.name, mode.name(), op, mean_mask, v[0], sop)
            }
            Some((Content::Map(img), sop)) => {
                let vals: Vec<f32> = img.data.iter().step_by(img.n).copied().collect();
                let mean = vals.iter().sum::<f32>() / vals.len() as f32;
                let var = vals.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / vals.len() as f32;
                let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
                eprintln!(
                    "  [content bit {bit}] '{}' {} op {:.2} mask mean {:.3}: map mean {:.3} std {:.3} range {:.3}..{:.3} @ {:.2}",
                    l.name, mode.name(), op, mean_mask, mean, var.sqrt(), lo, hi, sop
                )
            }
            None => {}
        }
    }
    if std::env::var_os("SHARD_TRACE_LAYERS").is_some() && bit == 0 {
        let mean = (0..npx).map(|i| m.at(i)).sum::<f32>() / npx.max(1) as f32;
        eprintln!(
            "  [{}px] layer '{}' mode={} op={} mask_mean={:.3} content={}",
            ctx.w,
            l.name,
            mode.name(),
            op,
            mean,
            match &content {
                Some((Content::Const(v), s)) => format!("const{:?}@{}", v, s),
                Some((Content::Map(_), s)) => format!("map@{}", s),
                None => "none".into(),
            }
        );
    }
    if let Some((content, sop)) = content {
        // experiment: blend colour channels on their sRGB-encoded values (SHARD_BLEND_SRGB=1)
        let srgb_blend = (matches!(bit, 0 | 4) && std::env::var_os("SHARD_BLEND_SRGB").is_some())
            || std::env::var("SHARD_BLEND_SRGB_BITS").is_ok_and(|v| v.split(',').any(|x| x.trim().parse::<u32>().ok() == Some(bit)));
        let (content, m) = (&content, &m);
        let content_at = |i: usize, c: usize| match content {
            Content::Const(v) => v[c.min(v.len() - 1)],
            Content::Map(img) => img.data[i * n + c],
        };
        for_texels(acc, ctx.alpha.as_mut(), |i, px, pa| {
            let a = (op * sop * m.at(i)).clamp(0.0, 1.0);
            if a <= 0.0 {
                return;
            }
            let pa_v = pa.as_ref().map(|p| **p).unwrap_or(1.0);
            let mut ar = pa_v;
            for (c, d) in px.iter_mut().enumerate() {
                let s = content_at(i, c);
                if srgb_blend && pa_v >= 1.0 {
                    let (bs, ss) = (linear_to_srgb(d.clamp(0.0, 1.0)), linear_to_srgb(s.clamp(0.0, 1.0)));
                    *d = srgb_to_linear((bs + (blend(mode, bs, ss) - bs) * a).clamp(0.0, 1.0));
                } else {
                    let (nd, na) = composite_px(mode, *d, s, a, pa_v);
                    *d = nd;
                    ar = na;
                }
            }
            if let Some(pa) = pa {
                *pa = ar;
            }
        });
        if std::env::var_os("SHARD_TRACE_LAYERS").is_some() && bit == 0 {
            let mean_c = |c: usize| acc.data.iter().skip(c.min(n - 1)).step_by(n).sum::<f32>() / npx.max(1) as f32;
            eprintln!(
                "    -> [{}] after '{}': mean(R,G,B)=({:.4}, {:.4}, {:.4})",
                ctx.name,
                l.name,
                mean_c(0),
                mean_c(1),
                mean_c(2)
            );
        }
        // SHARD_PROBE_PIXEL=u,v (0..1): value of that texel after every layer (resolution >= 256);
        // SHARD_PROBE_BIT selects the channel (default 0 = base colour)
        let probe_bit = std::env::var("SHARD_PROBE_BIT").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        if bit == probe_bit && acc.w >= 256 {
            if let Some((u, v)) = std::env::var("SHARD_PROBE_PIXEL").ok().and_then(|s| {
                let mut it = s.split(',').filter_map(|t| t.trim().parse::<f32>().ok());
                Some((it.next()?, it.next()?))
            }) {
                let (x, y) = (((u * acc.w as f32) as u32).min(acc.w - 1), ((v * acc.h as f32) as u32).min(acc.h - 1));
                let i = ((y * acc.w + x) as usize) * n;
                eprintln!(
                    "    @px {}x{} after '{}' [{} op {:.2}, mask {:.3}, content {:.4}]: {:.4}",
                    acc.w,
                    acc.h,
                    l.name,
                    mode.name(),
                    op * sop,
                    m.at((y * acc.w + x) as usize),
                    match &content {
                        Content::Const(v) => v[0],
                        Content::Map(img) => img.data[i],
                    },
                    acc.data[i]
                );
            }
        }
    }
}
