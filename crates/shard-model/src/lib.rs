//! ShardStroke layer-stack model (v0).
//!
//! ```text
//! Document -> Layer{ kind, name, visible, blend[], content[Effect], mask, children[Layer] }
//! Effect   -> Fill | Paint | Eraser | Smudge | Group | Filter | Levels | Anchor | Decal | Other
//! Source   -> Uniform | Bitmap | Procedural | Reference | Other
//! ```
//! The model is independent of any file format; [`import`] builds it from Painter data files.

pub mod edit;
pub mod import;

use std::collections::BTreeMap;

/// Set of channel types as a 128-bit bitset (bit N = channel type N, see `shard_formats::enums`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelSet(pub u128);

impl serde::Serialize for ChannelSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:x}", self.0))
    }
}

impl<'de> serde::Deserialize<'de> for ChannelSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let t = String::deserialize(d)?;
        u128::from_str_radix(&t, 16).map(ChannelSet).map_err(serde::de::Error::custom)
    }
}

impl ChannelSet {
    pub fn contains(&self, bit: u32) -> bool {
        bit < 128 && (self.0 >> bit) & 1 == 1
    }
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    pub fn count(&self) -> u32 {
        self.0.count_ones()
    }
    pub fn names(&self) -> Vec<String> {
        (0..128u32)
            .filter(|b| self.contains(*b))
            .map(|b| shard_formats::enums::channel_name(b).unwrap_or_else(|| format!("#{b}")))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlendMode(pub u8);

impl BlendMode {
    pub const NORMAL: BlendMode = BlendMode(2);
    pub const MULTIPLY: BlendMode = BlendMode(4);
    pub const PASSTHROUGH: BlendMode = BlendMode(25);
    pub fn name(&self) -> &'static str {
        shard_formats::enums::blending_mode_name(self.0 as i64).unwrap_or("?")
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChannelBlend {
    pub mode: BlendMode,
    pub channels: ChannelSet,
    pub opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Projection {
    Uv,
    Planar,
    Triplanar,
    Spherical,
    Cylindrical,
    UvSetToUvSet,
    Fill,
    Warp,
    Unknown(u8),
}

impl Projection {
    pub fn from_value(v: i64) -> Self {
        match v {
            0 => Self::Uv,
            1 => Self::Planar,
            2 => Self::Triplanar,
            3 => Self::Spherical,
            4 => Self::Cylindrical,
            5 => Self::UvSetToUvSet,
            6 => Self::Fill,
            7 => Self::Warp,
            x => Self::Unknown(x as u8),
        }
    }
}

/// Immutable-by-default shared storage for the heavy, rarely edited parts of a layer (hand-painted
/// strokes, polygon-fill hits). Cloning a layer stack, which is what undo snapshots do, then copies a
/// pointer instead of the data; editing goes through [`Shared::make_mut`] (copy on write).
#[derive(Debug, Default)]
pub struct Shared<T>(std::sync::Arc<T>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(self.0.clone())
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    /// Same allocation means equal without looking at the data (the common case when comparing a stack
    /// with its snapshot).
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl<T> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> From<T> for Shared<T> {
    fn from(v: T) -> Self {
        Shared(std::sync::Arc::new(v))
    }
}

impl<'a, T> IntoIterator for &'a Shared<Vec<T>> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Clone> Shared<T> {
    /// Mutable access; copies the data first when it is shared with a snapshot.
    pub fn make_mut(&mut self) -> &mut T {
        std::sync::Arc::make_mut(&mut self.0)
    }
}

impl<T> Shared<T> {
    /// True when `self` and `other` are the same allocation.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T: serde::Serialize> serde::Serialize for Shared<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Shared<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Shared::from)
    }
}

/// Make sure ids handed out by [`next_uid`] stay above `floor` (call after loading a project that stored
/// ids generated in an earlier session).
pub fn reserve_uids(floor: u64) {
    UID_COUNTER.fetch_max(floor, std::sync::atomic::Ordering::Relaxed);
}

static UID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 48);

/// Identity for layers and effects created at run time. Project ids are small integers; new ones start far
/// above them so they never collide.
pub fn next_uid() -> u64 {
    UID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Tweak {
    Float(f32),
    Int(i32),
    Int2([i32; 2]),
    Float2([f32; 2]),
    Float3([f32; 3]),
    Float4([f32; 4]),
    Bool(bool),
    Other,
}

/// `DataResolutionOverride`: the resolution a procedural source is evaluated at.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResolutionOverride {
    /// 0 = Auto, 1 = Texture Set, 2 = UV Tile, 3 = Custom (order of Painter's dropdown).
    pub mode: i32,
    /// Factor as a power of two: -3 = 1/8, -1 = 1/2, 0 = same as context, +2 = x4.
    pub log2_offset: i32,
    /// Size in texels used by the Custom mode.
    pub manual: [u32; 2],
}

impl Default for ResolutionOverride {
    fn default() -> Self {
        Self { mode: 0, log2_offset: 0, manual: [1024, 1024] }
    }
}

impl ResolutionOverride {
    /// Size the graph is evaluated at when the context (texture set / UV tile) is `w` x `h`.
    pub fn target(&self, w: u32, h: u32) -> (u32, u32) {
        if self.mode == 3 {
            return (self.manual[0].clamp(16, 8192), self.manual[1].clamp(16, 8192));
        }
        let k = 2f32.powi(self.log2_offset.clamp(-3, 3));
        (((w as f32 * k).round() as u32).clamp(16, 8192), ((h as f32 * k).round() as u32).clamp(16, 8192))
    }
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Procedural {
    /// Resource URL as stored (e.g. `/dirt_1/ratio_dirt_1?version=...`).
    pub resource: Option<String>,
    pub outputs: Vec<ProceduralOutput>,
    /// Resolution override of the source that holds this graph.
    pub resolution: ResolutionOverride,
    pub tweaks: BTreeMap<String, Tweak>,
    pub inputs: Vec<String>,
    pub input_sources: Vec<Source>,
    /// Graph image input each of `input_sources` feeds (`sourcesInputs[].input.identifier`: the mask editor's
    /// `texture`, a generator's custom grunge...).
    #[serde(default)]
    pub input_ids: Vec<String>,
}

impl Procedural {
    /// Graph name without path and version (`mg_dirt`, `blur`, ...).
    pub fn graph_name(&self) -> Option<&str> {
        let r = self.resource.as_deref()?;
        let base = r.split('?').next()?;
        let name = base.rsplit(|c| c == '/' || c == '\\').next()?;
        // a resource taken from the shelf is `file:<path>`: drop the extension
        if r.starts_with("file:") {
            Some(name.rsplit_once('.').map_or(name, |(n, _)| n))
        } else {
            Some(name)
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProceduralOutput {
    pub id: String,
    pub channels: ChannelSet,
    /// Output colour space: 0 = auto (data), 1 = raw, 2 = data, 3 = data - signed normalized
    /// (`workflowLegacy` as stored; every file seen so far has 0).
    pub color_space: i32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Uniform { channels: ChannelSet, color: Option<[f32; 3]>, opacity: f32 },
    Bitmap { channels: ChannelSet, resource: Option<String>, opacity: f32 },
    Procedural { channels: ChannelSet, procedural: Procedural, opacity: f32 },
    /// Value of an anchor point (`DataActionAnchor` uid `target`) elsewhere in the stack.
    Reference {
        channels: ChannelSet,
        opacity: f32,
        target: u64,
        /// The source's own levels (`DataSourceReference.levels`): an output range of 1 -> 0 reads the anchor
        /// inverted (a test pistol's grip dots subtract the inverted grip). `None` = identity.
        #[serde(default)]
        levels: Option<Levels>,
    },
    Other(String),
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Uniform { .. } => "uniform",
            Source::Bitmap { .. } => "bitmap",
            Source::Procedural { .. } => "procedural",
            Source::Reference { .. } => "reference",
            Source::Other(_) => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct UvTransform {
    /// Tiling (scale factors).
    pub scale: Option<[f32; 2]>,
    /// Offset (translation).
    pub position: Option<[f32; 2]>,
    /// Rotation, in radians as stored (see `UvXform::new` for its direction).
    pub rotation: Option<f32>,
    /// Pivot of rotation / scale (default centre).
    pub pivot: Option<[f32; 2]>,
    /// 0 = tiling, otherwise physical size.
    pub scale_mode: i32,
    /// `uvSamplingWrap` as stored.
    pub wrap: i32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fill {
    pub projection: Projection,
    pub filtering: i32,
    pub uv: UvTransform,
    pub sources: Vec<Source>,
    /// 3D placement of the planar / tri-planar / cylindrical projections (`ddd*` of `DataActionFill`).
    #[serde(default)]
    pub projector: Option<Projector>,
}

/// Placement of a 3D projection: a box of `size` at `position`, rotated by `euler` (radians), in the
/// normalised mesh space the strokes' cameras use. `wrap`: 0 clamps outside the box, 1 repeats.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Projector {
    pub position: [f32; 3],
    pub euler: [f32; 3],
    pub size: [f32; 3],
    pub wrap: i32,
}

/// One recorded key of a stroke (`DataStrokeKey`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StrokeKey {
    /// Position in the normalised view of the camera that recorded the stroke.
    pub pos: [f32; 2],
    /// Seconds since the start of the stroke.
    pub time: f32,
    /// Key type (0 = regular sample).
    pub kind: u8,
}

/// A hand-painted stroke (`DataStroke`): the samples plus the camera it was painted with.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stroke {
    pub keys: Vec<StrokeKey>,
    /// Camera at paint time, 4x4 floats exactly as stored (row-vector convention, translation in the
    /// last row; see docs/FORMATS.md 7.5k).
    pub view: [f32; 16],
    pub projection: [f32; 16],
    pub seed: u32,
    pub view_type: i32,
    pub lazy_distance: f32,
    pub pressure: Vec<f32>,
    /// Symmetry: `duplicateCount` copies of the stroke, the k-th through `duplicateMatrix`^k (a 4x4 in the
    /// same row-vector layout as the cameras; a mirror is `x -> -x`). 0 = no symmetry.
    #[serde(default)]
    pub duplicate_count: u32,
    #[serde(default)]
    pub duplicate_matrix: Option<[f32; 16]>,
}

/// Stamp brush settings of a paint/eraser/smudge action (`DataBrushStamp`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Brush {
    pub class: String,
    pub size: f32,
    pub size_jitter: f32,
    pub size_space: i32,
    pub flow: f32,
    pub flow_jitter: f32,
    pub culling_angle: f32,
    pub culling_enabled: bool,
    pub paint_through: bool,
    pub alignment: i32,
    pub filtering: i32,
    /// Stamp spacing as a fraction of the brush diameter.
    pub spacing: f32,
    pub follow_path: bool,
    pub position_jitter: f32,
    pub rotation: f32,
    pub rotation_jitter: f32,
    pub pressure_size: bool,
    pub pressure_size_min: f32,
    pub pressure_flow: bool,
    pub pressure_flow_min: f32,
    /// Alpha shape graph (`shape_v2`...) and its tweaks (`hardness`...).
    pub shape: Option<Procedural>,
}

impl Brush {
    pub fn hardness(&self) -> Option<f32> {
        match self.shape.as_ref()?.tweaks.get("hardness") {
            Some(Tweak::Float(h)) => Some(*h),
            _ => None,
        }
    }
}

/// `DataActionPaint` / `DataActionEraser` / `DataActionSmudge`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct PaintAction {
    pub brush: Brush,
    pub opacity: f32,
    pub color: Vec<Source>,
    pub strokes: Shared<Vec<Stroke>>,
    /// Image the strokes reveal instead of painting a plain brush mark (`sourceStencil`), placed in the view of
    /// each stroke by [`PaintAction::proj`].
    #[serde(default)]
    pub stencil: Option<Stencil>,
    /// `projMatrix`: 3x3 row-vector matrix (`[x y 1] * M`) from the stroke's view position to stencil space.
    #[serde(default)]
    pub proj: Option<[f32; 9]>,
    /// `projSamplingWrap`: what lies outside the stencil (0 = nothing).
    #[serde(default)]
    pub proj_wrap: i32,
}

/// Stencil source of a paint action.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Stencil {
    /// `DataSourceText`: text rendered with a system font.
    Text(TextStencil),
    /// `DataSourceBitmap`: a bitmap resource.
    Bitmap { resource: Option<String> },
    /// `DataSourceProcedural`: a graph (a symbol, a noise) rendered as the stencil.
    Procedural(Procedural),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextStencil {
    pub text: String,
    /// `urlToFontRes` (`resource://system_fonts/arialbd?version=...`).
    pub font: String,
    pub size: f32,
    pub auto_size: bool,
    /// Padding around the text when `auto_size` (fractions of the text box).
    pub padding: [f32; 2],
    pub offset: [f32; 2],
    pub h_align: i32,
    pub v_align: i32,
    pub letter_spacing: f32,
    pub line_spacing: f32,
    pub foreground: [f32; 3],
    pub background: [f32; 3],
    pub background_opacity: f32,
}

/// `DataLevels`: per-component input/output range and gamma (identity by default).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Levels {
    pub clamp: bool,
    pub gamma: [f32; 3],
    pub in_min: [f32; 3],
    pub in_max: [f32; 3],
    pub out_min: [f32; 3],
    pub out_max: [f32; 3],
}

impl Default for Levels {
    fn default() -> Self {
        Levels { clamp: false, gamma: [1.0; 3], in_min: [0.0; 3], in_max: [1.0; 3], out_min: [0.0; 3], out_max: [1.0; 3] }
    }
}

impl Levels {
    /// `out_min + (out_max - out_min) * ((x - in_min) / (in_max - in_min))^gamma` for component `c`: the stored
    /// gamma is the exponent as written (measured against Painter exports: the Curvalture levels of a test vehicle
    /// filter stack, `gamma 0.605`, and the roughness stretch of `Vehicle_Interior`; the inverse reading
    /// is `SHARD_LEVELS_INVERSE=1`).
    pub fn apply(&self, x: f32, c: usize) -> f32 {
        let c = c.min(2);
        let span = (self.in_max[c] - self.in_min[c]).abs().max(1e-6) * (self.in_max[c] - self.in_min[c]).signum();
        let t = ((x - self.in_min[c]) / span).max(0.0);
        let t = if self.clamp { t.min(1.0) } else { t };
        let g = self.gamma[c].max(1e-3);
        // read once: this runs for every pixel of every levels effect (the lookup cost ~0.9 s per mask at 2048)
        static INVERSE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let inverse = *INVERSE.get_or_init(|| std::env::var_os("SHARD_LEVELS_INVERSE").is_some());
        let e = if inverse { 1.0 / g } else { g };
        self.out_min[c] + (self.out_max[c] - self.out_min[c]) * t.powf(e)
    }
}

/// One click (`DataDecalHitPunctual`) or rectangle selection (`DataDecalHitRect`) of a decal / polygon-fill action.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecalHit {
    /// Object id of the hit in the document (hits are numbered in creation order).
    pub uid: u64,
    /// Click position in the viewport (0..1 on both axes). A rectangle selection has no position: it reads
    /// as the centre of the view (0.5, 0.5), what the compositor's rules for "centre clicks" were tuned on.
    pub position: [f32; 2],
    /// Both corners of a rectangle selection (`DataDecalHitRect`: a marquee dragged in the viewport),
    /// None for a click.
    #[serde(default)]
    pub rect: Option<[[f32; 2]; 2]>,
    /// Inverse of projection * view of the camera the click was made with (column major).
    pub proj_view_inv: [f32; 16],
    pub color: [f32; 3],
    /// What the click selects: 0 triangle, 1 polygon, 2 mesh element, 3 UV chunk.
    pub granularity: i32,
    pub paint_through: i32,
    pub view_type: i32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    Fill(Fill),
    Paint(PaintAction),
    Eraser(PaintAction),
    Smudge(PaintAction),
    Group { children: Vec<Effect> },
    Filter { procedural: Option<Procedural> },
    Levels(Levels),
    Anchor { name: Option<String> },
    /// Polygon / mesh-element / UV-chunk fill: every hit selects the geometry under a click.
    Decal(Shared<Vec<DecalHit>>),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Effect {
    /// Object id in the document (anchors are referenced by it).
    pub uid: u64,
    pub visible: bool,
    pub channels: ChannelSet,
    pub label: Option<String>,
    pub blend: Vec<ChannelBlend>,
    pub kind: EffectKind,
}

impl Effect {
    pub fn type_name(&self) -> &str {
        match &self.kind {
            EffectKind::Fill(_) => "fill",
            EffectKind::Paint(_) => "paint",
            EffectKind::Eraser(_) => "eraser",
            EffectKind::Smudge(_) => "smudge",
            EffectKind::Group { .. } => "group",
            EffectKind::Filter { .. } => "filter",
            EffectKind::Levels(_) => "levels",
            EffectKind::Anchor { .. } => "anchor",
            EffectKind::Decal(_) => "decal",
            EffectKind::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mask {
    pub enabled: bool,
    pub initial: i64,
    pub effects: Vec<Effect>,
    /// The user added this mask (the panel shows it even while it is still empty). Masks read from a
    /// project are shown when they hold effects.
    #[serde(default)]
    pub explicit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LayerKind {
    Layer,
    Group,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Layer {
    /// Stable identity of the layer: the project's object id, or a fresh one ([`next_uid`]) for layers
    /// created in a session. Selection and history refer to layers by it, not by position.
    pub uid: u64,
    pub kind: LayerKind,
    pub name: String,
    pub visible: bool,
    pub blend: Vec<ChannelBlend>,
    pub content: Vec<Effect>,
    pub mask: Option<Mask>,
    pub children: Vec<Layer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DocKind {
    SmartMaterial,
    SmartMask,
    Preset,
    Template,
    Project,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Document {
    pub kind: DocKind,
    pub name: String,
    /// Class of the decoded root object (`DataLayersPreset`, `DataDocument`, `DataToolPreset`, ...).
    pub root_class: String,
    pub layers: Vec<Layer>,
}

/// Aggregate counts used for corpus parity checks and reports.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub layers: usize,
    pub groups: usize,
    pub bitmaps: usize,
    pub effects: BTreeMap<String, usize>,
    pub sources: BTreeMap<&'static str, usize>,
    pub procedurals: BTreeMap<String, usize>,
}

impl Stats {
    pub fn add_document(&mut self, d: &Document) {
        self.add_layers(&d.layers);
    }

    fn add_layers(&mut self, ls: &[Layer]) {
        for l in ls {
            self.layers += 1;
            if l.kind == LayerKind::Group {
                self.groups += 1;
            }
            for e in &l.content {
                self.add_effect(e);
            }
            if let Some(m) = &l.mask {
                for e in &m.effects {
                    self.add_effect(e);
                }
            }
            self.add_layers(&l.children);
        }
    }

    fn add_effect(&mut self, e: &Effect) {
        *self.effects.entry(e.type_name().to_string()).or_default() += 1;
        match &e.kind {
            EffectKind::Fill(f) => {
                for s in &f.sources {
                    *self.sources.entry(s.kind()).or_default() += 1;
                    match s {
                        Source::Procedural { procedural, .. } => {
                            if let Some(n) = procedural.graph_name() {
                                *self.procedurals.entry(n.to_string()).or_default() += 1;
                            }
                        }
                        Source::Bitmap { .. } => self.bitmaps += 1,
                        _ => {}
                    }
                }
            }
            EffectKind::Filter { procedural: Some(p) } => {
                if let Some(n) = p.graph_name() {
                    *self.procedurals.entry(n.to_string()).or_default() += 1;
                }
            }
            EffectKind::Group { children } => {
                for c in children {
                    self.add_effect(c);
                }
            }
            _ => {}
        }
    }
}
