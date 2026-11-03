//! Application UI (egui). Layout and widgets follow docs/UI_SPEC.md (Painter structure, dark theme).

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, UiBuilder};
use shard_model::{
    BlendMode, ChannelBlend, ChannelSet, DocKind, Document, Effect, EffectKind, Fill, Layer, LayerKind,
    Projection, Source, UvTransform,
};

use crate::dock::{self, Pane};
use crate::icons::Icon;
use crate::paint::Tool;
use crate::scene::Scene;
use crate::theme;
use crate::widgets as w;

mod quick;

/// A mask open for painting.
struct MaskEdit {
    set: usize,
    path: Vec<usize>,
    res: u32,
    /// Mask image (base) plus the strokes painted now (overlay), in texture space.
    tex: crate::paint::PaintTexture,
    /// Keys of the stroke in progress: `(ndc x, ndc y, viewport aspect, seconds)`.
    keys: Vec<(f32, f32, f32, f32)>,
    started: std::time::Instant,
    /// World-space diameter of the brush for the stroke in progress.
    diameter: f32,
    camera: (glam::Vec3, glam::Vec3),
    last_upload: std::time::Instant,
    dirty: bool,
}

/// Finished thumbnails of one texture set (see `AppState::pump_thumbs`).
struct ThumbResult {
    sig: (usize, u64, u32, bool),
    items: Vec<shard_engine::compose::LayerThumb>,
}

/// Finished background composition of one texture set.
pub struct JobResult {
    set: usize,
    gen: u64,
    res: u32,
    channels: Vec<(u32, shard_compose::Image)>,
    /// Mask preview jobs: layer name and why the mask is not exact.
    info: Option<(String, Vec<String>, bool)>,
    /// Evaluated masks of the set (quantised, at most 512 wide) for the mask cache.
    masks: Vec<(Vec<usize>, u32, Vec<u8>)>,
    /// Every requested channel as 8 bit for the channel views: `(bit, width, height, components, bytes)`.
    views: Vec<(u32, u32, u32, u8, Vec<u8>)>,
}

/// Painter's own picture of a texture set, read from the project's texture cache (what Painter composed when the
/// file was saved). Shown until the set's stack is edited; our composition takes over from the first edit.
pub struct CacheLoad {
    set: usize,
    /// `refine_gen` of the set when the load started: a load for a stack edited meanwhile is dropped.
    gen: u64,
    res: u32,
    /// sRGB 8-bit RGB, `res` x `res`.
    base: Option<Vec<u8>>,
    rough: Option<shard_compose::Image>,
    metal: Option<shard_compose::Image>,
    /// The viewport normal (bake + the cached Normal channel + the cached Height), RGBA8.
    normal: Option<Vec<u8>>,
    /// Emissive colour, sRGB 8-bit RGB, `emissive_res` square (absent when the set has no emission).
    emissive: Option<(u32, Vec<u8>)>,
}

/// Read the cached channels of every listed set `(set, sceneMaterialUID, name, resolution, gen)` and send them one
/// set at a time (worker thread: decoding a 4K channel takes ~0.2 s).
fn load_painter_cache(
    path: PathBuf,
    mesh: crate::mesh::Mesh,
    urls: Vec<String>,
    sets: Vec<(usize, u64, String, u32, u64)>,
    tx: std::sync::mpsc::Sender<CacheLoad>,
) {
    crate::lower_thread_priority();
    let Ok(mut c) = shard_formats::Container::open(&path) else { return };
    let bitmaps = crate::bitmaps::ProjectBitmaps::open(&path, 2048);
    for (set, uid, name, size, gen) in sets {
        let t0 = std::time::Instant::now();
        if c.texture_cache_channels(uid).is_empty() {
            if std::env::var_os("SHARD_TRACE").is_some() {
                eprintln!("[painter cache] set {set} '{name}': nothing cached for uid {uid}");
            }
            continue;
        }
        let res = view_res(size);
        let cov = crate::export::coverage(&mesh, set, res, res);
        let iters = (res / 128).clamp(4, 16);
        // one component (or three) of a cached channel at `res`, island borders padded outwards
        let mut field = |ch: u32, k: usize| -> Option<shard_compose::Image> {
            let img = c.texture_cache(uid, ch)?;
            let side = res.min(img.width);
            let mut data = img.to_f32(k, side);
            if side == res {
                crate::export::dilate_f32(&mut data, k, &cov, res, res, iters);
            }
            Some(shard_compose::Image { w: side, h: side, n: k, data })
        };
        // a project blending in ACEScg caches its colour in ACEScg (sRGB-encoded): to sRGB like the export
        let aces = bitmaps.as_ref().is_some_and(|b| b.aces_working());
        let base = field(0, 3).map(|mut b| {
            if aces {
                for v in b.data.iter_mut() {
                    *v = shard_compose::srgb_to_linear(v.clamp(0.0, 1.0));
                }
                shard_compose::convert_colour(&mut b, &shard_compose::ACESCG_TO_SRGB);
                for v in b.data.iter_mut() {
                    *v = shard_compose::linear_to_srgb(v.clamp(0.0, 1.0));
                }
            }
            b.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect::<Vec<u8>>()
        });
        let base_res = base.as_ref().map(|b| ((b.len() / 3) as f32).sqrt() as u32).unwrap_or(res);
        // emission (same encoding as the base colour); an all-black map is not sent
        let emissive = field(4, 3).and_then(|mut e| {
            if e.data.iter().all(|v| *v <= 0.0) {
                return None;
            }
            if aces {
                for v in e.data.iter_mut() {
                    *v = shard_compose::srgb_to_linear(v.clamp(0.0, 1.0));
                }
                shard_compose::convert_colour(&mut e, &shard_compose::ACESCG_TO_SRGB);
                for v in e.data.iter_mut() {
                    *v = shard_compose::linear_to_srgb(v.clamp(0.0, 1.0));
                }
            }
            Some((e.w, e.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect::<Vec<u8>>()))
        });
        let rough = field(7, 1);
        let metal = field(13, 1);
        let height = field(1, 1);
        // the Normal channel is stored encoded (0..1) with its coverage: an unwritten texel is flat
        let stack_normal = c.texture_cache(uid, 22).map(|img| {
            let side = res.min(img.width);
            let raw = img.to_f32(4, side);
            let data: Vec<f32> = raw
                .chunks_exact(4)
                .flat_map(|p| {
                    let a = p[3].clamp(0.0, 1.0);
                    [0.5 + (p[0] - 0.5) * a, 0.5 + (p[1] - 0.5) * a, 1.0 + (p[2] - 1.0) * a]
                })
                .collect();
            shard_compose::Image { w: side, h: side, n: 3, data }
        });
        let normal = shard_engine::compose::assemble_job_normal(bitmaps.as_ref(), &urls, &name, res, height.as_ref(), stack_normal.as_ref(), Some((&mesh, set)))
            .map(|mut n| {
                let inner = crate::export::erode(&cov, res, res);
                crate::export::dilate_f32(&mut n.data, n.n, &inner, res, res, iters);
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                n.data.chunks_exact(n.n).flat_map(|p| [q(p[0]), q(p[1.min(n.n - 1)]), q(p[2.min(n.n - 1)]), 255]).collect::<Vec<u8>>()
            });
        // diagnostic: SHARD_CACHE_NORMAL_DIR=<plugin extraction> shows Painter's final normal
        // (final/<set>/NormalMixed.png, OpenGL: green flipped to the DirectX convention of the bakes) instead of
        // the assembled one
        let painter = std::env::var_os("SHARD_CACHE_NORMAL_DIR")
            .and_then(|d| shard_engine::oracle::read_png(&PathBuf::from(d).join("final").join(&name).join("NormalMixed.png")).ok())
            .filter(|img| img.n == 3);
        let normal = match painter {
            Some(img) => {
                eprintln!("[painter cache] set {set}: normal from SHARD_CACHE_NORMAL_DIR ({}x{})", img.w, img.h);
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                // bilinear to the view size
                let at = |x: f32, y: f32, c: usize| {
                    let (fx, fy) = ((x - 0.5).max(0.0), (y - 0.5).max(0.0));
                    let (x0, y0) = ((fx as u32).min(img.w - 1), (fy as u32).min(img.h - 1));
                    let (x1, y1) = ((x0 + 1).min(img.w - 1), (y0 + 1).min(img.h - 1));
                    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                    let p = |x: u32, y: u32| img.data[((y * img.w + x) * 3) as usize + c];
                    (p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx) * (1.0 - ty) + (p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx) * ty
                };
                let (sx, sy) = (img.w as f32 / res as f32, img.h as f32 / res as f32);
                let mut out = Vec::with_capacity((res * res * 4) as usize);
                for y in 0..res {
                    for x in 0..res {
                        let (u, v) = ((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                        out.extend_from_slice(&[q(at(u, v, 0)), q(1.0 - at(u, v, 1)), q(at(u, v, 2)), 255]);
                    }
                }
                Some(out)
            }
            None => normal,
        };
        if std::env::var_os("SHARD_TRACE").is_some() {
            eprintln!("[painter cache] set {set} '{name}' {res}x{res}: {:.2}s", t0.elapsed().as_secs_f32());
        }
        if tx.send(CacheLoad { set, gen, res: base_res, base, rough, metal, normal, emissive }).is_err() {
            return;
        }
    }
}

/// A channel image as the 8 bit data a channel view shows (colour channels sRGB-encoded here): 3 or 1 components.
fn quantise_view(img: &shard_compose::Image, srgb: bool) -> (u32, u32, u8, Vec<u8>) {
    use rayon::prelude::*;
    let comps = if img.n >= 3 { 3usize } else { 1 };
    let mut out = vec![0u8; (img.w as usize) * (img.h as usize) * comps];
    let q = |v: f32| ((if srgb { shard_compose::linear_to_srgb(v) } else { v.clamp(0.0, 1.0) }) * 255.0 + 0.5) as u8;
    out.par_chunks_mut(comps * 4096).enumerate().for_each(|(ci, chunk)| {
        for (k, px) in chunk.chunks_exact_mut(comps).enumerate() {
            let i = (ci * 4096 + k) * img.n;
            for (c, o) in px.iter_mut().enumerate() {
                *o = q(img.data[i + c]);
            }
        }
    });
    (img.w, img.h, comps as u8, out)
}

/// One undoable change; applying an entry returns the entry that reverts it.
pub enum UndoEntry {
    Paint { set: usize, tiles: Vec<crate::paint::TileSnap> },
    /// Layer stack of `set` as it was before the change named `label`.
    Layers { set: usize, layers: Vec<Layer>, session: usize, label: String },
}

impl UndoEntry {
    fn bytes(&self) -> usize {
        match self {
            UndoEntry::Paint { tiles, .. } => tiles.iter().map(|t| t.data.len()).sum(),
            // strokes and polygon-fill hits are shared with the live stack: only the nodes are new
            UndoEntry::Layers { layers, .. } => shard_model::edit::count_layers(layers) * 2048,
        }
    }

    /// Name of the edit this entry undoes (shown in the status bar and the history).
    pub fn label(&self) -> &str {
        match self {
            UndoEntry::Paint { .. } => "Paint stroke",
            UndoEntry::Layers { label, .. } => label,
        }
    }
}

/// Layer stack of one texture set (material). Each set has its own stack, like in Painter.
pub struct SetStack {
    pub name: String,
    pub layers: Vec<Layer>,
    /// Layer state last stored in the undo history (to detect and record changes).
    committed: Vec<Layer>,
    committed_session: usize,
    /// Number of layers added in this session at the top of `layers`.
    pub session_layers: usize,
    pub selected: Option<Vec<usize>>,
    pub collapsed: HashSet<Vec<usize>>,
}

impl SetStack {
    fn new(name: &str, layers: Vec<Layer>) -> Self {
        Self {
            name: name.to_string(),
            committed: layers.clone(),
            committed_session: 0,
            layers,
            session_layers: 0,
            selected: None,
            collapsed: HashSet::new(),
        }
    }
}

/// Resolution at which a texture set of `size` is composed for the viewport: its own, up to
/// `SHARD_VIEW_MAX_RES` (default 4096; the viewport used to stop at 1024 and looked soft on 2048 sets).
fn view_res(size: u32) -> u32 {
    let cap = std::env::var("SHARD_VIEW_MAX_RES").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(4096);
    size.clamp(64, cap.max(1024))
}

pub struct AppState {
    /// Diagnostic `SHARD_REOPEN="seconds;path"` (see `draw`) and the clock it counts from.
    reopen_spec: Option<String>,
    reopen_clock: std::time::Instant,
    /// File > Open found an autosave newer than the project: `(project, autosave)` waiting for the answer, asked
    /// in a window of the program (a native modal box froze it, and could hide behind it).
    recovery: Option<(PathBuf, PathBuf)>,
    pub scene: Scene,
    /// One stack per texture set (same order as `scene.texs`).
    pub stacks: Vec<SetStack>,
    pub doc_name: String,
    pub active_channel: u32,
    pub status: String,
    /// Free area the windows leave open over the viewport, in egui points (valid after `draw`): the scene
    /// is centred in it and the brush works in it.
    pub central: Rect,
    /// Post-processing (tone mapping + exposure) of the shaded viewport.
    pub post: crate::renderer::PostFx,
    /// Viewport statistics (frame times, FPS, GPU time, CPU / GPU usage).
    pub stats: crate::stats::Stats,
    /// Session log (seconds since start, message) and the last status logged.
    log: Vec<(f32, String)>,
    log_last: String,
    started: std::time::Instant,
    /// Viewport render mode: path traced (true) or real-time.
    pub path_trace: bool,
    /// The GPU can path trace; samples of the path-traced picture shown.
    pub path_trace_supported: bool,
    pub path_trace_samples: Option<u32>,
    /// Samples of the neural denoiser's picture on screen.
    pub path_trace_ai: Option<u32>,
    /// Post effects the project turns on that the viewport does not render yet (bloom, glare...).
    pub post_unrendered: Vec<&'static str>,
    /// `ao_intensity` and `emissive_intensity` of the project's shader (read at open).
    pub ao_intensity: f32,
    pub emissive_intensity: f32,
    /// Texture sets whose emissive map was sent to the viewport (a stack that stops writing it sends black).
    emissive_sent: HashSet<usize>,
    /// Environment rotation around the vertical axis, radians (the project's, then Shift + right drag or the
    /// viewport shading menu).
    pub env_yaw: f32,
    /// Exposure of the shaded view in stops (0 = as the project shows it).
    pub exposure_ev: f32,
    /// Draw the environment image behind the mesh.
    pub show_background: bool,
    /// Sample counts the GPU offers for the viewport's antialiasing (filled when the renderer starts).
    pub msaa_supported: Vec<u32>,
    /// The viewport shading popover (next to the render-mode spheres) is open.
    shading_open: bool,
    /// The environment picker (opened from the HDRI banner of that popover) is open; the banner's rect.
    hdri_picker_open: bool,
    hdri_banner_rect: Rect,
    /// Search text of the environment picker; focus it on the next frame.
    hdri_query: String,
    hdri_picker_focus: bool,
    /// Width of the viewport display pill in the last frame (to place it beside the toolbar when there is room).
    display_pill_w: f32,
    /// Set when the mesh changed; the renderer re-uploads vertex/index buffers.
    pub mesh_dirty: bool,
    /// Image-based lighting environment (the project's HDRI when available).
    pub env: crate::env::Environment,
    /// Set when `env` changed; the renderer re-uploads it.
    pub env_dirty: bool,
    /// Path of the open `.spp` (background refinement re-opens it on its own thread).
    project_path: Option<std::path::PathBuf>,
    /// The `.shard` file of this session (Ctrl+S writes here).
    pub shard_path: Option<std::path::PathBuf>,
    /// Layer copied with Ctrl+C / Ctrl+X.
    layer_clipboard: Option<Layer>,
    /// Layer being renamed in the panel: (path, text).
    renaming: Option<(Vec<usize>, String)>,
    /// Window aspect ratio of the painting session (see `strokes::estimate_aspect`).
    pub stroke_aspect: f32,
    refine_tx: std::sync::mpsc::Sender<JobResult>,
    refine_rx: std::sync::mpsc::Receiver<JobResult>,
    /// Pictures of the layers panel: the content / mask of each layer of a set on the shown channel,
    /// keyed `(set, layer uid, is_mask)`.
    thumb_tex: std::collections::HashMap<(usize, u64, bool), egui::TextureHandle>,
    thumb_tx: std::sync::mpsc::Sender<ThumbResult>,
    thumb_rx: std::sync::mpsc::Receiver<ThumbResult>,
    thumb_busy: bool,
    /// What `thumb_tex` was last made from: `(set, edit generation, channel, exact)`.
    thumb_done: Option<(usize, u64, u32, bool)>,
    /// The layers panel was drawn this recently: pictures are only worth computing while it is on screen.
    thumbs_wanted: std::time::Instant,
    /// Pending background compositions `(set, resolution, channel bits)`; one runs at a time.
    job_queue: std::collections::VecDeque<(usize, u32, Vec<u32>, Option<Vec<usize>>)>,
    /// Layer whose mask is shown in the viewport (Alt+click on its thumbnail): (set, path).
    mask_preview: Option<(usize, Vec<usize>)>,
    /// Layer whose mask content list is expanded in the layers panel.
    mask_open_ui: Option<Vec<usize>>,
    /// Masks evaluated by the last background composition of a set: `(set, layer path) -> (res, values)`.
    mask_cache: std::collections::HashMap<(usize, Vec<usize>), (u32, Vec<u8>)>,
    /// Interactive mask editing (Painter: the mask is a channel you can paint in).
    mask_edit: Option<MaskEdit>,
    /// Generation counter per set: results of outdated jobs are dropped.
    refine_gen: Vec<u64>,
    refine_pending: usize,
    /// Resolution of the colour job running on the worker (0 when idle): the layers' pictures wait only for
    /// the passes up to 1024, not for the long full-resolution ones.
    running_res: u32,
    /// Sets with a base composition running (at most one job per set; `SHARD_JOBS` jobs at a time).
    running_sets: Vec<usize>,
    /// Running passes of at most 1024 (the quick ones a full-resolution pass waits for).
    running_res_quick: usize,
    /// The "Open" dialog running on its own thread.
    open_dialog_rx: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
    /// Painter's cached textures of the open project, arriving set by set.
    cache_rx: Option<std::sync::mpsc::Receiver<CacheLoad>>,
    /// Sets showing Painter's cached picture, with the `refine_gen` it belongs to: compositions of that same
    /// generation (no edit since the file was opened) are not uploaded over it.
    cache_shown: std::collections::HashMap<usize, u64>,
    /// What the channel views show, per `(set, channel bit)`: `(pass resolution, width, height, components, bytes)`,
    /// filled by the background passes (the viewport never composes on the interface thread).
    view_cache: std::collections::HashMap<(usize, u32), (u32, u32, u32, u8, Vec<u8>)>,
    /// Passes already asked for to fill the cache: `(set, bit, resolution)`.
    view_requested: std::collections::HashSet<(usize, u32, u32)>,
    /// A mask preview job is running (own worker: never queued behind a base composition).
    mask_pending: usize,
    /// Per set: the background refinement matches the current layers.
    refine_done: Vec<bool>,
    /// A set waiting for the running refinement to finish.
    refine_wanted: Option<usize>,
    last_refine_check: usize,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    /// Counts every recorded edit (also undo / redo); autosave runs when it moved since the last save.
    edit_rev: u64,
    autosaved_rev: u64,
    last_autosave: std::time::Instant,
    autosave_slot: usize,
    autosave_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Time of the last layer-change entry (consecutive edits, e.g. a slider drag, are merged).
    last_layer_undo: Option<std::time::Instant>,
    /// User configuration (library locations...).
    pub config: crate::config::Config,
    /// Assets found in the configured libraries (the shelf).
    pub assets: Vec<crate::library::Asset>,
    settings_open: bool,
    /// Export Textures dialog: open, the texture sets ticked, the result of the last export.
    export_open: bool,
    export_sets: Vec<bool>,
    export_result: Option<String>,
    settings_tab: usize,
    asset_filter: Option<crate::library::AssetKind>,
    asset_query: String,
    /// `.exr` environments found in the HDRI folder, and the one in use (`None` = project / procedural).
    pub hdri_files: Vec<std::path::PathBuf>,
    pub hdri_current: Option<usize>,
    /// Previews of the HDRIs (None: being made, or the file could not be read), made by `hdri_thumb_worker`.
    hdri_thumbs: std::collections::HashMap<std::path::PathBuf, Option<egui::TextureHandle>>,
    hdri_thumb_tx: Option<std::sync::mpsc::Sender<std::path::PathBuf>>,
    hdri_thumb_rx: Option<std::sync::mpsc::Receiver<(std::path::PathBuf, Option<(u32, u32, Vec<u8>)>)>>,
    pub view: ViewKind,
    pub render_cmds: Vec<RenderCmd>,
    /// `.image` resource URLs of the project (mesh maps among them).
    bake_urls: Vec<String>,
    /// Mask effect shown in the properties dock: (layer path, effect position).
    props_sel: Option<(Vec<usize>, usize, bool)>,
    /// Open resource picker (the slot of a fill source was clicked).
    picker: Option<Picker>,
    /// Quick panel of the viewport's right click (colour, brush, recent colours and brushes).
    pub quick: quick::QuickPanel,
    /// Thumbnails of shelf textures (None = could not be decoded).
    thumbs: std::collections::HashMap<String, Thumb>,
    previews: crate::previews::PreviewLoader,
    /// Tool seen by the properties tab last frame (a tool change brings the tool's properties back).
    props_last_tool: Tool,
    /// Texture sets whose baked normal map was already sent to the renderer (it never changes).
    normal_sent: HashSet<usize>,
    /// Texture sets whose viewport normal map is the baked one combined with the stack's Height/Normal.
    stack_normal: HashSet<usize>,
    /// Diagnostic: textures exported by Painter are shown instead of the composed ones (`--painter-export`).
    pub painter_override: Option<String>,
    /// Baked AO per set with the island borders padded outwards (the bake has a black background).
    ao_padded: std::collections::HashMap<usize, std::sync::Arc<shard_compose::Image>>,
    /// Per set: did its normal bake use face normals (when decidable).
    face_votes: std::collections::HashMap<usize, bool>,
    /// Resolution/generation of the last result applied per (set, channel): older/lower ones are dropped.
    applied_res: std::collections::HashMap<(usize, u32), (u64, u32)>,
    /// A parameter of the open mask's generator/filter changed: re-evaluate the mask view.
    mask_reeval: bool,
    /// Full-resolution re-evaluation of the open mask is due at this time (low-res preview ran first).
    mask_full_due: Option<std::time::Instant>,
    /// Base-colour recomposition postponed while a mask is open (kept responsive); flushed when idle.
    defer_base: bool,
    force_base: bool,
    last_mask_change: std::time::Instant,
    /// Channel view needs rebuilding (throttled; it evaluates synchronously).
    view_dirty: bool,
    last_view_refresh: std::time::Instant,
    iface_cache: std::collections::HashMap<String, Option<std::sync::Arc<shard_formats::sbsar::Interface>>>,
    /// Bitmap resources of the open project (None for standalone presets).
    pub bitmaps: Option<crate::bitmaps::ProjectBitmaps>,
    /// Active texture set: the one painted and whose layer stack is shown.
    selected_set: usize,
    props_tab: usize,
    /// Arrangement of the windows (Properties, Assets, Layers, viewport...).
    dock: egui_dock::DockState<Pane>,
    /// Serialized form of `dock` last written to disk.
    layout_saved: String,
    /// Title of the Properties window: it follows the tool / the effect being edited.
    props_title: &'static str,
    /// Window to bring to front after the dock was shown (set while it is borrowed).
    pending_focus: Option<Pane>,
    /// Arrangement requested by the Window menu (reset / load) and applied after the dock was shown.
    pending_layout: Option<egui_dock::DockState<Pane>>,
    recompose: bool,
    pub quit: bool,
}

const CHANNEL_CHOICES: [u32; 6] = [0, 1, 7, 13, 22, 4];

/// Draws the windows of the dock with the application state.
struct PaneViewer<'a> {
    app: &'a mut AppState,
    /// The tab shown in each window: only those carry a close button (a cleaner tab bar).
    active: Vec<Pane>,
}

impl egui_dock::TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
        match tab {
            Pane::Properties => self.app.props_title.into(),
            p => p.title().into(),
        }
    }

    fn id(&mut self, tab: &mut Pane) -> egui::Id {
        egui::Id::new(("shard-pane", *tab))
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        self.app.pane_ui(ui, *tab);
    }

    fn closeable(&mut self, tab: &mut Pane) -> bool {
        self.active.contains(tab)
    }

    fn scroll_bars(&self, _tab: &Pane) -> [bool; 2] {
        [false, false]
    }
}

const BLEND_NAMES: [&str; 32] = [
    "Disable", "Replace", "Normal", "Darken", "Multiply", "ColorBurn", "LinearBurn", "Lighten", "Screen",
    "ColorDodge", "LinearDodge", "Overlay", "SoftLight", "HardLight", "VividLight", "LinearLight", "PinLight",
    "Difference", "Exclusion", "Tint", "Saturation", "Color", "Value", "Divide", "InverseDivide",
    "Passthrough", "NormalMapCombine", "NormalMapDetail", "NormalMapInverseDetail", "Subtract",
    "InverseSubtract", "SignedAddition",
];

/// What the viewport shows: the shaded material, one composited channel, or a baked mesh map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewKind {
    Material,
    Channel(u32),
    Bake(Bake),
}

pub use shard_compose::MeshMap as Bake;
use shard_engine::compose::{collect_key_u, compose_job, compose_job_masks, find_bake_url, layer_name_at, mask_job, SetMaps};

/// Painter's own default environment, "Studio Tomoco", from an installed Substance 3D Painter (or Designer).
/// `SHARD_DEFAULT_HDRI=<file.exr>` names another default, `SHARD_DEFAULT_HDRI=project` keeps the HDRI saved in
/// the project (or the procedural studio) instead.
pub fn studio_tomoco() -> Option<std::path::PathBuf> {
    if let Some(v) = std::env::var_os("SHARD_DEFAULT_HDRI") {
        if v.to_string_lossy().eq_ignore_ascii_case("project") {
            return None;
        }
        let p = std::path::PathBuf::from(v);
        return p.exists().then_some(p);
    }
    const CANDIDATES: [&str; 2] = [
        "C:/Program Files/Adobe/Adobe Substance 3D Painter/resources/starter_assets/environments/Studio/Studio Tomoco.exr",
        "C:/Program Files/Adobe/Adobe Substance 3D Designer/resources/view3d/maps/tomoco_studio.exr",
    ];
    CANDIDATES.iter().map(std::path::PathBuf::from).find(|p| p.exists())
}

/// Makes the HDRI previews asked for, one at a time: from the disk cache (`<config>/hdri_thumbs`, keyed by path, size
/// and date of the file) or read from the EXR (a few seconds for an 8K file) and stored there.
fn hdri_thumb_worker(
    rx: std::sync::mpsc::Receiver<std::path::PathBuf>,
    tx: std::sync::mpsc::Sender<(std::path::PathBuf, Option<(u32, u32, Vec<u8>)>)>,
    ctx: egui::Context,
) {
    crate::lower_thread_priority();
    let dir = crate::config::config_dir().map(|d| d.join("hdri_thumbs"));
    for path in rx {
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut h);
            if let Ok(m) = std::fs::metadata(&path) {
                m.len().hash(&mut h);
                m.modified().ok().hash(&mut h);
            }
            h.finish()
        };
        let file = dir.as_ref().map(|d| d.join(format!("{key:016x}.png")));
        let img = file.as_deref().and_then(read_rgba_png).or_else(|| {
            let r = crate::env::exr_preview(&path, 320).ok();
            if let (Some((w, h, px)), Some(f)) = (&r, &file) {
                if let Some(d) = f.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = crate::paint::save_png(f, *w, *h, px);
            }
            r
        });
        if tx.send((path, img)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// An 8-bit PNG as RGBA.
fn read_rgba_png(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let f = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let px = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        _ => return None,
    };
    Some((info.width, info.height, px))
}

/// Caption of a group of post effects.
fn effect_category(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).font(theme::semibold(9.5)).color(theme::TEXT_DISABLED));
    ui.add_space(2.0);
}

/// One post effect in its own box: name (and a hint) with its switch; its settings below while it is on.
fn effect_box(ui: &mut egui::Ui, name: &str, hint: &str, on: &mut bool, body: impl FnOnce(&mut egui::Ui)) {
    let active = *on;
    egui::Frame::new()
        .fill(Color32::from_gray(33))
        .stroke(Stroke::new(1.0, if active { theme::accent_soft() } else { theme::BORDER }))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(name).font(theme::semibold(12.0)).color(if active { Color32::WHITE } else { theme::TEXT }));
                    if !hint.is_empty() {
                        ui.label(RichText::new(hint).size(10.0).color(theme::TEXT_DIM));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::switch(ui, *on, true).clicked() {
                        *on = !*on;
                    }
                });
            });
            if *on {
                ui.add_space(2.0);
                body(ui);
            }
        });
    ui.add_space(6.0);
}

/// An HDRI preview (2:1) painted into `r`, cropped to its aspect around the horizon.
fn paint_hdri_thumb(ui: &egui::Ui, t: &egui::TextureHandle, r: Rect, radius: u8) {
    let v = (2.0 * r.height() / r.width()).min(1.0);
    let uv = Rect::from_min_max(pos2(0.0, 0.5 - v * 0.5), pos2(1.0, 0.5 + v * 0.5));
    egui::Image::new((t.id(), r.size())).uv(uv).corner_radius(radius).paint_at(ui, r);
}

/// Painter's post effects that need image passes (`editor/posteffects.ini` key, display name): listed in the viewport
/// shading menu, not rendered yet.
const UNRENDERED_POST: [(&str, &str); 6] = [
    ("glareParametersV2", "Glare"),
    ("sharpenParametersV2", "Sharpen"),
    ("dofParametersV2", "Depth of field"),
    ("lateralAberrationParametersV2", "Chromatic aberration"),
    ("lensFlareParametersV2", "Lens flare"),
    ("colorGradingParametersV2", "Color grading (LUT)"),
];

/// `.exr` files of the HDRI folder (`SHARD_HDRI_DIR`, default `~/Documents/HDRI`), sorted by name, after
/// Painter's default environment when it is installed.
fn scan_hdri_dir() -> Vec<std::path::PathBuf> {
    let dir = std::env::var_os("SHARD_HDRI_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join("Documents").join("HDRI")));
    let mut out: Vec<std::path::PathBuf> = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("exr")))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    if let Some(t) = studio_tomoco() {
        out.retain(|p| p != &t);
        out.insert(0, t);
    }
    out
}

fn doc_for(st: &SetStack) -> Document {
    shard_engine::compose::document(&st.name, &st.layers)
}

/// Roughness / metallic feeding the shaded material: constants or per-texel maps.
enum AuxSource {
    Const(f32, f32),
    Maps(shard_compose::Image, shard_compose::Image),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewGroup {
    Channels,
    MeshMaps,
}

/// Entries of the viewport channel selector, in display order.
pub const VIEW_CHOICES: [(ViewKind, &str); 12] = [
    (ViewKind::Material, "Material"),
    (ViewKind::Channel(0), "Base color"),
    (ViewKind::Channel(7), "Roughness"),
    (ViewKind::Channel(13), "Metallic"),
    (ViewKind::Channel(1), "Height"),
    (ViewKind::Channel(22), "Normal"),
    (ViewKind::Channel(4), "Emissive"),
    (ViewKind::Bake(Bake::AmbientOcclusion), "Ambient occlusion"),
    (ViewKind::Bake(Bake::Curvature), "Curvature"),
    (ViewKind::Bake(Bake::NormalBase), "Normal map from mesh"),
    (ViewKind::Bake(Bake::WorldNormal), "World space normal"),
    (ViewKind::Bake(Bake::Position), "Position"),
];

/// Work for the renderer queued by the UI state (drained once per frame by the host).
pub enum RenderCmd {
    /// Ambient-occlusion map `(set, w, h, rgba)` used by the shaded material.
    SetAux(usize, u32, u32, Vec<u8>),
    /// Per-set unlit images, or `None` to go back to the shaded material.
    SetView(Option<Vec<(u32, u32, Vec<u8>)>>),
    /// Baked tangent-space normal map `(set, w, h, rgba)` used by the shaded material.
    SetNormal(usize, u32, u32, Vec<u8>),
    /// Shade with face normals (the bake used them) instead of the authored vertex normals.
    FaceNormals(bool),
    /// Emissive colour map `(set, w, h, rgba)` (sRGB); 1x1 black = none.
    SetEmissive(usize, u32, u32, Vec<u8>),
}

fn image_rgba8(img: &shard_compose::Image) -> Vec<u8> {
    image_rgba8_enc(img, false)
}

/// RGBA8 of a float image; `srgb` encodes linear colour values for display.
fn image_rgba8_enc(img: &shard_compose::Image, srgb: bool) -> Vec<u8> {
    let q = |v: f32| ((if srgb { shard_compose::linear_to_srgb(v) } else { v.clamp(0.0, 1.0) }) * 255.0 + 0.5) as u8;
    img.data
        .chunks_exact(img.n)
        .flat_map(|p| {
            if img.n >= 3 {
                [q(p[0]), q(p[1]), q(p[2]), 255]
            } else {
                [q(p[0]), q(p[0]), q(p[0]), 255]
            }
        })
        .collect()
}

impl AppState {
    pub fn new() -> Self {
        let mut st = Self::blank();
        st.config = crate::config::Config::load();
        if let Some(h) = st.config.highlight {
            theme::set_accent(h);
        }
        shard_engine::oracle::set_enabled(!st.config.substance_engine_off);
        st.rescan_libraries();
        st
    }

    fn blank() -> Self {
        let (refine_tx, refine_rx) = std::sync::mpsc::channel();
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        Self {
            project_path: None,
            shard_path: None,
            layer_clipboard: None,
            renaming: None,
            stroke_aspect: 1.0,
            refine_tx,
            refine_rx,
            thumb_tex: std::collections::HashMap::new(),
            thumb_tx,
            thumb_rx,
            thumb_busy: false,
            thumb_done: None,
            thumbs_wanted: std::time::Instant::now() - std::time::Duration::from_secs(60),
            refine_gen: Vec::new(),
            refine_pending: 0,
            running_res: 0,
            running_sets: Vec::new(),
            running_res_quick: 0,
            open_dialog_rx: None,
            cache_rx: None,
            cache_shown: std::collections::HashMap::new(),
            view_cache: Default::default(),
            view_requested: Default::default(),
            mask_pending: 0,
            job_queue: std::collections::VecDeque::new(),
            mask_preview: None,
            mask_open_ui: None,
            mask_cache: std::collections::HashMap::new(),
            mask_edit: None,
            refine_done: Vec::new(),
            refine_wanted: None,
            last_refine_check: usize::MAX,
            scene: Scene::new(),
            stacks: vec![SetStack::new("Material", Vec::new())],
            doc_name: "Untitled".into(),
            active_channel: 0,
            status: "Ready".into(),
            central: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)),
            post: crate::renderer::PostFx::default(),
            stats: crate::stats::Stats::default(),
            log: Vec::new(),
            log_last: String::new(),
            started: std::time::Instant::now(),
            path_trace: false,
            path_trace_supported: false,
            path_trace_samples: None,
            path_trace_ai: None,
            post_unrendered: Vec::new(),
            ao_intensity: 1.0,
            emissive_intensity: 1.0,
            emissive_sent: HashSet::new(),
            env_yaw: 0.0,
            exposure_ev: 0.0,
            show_background: false,
            msaa_supported: vec![1],
            // SHARD_SHADING_OPEN=1: start with the viewport shading menu open (screenshots)
            shading_open: std::env::var_os("SHARD_SHADING_OPEN").is_some(),
            display_pill_w: 330.0,
            // SHARD_SHADING_OPEN=picker: the environment picker open too
            hdri_picker_open: std::env::var("SHARD_SHADING_OPEN").is_ok_and(|v| v == "picker"),
            hdri_banner_rect: Rect::NOTHING,
            hdri_query: String::new(),
            hdri_picker_focus: false,
            mesh_dirty: false,
            env: crate::env::Environment::procedural(),
            env_dirty: false,
            undo_stack: Vec::new(),
            edit_rev: 0,
            autosaved_rev: 0,
            last_autosave: std::time::Instant::now(),
            autosave_slot: 0,
            autosave_busy: Default::default(),
            redo_stack: Vec::new(),
            last_layer_undo: None,
            config: crate::config::Config::default(),
            assets: Vec::new(),
            // SHARD_SETTINGS_TAB=n: start with Settings open on page n (screenshots)
            settings_open: std::env::var_os("SHARD_SETTINGS_TAB").is_some(),
            // SHARD_EXPORT_OPEN=1: start with the Export Textures dialog open (screenshots)
            export_open: std::env::var_os("SHARD_EXPORT_OPEN").is_some(),
            export_sets: Vec::new(),
            export_result: None,
            settings_tab: std::env::var("SHARD_SETTINGS_TAB").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            asset_filter: None,
            asset_query: String::new(),
            hdri_files: scan_hdri_dir(),
            hdri_current: None,
            hdri_thumbs: std::collections::HashMap::new(),
            hdri_thumb_tx: None,
            hdri_thumb_rx: None,
            view: ViewKind::Material,
            render_cmds: Vec::new(),
            bake_urls: Vec::new(),
            props_sel: None,
            picker: None,
            quick: Default::default(),
            thumbs: std::collections::HashMap::new(),
            previews: crate::previews::PreviewLoader::new(),
            props_last_tool: Tool::Paint,
            normal_sent: HashSet::new(),
            stack_normal: HashSet::new(),
            painter_override: None,
            ao_padded: std::collections::HashMap::new(),
            face_votes: std::collections::HashMap::new(),
            applied_res: std::collections::HashMap::new(),
            mask_reeval: false,
            mask_full_due: None,
            defer_base: false,
            force_base: false,
            recovery: None,
            reopen_spec: std::env::var("SHARD_REOPEN").ok(),
            reopen_clock: std::time::Instant::now(),
            last_mask_change: std::time::Instant::now(),
            view_dirty: false,
            last_view_refresh: std::time::Instant::now(),
            iface_cache: std::collections::HashMap::new(),
            bitmaps: None,
            selected_set: 0,
            props_tab: 0,
            dock: {
                let mut d = dock::load_layout().unwrap_or_else(dock::default_layout);
                dock::dock_floating(&mut d);
                d
            },
            layout_saved: String::new(),
            props_title: "Properties",
            pending_focus: None,
            pending_layout: None,
            recompose: false,
            quit: false,
        }
    }

    /// Index of the active texture set (always valid).
    pub fn active(&self) -> usize {
        self.selected_set.min(self.stacks.len().saturating_sub(1))
    }

    /// Keep exactly one stack per texture set of the current mesh.
    fn sync_stacks(&mut self) {
        let n = self.scene.texs.len();
        while self.stacks.len() < n {
            let name = self.scene.mesh.set_names.get(self.stacks.len()).cloned().unwrap_or_default();
            self.stacks.push(SetStack::new(&name, Vec::new()));
        }
        self.stacks.truncate(n.max(1));
    }

    /// Save the session as a ShardStroke project: layer stacks, hand-painted overlays and the view, on top of
    /// the base scene (the Painter project or mesh it was opened from).
    pub fn save_project(&mut self, path: &std::path::Path) -> Result<(), String> {
        let pf = self.build_project_file(path);
        shard_engine::persist::save(path, &pf)?;
        self.shard_path = Some(path.to_path_buf());
        self.doc_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string();
        self.autosaved_rev = self.edit_rev;
        self.status = format!("Saved {}", path.display());
        Ok(())
    }

    /// The session as a project file for `path` (the base scene is stored relative to it).
    fn build_project_file(&self, path: &std::path::Path) -> shard_engine::persist::ProjectFile {
        use shard_engine::persist;
        let (kind, base_path) = if let Some(p) = &self.project_path {
            ("spp", p.clone())
        } else if let Some(p) = &self.scene.mesh_path {
            ("obj", p.clone())
        } else {
            ("none", PathBuf::new())
        };
        let base = if kind == "none" {
            persist::Base { kind: "none".into(), ..Default::default() }
        } else {
            persist::Base::of(kind, &base_path, path)
        };
        let overlays = self
            .scene
            .texs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.has_paint())
            .map(|(set, t)| {
                let (rgba, w, h) = t.overlay();
                persist::PaintOverlay { set, w, h, rgba: rgba.to_vec() }
            })
            .collect();
        let cam = &self.scene.cam;
        let b = &self.scene.brush;
        let view = serde_json::json!({
            "camera": { "yaw": cam.yaw, "pitch": cam.pitch, "dist": cam.dist, "target": [cam.target.x, cam.target.y, cam.target.z] },
            "brush": { "radius": b.radius, "hardness": b.hardness, "flow": b.flow, "opacity": b.opacity, "spacing": b.spacing, "color": b.color },
            "active_set": self.selected_set,
        });
        let pf = persist::ProjectFile {
            manifest: persist::Manifest {
                format: persist::FORMAT.into(),
                version: persist::VERSION,
                app: format!("shard-studio {}", env!("CARGO_PKG_VERSION")),
                name: path.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string(),
                base,
                sets: self.stacks.iter().map(|s| s.name.clone()).collect(),
            },
            sets: self
                .stacks
                .iter()
                .map(|s| persist::SetDoc { name: s.name.clone(), layers: s.layers.clone(), session_layers: s.session_layers })
                .collect(),
            overlays,
            view,
        };
        pf
    }

    // ------------------------------------------------------------ autosave

    /// Where `slot` of the autosave of this session lives: next to the project (or mesh) file.
    fn autosave_path(&self, slot: usize) -> Option<PathBuf> {
        let base = self
            .shard_path
            .as_ref()
            .or(self.project_path.as_ref())
            .or(self.scene.mesh_path.as_ref())
            .cloned();
        let (dir, stem) = match base {
            Some(p) => (
                p.parent()?.to_path_buf(),
                p.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string(),
            ),
            None => return None,
        };
        Some(dir.join(format!("{stem}_autosave_{slot}.shard")))
    }

    /// Write the autosave when the project was edited since the last save and the interval passed. The
    /// project file is assembled here (a clone of the stacks and overlays) and written on a worker thread.
    fn autosave_tick(&mut self) {
        let secs = self.config.autosave_secs;
        if secs == 0 || self.edit_rev == self.autosaved_rev || self.last_autosave.elapsed().as_secs() < secs as u64 {
            return;
        }
        if self.autosave_busy.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let slot = self.autosave_slot;
        let Some(path) = self.autosave_path(slot) else {
            return;
        };
        let pf = self.build_project_file(&path);
        self.autosave_slot = (slot + 1) % 2;
        self.autosaved_rev = self.edit_rev;
        self.last_autosave = std::time::Instant::now();
        let busy = self.autosave_busy.clone();
        busy.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::spawn(move || {
            crate::lower_thread_priority();
            if let Err(e) = shard_engine::persist::save(&path, &pf) {
                eprintln!("autosave failed: {e}");
            }
            busy.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }

    /// Ask whether to open the autosave of `path` instead, when one is newer than the file.
    pub fn offer_recovery(path: PathBuf) -> PathBuf {
        let Some(auto) = Self::newer_autosave(&path) else { return path };
        let age = std::fs::metadata(&auto)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
            .map(|d| {
                let m = d.as_secs() / 60;
                if m < 90 { format!("{m} min ago") } else { format!("{} h ago", m / 60) }
            })
            .unwrap_or_default();
        let yes = rfd::MessageDialog::new()
            .set_title("Autosave found")
            .set_description(format!(
                "{} has a newer autosave ({age}). Open the autosave instead?",
                path.file_name().and_then(|s| s.to_str()).unwrap_or("This project")
            ))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if matches!(yes, rfd::MessageDialogResult::Yes) {
            auto
        } else {
            path
        }
    }

    /// Open `path`, asking first (in the program's own window) when an autosave newer than it exists.
    fn request_open(&mut self, path: PathBuf) {
        match Self::newer_autosave(&path) {
            Some(auto) if std::env::var_os("SHARD_NO_PROMPT").is_none() => self.recovery = Some((path, auto)),
            _ => self.open_document(&path),
        }
    }

    fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some((orig, auto)) = self.recovery.clone() else { return };
        let age = std::fs::metadata(&auto)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
            .map(|d| {
                let m = d.as_secs() / 60;
                if m < 90 { format!("{m} min ago") } else { format!("{} h ago", m / 60) }
            })
            .unwrap_or_default();
        let mut choice: Option<bool> = None;
        egui::Window::new("Autosave found")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} has a newer autosave ({age}). Open the autosave instead?",
                    orig.file_name().and_then(|s| s.to_str()).unwrap_or("This project")
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Open the autosave").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Open the project").clicked() {
                        choice = Some(false);
                    }
                    if ui.button("Cancel").clicked() {
                        self.recovery = None;
                    }
                });
            });
        if let Some(use_auto) = choice {
            self.recovery = None;
            let p = if use_auto { auto } else { orig };
            self.open_document(&p);
        }
    }

    /// The newest autosave of `path` that is newer than the file itself.
    pub fn newer_autosave(path: &std::path::Path) -> Option<PathBuf> {
        let dir = path.parent()?;
        let stem = path.file_stem()?.to_str()?;
        let own = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        (0..2)
            .map(|k| dir.join(format!("{stem}_autosave_{k}.shard")))
            .filter_map(|p| {
                let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
                (t > own + std::time::Duration::from_secs(2)).then_some((t, p))
            })
            .max_by_key(|(t, _)| *t)
            .map(|(_, p)| p)
    }

    /// Ctrl+S: save to the project's file, asking for one the first time.
    pub fn save_current(&mut self, ask: bool) {
        let target = match (&self.shard_path, ask) {
            (Some(p), false) => Some(p.clone()),
            _ => {
                let mut d = rfd::FileDialog::new().add_filter("ShardStroke project", &["shard"]);
                if let Some(p) = self.project_path.as_ref().or(self.scene.mesh_path.as_ref()) {
                    if let Some(dir) = p.parent() {
                        d = d.set_directory(dir);
                    }
                    d = d.set_file_name(format!("{}.shard", p.file_stem().and_then(|s| s.to_str()).unwrap_or("project")));
                } else {
                    d = d.set_file_name("project.shard");
                }
                d.save_file()
            }
        };
        if let Some(mut p) = target {
            if p.extension().is_none() {
                p.set_extension("shard");
            }
            if let Err(e) = self.save_project(&p) {
                self.status = format!("Save failed: {e}");
            }
        }
    }

    /// Open a ShardStroke project: the base scene first, then the saved stacks, overlays and view.
    fn open_shard(&mut self, path: &std::path::Path) {
        use shard_engine::persist;
        let pf = match persist::load(path) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Open failed: {e}");
                return;
            }
        };
        let base = pf.manifest.base.clone();
        match base.kind.as_str() {
            "spp" | "obj" => {
                let Some(bp) = base.resolve(path) else {
                    self.status = format!("Open failed: base file not found: {}", base.path);
                    return;
                };
                if base.kind == "spp" {
                    self.open_document(&bp);
                } else {
                    match self.scene.load_mesh(&bp) {
                        Ok(()) => {
                            self.forget_jobs();
                            self.mesh_dirty = true;
                            self.project_path = None;
                            self.stacks.clear();
                            self.sync_stacks();
                        }
                        Err(e) => self.status = format!("Open failed: {e}"),
                    }
                }
                if self.status.starts_with("Open failed") {
                    return;
                }
                if std::fs::metadata(&bp).map(|m| m.len()).unwrap_or(0) != base.len {
                    log_note(&format!("base file {} changed since the project was saved", bp.display()));
                }
            }
            _ => {}
        }
        // saved stacks replace the base's: Painter's cached picture of the base no longer applies
        self.cache_rx = None;
        self.cache_shown.clear();
        let mut applied = 0;
        for sd in &pf.sets {
            if let Some(i) = self.stacks.iter().position(|s| s.name == sd.name) {
                self.stacks[i] = SetStack::new(&sd.name, sd.layers.clone());
                self.stacks[i].session_layers = sd.session_layers;
                applied += 1;
            }
        }
        for o in &pf.overlays {
            if let Some(t) = self.scene.texs.get_mut(o.set) {
                t.set_overlay(o.w, o.h, &o.rgba);
            }
        }
        let f = |v: &serde_json::Value| v.as_f64().map(|x| x as f32);
        let cam = &pf.view["camera"];
        if let (Some(y), Some(p), Some(d)) = (f(&cam["yaw"]), f(&cam["pitch"]), f(&cam["dist"])) {
            self.scene.cam.yaw = y;
            self.scene.cam.pitch = p;
            self.scene.cam.dist = d;
            if let Some(t) = cam["target"].as_array() {
                if t.len() == 3 {
                    self.scene.cam.target = glam::Vec3::new(
                        f(&t[0]).unwrap_or(0.0),
                        f(&t[1]).unwrap_or(0.0),
                        f(&t[2]).unwrap_or(0.0),
                    );
                }
            }
        }
        let br = &pf.view["brush"];
        let b = &mut self.scene.brush;
        for (dst, key) in [
            (&mut b.radius, "radius"),
            (&mut b.hardness, "hardness"),
            (&mut b.flow, "flow"),
            (&mut b.opacity, "opacity"),
            (&mut b.spacing, "spacing"),
        ] {
            if let Some(v) = f(&br[key]) {
                *dst = v;
            }
        }
        if let Some(c) = br["color"].as_array() {
            if c.len() == 3 {
                b.color = [f(&c[0]).unwrap_or(0.0), f(&c[1]).unwrap_or(0.0), f(&c[2]).unwrap_or(0.0)];
            }
        }
        if let Some(s) = pf.view["active_set"].as_u64() {
            if (s as usize) < self.stacks.len() {
                self.selected_set = s as usize;
            }
        }
        self.shard_path = Some(path.to_path_buf());
        self.doc_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string();
        self.clear_undo();
        self.mark_recompose();
        self.do_recompose(None);
        self.status = format!("Opened {} ({applied} texture set stack(s) restored)", path.display());
    }

    pub fn open_document(&mut self, path: &PathBuf) {
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("shard") || e.eq_ignore_ascii_case("pigma")) {
            self.open_shard(path);
            return;
        }
        let is_spp = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("spp")).unwrap_or(false);
        if is_spp {
            let stacks = match shard_model::import::import_project_stacks(path) {
                Ok(s) => s,
                Err(e) => {
                    self.status = format!("Open failed: {e}");
                    return;
                }
            };
            self.forget_jobs();
            self.doc_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string();
            let mesh_status = match self.scene.load_project_mesh(path) {
                Ok(n) => format!("{n} meshes"),
                Err(e) => format!("mesh: {e}"),
            };
            self.scene.cam = crate::scene::Camera::new();
            self.mesh_dirty = true;
            self.env = crate::env::Environment::from_project(path).unwrap_or_else(crate::env::Environment::procedural);
            self.env_dirty = true;
            // the viewport lights the model like Painter's default does (Studio Tomoco)
            self.use_default_environment();
            self.apply_project_display(path);
            self.bitmaps = crate::bitmaps::ProjectBitmaps::open(path, 1024);
            self.project_path = Some(path.clone());
            self.bake_urls = self.bitmaps.as_ref().map(|b| b.resource_urls()).unwrap_or_default();
            for st in &stacks {
                self.bake_urls.extend(shard_engine::compose::assignment_records(&st.name, &st.mesh_maps));
            }
            self.normal_sent.clear();
            self.stack_normal.clear();
            self.ao_padded.clear();
            self.face_votes.clear();
            self.applied_res.clear();
            // one stack per scene texture set, matched by material name; resolution from the project
            let names: Vec<String> = self.scene.mesh.set_names.clone();
            let res: Vec<(u32, u32)> = names
                .iter()
                .map(|n| stacks.iter().find(|s| &s.name == n).map(|s| s.resolution).unwrap_or((1024, 1024)))
                .collect();
            self.scene.set_resolutions(&res);
            self.stacks = names
                .iter()
                .map(|n| {
                    let layers = stacks.iter().find(|s| &s.name == n).map(|s| s.layers.clone()).unwrap_or_default();
                    SetStack::new(n, layers)
                })
                .collect();
            if self.stacks.is_empty() {
                self.stacks.push(SetStack::new("Material", Vec::new()));
            }
            self.selected_set = 0;
            {
                let mut us = Vec::new();
                for st in &self.stacks {
                    collect_key_u(&st.layers, &mut us);
                }
                self.stroke_aspect = crate::strokes::estimate_aspect(us);
            }
            self.status = format!("Opened {} ({mesh_status}, {} texture sets)", path.display(), self.stacks.len());
            self.do_recompose(None); // base colours now, so sets start as flat 1x1 GPU textures
            // Painter's own picture of every set (its texture cache) is shown while the stack is untouched: what the
            // file looks like in Painter, at once. SHARD_NO_PAINTER_CACHE=1 shows only our composition.
            self.cache_shown.clear();
            self.cache_rx = None;
            if std::env::var_os("SHARD_NO_PAINTER_CACHE").is_none() {
                let sets: Vec<(usize, u64, String, u32, u64)> = (0..self.stacks.len().min(self.scene.texs.len()))
                    .filter_map(|i| {
                        let st = stacks.iter().find(|s| s.name == self.stacks[i].name)?;
                        (st.uid != 0).then(|| {
                            (i, st.uid, st.name.clone(), self.scene.texs[i].w, self.refine_gen.get(i).copied().unwrap_or(0))
                        })
                    })
                    .collect();
                if !sets.is_empty() {
                    // up to three readers (a 4K set takes a few seconds to decode, pad and assemble): the active set
                    // goes first in the first reader
                    let (tx, rx) = std::sync::mpsc::channel();
                    let active = self.active();
                    let mut sets = sets;
                    sets.sort_by_key(|s| s.0 != active);
                    let n = sets.len().min(3);
                    let mut parts: Vec<Vec<(usize, u64, String, u32, u64)>> = vec![Vec::new(); n];
                    for (k, s) in sets.into_iter().enumerate() {
                        parts[k % n].push(s);
                    }
                    for part in parts {
                        let (p, mesh, urls, tx) = (path.clone(), self.scene.mesh.clone(), self.bake_urls.clone(), tx.clone());
                        std::thread::spawn(move || load_painter_cache(p, mesh, urls, part, tx));
                    }
                    self.cache_rx = Some(rx);
                }
            }
            self.view = ViewKind::Material;
            self.clear_undo();
            self.render_cmds.push(RenderCmd::SetView(None));
            return;
        }
        match shard_model::import::import_file(path) {
            Ok(d) => {
                // smart material / preset: its layers go to the active texture set
                self.doc_name = d.name.clone();
                self.sync_stacks();
                let a = self.active();
                let (old, old_session) = (self.stacks[a].layers.clone(), self.stacks[a].session_layers);
                self.stacks[a] = SetStack::new(&self.stacks[a].name.clone(), d.layers);
                self.stacks[a].committed = old;
                self.stacks[a].committed_session = old_session;
                self.status = format!("Opened {} into texture set '{}'", path.display(), self.stacks[a].name);
                self.mark_recompose();
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    /// The project's own display settings (Painter's Display Settings, stored with it): tone mapping, the contrast of
    /// the colour correction and the shader's `ao_intensity`. The viewport shows the project as Painter does.
    fn apply_project_display(&mut self, path: &std::path::Path) {
        self.post = crate::renderer::PostFx::default();
        self.post_unrendered.clear();
        self.ao_intensity = 1.0;
        self.emissive_intensity = 1.0;
        self.env_yaw = 0.0;
        let Ok(mut c) = shard_formats::Container::open(path) else { return };
        if let Ok(v) = c.decode("editor/posteffects.ini") {
            let j = v.to_json();
            let on = |o: &serde_json::Value| o["enable"].as_bool().unwrap_or(false);
            let f = |o: &serde_json::Value, k: &str, d: f32| o[k].as_f64().map(|v| v as f32).unwrap_or(d);
            let p = &mut self.post;
            p.enabled = j["enable"].as_bool().unwrap_or(true);
            p.tone_mapping = on(&j["toneMappingParametersV2"]);
            let cc = &j["colorCorrectionParametersV2"];
            p.color_correction = on(cc);
            p.brightness = f(cc, "brightness", 0.0);
            p.contrast = f(cc, "contrast", 1.0);
            p.gamma = f(cc, "gamma", 1.0);
            p.saturation = f(cc, "saturation", 0.0);
            p.temperature = f(cc, "temperature", 0.0);
            p.magenta_green = f(cc, "magentagreen", 0.0);
            let vg = &j["vignetteParametersV2"];
            p.vignette = on(vg);
            p.vignette_level = f(vg, "level", 1.0);
            p.vignette_radius = f(vg, "radius", 0.5);
            p.vignette_feather = f(vg, "feather", 0.5);
            p.vignette_ratio = f(vg, "ratio", 1.5);
            let fg = &j["filmGrainParametersV2"];
            p.grain = on(fg);
            p.grain_level = f(fg, "level", 0.01);
            let bl = &j["bloomParametersV2"];
            p.bloom = on(bl);
            p.bloom_level = f(bl, "level", 1.0);
            p.bloom_threshold = f(bl, "softThreshold", 1.5);
            p.bloom_falloff = f(bl, "falloff", 0.8);
            for (key, name) in UNRENDERED_POST {
                if p.enabled && on(&j[key]) {
                    self.post_unrendered.push(name);
                }
            }
        }
        if let Ok(v) = c.decode("editor/viewersettings2.ini") {
            let j = v.to_json();
            // Environment Rotation: stored as a fraction of a turn (a test screen's 1.0 shows 360 in Painter's
            // Display Settings); SHARD_ENV_ROT_SIGN=-1 turns it the other way
            if let Some(rot) = j["materialSettings"]["cubemapRotation"].as_f64() {
                let sign = std::env::var("SHARD_ENV_ROT_SIGN").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
                self.env_yaw = sign * rot as f32 * std::f32::consts::TAU;
            }
            let tweaks = j["materialSettings"]["shaders"][0]["tweaks"].as_array().cloned().unwrap_or_default();
            for t in tweaks {
                if t["identifier"].as_str() == Some("ao_intensity") {
                    self.ao_intensity = t["value"].as_f64().unwrap_or(1.0) as f32;
                }
                if t["identifier"].as_str() == Some("emissive_intensity") {
                    self.emissive_intensity = t["value"].as_f64().unwrap_or(1.0) as f32;
                }
            }
        }
    }

    /// Recompute the base colour of every texture set from its own layer stack. When a stack
    /// contains constructs the compositor cannot evaluate yet (bitmaps, procedurals, filters...),
    /// only the layers added in this session are evaluated, over a neutral grey base.
    /// Ask for a recomposition of the active set at the end of the frame (SHARD_TRACE logs who asked).
    #[track_caller]
    fn mark_recompose(&mut self) {
        if std::env::var_os("SHARD_TRACE").is_some() && !self.recompose {
            eprintln!("[recompose asked] {:.1}s by {}", self.reopen_clock.elapsed().as_secs_f32(), std::panic::Location::caller());
        }
        self.recompose = true;
    }

    #[track_caller]
    fn do_recompose(&mut self, only: Option<usize>) {
        if std::env::var_os("SHARD_TRACE").is_some() {
            eprintln!("[recompose] {:.1}s only={only:?} from {}", self.reopen_clock.elapsed().as_secs_f32(), std::panic::Location::caller());
        }
        self.sync_stacks();
        let mut unsupported_active = 0usize;
        let active = self.active();
        for i in 0..self.stacks.len().min(self.scene.texs.len()) {
            if only.is_some_and(|o| o != i) {
                continue;
            }
            let st = &self.stacks[i];
            let doc = Document {
                kind: DocKind::Project,
                name: st.name.clone(),
                root_class: String::new(),
                layers: st.layers.clone(),
            };
            let full = shard_compose::compose(&doc, 4, 4, &[0]);
            let color = if full.unsupported.is_empty() && !st.layers.is_empty() {
                let p = full.channels[&0].pixel(0, 0);
                Some([p[0], p[1], p[2]])
            } else if st.layers.is_empty() {
                let eff = Document { layers: vec![new_fill_layer("Base", [0.62, 0.62, 0.64])], ..doc };
                let c = shard_compose::compose(&eff, 4, 4, &[0]);
                let p = c.channels[&0].pixel(0, 0);
                Some([p[0], p[1], p[2]])
            } else {
                None // evaluated spatially below
            };
            if i == active {
                unsupported_active = full.unsupported.len();
            }
            // while a mask is open the base colour is not shown: do not compete with the mask preview
            if self.mask_preview.is_some() && !self.force_base {
                self.defer_base = true;
                self.last_mask_change = std::time::Instant::now();
                continue;
            }
            // viewport resolution follows the texture size of the set (capped for interactivity)
            let size = self.scene.texs[i].w;
            let res = size.clamp(64, 1024);
            match color {
                Some(c) => {
                    self.scene.texs[i].set_base_solid(c.map(shard_compose::linear_to_srgb));
                    // roughness / metallic of a fully supported stack are constants
                    let rm = shard_compose::compose(&doc_for(&self.stacks[i]), 4, 4, &[7, 13]);
                    let (r, m) = (rm.channels[&7].pixel(0, 0)[0], rm.channels[&13].pixel(0, 0)[0]);
                    self.push_aux(i, &AuxSource::Const(r, m));
                }
                None => {
                    // evaluated on a worker thread; until it arrives keep what is shown (neutral grey first)
                    if !self.scene.texs[i].has_image_base() {
                        self.scene.texs[i].set_base_solid([0.4, 0.4, 0.4]);
                    }
                    if self.refine_done.len() <= i {
                        self.refine_done.resize(i + 1, false);
                    }
                    self.refine_done[i] = false;
                    // a quick low-res pass first so something appears at once (the active set only: the others
                    // would spend seconds each on a picture that is replaced at once), then 1024, then the
                    // texture set's own resolution -- the viewport shows the set at its real size
                    let bits = self.colour_bits(i);
                    let full = view_res(size);
                    if res > 512 && i == active {
                        self.queue_compose(i, 512, bits.clone(), true);
                        self.queue_compose(i, res, bits.clone(), false);
                    } else {
                        self.queue_compose(i, res, bits.clone(), true);
                    }
                    // Height / Normal (when the stack writes them) follow the colour pass in their own job
                    let normal = self.writes_normal(i);
                    if normal {
                        self.queue_compose(i, res, vec![1, 22], false);
                    } else if self.stack_normal.remove(&i) {
                        // no longer written: the baked normal map again with the next colour result
                        self.normal_sent.remove(&i);
                    }
                    if full > res {
                        // a 4K set gets a 2048 step first: it fits the checkpoints (a 4K picture does not), so after
                        // the first one an edit shows sharp in seconds while the 4K pass refines behind it
                        if full > 2048 && std::env::var_os("SHARD_NO_2K_STEP").is_none() {
                            self.queue_compose(i, 2048, bits.clone(), false);
                            if normal {
                                self.queue_compose(i, 2048, vec![1, 22], false);
                            }
                        }
                        self.queue_compose(i, full, bits, false);
                        if normal {
                            self.queue_compose(i, full, vec![1, 22], false);
                        }
                    }
                }
            }
            continue;
        }
        if unsupported_active > 0 {
            let st = &self.stacks[active];
            self.status = format!(
                "{}: {} construct(s) not evaluated exactly; procedurals are approximated",
                st.name, unsupported_active
            );
        }
        self.recompose = false;
        if self.mask_reeval {
            self.mask_reeval = false;
            if let Some((set, path)) = self.mask_preview.clone() {
                let full = self.scene.texs.get(set).map(|t| t.w.clamp(64, 1024)).unwrap_or(512);
                self.queue_job(set, full.min(256), vec![2000], false, Some(path));
                if full > 256 {
                    self.mask_full_due = Some(std::time::Instant::now() + std::time::Duration::from_millis(350));
                }
            }
        }
        // an open mask owns the viewport; the channel view is rebuilt later, throttled
        if self.mask_edit.is_none() && matches!(self.view, ViewKind::Channel(_)) {
            self.view_dirty = true;
        }
    }

    /// Approximate evaluation of channel `bit` of texture set `set` at `res`x`res`: bitmaps and mesh maps
    /// from the project, known procedurals approximated, unsupported layers left out.
    pub(crate) fn approx_compose(&self, set: usize, bits: &[u32], res: u32) -> shard_compose::Composite {
        let st = &self.stacks[set];
        compose_job(&doc_for(st), self.bitmaps.as_ref(), &self.bake_urls, &st.name, &self.scene.mesh, set, bits, res, self.stroke_aspect)
    }

    /// Does the stack of `set` write Height or Normal (they change the normal map shown in the viewport)?
    /// Channels of a set's colour passes: base colour, roughness, metallic, and emissive when the stack writes it.
    /// A stack that no longer writes emissive gets a black map at once.
    fn colour_bits(&mut self, set: usize) -> Vec<u32> {
        let emissive = self.stacks.get(set).is_some_and(|st| shard_compose::writes_channel(&doc_for(st), 4));
        if emissive {
            vec![0, 4, 7, 13]
        } else {
            if self.emissive_sent.remove(&set) {
                self.render_cmds.push(RenderCmd::SetEmissive(set, 1, 1, vec![0, 0, 0, 255]));
            }
            vec![0, 7, 13]
        }
    }

    fn writes_normal(&self, set: usize) -> bool {
        if std::env::var_os("SHARD_NO_STACK_NORMAL").is_some() {
            return false;
        }
        self.stacks.get(set).is_some_and(|st| {
            let doc = doc_for(st);
            [1u32, 22].into_iter().any(|b| shard_compose::writes_channel(&doc, b))
        })
    }

    /// Final normal map of `set` at `res` (baked normal + the stack's Height and Normal channels); `None` when
    /// the stack writes neither.
    pub(crate) fn approx_normal(&self, set: usize, res: u32) -> Option<shard_compose::Image> {
        let st = self.stacks.get(set)?;
        shard_engine::compose::normal_job(
            &doc_for(st),
            self.bitmaps.as_ref(),
            &self.bake_urls,
            &st.name,
            &self.scene.mesh,
            set,
            res,
            self.stroke_aspect,
            None,
        )
    }

    /// Queue the evaluation of `set` at `res` (channels `bits`) for the worker thread. An edit
    /// (`bump`) invalidates results of older jobs of the same set; the active set runs first.
    fn queue_compose(&mut self, set: usize, res: u32, bits: Vec<u32>, bump: bool) {
        self.queue_job(set, res, bits, bump, None);
    }

    fn queue_job(&mut self, set: usize, res: u32, bits: Vec<u32>, bump: bool, mask_path: Option<Vec<usize>>) {
        if self.painter_override.is_some() && mask_path.is_none() {
            return; // Painter's own textures are shown
        }
        if self.refine_gen.len() <= set {
            self.refine_gen.resize(set + 1, 0);
        }
        if bump {
            self.refine_gen[set] += 1;
            self.view_cache.retain(|k, _| k.0 != set);
            self.view_requested.retain(|k| k.0 != set);
        }
        // an edit makes everything queued for the set stale (its passes are queued again by the caller); a
        // follow-up pass of the same edit only replaces the pass of the same size and kind
        let is_mask = mask_path.is_some();
        self.job_queue.retain(|(s, r, b, mp)| {
            !(*s == set && mp.is_some() == is_mask && if bump && !is_mask { true } else { *r == res && *b == bits })
        });
        let p = self.job_priority(set, res, &bits, is_mask);
        let at = if p == 0 {
            0
        } else {
            self.job_queue
                .iter()
                .position(|(s, r, b, mp)| self.job_priority(*s, *r, b, mp.is_some()) > p)
                .unwrap_or(self.job_queue.len())
        };
        self.job_queue.insert(at, (set, res, bits, mask_path));
        self.pump_jobs();
    }

    /// Order of the background passes (lower runs first): the active set up to 1024, the other sets up to 1024,
    /// Height / Normal, then the sets at their own resolution (the active one first).
    fn job_priority(&self, set: usize, res: u32, bits: &[u32], is_mask: bool) -> u8 {
        if is_mask {
            return 0;
        }
        let active = set == self.active();
        let normal_only = !bits.is_empty() && bits.iter().all(|b| matches!(*b, 1 | 22));
        match (res > 1024, active, normal_only) {
            (false, true, false) => 0,
            (false, false, false) => 1,
            (false, _, true) => 2,
            (true, true, _) => 3,
            (true, false, _) => 4,
        }
    }

    /// Put the queue back in order after the active set changed.
    fn resort_jobs(&mut self) {
        let mut jobs: Vec<_> = std::mem::take(&mut self.job_queue).into_iter().collect();
        jobs.sort_by_key(|(s, r, b, mp)| self.job_priority(*s, *r, b, mp.is_some()));
        self.job_queue = jobs.into();
    }

    fn pump_jobs(&mut self) {
        // mask previews have their own worker: a long base composition never delays them
        if self.mask_pending == 0 {
            if let Some(i) = self.job_queue.iter().position(|j| j.3.is_some()) {
                if let Some(job) = self.job_queue.remove(i) {
                    self.mask_pending = 1;
                    self.spawn_job(job);
                }
            }
        }
        // Several sets compose at once: much of a job is serial (stroke replay, the external renderer's
        // processes), so a few of them keep the cores busy (a test vehicle's eight sets at full resolution: 145 s of
        // jobs one after the other, 117 s two at a time, 96 s three at a time on 12 threads). SHARD_JOBS=n.
        static MAX_JOBS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let max_jobs = *MAX_JOBS.get_or_init(|| {
            let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            let default = if cores >= 8 { 3 } else { 2 };
            std::env::var("SHARD_JOBS").ok().and_then(|v| v.parse().ok()).unwrap_or(default).max(1)
        });
        while self.refine_pending < max_jobs {
            let busy = &self.running_sets;
            // A pass at the set's own resolution waits for the quick ones (<= 1024) of every set: run beside them it
            // took their cores (a 1024 pass after hiding a group went from 1.5 s to 13 s next to a 4K one).
            let quick_pending = self.running_res_quick > 0 || self.job_queue.iter().any(|j| j.3.is_none() && j.1 <= 1024);
            let Some(i) = self
                .job_queue
                .iter()
                .position(|j| j.3.is_none() && !busy.contains(&j.0) && (j.1 <= 1024 || !quick_pending))
            else {
                break;
            };
            let Some(job) = self.job_queue.remove(i) else { break };
            self.refine_pending += 1;
            self.running_sets.push(job.0);
            if job.1 <= 1024 {
                self.running_res_quick += 1;
            }
            self.running_res = job.1;
            self.spawn_job(job);
        }
    }

    /// Another project replaces this one: the queued passes go, and the passes still running (old set indices,
    /// another mesh) come back with a generation no set has any more, so their results are dropped. They still
    /// count as running until they end. (A queued pass for set 1 of one project ran on a one-set project and crashed.)
    fn forget_jobs(&mut self) {
        self.job_queue.clear();
        for g in &mut self.refine_gen {
            *g += 1 << 32;
        }
        self.mask_preview = None;
        self.mask_cache.clear();
    }

    fn spawn_job(&mut self, job: (usize, u32, Vec<u32>, Option<Vec<usize>>)) {
        let (set, res, bits, mask_path) = job;
        if set >= self.stacks.len() {
            // no such set (any more): drop the pass and free its slot
            if mask_path.is_some() {
                self.mask_pending = 0;
            } else {
                self.refine_pending = self.refine_pending.saturating_sub(1);
                self.running_sets.retain(|s| *s != set);
                if res <= 1024 {
                    self.running_res_quick = self.running_res_quick.saturating_sub(1);
                }
            }
            return;
        }
        let Some(path) = self.project_path.clone() else {
            self.refine_pending = 0;
            self.running_sets.clear();
            self.running_res_quick = 0;
            self.mask_pending = 0;
            return;
        };
        let gen = self.refine_gen.get(set).copied().unwrap_or(0);
        let doc = doc_for(&self.stacks[set]);
        let urls = self.bake_urls.clone();
        let name = self.stacks[set].name.clone();
        let mesh = self.scene.mesh.clone();
        let tx = self.refine_tx.clone();
        let aspect = self.stroke_aspect;
        let layer_name = mask_path.as_ref().map(|p| layer_name_at(&self.stacks[set].layers, p));
        if mask_path.is_none() {
            self.status = format!("Composing '{name}' at {res}x{res} in the background...");
        }
        std::thread::spawn(move || {
            crate::lower_thread_priority();
            // the interface thread keeps its core: background work runs below normal priority
            crate::lower_thread_priority();
            if let (Some(mp), Some(ln)) = (mask_path, layer_name) {
                let t0 = std::time::Instant::now();
                let bitmaps = crate::bitmaps::ProjectBitmaps::open(&path, 1024);
                let t_open = t0.elapsed();
                let r = mask_job(&doc, bitmaps.as_ref(), &urls, &name, &mesh, set, &mp, res, aspect);
                if std::env::var_os("SHARD_TRACE").is_some() {
                    eprintln!("[mask job] {res}x{res}: open {:?}, eval {:?}", t_open, t0.elapsed() - t_open);
                }
                let (channels, info) = match r {
                    Some(m) => {
                        let img = shard_compose::Image { w: res, h: res, n: 1, data: m.values };
                        (vec![(2000u32, img)], Some((ln, m.notes, m.reliable)))
                    }
                    None => (Vec::new(), Some((ln, vec!["mask not found".to_string()], false))),
                };
                let _ = tx.send(JobResult { set, gen, res, channels, info, masks: Vec::new(), views: Vec::new() });
                return;
            }
            let t0 = std::time::Instant::now();
            let bitmaps = crate::bitmaps::ProjectBitmaps::open(&path, res.clamp(1024, 4096));
            let (c, masks) = compose_job_masks(&doc, bitmaps.as_ref(), &urls, &name, &mesh, set, &bits, res, aspect);
            if std::env::var_os("SHARD_TRACE").is_some() {
                eprintln!("[job] '{name}' {res}x{res} bits={bits:?}: {:.1}s", t0.elapsed().as_secs_f32());
            }
            // pad the island borders: bilinear/mip sampling at UV seams must not blend with the (arbitrary)
            // texels outside the islands, which shows up as dark cut lines along seams
            let cov = crate::export::coverage(&mesh, set, res, res);
            let iters = (res / 128).clamp(4, 16);
            let mut channels: Vec<(u32, shard_compose::Image)> = bits
                .iter()
                .filter(|b| !matches!(**b, 1 | 22))
                .filter_map(|b| {
                    c.channels.get(b).map(|i| {
                        let mut img = i.clone();
                        if img.w == res && img.h == res {
                            crate::export::dilate_f32(&mut img.data, img.n, &cov, res, res, iters);
                        }
                        (*b, img)
                    })
                })
                .collect();
            // the stack writes Height / Normal: the viewport normal map is the baked one with them combined
            if bits.iter().any(|b| matches!(*b, 1 | 22)) {
                let t1 = std::time::Instant::now();
                if let Some(mut n) = shard_engine::compose::assemble_job_normal(
                    bitmaps.as_ref(),
                    &urls,
                    &name,
                    res,
                    c.channels.get(&1),
                    c.channels.get(&22),
                    Some((&mesh, set)),
                ) {
                    // the outermost covered ring is blended with the background by the bake
                    let inner = crate::export::erode(&cov, res, res);
                    crate::export::dilate_f32(&mut n.data, n.n, &inner, res, res, iters);
                    channels.push((3000, n));
                }
                if std::env::var_os("SHARD_TRACE").is_some() {
                    eprintln!("[job] '{name}' normal assembled: {:.2}s", t1.elapsed().as_secs_f32());
                }
            }
            // the channel views (Base color, Roughness... in the viewport) show these, at the pass's own resolution
            let views: Vec<(u32, u32, u32, u8, Vec<u8>)> = bits
                .iter()
                .filter_map(|b| {
                    let img = channels.iter().find(|(k, _)| k == b).map(|(_, i)| i).or_else(|| c.channels.get(b))?;
                    let (w, h, comps, bytes) = quantise_view(img, matches!(*b, 0 | 4));
                    Some((*b, w, h, comps, bytes))
                })
                .collect();
            // the base colour reaches the viewport sRGB-encoded: done here, in parallel, not on the interface thread
            for (bit, img) in channels.iter_mut() {
                if *bit == 0 || *bit == 4 {
                    use rayon::prelude::*;
                    img.data.par_chunks_mut(8192).for_each(|c| c.iter_mut().for_each(|v| *v = shard_compose::linear_to_srgb(*v)));
                }
            }
            let _ = tx.send(JobResult { set, gen, res, channels, info: None, masks, views });
        });
    }

    /// The set at its own resolution (the viewport's cap, see `view_res`) when it exceeds 1024: every channel.
    fn request_refine(&mut self, set: usize) {
        let size = self.scene.texs.get(set).map(|t| t.w).unwrap_or(0);
        let full = view_res(size);
        if full <= 1024 || self.refine_done.get(set).copied().unwrap_or(false) {
            return;
        }
        // only after the first (<= 1024) result of the set exists, and once per edit
        if self.scene.texs[set].has_image_base() && !self.job_queue.iter().any(|(s, r, _, mp)| *s == set && *r == full && mp.is_none()) {
            let bits = self.colour_bits(set);
            self.queue_compose(set, full, bits, false);
            if self.writes_normal(set) {
                self.queue_compose(set, full, vec![1, 22], false);
            }
        }
    }

    /// Write every mask of the active set as `<n>_<layer>.png` plus `index.txt` (diagnostic).
    pub fn dump_masks(&mut self, dir: &std::path::Path, res: u32) -> Result<usize, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let set = self.active();
        let st = &self.stacks[set];
        let doc = doc_for(st);
        let none = shard_compose::NoBitmaps;
        let provider: &dyn shard_compose::BitmapProvider = self.bitmaps.as_ref().map(|b| b as _).unwrap_or(&none);
        let no_maps = shard_compose::NoMaps;
        // the Substance engine like the app's own passes (SHARD_DUMP_APPROX=1: the native approximations)
        let exact = std::env::var_os("SHARD_DUMP_APPROX").is_none();
        let set_maps = self.bitmaps.as_ref().map(|b| SetMaps::new(b, &self.bake_urls, &st.name, exact, Some(&self.scene.mesh), set));
        let maps: &dyn shard_compose::MeshMaps = set_maps.as_ref().map(|m| m as _).unwrap_or(&no_maps);
        let raster = crate::strokes::SetStrokes::new(&self.scene.mesh, set, self.stroke_aspect);
        let strokes: Option<&dyn shard_compose::StrokeRaster> = Some(&raster);
        let aces_working = self.bitmaps.as_ref().is_some_and(|b| b.aces_working());
        let opts = shard_compose::Options { bitmaps: provider, maps, strokes, skip_unsupported: true, allow_approx: true, aces_working };
        let mut index = String::new();
        for (label, b) in [("AO", Bake::AmbientOcclusion), ("curvature", Bake::Curvature), ("normal", Bake::NormalBase)] {
            let url = find_bake_url(&self.bake_urls, &st.name, b).map(str::to_string);
            let loaded = self.bake_image(set, b).map(|i| (i.w, i.h, i.n));
            index.push_str(&format!("# bake {label}: url={url:?} loaded={loaded:?}
"));
        }
        index.push_str(&format!("# bake_urls={:?}
", self.bake_urls));
        let all = shard_compose::all_masks(&doc, res, res, &opts);
        for (n, (path, name, m)) in all.iter().enumerate() {
            let safe: String = name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
            let mean = m.values.iter().sum::<f32>() / m.values.len().max(1) as f32;
            let rgba: Vec<u8> = m
                .values
                .iter()
                .flat_map(|v| {
                    let g = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    [g, g, g, 255]
                })
                .collect();
            let file = format!("{n:02}_{safe}.png");
            crate::paint::save_png(&dir.join(&file), res, res, &rgba)?;
            index.push_str(&format!(
                "{file}	path={path:?}	mean={mean:.3}	{}	{}
",
                if m.reliable { "exact-ish" } else { "NOT EXACT" },
                m.notes.join(" | ")
            ));
        }
        std::fs::write(dir.join("index.txt"), index).map_err(|e| e.to_string())?;
        Ok(all.len())
    }

    /// Open a mask for interactive painting and show it in the viewport.
    fn enter_mask_edit(&mut self, set: usize, path: Vec<usize>, res: u32, values: Vec<f32>) {
        trace_view("enter_mask_edit");
        let mut tex = crate::paint::PaintTexture::new(res, res);
        tex.set_base_image(res, res, 1, &values);
        let name = layer_name_at(&self.stacks[set].layers, &path);
        self.mask_edit = Some(MaskEdit {
            set,
            path,
            res,
            tex,
            keys: Vec::new(),
            started: std::time::Instant::now(),
            diameter: 0.02,
            camera: (glam::Vec3::ZERO, glam::Vec3::ZERO),
            last_upload: std::time::Instant::now(),
            dirty: true,
        });
        self.upload_mask_view();
        self.status = format!("Mask of '{name}': paint with the brush (white adds, black removes). Alt+click the thumbnail to leave.");
    }

    /// Resolve the mask being edited and show it unlit (other sets dark).
    fn upload_mask_view(&mut self) {
        let Some(me) = self.mask_edit.as_mut() else { return };
        let mut images: Vec<(u32, u32, Vec<u8>)> = vec![(1, 1, vec![20, 20, 22, 255]); self.stacks.len()];
        images[me.set] = (me.res, me.res, me.tex.resolve_full());
        me.dirty = false;
        me.last_upload = std::time::Instant::now();
        self.render_cmds.push(RenderCmd::SetView(Some(images)));
    }

    /// Mouse down in the viewport (paint target = the open mask, else the texture set).
    pub fn paint_begin(&mut self, aspect: f32, vp: (f32, f32), ndc: glam::Vec2) -> Option<glam::Vec2> {
        self.quick_note_stroke();
        if self.mask_edit.is_none() {
            self.begin_stroke();
            return self.scene.paint_stroke(aspect, vp, ndc, None);
        }
        let (cam, brush) = (&self.scene.cam, &self.scene.brush);
        let (o, d) = cam.ray(aspect, ndc);
        let diameter = match self.scene.mesh.raycast(o, d) {
            Some(h) => {
                let dist = (h.point - cam.eye()).length();
                let world_per_px = 2.0 * (45f32.to_radians() * 0.5).tan() * dist / vp.1.max(1.0);
                (2.0 * brush.radius * world_per_px).max(1e-4)
            }
            None => 0.02,
        };
        let eye_target = (cam.eye(), cam.target);
        let me = self.mask_edit.as_mut().unwrap();
        me.keys.clear();
        me.started = std::time::Instant::now();
        me.diameter = diameter;
        me.camera = eye_target;
        self.paint_move(aspect, vp, ndc, None)
    }

    /// Mouse move while painting.
    pub fn paint_move(
        &mut self,
        aspect: f32,
        vp: (f32, f32),
        ndc: glam::Vec2,
        prev: Option<glam::Vec2>,
    ) -> Option<glam::Vec2> {
        let Some(me) = self.mask_edit.as_mut() else {
            return self.scene.paint_stroke(aspect, vp, ndc, prev);
        };
        let set = me.set;
        // paint the mask texture through the regular projection brush
        if set < self.scene.texs.len() {
            std::mem::swap(&mut self.scene.texs[set], &mut me.tex);
            let r = self.scene.paint_stroke(aspect, vp, ndc, prev);
            std::mem::swap(&mut self.scene.texs[set], &mut me.tex);
            // recorded against a plain projection: remove the shift that centres the scene in the free area
            let c = self.scene.cam.center;
            me.keys.push((ndc.x - c.x, ndc.y - c.y, aspect, me.started.elapsed().as_secs_f32()));
            me.dirty = true;
            let upload = me.last_upload.elapsed().as_millis() > 40;
            if upload {
                self.upload_mask_view();
            }
            return r;
        }
        None
    }

    /// Mouse up: keep the stroke as a paint effect of the layer's mask and recompose.
    fn finish_mask_stroke(&mut self) {
        use shard_model::{Brush as MBrush, ChannelSet, PaintAction, Source, Stroke, StrokeKey};
        let (aspect_est, brush, tool) = (self.stroke_aspect, self.scene.brush, self.scene.tool);
        let Some(me) = self.mask_edit.as_mut() else { return };
        if me.dirty {
            // final image of the stroke
        }
        if me.keys.is_empty() {
            return;
        }
        let (eye, target) = me.camera;
        let v = glam::Mat4::look_at_rh(eye, target, glam::Vec3::Y);
        let p = glam::Mat4::perspective_rh(45f32.to_radians(), 1.0, 0.05, 100.0);
        let keys: Vec<StrokeKey> = me
            .keys
            .iter()
            .map(|(x, y, asp, t)| StrokeKey { pos: [(x * asp + aspect_est) * 0.5, (y + 1.0) * 0.5], time: *t, kind: 0 })
            .collect();
        let stroke = Stroke {
            keys,
            view: v.to_cols_array(),
            projection: p.to_cols_array(),
            seed: 1,
            view_type: 1,
            lazy_distance: 0.0,
            pressure: Vec::new(),
            duplicate_count: 0,
            duplicate_matrix: None,
        };
        let value = (0.299 * brush.color[0] + 0.587 * brush.color[1] + 0.114 * brush.color[2]).clamp(0.0, 1.0);
        let mut shape = shard_model::Procedural::default();
        shape.resource = Some("/shape_v2/shape_v2".to_string());
        shape.tweaks.insert("hardness".into(), shard_model::Tweak::Float(brush.hardness));
        let action = PaintAction {
            brush: MBrush {
                class: "DataBrushStamp".into(),
                size: me.diameter,
                size_space: 1,
                flow: brush.flow,
                spacing: (brush.spacing / 100.0).max(0.02),
                culling_enabled: true,
                culling_angle: 0.0,
                shape: Some(shape),
                ..Default::default()
            },
            opacity: brush.opacity,
            color: vec![Source::Uniform { channels: ChannelSet(0), color: Some([value; 3]), opacity: 1.0 }],
            strokes: vec![stroke].into(),
            ..Default::default()
        };
        let kind = if tool == Tool::Eraser { EffectKind::Eraser(action) } else { EffectKind::Paint(action) };
        let (set, path) = (me.set, me.path.clone());
        me.keys.clear();
        // append to the mask of the layer
        let mut layers = &mut self.stacks[set].layers;
        let mut target: Option<&mut Layer> = None;
        for (k, &i) in path.iter().enumerate() {
            let Some(l) = layers.get_mut(i) else { return };
            if k + 1 == path.len() {
                target = Some(l);
                break;
            }
            layers = &mut l.children;
        }
        if let Some(l) = target {
            if let Some(m) = l.mask.as_mut() {
                m.effects.push(Effect {
                    uid: 0,
                    visible: true,
                    channels: ChannelSet(u128::MAX),
                    label: None,
                    blend: Vec::new(),
                    kind,
                });
                self.mark_recompose();
            }
        }
        self.upload_mask_view();
    }

    /// Path of the first layer called `name` in the active set (diagnostics).
    pub fn find_layer_path(&self, name: &str) -> Option<Vec<usize>> {
        fn walk(ls: &[Layer], name: &str, path: &mut Vec<usize>) -> bool {
            for (i, l) in ls.iter().enumerate() {
                path.push(i);
                if l.name == name || walk(&l.children, name, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut p = Vec::new();
        walk(&self.stacks[self.active()].layers, name, &mut p).then_some(p)
    }

    /// Diagnostic: open the mask of `name`, paint a white stroke across it and recompose.
    pub fn simulate_mask_paint(&mut self, name: &str) -> Result<(), String> {
        let path = self.find_layer_path(name).ok_or("layer not found")?;
        self.wait_jobs();
        self.toggle_mask_preview(path);
        self.wait_jobs();
        if self.mask_edit.is_none() {
            return Err("mask could not be opened".into());
        }
        self.scene.brush.color = [1.0, 1.0, 1.0];
        self.scene.brush.radius = 40.0;
        let (aspect, vp) = (1.5f32, (900.0f32, 600.0f32));
        let mut prev = self.paint_begin(aspect, vp, glam::Vec2::new(-0.3, 0.0));
        for i in 1..=40 {
            let t = i as f32;
            prev = self.paint_move(aspect, vp, glam::Vec2::new(-0.3 + 0.015 * t, 0.2 * (t * 0.2).sin()), prev);
        }
        self.end_stroke();
        self.do_recompose(Some(self.active()));
        self.wait_jobs();
        Ok(())
    }

    /// Diagnostic: open the mask of `name`, hide each of its effects in turn and report mean value and time.
    pub fn simulate_effect_toggle(&mut self, name: &str) -> Result<(), String> {
        let path = self.find_layer_path(name).ok_or("layer not found")?;
        let set = self.active();
        let mean = |me: &Option<MaskEdit>| -> f32 {
            me.as_ref().map(|m| {
                let v = m.tex.resolve_full();
                v.chunks_exact(4).map(|p| p[0] as f32 / 255.0).sum::<f32>() / (v.len() / 4).max(1) as f32
            }).unwrap_or(-1.0)
        };
        let t = std::time::Instant::now();
        self.toggle_mask_preview(path.clone());
        self.wait_mask();
        println!("open mask: mean={:.4} in {:?}", mean(&self.mask_edit), t.elapsed());
        let n = layer_at_mut(&mut self.stacks[set].layers, &path)
            .and_then(|l| l.mask.as_ref())
            .map(|m| m.effects.len())
            .unwrap_or(0);
        for pos in 0..n {
            let label = {
                let e = &mut layer_at_mut(&mut self.stacks[set].layers, &path).unwrap().mask.as_mut().unwrap().effects[pos];
                e.visible = false;
                effect_label(e)
            };
            let t = std::time::Instant::now();
            self.mask_reeval = true;
            self.do_recompose(Some(set));
            self.wait_mask();
            println!("hide #{pos} '{label}': mean={:.4} in {:?}", mean(&self.mask_edit), t.elapsed());
            layer_at_mut(&mut self.stacks[set].layers, &path).unwrap().mask.as_mut().unwrap().effects[pos].visible = true;
        }
        Ok(())
    }

    /// Alt+click on a mask thumbnail: show that layer's mask in the viewport (again: back to the material).
    pub fn toggle_mask_preview(&mut self, path: Vec<usize>) {
        let set = self.active();
        if self.mask_preview.as_ref() == Some(&(set, path.clone())) {
            self.mask_preview = None;
            self.mask_edit = None;
            self.set_view(self.view);
            if self.defer_base {
                self.defer_base = false;
                self.do_recompose(None);
            }
            self.status = "Mask editing off".into();
            return;
        }
        self.mask_preview = Some((set, path.clone()));
        let res = self.scene.texs.get(set).map(|t| t.w.clamp(64, 1024)).unwrap_or(512);
        if let Some((cached, vals)) = self.mask_cache.get(&(set, path.clone())) {
            // already evaluated by the background composition: instant, then refined when it is coarser than
            // the set (the result of that job replaces this picture)
            let data: Vec<f32> = vals.iter().map(|v| *v as f32 / 255.0).collect();
            let cached = *cached;
            self.enter_mask_edit(set, path.clone(), cached, data);
            let want = self.scene.texs.get(set).map(|t| view_res(t.w)).unwrap_or(512);
            if cached < want {
                // the 1K pass first when the cache is coarser than that; its result chains to the set's size
                self.queue_job(set, if cached < res { res } else { want }, vec![2000], false, Some(path));
            }
            return;
        }
        self.status = "Evaluating mask...".into();
        self.queue_job(set, res, vec![2000], false, Some(path));
    }

    /// Block until the mask preview jobs are done (base compositions may still run).
    fn wait_mask(&mut self) {
        let t = std::time::Instant::now();
        loop {
            self.poll_refine();
            if (self.mask_pending == 0 && !self.job_queue.iter().any(|j| j.3.is_some())) || t.elapsed().as_secs() > 120 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Apply what the background compositions finished so far (headless runs).
    pub fn poll_jobs_once(&mut self) {
        self.poll_refine();
    }

    /// Block until every queued background composition has been applied (headless runs).
    pub fn wait_jobs(&mut self) {
        let t = std::time::Instant::now();
        loop {
            self.poll_refine();
            if (self.refine_pending == 0 && self.mask_pending == 0 && self.job_queue.is_empty()) || t.elapsed().as_secs() > 300 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    /// Keep the pictures of the layers panel up to date: a quick pass with the built-in approximations, then
    /// (once nothing else is running) one with the exact Substance engine. Runs on a worker thread, only while
    /// the panel is on screen and no composition is pending.
    fn pump_thumbs(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.thumb_rx.try_recv() {
            self.thumb_busy = false;
            let (set, _, _, _) = r.sig;
            let keep: std::collections::HashSet<(u64, bool)> = r.items.iter().map(|t| (t.uid, t.mask)).collect();
            self.thumb_tex.retain(|k, _| k.0 != set || keep.contains(&(k.1, k.2)));
            for t in r.items {
                let img = egui::ColorImage::from_rgba_unmultiplied([t.size as usize; 2], &t.rgba);
                let tex = ctx.load_texture(format!("thumb-{set}-{}-{}", t.uid, t.mask), img, egui::TextureOptions::LINEAR);
                self.thumb_tex.insert((set, t.uid, t.mask), tex);
            }
            self.thumb_done = Some(r.sig);
        }
        if self.thumb_busy || self.thumbs_wanted.elapsed().as_millis() > 800 {
            return;
        }
        let short_pass = |r: u32| r <= 1024;
        if (self.refine_pending != 0 && short_pass(self.running_res))
            || self.mask_pending != 0
            || self.job_queue.iter().any(|(_, r, _, _)| short_pass(*r))
            || self.mask_edit.is_some()
        {
            return;
        }
        let set = self.active();
        let Some(stack) = self.stacks.get(set) else { return };
        if stack.layers.is_empty() {
            return;
        }
        let gen = self.refine_gen.get(set).copied().unwrap_or(0);
        let bit = self.active_channel;
        let exact = match self.thumb_done {
            Some((s, g, b, e)) if (s, g, b) == (set, gen, bit) => {
                if e || shard_engine::oracle::installed_path().is_none() || self.config.substance_engine_off {
                    return; // up to date
                }
                true
            }
            _ => false,
        };
        let doc = doc_for(stack);
        let (urls, name, mesh, aspect) = (self.bake_urls.clone(), stack.name.clone(), self.scene.mesh.clone(), self.stroke_aspect);
        let path = self.project_path.clone();
        let (tx, sig) = (self.thumb_tx.clone(), (set, gen, bit, exact));
        self.thumb_busy = true;
        std::thread::spawn(move || {
            crate::lower_thread_priority();
            let bitmaps = path.as_ref().and_then(|p| crate::bitmaps::ProjectBitmaps::open(p, 1024));
            let t0 = std::time::Instant::now();
            let items = shard_engine::compose::thumbs_job(&doc, bitmaps.as_ref(), &urls, &name, &mesh, set, bit, 128, aspect, exact);
            if std::env::var_os("SHARD_TRACE").is_some() {
                eprintln!("[thumbs] '{name}' bit {bit} exact={exact}: {} pictures in {:.1}s", items.len(), t0.elapsed().as_secs_f32());
            }
            let _ = tx.send(ThumbResult { sig, items });
        });
    }

    /// Bits of the texture sets whose base composition is queued or running (the viewport stripes them).
    /// Mask previews and the passes past the first full one do not count.
    pub fn loading_sets(&self) -> u32 {
        if std::env::var_os("SHARD_NO_LOADING_STRIPES").is_some() {
            return 0;
        }
        let mut m = 0u32;
        for &s in &self.running_sets {
            m |= 1u32.checked_shl(s as u32).unwrap_or(0);
        }
        // a set with a picture already on screen is only being refined (the full-resolution pass): not loading
        for (s, r, _, mask) in &self.job_queue {
            let refining = *r > 1024 && self.scene.texs.get(*s).is_some_and(|t| t.has_image_base());
            if mask.is_none() && !refining {
                m |= 1u32.checked_shl(*s as u32).unwrap_or(0);
            }
        }
        if self.running_res_quick == 0 {
            // only full-resolution passes run: the sets that already show a picture are refining
            for &s in &self.running_sets {
                if self.scene.texs.get(s).is_some_and(|t| t.has_image_base())
                    && !self.job_queue.iter().any(|(q, r, _, mp)| *q == s && *r <= 1024 && mp.is_none())
                {
                    m &= !1u32.checked_shl(s as u32).unwrap_or(0);
                }
            }
        }
        // a set showing Painter's own picture is not loading (its passes only feed the thumbnails and masks)
        for s in self.cache_shown.keys() {
            m &= !1u32.checked_shl(*s as u32).unwrap_or(0);
        }
        m
    }

    /// The pictures of the layers panel match the current stack (diagnostics and `--shot` wait for this).
    pub fn thumbs_settled(&self) -> bool {
        if self.thumb_busy {
            return false;
        }
        // HDRI previews asked for by the shading menu and not in yet
        if self.hdri_thumbs.values().any(|t| t.is_none()) && self.hdri_thumb_tx.is_some() {
            return false;
        }
        // nothing to wait for while the layers panel is not on screen (no pictures are made then)
        if self.thumbs_wanted.elapsed().as_millis() > 800 {
            return true;
        }
        let set = self.active();
        let gen = self.refine_gen.get(set).copied().unwrap_or(0);
        let exact_possible = shard_engine::oracle::installed_path().is_some() && !self.config.substance_engine_off;
        matches!(self.thumb_done, Some((s, g, b, e)) if (s, g, b) == (set, gen, self.active_channel) && (e || !exact_possible))
    }

    /// The "Open" file dialog on a thread of its own: the interface keeps drawing while Windows lists the folder
    /// (a big Downloads folder stayed on "Working on it..." with the dialog run inside a frame).
    fn open_dialog(&mut self) {
        if self.open_dialog_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .add_filter("Painter and ShardStroke projects", &["shard", "pigma", "spp", "spsm", "spmsk", "sppr", "spt"])
                .pick_file();
            let _ = tx.send(picked);
        });
        self.open_dialog_rx = Some(rx);
    }

    fn poll_open_dialog(&mut self) {
        let Some(rx) = &self.open_dialog_rx else { return };
        match rx.try_recv() {
            Ok(picked) => {
                self.open_dialog_rx = None;
                if let Some(p) = picked {
                    self.request_open(p);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.open_dialog_rx = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Upload the Painter cache of the sets that arrived (only for a stack not edited since the file was opened).
    fn poll_painter_cache(&mut self) {
        let Some(rx) = &self.cache_rx else { return };
        let mut loads = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(l) => loads.push(l),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.cache_rx = None;
                    break;
                }
            }
        }
        for l in loads {
            if self.refine_gen.get(l.set).copied().unwrap_or(0) != l.gen || l.set >= self.scene.texs.len() {
                continue; // edited meanwhile: our composition is the truth now
            }
            if let Some(rgb) = &l.base {
                self.scene.texs[l.set].set_base_rgb8(l.res, l.res, rgb.clone());
            }
            if let (Some(r), Some(m)) = (&l.rough, &l.metal) {
                self.push_aux(l.set, &AuxSource::Maps(r.clone(), m.clone()));
            }
            if let Some((side, e)) = &l.emissive {
                let rgba: Vec<u8> = e.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
                self.emissive_sent.insert(l.set);
                self.render_cmds.push(RenderCmd::SetEmissive(l.set, *side, *side, rgba));
            }
            if let Some(n) = &l.normal {
                let side = ((n.len() / 4) as f32).sqrt() as u32;
                self.normal_sent.insert(l.set);
                self.stack_normal.insert(l.set);
                self.render_cmds.push(RenderCmd::SetNormal(l.set, side, side, n.clone()));
            }
            self.cache_shown.insert(l.set, l.gen);
            // The passes queued for the picture are not needed while Painter's is shown: the set's own resolution
            // and the Height / Normal ones only fed the viewport (a test pistol: 90 s + 83 s at 4K). The first colour
            // pass (<= 1024) stays for the thumbnails, masks and channel views; an edit queues everything again.
            // SHARD_CACHE_KEEP_PASSES=1 keeps them.
            if std::env::var_os("SHARD_CACHE_KEEP_PASSES").is_none() {
                let set = l.set;
                // (the 1024 passes, Height / Normal included, stay: they are the reference edits are measured from)
                self.job_queue.retain(|(s, r, _, mp)| !(*s == set && mp.is_none() && *r > 1024));
            }
            if self.refine_done.len() <= l.set {
                self.refine_done.resize(l.set + 1, false);
            }
            self.refine_done[l.set] = true;
            self.status = format!("Showing Painter's saved textures ({} set(s)); edits recompose", self.cache_shown.len());
        }
    }

    fn poll_refine(&mut self) {
        self.poll_open_dialog();
        self.poll_painter_cache();
        while let Ok(r) = self.refine_rx.try_recv() {
            if r.info.is_some() {
                self.mask_pending = 0;
            } else {
                self.refine_pending = self.refine_pending.saturating_sub(1);
                self.running_sets.retain(|s| *s != r.set);
                if r.res <= 1024 {
                    self.running_res_quick = self.running_res_quick.saturating_sub(1);
                }
                if self.refine_pending == 0 {
                    self.running_res = 0;
                }
            }
            if let Some((name, notes, reliable)) = r.info.clone() {
                // mask preview: still wanted?
                if self.mask_preview.as_ref().map(|(s, _)| *s) == Some(r.set) {
                    if let Some((_, img)) = r.channels.into_iter().find(|(b, _)| *b == 2000) {
                        if let Some((set, path)) = self.mask_preview.clone() {
                            let shown = img.w;
                            self.enter_mask_edit(set, path.clone(), img.w, img.data);
                            // a set bigger than 1K shows its mask at the set's own resolution once the 1K
                            // pass (the debounced full one) is in
                            let want = self.scene.texs.get(set).map(|t| t.w.clamp(64, 2048)).unwrap_or(512);
                            if shown >= 1024 && shown < want {
                                self.queue_job(set, want, vec![2000], false, Some(path));
                            }
                        }
                    }
                    self.status = if reliable {
                        format!("Mask of '{name}'")
                    } else {
                        format!("Mask of '{name}' (not exact: {})", notes.join("; "))
                    };
                }
                continue;
            }
            if self.refine_gen.get(r.set).copied() != Some(r.gen) {
                continue; // an edit happened meanwhile; its own job is queued
            }
            if !r.masks.is_empty() {
                self.mask_cache.retain(|(s, _), _| *s != r.set);
                for (path, res, vals) in r.masks {
                    self.mask_cache.insert((r.set, path), (res, vals));
                }
            }
            let size = self.scene.texs.get(r.set).map(|t| t.w).unwrap_or(0);
            for (bit, w, h, comps, bytes) in r.views {
                let better = self.view_cache.get(&(r.set, bit)).is_none_or(|e| e.0 <= r.res);
                if better {
                    self.view_cache.insert((r.set, bit), (r.res, w, h, comps, bytes));
                    if matches!(self.view, ViewKind::Channel(b) if b == bit) {
                        self.view_dirty = true;
                    }
                }
            }
            // the set shows Painter's own picture of this very stack: keep it (thumbnails, masks and channel views
            // above still come from our composition)
            if self.cache_shown.get(&r.set) == Some(&r.gen) {
                continue;
            }
            self.cache_shown.remove(&r.set);
            let mut maps: (Option<shard_compose::Image>, Option<shard_compose::Image>) = (None, None);
            let mut stack_normal: Option<shard_compose::Image> = None;
            for (bit, img) in r.channels {
                // jobs of one edit finish out of order (a 2048 refine overtakes the 1024 pass): never
                // replace a better image of the same edit with a coarser one
                let prev = self.applied_res.get(&(r.set, bit)).copied();
                if prev.is_some_and(|(g, rs)| g == r.gen && r.res < rs) {
                    continue;
                }
                self.applied_res.insert((r.set, bit), (r.gen, r.res));
                match bit {
                    0 => {
                        if let Some(t) = self.scene.texs.get_mut(r.set) {
                            // already sRGB-encoded by the worker
                            t.set_base_image(img.w, img.h, img.n, &img.data);
                        }
                    }
                    4 => {
                        // already sRGB-encoded by the worker
                        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        let rgba: Vec<u8> =
                            img.data.chunks_exact(img.n).flat_map(|p| [q(p[0]), q(p[1.min(img.n - 1)]), q(p[2.min(img.n - 1)]), 255]).collect();
                        self.emissive_sent.insert(r.set);
                        self.render_cmds.push(RenderCmd::SetEmissive(r.set, img.w, img.h, rgba));
                    }
                    7 => maps.0 = Some(img),
                    13 => maps.1 = Some(img),
                    3000 => stack_normal = Some(img),
                    _ => {}
                }
            }
            if let Some(n) = stack_normal {
                // baked normal combined with the stack's Height / Normal channels
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                let mut rgba = Vec::with_capacity((n.w * n.h * 4) as usize);
                for p in n.data.chunks_exact(n.n) {
                    rgba.extend_from_slice(&[q(p[0]), q(p[1.min(n.n - 1)]), q(p[2.min(n.n - 1)]), 255]);
                }
                self.normal_sent.insert(r.set);
                self.stack_normal.insert(r.set);
                self.render_cmds.push(RenderCmd::SetNormal(r.set, n.w, n.h, rgba));
            }
            if let (Some(rough), Some(metal)) = maps {
                self.push_aux(r.set, &AuxSource::Maps(rough, metal));
            }
            if self.refine_done.len() <= r.set {
                self.refine_done.resize(r.set + 1, false);
            }
            let want = view_res(size);
            if r.res >= want || want <= 1024 {
                self.refine_done[r.set] = true;
            }
            self.status = format!("Composed {}x{}", r.res, r.res);
            if r.res >= 1024 && r.res < want && r.set == self.active() {
                self.request_refine(r.set);
            }
        }
        self.pump_jobs();
        if let Some(t) = self.mask_full_due {
            if std::time::Instant::now() >= t && self.mask_pending == 0 {
                self.mask_full_due = None;
                if let Some((set, path)) = self.mask_preview.clone() {
                    let full = self.scene.texs.get(set).map(|t| t.w.clamp(64, 1024)).unwrap_or(512);
                    self.queue_job(set, full, vec![2000], false, Some(path));
                }
            }
        }
        if self.defer_base
            && self.mask_full_due.is_none()
            && self.refine_pending == 0
            && self.mask_pending == 0
            && self.last_mask_change.elapsed().as_millis() > 1500
        {
            self.defer_base = false;
            self.force_base = true;
            self.do_recompose(None);
            self.force_base = false;
        }
        if self.view_dirty && self.mask_edit.is_none() && self.last_view_refresh.elapsed().as_millis() > 500 {
            self.view_dirty = false;
            if matches!(self.view, ViewKind::Channel(_)) {
                self.refresh_view();
            }
        }
        // switching to another texture set refines it on demand
        let a = self.active();
        if self.last_refine_check != a {
            self.last_refine_check = a;
            self.resort_jobs();
            self.request_refine(a);
        }
        self.autosave_tick();
    }

    /// Resource URL of `bake` for texture set `set`, matched by the set (material) name.
    fn bake_url(&self, set: usize, bake: Bake) -> Option<&str> {
        find_bake_url(&self.bake_urls, &self.stacks.get(set)?.name, bake)
    }

    pub(crate) fn bake_image(&self, set: usize, bake: Bake) -> Option<std::sync::Arc<shard_compose::Image>> {
        use shard_compose::BitmapProvider;
        let url = self.bake_url(set, bake)?;
        self.bitmaps.as_ref()?.bitmap(url)
    }

    /// Was the baked normal map made against the geometric face normals (strong tangent-space tilt on
    /// triangles whose authored vertex normals deviate) or against the smooth vertex normals (nearly flat
    /// there)? `None` when the mesh has too few deviating triangles to tell.
    fn bake_used_face_normals(&self, set: usize, map: &shard_compose::Image) -> Option<bool> {
        let m = &self.scene.mesh;
        let (mut count, mut tilt) = (0u32, 0.0f32);
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            if m.tri_mat.get(t).copied() != Some(set as u32) {
                continue;
            }
            let v: [&crate::mesh::Vertex; 3] = [&m.vertices[tri[0] as usize], &m.vertices[tri[1] as usize], &m.vertices[tri[2] as usize]];
            let p: [glam::Vec3; 3] = [v[0].pos.into(), v[1].pos.into(), v[2].pos.into()];
            let fnr = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            if fnr == glam::Vec3::ZERO {
                continue;
            }
            let avg = (glam::Vec3::from(v[0].normal) + glam::Vec3::from(v[1].normal) + glam::Vec3::from(v[2].normal)).normalize_or_zero();
            let fnr = if fnr.dot(avg) < 0.0 { -fnr } else { fnr };
            let dev = v.iter().map(|x| glam::Vec3::from(x.normal).normalize_or_zero().dot(fnr)).fold(1.0f32, f32::min);
            if dev > 0.99 {
                continue; // vertex normals agree with the face: says nothing
            }
            let uv = [(v[0].uv[0] + v[1].uv[0] + v[2].uv[0]) / 3.0, (v[0].uv[1] + v[1].uv[1] + v[2].uv[1]) / 3.0];
            let (x, y) = (
                ((uv[0] * map.w as f32) as i64).clamp(0, map.w as i64 - 1) as u32,
                (((1.0 - uv[1]) * map.h as f32) as i64).clamp(0, map.h as i64 - 1) as u32,
            );
            let px = map.pixel(x, y);
            let (nx, ny) = (px[0] * 2.0 - 1.0, px[1.min(map.n - 1)] * 2.0 - 1.0);
            tilt += (nx * nx + ny * ny).sqrt();
            count += 1;
        }
        if std::env::var_os("SHARD_NORMAL_PROBE").is_some() {
            // which tangent-frame convention turns the baked map back into the face normal on flat faces
            // whose vertex normals deviate? (stored tangents; centroid sample, all deviating triangles)
            let mut sums = [0.0f64; 8];
            let mut cnt = 0u32;
            for (t, tri) in m.indices.chunks_exact(3).enumerate() {
                if m.tri_mat.get(t).copied() != Some(set as u32) {
                    continue;
                }
                let v: [&crate::mesh::Vertex; 3] = [&m.vertices[tri[0] as usize], &m.vertices[tri[1] as usize], &m.vertices[tri[2] as usize]];
                let p: [glam::Vec3; 3] = [v[0].pos.into(), v[1].pos.into(), v[2].pos.into()];
                let fnr = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                if fnr == glam::Vec3::ZERO || v[0].tangent == [0.0; 3] {
                    continue;
                }
                let avg = (glam::Vec3::from(v[0].normal) + glam::Vec3::from(v[1].normal) + glam::Vec3::from(v[2].normal)).normalize_or_zero();
                let fnr = if fnr.dot(avg) < 0.0 { -fnr } else { fnr };
                let dev = v.iter().map(|x| glam::Vec3::from(x.normal).normalize_or_zero().dot(fnr)).fold(1.0f32, f32::min);
                if dev > 0.96 {
                    continue; // only triangles whose vertex normals clearly leave the face
                }
                let third = 1.0 / 3.0;
                let nrm = ((glam::Vec3::from(v[0].normal) + glam::Vec3::from(v[1].normal) + glam::Vec3::from(v[2].normal)) * third).normalize_or_zero();
                let tg = ((glam::Vec3::from(v[0].tangent) + glam::Vec3::from(v[1].tangent) + glam::Vec3::from(v[2].tangent)) * third).normalize_or_zero();
                let bt = ((glam::Vec3::from(v[0].bitangent) + glam::Vec3::from(v[1].bitangent) + glam::Vec3::from(v[2].bitangent)) * third).normalize_or_zero();
                let uv = [(v[0].uv[0] + v[1].uv[0] + v[2].uv[0]) * third, (v[0].uv[1] + v[1].uv[1] + v[2].uv[1]) * third];
                let (x, y) = (
                    ((uv[0] * map.w as f32) as i64).clamp(0, map.w as i64 - 1) as u32,
                    (((1.0 - uv[1]) * map.h as f32) as i64).clamp(0, map.h as i64 - 1) as u32,
                );
                let px = map.pixel(x, y);
                let (nx, ny, nz) = (px[0] * 2.0 - 1.0, px[1.min(map.n - 1)] * 2.0 - 1.0, px[2.min(map.n - 1)] * 2.0 - 1.0);
                for (k, (sx, sy)) in [(1.0f32, 1.0f32), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)].into_iter().enumerate() {
                    let w = (tg * (nx * sx) + bt * (ny * sy) + nrm * nz.max(0.05)).normalize_or_zero();
                    sums[k] += w.dot(fnr).clamp(-1.0, 1.0).acos() as f64;
                    // swapped roles of the two tilt components
                    let w2 = (tg * (ny * sx) + bt * (nx * sy) + nrm * nz.max(0.05)).normalize_or_zero();
                    sums[k + 4] += w2.dot(fnr).clamp(-1.0, 1.0).acos() as f64;
                }
                cnt += 1;
            }
            if cnt > 0 {
                let deg = |s: f64| (s / cnt as f64).to_degrees();
                eprintln!(
                    "[normal probe] set {set}: {cnt} deviating triangles; mean angle(world normal, face normal): x+y+ {:.1}, x+y- {:.1}, x-y+ {:.1}, x-y- {:.1} | swapped: {:.1}, {:.1}, {:.1}, {:.1}",
                    deg(sums[0]), deg(sums[1]), deg(sums[2]), deg(sums[3]), deg(sums[4]), deg(sums[5]), deg(sums[6]), deg(sums[7])
                );
            }
        }
        if count < 12 {
            return None;
        }
        let mean = tilt / count as f32;
        if std::env::var_os("SHARD_TRACE").is_some() {
            eprintln!("[normals] set {set}: {count} triangles with deviating vertex normals, mean map tilt {mean:.3}");
        }
        // a clear signal only: smooth-bake maps on curved hulls sit around 0.1 (a test vehicle), face-normal
        // bakes of faceted parts well above 0.3 (a test screen)
        Some(mean > 0.2)
    }

    /// Diagnostic: load `<prefix><set>_BaseColor.png`, `_OcclusionRoughnessMetallic.png` and `_Normal.png`
    /// (Painter's own export) as the material of every texture set, to tell texture error from shading error.
    pub fn apply_painter_export(&mut self, prefix: &str) -> Result<usize, String> {
        fn load(path: &str) -> Result<(u32, u32, Vec<u8>), String> {
            let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
            let mut dec = png::Decoder::new(std::io::BufReader::new(f));
            dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
            let mut r = dec.read_info().map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; r.output_buffer_size()];
            let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
            let ch = info.color_type.samples();
            let rgba: Vec<u8> = buf[..info.buffer_size()]
                .chunks_exact(ch)
                .flat_map(|p| match ch {
                    1 => [p[0], p[0], p[0], 255],
                    2 => [p[0], p[0], p[0], p[1]],
                    3 => [p[0], p[1], p[2], 255],
                    _ => [p[0], p[1], p[2], p[3]],
                })
                .collect();
            Ok((info.width, info.height, rgba))
        }
        let flip_green = std::env::var_os("SHARD_EXPORT_NORMAL_FLIPY").is_some();
        let mut done = 0;
        for i in 0..self.stacks.len().min(self.scene.texs.len()) {
            let name = self.stacks[i].name.clone();
            let base = format!("{prefix}{name}_");
            let Ok((w, h, bc)) = load(&format!("{base}BaseColor.png")) else { continue };
            let enc: Vec<f32> = bc.chunks_exact(4).flat_map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
            self.scene.texs[i].set_base_image(w, h, 3, &enc);
            if let Ok((aw, ah, mut orm)) = load(&format!("{base}OcclusionRoughnessMetallic.png")) {
                orm.chunks_exact_mut(4).for_each(|p| p[3] = 255);
                self.render_cmds.push(RenderCmd::SetAux(i, aw, ah, orm));
            }
            if let Ok((nw, nh, mut nrm)) = load(&format!("{base}Normal.png")) {
                if flip_green {
                    nrm.chunks_exact_mut(4).for_each(|p| p[1] = 255 - p[1]);
                }
                nrm.chunks_exact_mut(4).for_each(|p| p[3] = 255);
                self.normal_sent.insert(i);
                self.render_cmds.push(RenderCmd::SetNormal(i, nw, nh, nrm));
            }
            done += 1;
        }
        if done == 0 {
            return Err(format!("no '{prefix}<set>_BaseColor.png' found"));
        }
        self.painter_override = Some(prefix.to_string());
        self.status = format!("Showing Painter's exported textures ({done} set(s))");
        Ok(done)
    }

    /// Baked AO of a set with the island borders padded outwards. The bake has a pure black background and
    /// anti-aliased island edges, so bilinear sampling at a UV seam would otherwise draw a dark line.
    fn padded_ao(&mut self, set: usize) -> Option<std::sync::Arc<shard_compose::Image>> {
        if let Some(a) = self.ao_padded.get(&set) {
            return Some(a.clone());
        }
        let size = self.scene.texs.get(set).map(|t| t.w).unwrap_or(1024);
        let src = {
            // the bake at the set's own size (the shared provider keeps bakes at 1024 at most)
            let url = self.bake_url(set, Bake::AmbientOcclusion).map(str::to_string);
            let full = url.and_then(|u| self.bitmaps.as_ref().and_then(|b| b.bitmap_full(&u, view_res(size).max(1024))));
            match full {
                Some(img) => std::sync::Arc::new(img),
                None => self.bake_image(set, Bake::AmbientOcclusion)?,
            }
        };
        if std::env::var_os("SHARD_NO_AO_PAD").is_some() {
            return Some(src); // diagnostic: the raw bake with its black background
        }
        let cov = crate::export::erode(&crate::export::coverage(&self.scene.mesh, set, src.w, src.h), src.w, src.h);
        let mut data = src.data.clone();
        let iters = (src.w / 32).clamp(8, 64);
        crate::export::dilate_f32(&mut data, src.n, &cov, src.w, src.h, iters);
        let padded = std::sync::Arc::new(shard_compose::Image { w: src.w, h: src.h, n: src.n, data });
        self.ao_padded.insert(set, padded.clone());
        Some(padded)
    }

    /// Auxiliary shading map of a set: R = baked ambient occlusion, G = roughness, B = metallic.
    fn push_aux(&mut self, i: usize, src: &AuxSource) {
        if self.painter_override.is_some() {
            return;
        }
        let ao = self.padded_ao(i);
        if let Some(dir) = std::env::var_os("SHARD_DUMP_BAKES") {
            // diagnostic: write the baked maps of the set (AO, normal) as PNG
            if let Some(a) = &ao {
                let rgba: Vec<u8> = a
                    .data
                    .chunks_exact(a.n)
                    .flat_map(|p| {
                        let g = (p[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        [g, g, g, 255]
                    })
                    .collect();
                let f = std::path::Path::new(&dir).join(format!("set{i}_ao.png"));
                let _ = crate::paint::save_png(&f, a.w, a.h, &rgba);
            }
        }
        // The map is as large as its largest source (the set's own size once its pass is in): never smaller, so
        // the roughness / metallic / AO that arrive at 2048 are not thrown away at 1024 -- and every source is
        // read with its own size (a source is sampled at the pixel of the map, scaled by ITS dimensions).
        let nn: u32 = match src {
            AuxSource::Maps(r, m) => r.w.max(m.w),
            AuxSource::Const(..) => 64,
        }
        .max(ao.as_ref().map(|a| a.w).unwrap_or(0))
        .clamp(64, 4096);
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        // bilinear read of channel 0 at the centre of pixel (x, y) of an `nn` x `nn` map
        let tap = |img: &shard_compose::Image, x: u32, y: u32| -> f32 {
            if img.w == nn && img.h == nn {
                return img.data[(y as usize * img.w as usize + x as usize) * img.n];
            }
            let fx = ((x as f32 + 0.5) * img.w as f32 / nn as f32 - 0.5).clamp(0.0, img.w as f32 - 1.0);
            let fy = ((y as f32 + 0.5) * img.h as f32 / nn as f32 - 0.5).clamp(0.0, img.h as f32 - 1.0);
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(img.w as usize - 1), (y0 + 1).min(img.h as usize - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let v = |xx: usize, yy: usize| img.data[(yy * img.w as usize + xx) * img.n];
            (v(x0, y0) * (1.0 - tx) + v(x1, y0) * tx) * (1.0 - ty) + (v(x0, y1) * (1.0 - tx) + v(x1, y1) * tx) * ty
        };
        let mut data = vec![255u8; (nn as usize) * (nn as usize) * 4];
        {
            use rayon::prelude::*;
            let ao_ref = ao.as_deref();
            data.par_chunks_mut(nn as usize * 4).enumerate().for_each(|(y, row)| {
                for (x, px) in row.chunks_exact_mut(4).enumerate() {
                    let (x, y) = (x as u32, y as u32);
                    let a = ao_ref.map(|a| tap(a, x, y)).unwrap_or(1.0);
                    let (r, m) = match src {
                        AuxSource::Const(r, m) => (*r, *m),
                        AuxSource::Maps(r, m) => (tap(r, x, y), tap(m, x, y)),
                    };
                    px[0] = q(a);
                    px[1] = q(r);
                    px[2] = q(m);
                }
            });
        }
        self.render_cmds.push(RenderCmd::SetAux(i, nn, nn, data));
        if self.normal_sent.insert(i) {
            // the baked normal map never changes: decode it once, at the texture size (up to 2048),
            // convert straight to RGBA8 and keep no float copy
            let cap = std::env::var("SHARD_NORMAL_MAX").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(4096);
            let max = self.scene.texs.get(i).map(|t| t.w.clamp(256, cap)).unwrap_or(1024);
            let full = self
                .bake_url(i, Bake::NormalBase)
                .and_then(|u| self.bitmaps.as_ref().and_then(|b| b.bitmap_full(u, max)));
            if let Some(mut n) = full {
                if n.w == n.h {
                    // the outermost covered ring is blended with the background by the bake: drop it
                    // before extending the borders outwards
                    let cov = crate::export::erode(&crate::export::coverage(&self.scene.mesh, i, n.w, n.h), n.w, n.h);
                    crate::export::dilate_f32(&mut n.data, n.n, &cov, n.w, n.h, (n.w / 64).clamp(8, 32));
                }
                if let Some(face) = self.bake_used_face_normals(i, &n) {
                    // one shading mode for the viewport: the majority of the sets that could tell
                    self.face_votes.insert(i, face);
                    // Diagnostic only: the face-normal mode made faceted bolts/nuts on a test screen, so the
                    // detection is not applied unless SHARD_AUTO_FACE_NORMALS is set (vertex normals stay).
                    let yes = self.face_votes.values().filter(|v| **v).count();
                    let on = std::env::var_os("SHARD_AUTO_FACE_NORMALS").is_some() && yes * 2 > self.face_votes.len();
                    self.render_cmds.push(RenderCmd::FaceNormals(on));
                }
                let mut rgba = Vec::with_capacity((n.w * n.h * 4) as usize);
                for p in n.data.chunks_exact(n.n) {
                    rgba.extend_from_slice(&[q(p[0]), q(p[1.min(n.n - 1)]), q(p[2.min(n.n - 1)]), 255]);
                }
                self.render_cmds.push(RenderCmd::SetNormal(i, n.w, n.h, rgba));
            }
        }
    }

    // ------------------------------------------------------------ undo / redo

    const UNDO_BUDGET: usize = 256 * 1024 * 1024;

    fn push_undo(&mut self, e: UndoEntry) {
        self.edit_rev += 1;
        self.redo_stack.clear();
        self.undo_stack.push(e);
        let mut total: usize = self.undo_stack.iter().map(UndoEntry::bytes).sum();
        while total > Self::UNDO_BUDGET && self.undo_stack.len() > 1 {
            total -= self.undo_stack.remove(0).bytes();
        }
    }

    /// Mouse down in the viewport: start recording the stroke.
    pub fn begin_stroke(&mut self) {
        if self.mask_edit.is_none() {
            self.scene.begin_stroke();
        }
    }

    /// Mouse up: store the stroke's original tiles in the undo history.
    pub fn end_stroke(&mut self) {
        self.quick_note_brush();
        if self.mask_edit.is_some() {
            self.finish_mask_stroke();
            return;
        }
        if let Some((set, tiles)) = self.scene.end_stroke() {
            self.push_undo(UndoEntry::Paint { set, tiles });
        }
    }

    /// Record a layer-stack change of set `set` (called when the stack differs from the committed one).
    fn record_layer_change(&mut self, set: usize) {
        let Some(st) = self.stacks.get_mut(set) else { return };
        if st.layers == st.committed {
            return;
        }
        let before = std::mem::replace(&mut st.committed, st.layers.clone());
        let before_session = std::mem::replace(&mut st.committed_session, st.session_layers);
        let merge = self.last_layer_undo.is_some_and(|t| t.elapsed().as_millis() < 700)
            && matches!(self.undo_stack.last(), Some(UndoEntry::Layers { set: s, .. }) if *s == set);
        self.last_layer_undo = Some(std::time::Instant::now());
        if merge {
            self.redo_stack.clear(); // keep the older "before" state, renamed for the merged change
            let now = &self.stacks[set].layers;
            if let Some(UndoEntry::Layers { layers, label, .. }) = self.undo_stack.last_mut() {
                *label = shard_model::edit::describe_change(layers, now);
            }
        } else {
            let label = shard_model::edit::describe_change(&before, &self.stacks[set].layers);
            self.push_undo(UndoEntry::Layers { set, layers: before, session: before_session, label });
        }
    }

    fn apply_undo_entry(&mut self, e: UndoEntry) -> UndoEntry {
        match e {
            UndoEntry::Paint { set, tiles } => {
                let prev = self.scene.texs.get_mut(set).map(|t| t.swap_tiles(tiles)).unwrap_or_default();
                UndoEntry::Paint { set, tiles: prev }
            }
            UndoEntry::Layers { set, layers, session, label } => {
                trace_view("undo_layers");
                // keep an open mask view (re-evaluated after the undo) unless its layer is gone
                let keep = self.mask_preview.as_ref().is_some_and(|(s, _)| *s == set);
                let st = &mut self.stacks[set];
                // the selected layer stays selected when it still exists after the change (by identity)
                let sel_uid = st.selected.as_ref().and_then(|p| shard_model::edit::uid_at(&st.layers, p));
                let cur = UndoEntry::Layers {
                    set,
                    layers: std::mem::replace(&mut st.layers, layers),
                    session: st.session_layers,
                    label,
                };
                st.session_layers = session;
                st.committed = st.layers.clone();
                st.committed_session = session;
                st.selected = sel_uid.and_then(|u| shard_model::edit::path_of_uid(&st.layers, u));
                self.last_layer_undo = None;
                if keep {
                    let path = self.mask_preview.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
                    let exists = layer_at_mut(&mut self.stacks[set].layers, &path)
                        .is_some_and(|l| l.mask.is_some());
                    if exists {
                        self.mask_reeval = true;
                    } else {
                        self.mask_edit = None;
                        self.mask_preview = None;
                        self.set_view(self.view);
                    }
                } else {
                    self.mask_edit = None;
                    self.mask_preview = None;
                }
                self.do_recompose(Some(set));
                cur
            }
        }
    }

    pub fn undo(&mut self) {
        self.edit_rev += 1;
        match self.undo_stack.pop() {
            Some(e) => {
                let label = e.label().to_string();
                let inv = self.apply_undo_entry(e);
                self.redo_stack.push(inv);
                self.status = format!("Undo: {label}");
            }
            None => self.status = "Nothing to undo".into(),
        }
    }

    pub fn redo(&mut self) {
        self.edit_rev += 1;
        match self.redo_stack.pop() {
            Some(e) => {
                let label = e.label().to_string();
                let inv = self.apply_undo_entry(e);
                self.undo_stack.push(inv);
                self.status = format!("Redo: {label}");
            }
            None => self.status = "Nothing to redo".into(),
        }
    }

    /// Names of the recorded edits, oldest first (the History panel's list).
    pub fn undo_labels(&self) -> Vec<String> {
        self.undo_stack.iter().map(|e| e.label().to_string()).collect()
    }

    pub fn clear_undo(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_layer_undo = None;
    }

    /// Change the texture size of a set (its hand-painted overlay is reset) and recompose it.
    pub fn resize_set(&mut self, set: usize, w: u32, h: u32) {
        self.undo_stack.retain(|e| !matches!(e, UndoEntry::Paint { set: s, .. } if *s == set));
        self.redo_stack.retain(|e| !matches!(e, UndoEntry::Paint { set: s, .. } if *s == set));
        if let Some(t) = self.scene.texs.get_mut(set) {
            *t = crate::paint::PaintTexture::new(w, h);
            self.do_recompose(Some(set));
            self.status = format!(
                "Texture set '{}' resized to {w}x{h} (paint on this set was reset)",
                self.stacks.get(set).map(|s| s.name.as_str()).unwrap_or("?")
            );
        }
    }

    /// Painter's default environment ("Studio Tomoco") when it is installed; otherwise nothing changes.
    pub fn use_default_environment(&mut self) {
        if let Some(p) = studio_tomoco() {
            self.load_hdri_path(&p);
        }
    }

    /// Use the `i`-th file of the HDRI folder as environment (lighting, reflections and background).
    pub fn load_hdri(&mut self, i: usize) {
        let Some(path) = self.hdri_files.get(i).cloned() else { return };
        self.load_hdri_path(&path);
        self.hdri_current = Some(i);
    }

    /// Preview of an HDRI (a tone-mapped 2:1 picture made on a worker thread, cached on disk); None while it is
    /// being made or when the file cannot be read.
    fn hdri_thumb(&mut self, ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureHandle> {
        if let Some(rx) = &self.hdri_thumb_rx {
            while let Ok((p, img)) = rx.try_recv() {
                let tex = img.map(|(w, h, px)| {
                    ctx.load_texture(
                        format!("hdri-{}", p.display()),
                        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px),
                        egui::TextureOptions::LINEAR,
                    )
                });
                self.hdri_thumbs.insert(p, tex);
            }
        }
        if let Some(t) = self.hdri_thumbs.get(path) {
            return t.clone();
        }
        if self.hdri_thumb_tx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (rtx, rrx) = std::sync::mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || hdri_thumb_worker(rx, rtx, c));
            self.hdri_thumb_tx = Some(tx);
            self.hdri_thumb_rx = Some(rrx);
        }
        if let Some(tx) = &self.hdri_thumb_tx {
            let _ = tx.send(path.to_path_buf());
        }
        self.hdri_thumbs.insert(path.to_path_buf(), None);
        None
    }

    /// Environment from an `.exr` file (HDRI folder or a library).
    pub fn load_hdri_path(&mut self, path: &std::path::Path) {
        match crate::env::Environment::from_exr(path) {
            Ok(e) => {
                self.status = format!("Environment: {} ({}x{})", e.name, e.width, e.height);
                self.env = e;
                self.env_dirty = true;
                self.hdri_current = self.hdri_files.iter().position(|p| p == path);
            }
            Err(e) => self.status = format!("HDRI failed: {e}"),
        }
    }

    /// Re-scan the configured libraries; their environments join the HDRI selector.
    pub fn rescan_libraries(&mut self) {
        self.assets = crate::library::scan(&self.config.libraries);
        let mut files = scan_hdri_dir();
        for a in &self.assets {
            if a.kind == crate::library::AssetKind::Environment
                && a.path.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("exr"))
                && !files.contains(&a.path)
            {
                files.push(a.path.clone());
            }
        }
        // the environment in use keeps its place in the new list
        let current = self.hdri_current.and_then(|i| self.hdri_files.get(i).cloned());
        self.hdri_current = current.and_then(|c| files.iter().position(|p| *p == c));
        self.hdri_files = files;
    }

    /// Double-click on a shelf asset.
    fn use_asset(&mut self, i: usize) {
        use crate::library::AssetKind;
        let Some(a) = self.assets.get(i).cloned() else { return };
        match a.kind {
            AssetKind::SmartMaterial | AssetKind::SmartMask => self.open_document(&a.path),
            AssetKind::Environment => {
                if a.path.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("exr")) {
                    self.load_hdri_path(&a.path);
                } else {
                    self.status = format!("{}: only .exr environments are supported for now", a.name);
                }
            }
            k => self.status = format!("{} ({}): using this asset type is not implemented yet", a.name, k.label()),
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        const PAGES: [(Icon, &str, &str); 5] = [
            (Icon::Shader, "General", "Procedural engine and autosave"),
            (Icon::Brush, "Appearance", "Highlight and loading colors"),
            (Icon::Display, "Viewport", "Render mode and antialiasing"),
            (Icon::Folder, "Libraries", "Asset folders shown in the shelf"),
            (Icon::Select, "Shortcuts", "Substance 3D Painter's keys"),
        ];
        let screen = ctx.screen_rect();
        let size = vec2(820.0_f32.min(screen.width() - 40.0), 580.0_f32.min(screen.height() - 60.0));
        let frame = egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(14)
            .shadow(egui::epaint::Shadow { offset: [0, 14], blur: 44, spread: 0, color: Color32::from_black_alpha(170) });
        let mut close = false;
        self.settings_tab = self.settings_tab.min(PAGES.len() - 1);
        // modal: the program dims behind the window; Esc or a click outside closes it
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .area(egui::Modal::default_area(egui::Id::new("settings")).fade_in(false))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                let (full, _) = ui.allocate_exact_size(size, Sense::hover());
                // ---- navigation
                let side = Rect::from_min_size(full.min, vec2(200.0, full.height()));
                ui.painter().rect_filled(
                    side,
                    egui::CornerRadius { nw: 14, sw: 14, ne: 0, se: 0 },
                    theme::PANEL_ALT,
                );
                ui.painter().line_segment([side.right_top(), side.right_bottom()], Stroke::new(1.0, theme::BORDER));
                let mut nav = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink2(vec2(12.0, 18.0))).layout(Layout::top_down(Align::Min)));
                nav.label(RichText::new("Settings").font(theme::semibold(17.0)).color(Color32::WHITE));
                nav.label(RichText::new("ShardStroke preferences").size(11.0).color(theme::TEXT_DIM));
                nav.add_space(16.0);
                for (k, (icon, name, _)) in PAGES.iter().enumerate() {
                    let (r, resp) = nav.allocate_exact_size(vec2(nav.available_width(), 34.0), Sense::click());
                    let sel = self.settings_tab == k;
                    let p = nav.painter();
                    if sel {
                        p.rect_filled(r, 8.0, theme::accent_soft());
                        p.rect_filled(Rect::from_min_size(r.min + vec2(0.0, 8.0), vec2(3.0, r.height() - 16.0)), 2.0, theme::accent());
                    } else if resp.hovered() {
                        p.rect_filled(r, 8.0, theme::WIDGET);
                    }
                    let col = if sel { Color32::WHITE } else { theme::TEXT };
                    crate::icons::draw(p, Rect::from_center_size(pos2(r.min.x + 22.0, r.center().y), vec2(16.0, 16.0)), *icon, if sel { theme::accent_hover() } else { theme::TEXT_DIM });
                    p.text(pos2(r.min.x + 40.0, r.center().y), Align2::LEFT_CENTER, *name, theme::semibold(12.5), col);
                    if resp.hovered() {
                        nav.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        self.settings_tab = k;
                    }
                    nav.add_space(2.0);
                }
                // ---- page
                let content = Rect::from_min_max(pos2(side.max.x, full.min.y), full.max);
                let close_r = Rect::from_center_size(pos2(content.max.x - 24.0, content.min.y + 24.0), vec2(26.0, 26.0));
                let cresp = ui.interact(close_r, ui.id().with("settings_close"), Sense::click());
                if cresp.hovered() {
                    ui.painter().rect_filled(close_r, 6.0, theme::WIDGET_HOVER);
                }
                crate::icons::draw(ui.painter(), close_r.shrink(7.0), Icon::Close, if cresp.hovered() { Color32::WHITE } else { theme::TEXT_DIM });
                if cresp.on_hover_text("Close (Esc)").clicked() {
                    close = true;
                }
                let mut page = ui.new_child(
                    egui::UiBuilder::new().max_rect(content.shrink2(vec2(26.0, 20.0))).layout(Layout::top_down(Align::Min)),
                );
                let (_, title, sub) = PAGES[self.settings_tab];
                page.label(RichText::new(title).font(theme::semibold(19.0)).color(Color32::WHITE));
                page.label(RichText::new(sub).size(11.5).color(theme::TEXT_DIM));
                page.add_space(14.0);
                egui::ScrollArea::vertical().id_salt(("settings_page", self.settings_tab)).auto_shrink([false, false]).show(&mut page, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    match self.settings_tab {
                        0 => self.settings_general(ui),
                        1 => self.settings_appearance(ui, ctx),
                        2 => self.settings_viewport(ui),
                        3 => self.settings_libraries(ui),
                        _ => settings_shortcuts(ui),
                    }
                    ui.add_space(8.0);
                });
            });
        if close || modal.should_close() {
            self.settings_open = false;
        }
    }

    fn settings_general(&mut self, ui: &mut egui::Ui) {
        let installed = shard_engine::oracle::installed_path();
        settings_card(ui, "Procedural engine", |ui| {
            let on = !self.config.substance_engine_off;
            let mut toggled = false;
            settings_row(
                ui,
                "Substance engine",
                "Render grunges, generators and filters with the installed Substance 3D Designer renderer (exact). \
                 Off: the built-in approximations.",
                |ui| toggled = w::switch(ui, on, installed.is_some()).clicked(),
            );
            if toggled {
                let on = !on;
                self.config.substance_engine_off = !on;
                self.config.save();
                shard_engine::oracle::set_enabled(on);
                self.mark_recompose();
                self.mask_reeval = true;
                self.refine_done.iter_mut().for_each(|d| *d = false);
                self.do_recompose(None);
                self.status = if on { "Substance engine on" } else { "Substance engine off: native approximations" }.into();
            }
            settings_divider(ui);
            let where_ = match &installed {
                Some(p) => p.display().to_string(),
                None => "Substance 3D Designer was not found: procedurals use the built-in approximations.".into(),
            };
            settings_row(ui, "Renderer", &where_, |_| {});
            settings_divider(ui);
            let mb = shard_engine::oracle::cache_bytes() as f64 / (1024.0 * 1024.0);
            settings_row(ui, "Render cache", &format!("{mb:.0} MB of rendered procedurals on disk."), |ui| {
                if ui.button("Clear cache").clicked() {
                    shard_engine::oracle::clear_cache();
                }
            });
        });
        settings_note(
            ui,
            "The renderer is used as an external tool for your own use: exact textures for grunges, generators and \
             filters. It is never bundled.",
        );
        settings_card(ui, "Autosave", |ui| {
            settings_row(ui, "Backup interval", "Backups are <project>_autosave_0.shard / _1.shard next to the project file.", |ui| {
                let mut secs = self.config.autosave_secs;
                let choices = [("Off", 0u32), ("1 min", 60), ("2 min", 120), ("5 min", 300), ("10 min", 600)];
                let current = choices.iter().find(|c| c.1 == secs).map(|c| c.0.to_string()).unwrap_or_else(|| format!("{secs} s"));
                egui::ComboBox::from_id_salt("autosave_secs").width(110.0).selected_text(current).show_ui(ui, |ui| {
                    for (label, v) in choices {
                        ui.selectable_value(&mut secs, v, label);
                    }
                });
                if secs != self.config.autosave_secs {
                    self.config.autosave_secs = secs;
                    self.config.save();
                }
            });
        });
    }

    fn settings_appearance(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        settings_card(ui, "Highlight color", |ui| {
            let mut chosen: Option<[u8; 3]> = None;
            settings_row(ui, "Accent", "The only colored element of the interface: active tool, selection, sliders, focus.", |ui| {
                let cur = theme::accent_rgb();
                let mut c = Color32::from_rgb(cur[0], cur[1], cur[2]);
                if ui.color_edit_button_srgba(&mut c).on_hover_text("Custom color").changed() {
                    chosen = Some([c.r(), c.g(), c.b()]);
                }
                ui.add_space(6.0);
                for (name, rgb) in theme::ACCENT_PRESETS.iter().rev() {
                    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 9.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                    if cur == *rgb {
                        ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(2.0, Color32::WHITE));
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.on_hover_text(*name).clicked() {
                        chosen = Some(*rgb);
                    }
                }
            });
            if let Some(rgb) = chosen {
                theme::set_accent(rgb);
                theme::apply_visuals(ctx);
                self.config.highlight = Some(rgb);
                self.config.save();
            }
        });
        settings_card(ui, "Loading", |ui| {
            // a texture set whose composition is on its way: stripes and an outline in the viewport
            let mut changed = false;
            let rows = [
                ("Stripes", "Drawn over a texture set while its composition is on its way.", crate::config::LOADING_FILL, true),
                ("Outline", "Outline of that texture set.", crate::config::LOADING_OUTLINE, false),
            ];
            for (k, (label, desc, default, fill)) in rows.into_iter().enumerate() {
                if k > 0 {
                    settings_divider(ui);
                }
                let value = if fill { &mut self.config.loading_fill } else { &mut self.config.loading_outline };
                settings_row(ui, label, desc, |ui| {
                    let mut c = Color32::from_rgb(value[0], value[1], value[2]);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        *value = [c.r(), c.g(), c.b()];
                        changed = true;
                    }
                    if *value != default && ui.small_button("Reset").clicked() {
                        *value = default;
                        changed = true;
                    }
                });
            }
            if changed {
                self.config.save();
            }
        });
    }

    fn settings_viewport(&mut self, ui: &mut egui::Ui) {
        settings_card(ui, "Antialiasing", |ui| {
            settings_row(ui, "Multisampling", "Smooths the edges of the mesh. Higher is smoother and costs more GPU time.", |ui| {
                let levels = [(1u32, "Off"), (2, "2x"), (4, "4x"), (8, "8x")];
                let items: Vec<(&str, bool, &str)> =
                    levels.iter().map(|(n, l)| (*l, self.msaa_supported.contains(n), "Not supported by this GPU")).collect();
                let want = self.config.viewport_msaa;
                let cur = levels.iter().rposition(|(n, _)| *n <= want && self.msaa_supported.contains(n)).unwrap_or(0);
                ui.allocate_ui(vec2(220.0, 26.0), |ui| {
                    if let Some(k) = w::segmented(ui, &items, cur) {
                        self.config.viewport_msaa = levels[k].0;
                        self.config.save();
                    }
                });
            });
        });
        settings_card(ui, "Render", |ui| {
            settings_row(
                ui,
                "Mode",
                "Real-time: rasterized physically based shading with image-based lighting. Path tracing: full ray traced \
                 render, with lighting from the environment, real shadows and reflections. Same as the two spheres in the \
                 viewport.",
                |ui| {
                    let items = [("Real-time", true, ""), ("Path tracing", self.path_trace_supported, "Not supported by this GPU")];
                    ui.allocate_ui(vec2(220.0, 26.0), |ui| {
                        if let Some(k) = w::segmented(ui, &items, self.path_trace as usize) {
                            self.path_trace = k == 1;
                        }
                    });
                },
            );
        });
        let mut ai_toggled = false;
        settings_card(ui, "Path tracing", |ui| {
            settings_row(
                ui,
                "Samples",
                &format!(
                    "One sample per pixel each frame while the view is still, up to {}. Moving the camera or editing starts again.",
                    crate::pathtrace::MAX_SAMPLES
                ),
                |ui| {
                    let text = match (self.path_trace, self.path_trace_samples) {
                        (true, Some(n)) => format!("{n} / {}", crate::pathtrace::MAX_SAMPLES),
                        _ => "Not in use".into(),
                    };
                    ui.label(RichText::new(text).size(11.0).color(if self.path_trace { theme::accent_hover() } else { theme::TEXT_DIM }));
                },
            );
            settings_divider(ui);
            settings_row(ui, "Denoise while moving", "A fast filter cleans the picture on every frame, so it never shows grain.", |ui| {
                ui.label(RichText::new("Always on").size(11.0).color(theme::TEXT_DIM));
            });
            settings_divider(ui);
            let installed = crate::oidn::available();
            settings_row(
                ui,
                "AI denoise when still",
                "When the view stops, Intel Open Image Denoise cleans the accumulated picture (at 32, 256, 1024 and 4096 \
                 samples), on the GPU when it can.",
                |ui| {
                    if installed {
                        ai_toggled = w::switch(ui, self.config.ai_denoise, true).clicked();
                    } else {
                        ui.label(RichText::new("Not installed").size(11.0).color(theme::TEXT_DISABLED));
                    }
                },
            );
        });
        if ai_toggled {
            self.config.ai_denoise = !self.config.ai_denoise;
            self.config.save();
        }
        settings_card(ui, "Statistics", |ui| {
            let mut toggled = false;
            settings_row(
                ui,
                "Show statistics",
                "Frame rate, frame time (CPU and GPU) and the CPU / GPU usage of ShardStroke in the corner of the viewport. While \
                 shown, the viewport draws continuously.",
                |ui| toggled = w::switch(ui, self.config.show_stats, true).clicked(),
            );
            if toggled {
                self.config.show_stats = !self.config.show_stats;
                self.config.save();
            }
        });
        settings_note(ui, "Environment, exposure, color profile and post effects are in the viewport's shading menu (the arrow next to the spheres).");
    }

    fn settings_libraries(&mut self, ui: &mut egui::Ui) {
        let (mut remove, mut make_default, mut changed) = (None, None, false);
        settings_card(ui, "Your libraries", |ui| {
            if self.config.libraries.is_empty() {
                ui.label(RichText::new("No libraries configured yet.").size(11.5).color(theme::TEXT_DIM));
            }
            for (i, l) in self.config.libraries.iter().enumerate() {
                if i > 0 {
                    settings_divider(ui);
                }
                settings_row(ui, &l.name, &l.path.display().to_string(), |ui| {
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                    if l.default {
                        ui.label(RichText::new("Default").size(11.0).color(theme::accent_hover()));
                    } else if ui.button("Make default").clicked() {
                        make_default = Some(i);
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Add folder...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().set_title("Add a library folder").pick_folder() {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("library").to_string();
                        changed |= self.config.add_library(&name, &p);
                    }
                }
                if ui.button("Rescan").clicked() {
                    changed = true;
                }
            });
        });
        let detected: Vec<_> = crate::config::detect_painter_libraries()
            .into_iter()
            .filter(|(_, p)| !self.config.libraries.iter().any(|l| &l.path == p))
            .collect();
        if !detected.is_empty() {
            settings_card(ui, "Found on this computer", |ui| {
                for (k, (name, p)) in detected.iter().enumerate() {
                    if k > 0 {
                        settings_divider(ui);
                    }
                    settings_row(ui, name, &p.display().to_string(), |ui| {
                        if ui.button("Add").clicked() {
                            changed |= self.config.add_library(name, p);
                        }
                    });
                }
            });
        }
        settings_note(
            ui,
            "Libraries are read in place from your own folders. ShardStroke never copies, bundles or redistributes their \
             content; check the licence of any third-party assets you use.",
        );
        if let Some(i) = make_default {
            self.config.set_default(i);
            changed = true;
        }
        if let Some(i) = remove {
            self.config.remove_library(i);
            changed = true;
        }
        if changed {
            self.config.save();
            self.rescan_libraries();
            self.status = format!("Libraries: {} asset(s) in {} location(s)", self.assets.len(), self.config.libraries.len());
        }
    }

    /// Painter's "display next/previous channel" (`C`) and "mesh map" (`B`) shortcuts.
    pub fn cycle_view(&mut self, group: ViewGroup, dir: i32) {
        let list: Vec<ViewKind> = VIEW_CHOICES
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| match (group, k) {
                (ViewGroup::Channels, ViewKind::Channel(_)) => true,
                (ViewGroup::MeshMaps, ViewKind::Bake(_)) => true,
                _ => false,
            })
            .collect();
        if list.is_empty() {
            return;
        }
        let next = match list.iter().position(|k| *k == self.view) {
            Some(i) => list[(i as i32 + dir).rem_euclid(list.len() as i32) as usize],
            None => list[0],
        };
        self.set_view(next);
        self.status = format!("View: {}", VIEW_CHOICES.iter().find(|(k, _)| *k == next).map(|(_, l)| *l).unwrap_or("?"));
    }

    pub fn set_view(&mut self, v: ViewKind) {
        trace_view("set_view");
        self.mask_preview = None;
        self.mask_edit = None;
        self.view = v;
        self.refresh_view();
    }

    /// Rebuild the per-set images of the selected channel / mesh-map view.
    fn refresh_view(&mut self) {
        trace_view("refresh_view");
        self.last_view_refresh = std::time::Instant::now();
        let images: Option<Vec<(u32, u32, Vec<u8>)>> = match self.view {
            ViewKind::Material => None,
            ViewKind::Bake(b) => Some(
                (0..self.stacks.len())
                    .map(|i| match self.bake_image(i, b) {
                        Some(img) => (img.w, img.h, image_rgba8(&img)),
                        None => (1, 1, vec![40, 40, 40, 255]), // set has no such map
                    })
                    .collect(),
            ),
            ViewKind::Channel(bit) => {
                let mut out: Vec<(u32, u32, Vec<u8>)> = Vec::new();
                for i in 0..self.stacks.len() {
                    let size = self.scene.texs.get(i).map(|t| t.w).unwrap_or(1024);
                    let full = view_res(size);
                    let have = self.view_cache.get(&(i, bit)).map(|e| e.0).unwrap_or(0);
                    // every pass the set goes through (1024, then its own size) is asked for once
                    for want in [size.clamp(64, 1024), full] {
                        if have < want && self.view_requested.insert((i, bit, want)) {
                            self.queue_compose(i, want, vec![bit], false);
                        }
                    }
                    match self.view_cache.get(&(i, bit)) {
                        Some((_, w, h, comps, bytes)) => {
                            let mut rgba = vec![255u8; (*w as usize) * (*h as usize) * 4];
                            for (px, src) in rgba.chunks_exact_mut(4).zip(bytes.chunks_exact(*comps as usize)) {
                                px[0] = src[0];
                                px[1] = src[(*comps as usize > 1) as usize];
                                px[2] = src[if *comps as usize > 2 { 2 } else { 0 }];
                            }
                            out.push((*w, *h, rgba));
                        }
                        // not composed yet: dark until the pass arrives (the view refreshes by itself)
                        None => out.push((1, 1, vec![40, 40, 40, 255])),
                    }
                }
                if out.iter().any(|(w, _, _)| *w == 1) {
                    self.status = "Composing the channel view in the background...".to_string();
                }
                Some(out)
            }
        };
        self.render_cmds.push(RenderCmd::SetView(images));
    }

    pub fn draw(&mut self, ctx: &egui::Context) {
        // every new status message goes to the Log panel
        if self.status != self.log_last {
            self.log_last = self.status.clone();
            self.log.push((self.started.elapsed().as_secs_f32(), self.status.clone()));
            if self.log.len() > 2000 {
                self.log.drain(..500);
            }
        }
        // the Open dialog runs on its own thread: look for its answer every frame, and keep frames coming meanwhile
        self.poll_open_dialog();
        if self.open_dialog_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // diagnostic: SHARD_REOPEN="seconds;path" opens `path` that many seconds after the start (the File > Open
        // path of a project opened over another one, which the command line cannot reproduce)
        if let Some(spec) = self.reopen_spec.take() {
            match spec.split_once(';') {
                Some((secs, path)) if self.reopen_clock.elapsed().as_secs_f32() >= secs.parse().unwrap_or(0.0) => {
                    self.open_document(&PathBuf::from(path));
                }
                _ => self.reopen_spec = Some(spec),
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        // diagnostic: SHARD_TOGGLE="seconds;layer name" toggles the eye of that layer of the active set once, like a
        // click in the layers panel (to time what an edit costs)
        if let Ok(spec) = std::env::var("SHARD_TOGGLE") {
            static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if let Some((secs, name)) = spec.split_once(';') {
                if !DONE.load(std::sync::atomic::Ordering::Relaxed) && self.reopen_clock.elapsed().as_secs_f32() >= secs.parse().unwrap_or(0.0) {
                    fn flip(ls: &mut [Layer], name: &str) -> bool {
                        for l in ls.iter_mut() {
                            if l.name == name {
                                l.visible = !l.visible;
                                return true;
                            }
                            if flip(&mut l.children, name) {
                                return true;
                            }
                        }
                        false
                    }
                    let a = self.active();
                    if let Some(st) = self.stacks.get_mut(a) {
                        if flip(&mut st.layers, name) {
                            DONE.store(true, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("[toggle] {:.1}s: '{name}' toggled", self.reopen_clock.elapsed().as_secs_f32());
                            self.mark_recompose();
                        }
                    }
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        self.sync_stacks();
        self.poll_refine();
        self.pump_thumbs(ctx);
        self.settings_window(ctx);
        self.export_window(ctx);
        self.recovery_window(ctx);
        self.pump_previews(ctx);
        self.picker_window(ctx);
        self.scene.active_set = self.active();
        // the window is the background; menu and status bars sit on it, the tool strips and the dock windows
        // are rounded cards with a gap between them
        let bar = |h: i8| egui::Frame::new().fill(theme::bar()).inner_margin(egui::Margin::symmetric(10, h));
        let gap = theme::GAP as i8;
        let card = |m: i8, outer: egui::Margin| {
            egui::Frame::new()
                .fill(theme::card())
                .corner_radius(theme::RADIUS_CARD)
                .inner_margin(egui::Margin::same(m))
                .outer_margin(outer)
        };
        egui::TopBottomPanel::top("menu").exact_height(30.0).frame(bar(2)).show(ctx, |ui| {
            self.menu_bar(ui)
        });
        egui::TopBottomPanel::bottom("status").exact_height(24.0).frame(bar(2)).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(&self.status).color(theme::TEXT_DIM).size(11.5));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}  |  {} tris  |  ShardStroke 0.1 (wgpu)",
                            self.scene.mesh_name,
                            self.scene.mesh.triangle_count()
                        ))
                        .color(theme::TEXT_DIM)
                        .size(11.5),
                    );
                });
            });
        });
        let left = egui::Margin { left: gap, right: 0, top: gap, bottom: gap };
        let right = egui::Margin { left: 0, right: gap, top: gap, bottom: gap };
        egui::SidePanel::left("tools")
            .exact_width(46.0)
            .resizable(false)
            .frame(card(4, left))
            .show(ctx, |ui| self.tool_strip(ui));
        egui::SidePanel::right("dockstrip")
            .exact_width(46.0)
            .resizable(false)
            .frame(card(4, right))
            .show(ctx, |ui| self.dock_strip(ui));
        // every other window is a tab of the dock: drag a tab to move it, drop it on an edge to split
        let mut dock = std::mem::replace(&mut self.dock, egui_dock::DockState::new(Vec::new()));
        {
            let active: Vec<Pane> = dock
                .iter_all_nodes()
                .filter_map(|(_, n)| match n {
                    egui_dock::Node::Leaf { tabs, active, .. } => tabs.get(active.0).copied(),
                    _ => None,
                })
                .collect();
            let mut viewer = PaneViewer { app: self, active };
            egui_dock::DockArea::new(&mut dock)
                .style(dock::style(ctx))
                .show_close_buttons(true)
                .show_add_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show(ctx, &mut viewer);
        }
        self.dock = dock;
        if let Some(l) = self.pending_layout.take() {
            self.dock = l;
        }
        if let Some(p) = self.pending_focus.take() {
            self.show_pane(p);
        }
        // the free area between the windows: the scene fills the window and is centred in it
        let screen = ctx.screen_rect();
        if let Some(free) = dock::spacer_rect(&self.dock).filter(|r| r.is_finite() && r.width() > 8.0) {
            self.central = free;
        } else if self.dock.main_surface().iter().all(|n| n.tabs_count() > 0) {
            self.central = screen;
        }
        let c = self.central.center();
        self.scene.cam.center = glam::Vec2::new(
            (c.x - screen.center().x) / (screen.width() * 0.5).max(1.0),
            -(c.y - screen.center().y) / (screen.height() * 0.5).max(1.0),
        );
        self.viewport_chrome(ctx);
        self.quick_panel(ctx);
        self.save_layout_when_idle(ctx);
        if self.recompose {
            let a = self.active();
            self.record_layer_change(a);
            self.do_recompose(Some(a));
        }
    }

    // ------------------------------------------------------------ menu

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().button_padding = vec2(8.0, 2.0);
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                ui.set_min_width(220.0);
                ui.add_enabled(false, egui::Button::new("New project...   Ctrl+N"));
                if ui.button("Open...   Ctrl+O").clicked() {
                    ui.close_menu();
                    self.open_dialog();
                }
                ui.add_enabled(false, egui::Button::new("Recent projects"));
                ui.add_enabled(false, egui::Button::new("Open sample project..."));
                ui.separator();
                ui.add_enabled(false, egui::Button::new("Close   Ctrl+F4"));
                if ui.button("Save   Ctrl+S").clicked() {
                    ui.close_menu();
                    self.save_current(false);
                }
                if ui.button("Save as...   Ctrl+Shift+S").clicked() {
                    ui.close_menu();
                    self.save_current(true);
                }
                ui.separator();
                if ui.button("Import mesh (OBJ)...").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().add_filter("OBJ", &["obj"]).pick_file() {
                        self.status = match self.scene.load_mesh(&p) {
                            Ok(()) => {
                                self.mesh_dirty = true;
                                format!("Loaded mesh {}", p.display())
                            }
                            Err(e) => format!("Mesh load failed: {e}"),
                        };
                    }
                }
                if ui.button("Export Textures...   Ctrl+Shift+E").clicked() {
                    ui.close_menu();
                    self.open_export();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    self.quit = true;
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(!self.undo_stack.is_empty(), egui::Button::new("Undo   Ctrl+Z")).clicked() {
                    self.undo();
                    ui.close_menu();
                }
                if ui.add_enabled(!self.redo_stack.is_empty(), egui::Button::new("Redo   Ctrl+Y")).clicked() {
                    self.redo();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Clear paint").clicked() {
                    self.scene.clear_paint();
                    ui.close_menu();
                }
                ui.add_enabled(false, egui::Button::new("Project Configuration..."));
                if ui.button("Settings...").clicked() {
                    self.settings_open = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("Mode", |ui| {
                ui.add_enabled(false, egui::Button::new("Bake Mesh Maps   F8"));
                ui.button("Painting   F9");
                ui.add_enabled(false, egui::Button::new("Rendering   F10"));
            });
            ui.menu_button("Window", |ui| {
                ui.set_min_width(200.0);
                ui.menu_button("Views", |ui| {
                    for pane in Pane::ALL {
                        let shown = self.dock.find_tab(&pane).is_some();
                        let mut on = shown;
                        let label = pane.title();
                        if ui.add(egui::Checkbox::new(&mut on, label)).clicked() {
                            if on {
                                self.show_pane(pane);
                            } else if let Some(at) = self.dock.find_tab(&pane) {
                                self.dock.remove_tab(at);
                            }
                        }
                    }
                });
                ui.add_enabled(false, egui::Button::new("Toolbars"));
                if ui.button("Save UI Layout...").clicked() {
                    ui.close_menu();
                    if let (Some(json), Some(path)) = (
                        dock::layout_json(&self.dock),
                        rfd::FileDialog::new().add_filter("ShardStroke UI layout", &["json"]).set_file_name("layout.json").save_file(),
                    ) {
                        self.status = match std::fs::write(&path, json) {
                            Ok(()) => format!("UI layout saved to {}", path.display()),
                            Err(e) => format!("UI layout not saved: {e}"),
                        };
                    }
                }
                if ui.button("Load UI Layout...").clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new().add_filter("ShardStroke UI layout", &["json"]).pick_file() {
                        match std::fs::read_to_string(&path).ok().and_then(|t| dock::parse_layout(&t)) {
                            Some(l) => self.dock = l,
                            None => self.status = format!("{} is not a UI layout", path.display()),
                        }
                    }
                }
                ui.add_enabled(false, egui::Button::new("Hide UI   Tab"));
                if ui.button("Reset UI").clicked() {
                    ui.close_menu();
                    self.dock = dock::default_layout();
                }
            });
            ui.menu_button("Viewport", |ui| {
                if ui.button("Show entire mesh   F").clicked() {
                    self.scene.cam = crate::scene::Camera::new();
                    ui.close_menu();
                }
                ui.add_enabled(false, egui::Button::new("Display next channel   C"));
                ui.add_enabled(false, egui::Button::new("Display material   M"));
            });
            ui.add_enabled(false, egui::Button::new("JavaScript").frame(false));
            ui.add_enabled(false, egui::Button::new("Python").frame(false));
            ui.menu_button("Help", |ui| {
                ui.label("ShardStroke (early build)");
            });
        });
    }

    // ------------------------------------------------------------ tool strip / dock strip

    fn tool_strip(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            let items: [(Icon, Option<Tool>, &str); 7] = [
                (Icon::Brush, Some(Tool::Paint), "Paint (1)"),
                (Icon::Eraser, Some(Tool::Eraser), "Eraser (2)"),
                (Icon::Cube, None, "Projection (3)"),
                (Icon::PolyFill, None, "Polygon Fill (4)"),
                (Icon::Smudge, None, "Smudge (5)"),
                (Icon::Clone, None, "Clone (6)"),
                (Icon::Picker, None, "Material picker (P)"),
            ];
            for (icon, tool, tip) in items {
                let enabled = tool.is_some();
                let sel = tool == Some(self.scene.tool);
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), if enabled { Sense::click() } else { Sense::hover() });
                w::paint_icon_button(ui, rect, &resp, icon, sel, enabled);
                w::flyout_mark(ui, rect);
                let resp = if enabled { resp.on_hover_text(tip) } else { resp.on_hover_text(format!("{tip} - not implemented yet")) };
                if resp.clicked() {
                    if let Some(t) = tool {
                        self.scene.tool = t;
                        self.props_sel = None;
                    }
                }
            }
            ui.add_space(4.0);
            w::separator(ui);
            ui.add_space(4.0);
            for (icon, tip) in [(Icon::Select, "Select"), (Icon::Bake, "Bake"), (Icon::Camera, "Camera")] {
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                w::paint_icon_button(ui, rect, &resp, icon, false, false);
                resp.on_hover_text(format!("{tip} - not implemented yet"));
            }
        });
    }

    fn dock_strip(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            let icons = [
                (Icon::Export, "Export textures"),
                (Icon::Display, "Display settings"),
                (Icon::Shader, "Shader settings"),
                (Icon::Environment, "Environment"),
                (Icon::History, "History"),
                (Icon::Assets, "Assets"),
            ];
            for (i, (icon, tip)) in icons.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                // the panel this button shows (highlighted while it is in the layout)
                let pane = match i {
                    1 | 3 => Some(Pane::DisplaySettings),
                    2 => Some(Pane::ShaderSettings),
                    4 => Some(Pane::History),
                    5 => Some(Pane::Assets),
                    _ => None,
                };
                let shown = pane.is_some_and(|p| self.dock.find_tab(&p).is_some());
                if i == 0 {
                    // the primary action of the strip
                    let fill = if resp.hovered() { theme::accent_hover() } else { theme::accent() };
                    ui.painter().rect_filled(rect, theme::RADIUS as f32 + 2.0, fill);
                    crate::icons::draw(ui.painter(), rect.shrink(8.0), *icon, theme::on_accent());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                } else {
                    w::paint_icon_button(ui, rect, &resp, *icon, shown, true);
                }
                if resp.on_hover_text(*tip).clicked() {
                    match pane {
                        Some(p) => self.show_pane(p),
                        None => self.export_dialog(),
                    }
                }
            }
        });
    }

    fn export_dialog(&mut self) {
        self.open_export();
    }

    /// File > Export Textures (Ctrl+Shift+E): the export dialog, every texture set ticked.
    pub fn open_export(&mut self) {
        self.export_open = true;
        self.export_sets = vec![true; self.stacks.len()];
        self.export_result = None;
    }

    /// The Export Textures dialog: texture sets, output folder, template, file type, size, padding, normals.
    fn export_window(&mut self, ctx: &egui::Context) {
        use crate::export::{templates, BitDepth, FileType, NormalFormat};
        if !self.export_open {
            return;
        }
        let all = templates();
        let cfg = self.config.export;
        let tpl = (cfg[0].max(0) as usize).min(all.len() - 1);
        let frame = egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(14)
            .inner_margin(egui::Margin::same(22))
            .shadow(egui::epaint::Shadow { offset: [0, 14], blur: 44, spread: 0, color: Color32::from_black_alpha(170) });
        let mut run = false;
        let mut close = false;
        let mut cfg_new = cfg;
        let modal = egui::Modal::new(egui::Id::new("export_textures"))
            .area(egui::Modal::default_area(egui::Id::new("export_textures")).fade_in(false))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                ui.set_width(760.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Export Textures").font(theme::semibold(19.0)).color(Color32::WHITE));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::icon_button(ui, Icon::Close, 24.0, false, true).on_hover_text("Close (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
                ui.label(RichText::new("Write the texture sets with an output template").size(11.5).color(theme::TEXT_DIM));
                ui.add_space(14.0);
                ui.horizontal_top(|ui| {
                    // ---- texture sets
                    ui.vertical(|ui| {
                        ui.set_width(220.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("TEXTURE SETS").font(theme::semibold(10.5)).color(theme::TEXT_DIM));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("None").clicked() {
                                    self.export_sets.iter_mut().for_each(|v| *v = false);
                                }
                                if ui.small_button("All").clicked() {
                                    self.export_sets.iter_mut().for_each(|v| *v = true);
                                }
                            });
                        });
                        egui::Frame::new().fill(theme::WELL).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical().id_salt("export_sets").max_height(330.0).show(ui, |ui| {
                                self.export_sets.resize(self.stacks.len(), true);
                                for (i, st) in self.stacks.iter().enumerate() {
                                    let size = self.scene.texs.get(i).map(|t| t.w).unwrap_or(0);
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut self.export_sets[i], RichText::new(&st.name).size(11.5));
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            ui.label(RichText::new(format!("{size}")).size(10.5).color(theme::TEXT_DIM));
                                        });
                                    });
                                }
                            });
                        });
                    });
                    ui.add_space(18.0);
                    // ---- settings
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        let dir = self.config.export_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(choose a folder)".into());
                        settings_row(ui, "Output directory", &dir, |ui| {
                            if ui.button("Browse...").clicked() {
                                if let Some(d) = rfd::FileDialog::new().set_title("Export textures to").pick_folder() {
                                    self.config.export_dir = Some(d);
                                    self.config.save();
                                }
                            }
                        });
                        settings_divider(ui);
                        let names: Vec<&str> = all.iter().map(|t| t.name).collect();
                        if let Some(k) = w::form_combo(ui, "export_tpl", "Output template", names[tpl], &names, true) {
                            cfg_new[0] = k as i32;
                        }
                        let t = &all[tpl];
                        ui.label(RichText::new(t.description).size(11.0).color(theme::TEXT_DIM));
                        let files: Vec<String> = if t.outputs.is_empty() {
                            vec!["$set_BCR".into(), "$set_NMO".into()]
                        } else {
                            t.outputs.iter().map(|o| format!("{}{}", o.name, if o.optional { " (if any)" } else { "" })).collect()
                        };
                        ui.label(RichText::new(files.join("   ")).font(FontId::monospace(10.5)).color(theme::TEXT));
                        settings_divider(ui);
                        let arma = t.outputs.is_empty();
                        // templates that keep data in alpha need a format with alpha
                        let needs_alpha = arma || t.outputs.iter().any(|o| o.channels[3] != crate::export::Src::One);
                        let ft = FileType::ALL[(cfg[1].max(0) as usize).min(FileType::ALL.len() - 1)];
                        let types: Vec<(&str, bool, &str)> = FileType::ALL
                            .iter()
                            .map(|f| (f.label(), f.has_alpha() || !needs_alpha, "This template keeps data in the alpha channel"))
                            .collect();
                        ui.label(RichText::new("File type").size(11.5).color(theme::TEXT));
                        if let Some(k) = w::segmented(ui, &types, FileType::ALL.iter().position(|f| *f == ft).unwrap_or(0)) {
                            cfg_new[1] = k as i32;
                            cfg_new[5] = 0;
                        }
                        let depths = ft.depths();
                        let cur_depth = depths.get(cfg[5].max(0) as usize).copied().unwrap_or(depths[0]);
                        let labels: Vec<&str> = depths.iter().map(|d| d.label()).collect();
                        if let Some(k) = w::form_combo(ui, "export_depth", "Bit depth", cur_depth.label(), &labels, depths.len() > 1) {
                            cfg_new[5] = k as i32;
                        }
                        if cur_depth == BitDepth::F16 || cur_depth == BitDepth::F32 {
                            ui.label(RichText::new("Float files: base and emissive colour are written linear.").size(10.5).color(theme::TEXT_DIM));
                        }
                        let sizes = [(0, "Each set's own"), (512, "512"), (1024, "1024"), (2048, "2048"), (4096, "4096")];
                        let cur = sizes.iter().find(|s| s.0 == cfg[2]).map(|s| s.1).unwrap_or("Each set's own");
                        let labels: Vec<&str> = sizes.iter().map(|s| s.1).collect();
                        if let Some(k) = w::form_combo(ui, "export_size", "Size", cur, &labels, true) {
                            cfg_new[2] = sizes[k].0;
                        }
                        let pads = [(-1, "Dilation infinite"), (32, "32 px"), (16, "16 px"), (8, "8 px"), (0, "None")];
                        let cur = pads.iter().find(|p| p.0 == cfg[3]).map(|p| p.1).unwrap_or("Dilation infinite");
                        let labels: Vec<&str> = pads.iter().map(|p| p.1).collect();
                        if let Some(k) = w::form_combo(ui, "export_pad", "Padding", cur, &labels, true) {
                            cfg_new[3] = pads[k].0;
                        }
                        let tpl_gl = matches!(t.normal, NormalFormat::OpenGl);
                        let from_tpl = format!("From template ({})", if tpl_gl { "OpenGL" } else { "DirectX" });
                        let normals = [from_tpl.as_str(), "OpenGL", "DirectX"];
                        let cur = normals[(cfg[4].clamp(0, 2)) as usize];
                        if let Some(k) = w::form_combo(ui, "export_nrm", "Normal map", cur, &normals, !arma) {
                            cfg_new[4] = k as i32;
                        }
                    });
                });
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if let Some(r) = &self.export_result {
                        ui.add(egui::Label::new(RichText::new(r).size(11.0).color(theme::TEXT_DIM)).wrap());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = self.export_sets.iter().filter(|v| **v).count();
                        let ready = n > 0 && self.config.export_dir.is_some();
                        let label = RichText::new(format!("Export {n} set(s)")).font(theme::semibold(12.5)).color(theme::on_accent());
                        if ui.add_enabled(ready, egui::Button::new(label).fill(theme::accent()).min_size(vec2(130.0, 30.0))).clicked() {
                            run = true;
                        }
                        if ui.add(egui::Button::new("Cancel").min_size(vec2(90.0, 30.0))).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if cfg_new != cfg {
            self.config.export = cfg_new;
            self.config.save();
        }
        if run {
            let c = self.config.export;
            let file_type = FileType::ALL[(c[1].max(0) as usize).min(FileType::ALL.len() - 1)];
            let opts = crate::export::ExportOptions {
                template: c[0].max(0) as usize,
                file_type,
                depth: file_type.depths().get(c[5].max(0) as usize).copied().unwrap_or(file_type.depths()[0]),
                size: (c[2] > 0).then_some(c[2] as u32),
                padding: (c[3] >= 0).then_some(c[3] as u32),
                normal: match c[4] {
                    1 => Some(NormalFormat::OpenGl),
                    2 => Some(NormalFormat::DirectX),
                    _ => None,
                },
            };
            let dir = self.config.export_dir.clone().unwrap_or_default();
            let t0 = std::time::Instant::now();
            let mut files = Vec::new();
            let mut err = None;
            for set in (0..self.stacks.len()).filter(|i| self.export_sets.get(*i).copied().unwrap_or(false)) {
                match crate::export::export_template(self, set, &dir, &opts) {
                    Ok(f) => files.extend(f),
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
            let msg = match err {
                Some(e) => format!("Export failed: {e}"),
                None => format!("Exported {} file(s) to {} in {:.1} s", files.len(), dir.display(), t0.elapsed().as_secs_f32()),
            };
            self.status = msg.clone();
            self.export_result = Some(msg);
        }
        if close || modal.should_close() {
            self.export_open = false;
        }
    }

    /// Display Settings (Painter's dock panel): environment, camera, viewer, post effects.
    fn display_settings_pane(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("display_settings").auto_shrink([false, false]).show(ui, |ui| {
            w::form_title(ui, "ENVIRONMENT SETTINGS", true, false);
            let names: Vec<String> = self
                .hdri_files
                .iter()
                .map(|p| p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").replace('_', " "))
                .collect();
            let current = self.hdri_current.and_then(|i| names.get(i).cloned()).unwrap_or_else(|| self.env.name.clone());
            let items: Vec<&str> = names.iter().map(String::as_str).collect();
            if let Some(k) = w::form_combo(ui, "ds_hdri", "Environment map", &current, &items, !items.is_empty()) {
                self.load_hdri(k);
            }
            let mut deg = self.env_yaw.to_degrees().rem_euclid(360.0);
            if w::form_slider(ui, "Environment rotation", &mut deg, 0.0..=360.0, 0) {
                self.env_yaw = deg.to_radians();
            }
            w::form_slider(ui, "Environment exposure (EV)", &mut self.exposure_ev, -5.0..=5.0, 2);
            if w::form_toggle(ui, "Show environment", self.show_background) {
                self.show_background = !self.show_background;
            }
            w::form_title(ui, "CAMERA SETTINGS", false, false);
            w::form_slider(ui, "Field of view (deg)", &mut self.scene.cam.fov, 5.0..=120.0, 0);
            w::form_combo(ui, "ds_cs", "Color profile", "ACES - sRGB", &["ACES - sRGB"], false);
            w::form_title(ui, "VIEWER SETTINGS", false, false);
            w::form_slider(ui, "Ambient occlusion intensity", &mut self.ao_intensity, 0.0..=1.0, 2);
            let levels = [(1u32, "Off"), (2, "2x"), (4, "4x"), (8, "8x")];
            let items: Vec<(&str, bool, &str)> =
                levels.iter().map(|(n, l)| (*l, self.msaa_supported.contains(n), "Not supported by this GPU")).collect();
            let want = self.config.viewport_msaa;
            let cur = levels.iter().rposition(|(n, _)| *n <= want && self.msaa_supported.contains(n)).unwrap_or(0);
            ui.label(RichText::new("Antialiasing").size(11.5).color(theme::TEXT));
            if let Some(k) = w::segmented(ui, &items, cur) {
                self.config.viewport_msaa = levels[k].0;
                self.config.save();
            }
            ui.add_space(6.0);
            self.post_effects_ui(ui);
        });
    }

    /// Shader Settings: the shader of the project and its parameters.
    fn shader_settings_pane(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("shader_settings").auto_shrink([false, false]).show(ui, |ui| {
            w::form_title(ui, "SHADER", true, false);
            w::form_combo(ui, "ss_shader", "Shader", "PBR - Metallic Roughness", &["PBR - Metallic Roughness"], false);
            ui.label(
                RichText::new(
                    "GGX specular with Smith visibility, Lambert diffuse, split-sum image-based lighting with \
                     spherical-harmonics irradiance. Channels: base color, roughness, metallic, normal, height, emissive.",
                )
                .size(11.0)
                .color(theme::TEXT_DIM),
            );
            w::form_title(ui, "PARAMETERS", false, false);
            w::form_slider(ui, "AO intensity", &mut self.ao_intensity, 0.0..=1.0, 2);
            w::form_slider(ui, "Emissive intensity", &mut self.emissive_intensity, 0.0..=10.0, 2);
            ui.label(
                RichText::new("Read from the project's shader instance when it is opened (ao_intensity, emissive_intensity).")
                    .size(10.5)
                    .color(theme::TEXT_DISABLED),
            );
        });
    }

    /// History: every undoable step; a click goes back (or forward) to it.
    fn history_pane(&mut self, ui: &mut egui::Ui) {
        let done: Vec<String> = self.undo_stack.iter().map(|e| e.label().to_string()).collect();
        let undone: Vec<String> = self.redo_stack.iter().rev().map(|e| e.label().to_string()).collect();
        let n = done.len() as i64;
        let mut jump = 0i64;
        egui::ScrollArea::vertical().id_salt("history").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            let rows = std::iter::once(("Project opened".to_string(), false))
                .chain(done.iter().map(|l| (l.clone(), false)))
                .chain(undone.iter().map(|l| (l.clone(), true)));
            for (i, (label, is_undone)) in rows.enumerate() {
                // row i is the state after i steps
                let steps = n - i as i64;
                let text = RichText::new(if is_undone { format!("{label} (undone)") } else { label }).size(11.5);
                let text = if is_undone { text.color(theme::TEXT_DISABLED) } else { text };
                if ui.selectable_label(steps == 0, text).on_hover_text("Return to this step").clicked() {
                    jump = steps;
                }
            }
        });
        for _ in 0..jump.max(0) {
            self.undo();
        }
        for _ in 0..(-jump).max(0) {
            self.redo();
        }
    }

    /// Log: the messages of the session (the history of the status line).
    fn log_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.small_button("Copy").clicked() {
                let text: Vec<String> = self.log.iter().map(|(t, m)| format!("{t:>8.1}s  {m}")).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
            if ui.small_button("Clear").clicked() {
                self.log.clear();
            }
        });
        egui::ScrollArea::vertical().id_salt("log").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for (t, m) in &self.log {
                ui.add(egui::Label::new(RichText::new(format!("{t:>7.1}s  {m}")).font(FontId::monospace(10.5)).color(theme::TEXT)).wrap());
            }
        });
    }

    // ------------------------------------------------------------ left dock

    fn properties_pane(&mut self, ui: &mut egui::Ui) {
        // the properties tab is universal: it follows the last thing the user picked (a tool or a layer /
        // effect) instead of keeping a stale selection
        if self.scene.tool != self.props_last_tool {
            self.props_last_tool = self.scene.tool;
            self.props_sel = None;
        }
        let info = if self.props_sel.is_some() {
            let r = self.props_panel();
            if r.is_none() {
                self.props_sel = None;
            }
            r
        } else {
            None
        };
        self.props_title = match (&info, self.scene.tool) {
            (Some((PanelKind::Fill, _)), _) => "Properties - Fill",
            (Some((PanelKind::Generator, _)), _) => "Properties - Generator",
            (Some((PanelKind::Filter, _)), _) => "Properties - Filter",
            (None, Tool::Paint) => "Properties - Paint",
            (None, Tool::Eraser) => "Properties - Eraser",
        };
        if let Some((kind, iface)) = info {
            if self.props_body(ui, kind, iface) {
                return;
            }
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.preview_row(ui);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, icon) in [Icon::Brush, Icon::Alpha, Icon::Stencil, Icon::Material].into_iter().enumerate() {
                    if w::icon_button(ui, icon, 28.0, self.props_tab == i, true).clicked() {
                        self.props_tab = i;
                    }
                }
            });
            ui.add_space(6.0);
            w::title_row(ui, "BRUSH", true);
            self.brush_sections(ui);
            w::separator(ui);
            w::title_row(ui, "ALPHA", false);
            self.alpha_section(ui);
            w::separator(ui);
            w::title_row(ui, "STENCIL", false);
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Tiling mode");
                    egui::ComboBox::from_id_salt("stencil_tiling").selected_text("No Tiling").show_ui(ui, |_| {});
                });
                resource_button(ui, "Stencil", "No Resource Selected");
            });
            w::separator(ui);
            w::title_row(ui, "MATERIAL", true);
            self.material_section(ui);
            ui.add_space(20.0);
        });
    }

    /// What the universal PROPERTIES tab shows for the selected effect (mask row or fill/filter layer
    /// content), like Painter: FILL (projection, UV transformations and the source parameters), GENERATOR
    /// (graph parameters) or FILTER. `None` = nothing selected (the tool's own properties are shown).
    fn props_panel(&mut self) -> Option<(PanelKind, Option<std::sync::Arc<shard_formats::sbsar::Interface>>)> {
        use shard_model::EffectKind as K;
        let (path, pos, in_mask) = self.props_sel.clone()?;
        let a = self.active();
        let (is_filter, is_fill, resource, graph) = {
            let e = props_effect_mut(&mut self.stacks[a].layers, &path, pos, in_mask)?;
            let (resource, graph) = match effect_procedural_mut(e) {
                Some(p) => (p.resource.clone(), p.graph_name().map(str::to_string)),
                None => (None, None),
            };
            (matches!(e.kind, K::Filter { .. }), matches!(e.kind, K::Fill(_)), resource, graph)
        };
        if !is_filter && !is_fill {
            return None;
        }
        let iface = resource.and_then(|r| {
            if !self.iface_cache.contains_key(&r) {
                let i = self.bitmaps.as_ref().and_then(|b| b.sbsar_interface(&r)).map(std::sync::Arc::new);
                self.iface_cache.insert(r.clone(), i);
            }
            self.iface_cache.get(&r).cloned().flatten()
        });
        // Painter: the mesh-aware "Generators" (mg_*) have their own tab; grunges and other procedural
        // textures are plain fills with a grayscale / material source
        let cat = iface.as_ref().map(|i| i.category.to_lowercase()).unwrap_or_default();
        let kind = if is_filter || cat.contains("filter") {
            PanelKind::Filter
        } else if graph.as_deref().is_some_and(|n| n.starts_with("mg_")) {
            PanelKind::Generator
        } else {
            PanelKind::Fill
        };
        Some((kind, iface))
    }

    /// Body of the properties tab for the selected effect. Returns whether anything was drawn.
    fn props_body(
        &mut self,
        ui: &mut egui::Ui,
        kind: PanelKind,
        iface: Option<std::sync::Arc<shard_formats::sbsar::Interface>>,
    ) -> bool {
        use shard_model::EffectKind as K;
        let Some((path, pos, in_mask)) = self.props_sel.clone() else { return false };
        let a = self.active();
        // previews of the resources of the fill's sources (resolved before the effect is borrowed)
        let resources: Vec<String> = match props_effect_mut(&mut self.stacks[a].layers, &path, pos, in_mask) {
            Some(Effect { kind: K::Fill(f), .. }) => f
                .sources
                .iter()
                .filter_map(|s| match s {
                    Source::Procedural { procedural, .. } => procedural.resource.clone(),
                    Source::Bitmap { resource, .. } => resource.clone(),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let slot_thumbs: std::collections::HashMap<String, egui::TextureHandle> =
            resources.into_iter().filter_map(|r| self.source_preview(&r).map(|t| (r, t))).collect();
        let ctx_res = self.scene.texs.get(a).map(|t| (t.w, t.h)).unwrap_or((1024, 1024));
        let Some(e) = props_effect_mut(&mut self.stacks[a].layers, &path, pos, in_mask) else { return false };
        let mut changed = false;
        let mut open_picker: Option<usize> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match (&mut e.kind, kind) {
                (K::Fill(f), PanelKind::Fill) => {
                    changed |= draw_fill_section(ui, f);
                    changed |= draw_fill_sources(ui, f, in_mask, iface.as_deref(), &slot_thumbs, ctx_res, &mut open_picker);
                }
                (K::Fill(f), _) => {
                    // generator: its graph parameters only
                    if let Some(proc_) = f.sources.iter_mut().find_map(|s| match s {
                        Source::Procedural { procedural, .. } => Some(procedural),
                        _ => None,
                    }) {
                        if let Some(i) = iface.as_deref() {
                            ui.label(RichText::new(&i.label).strong().size(13.0));
                            if !i.description.is_empty() {
                                ui.label(RichText::new(&i.description).color(theme::TEXT_DIM).size(10.5));
                            }
                            ui.add_space(4.0);
                            changed |= draw_interface_params(ui, i, proc_);
                        }
                    }
                }
                (K::Filter { procedural: Some(proc_) }, _) => {
                    if let Some(i) = iface.as_deref() {
                        ui.label(RichText::new(&i.label).strong().size(13.0));
                        ui.add_space(4.0);
                        changed |= draw_interface_params(ui, i, proc_);
                    }
                }
                _ => {}
            }
            ui.add_space(20.0);
        });
        if changed {
            self.mark_recompose();
            self.mask_reeval = true;
        }
        if let Some(k) = open_picker {
            self.open_picker(path, pos, in_mask, k);
        }
        true
    }

    // ------------------------------------------------------------ resource picker

    /// Diagnostics: open the picker on the first source of the effect shown in PROPERTIES.
    pub fn open_picker_on_props(&mut self, tab: usize) {
        if let Some((path, pos, in_mask)) = self.props_sel.clone() {
            self.open_picker(path, pos, in_mask, 0);
            if let Some(p) = &mut self.picker {
                p.tab = tab;
            }
        }
    }

    /// Open the picker for source `source` of the fill at (`path`, `pos`, `in_mask`).
    fn open_picker(&mut self, path: Vec<usize>, pos: usize, in_mask: bool, source: usize) {
        let a = self.active();
        // resources already used by this document (they are embedded in the project)
        let mut items: Vec<PickItem> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        fn walk(ls: &[Layer], items: &mut Vec<PickItem>, seen: &mut std::collections::HashSet<String>) {
            fn eff(es: &[Effect], items: &mut Vec<PickItem>, seen: &mut std::collections::HashSet<String>) {
                for e in es {
                    let mut srcs: Vec<&Source> = Vec::new();
                    match &e.kind {
                        EffectKind::Fill(f) => srcs.extend(f.sources.iter()),
                        EffectKind::Group { children } => eff(children, items, seen),
                        _ => {}
                    }
                    for s in srcs {
                        match s {
                            Source::Procedural { procedural, .. } => {
                                if let (Some(r), Some(n)) = (&procedural.resource, procedural.graph_name()) {
                                    if seen.insert(r.clone()) {
                                        items.push(PickItem {
                                            name: n.to_string(),
                                            kind: PickKind::Procedural,
                                            path: None,
                                            resource: Some(r.clone()),
                                            origin: "This project".into(),
                                            outputs: procedural.outputs.clone(),
                                        });
                                    }
                                }
                            }
                            Source::Bitmap { resource: Some(r), .. } => {
                                if seen.insert(r.clone()) {
                                    let n = r.split('?').next().unwrap_or(r).trim_start_matches('/').to_string();
                                    items.push(PickItem {
                                        name: n,
                                        kind: PickKind::Texture,
                                        path: None,
                                        resource: Some(r.clone()),
                                        origin: "This project".into(),
                                        outputs: Vec::new(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            for l in ls {
                eff(&l.content, items, seen);
                if let Some(m) = &l.mask {
                    eff(&m.effects, items, seen);
                }
                walk(&l.children, items, seen);
            }
        }
        for st in &self.stacks {
            walk(&st.layers, &mut items, &mut seen);
        }
        // the shelf
        use crate::library::AssetKind;
        for asset in &self.assets {
            let kind = match asset.kind {
                AssetKind::Procedural => PickKind::Procedural,
                AssetKind::Texture => PickKind::Texture,
                AssetKind::Alpha => PickKind::Alpha,
                _ => continue,
            };
            let lib = self.config.libraries.get(asset.library).map(|l| l.name.clone()).unwrap_or_else(|| "Shelf".into());
            items.push(PickItem {
                name: asset.name.clone(),
                kind,
                path: Some(asset.path.clone()),
                resource: None,
                origin: lib,
                outputs: Vec::new(),
            });
        }
        let anchors = collect_anchors(&self.stacks[a].layers);
        self.picker = Some(Picker {
            target: (path, pos, in_mask, source),
            tab: 0,
            query: String::new(),
            kinds: [true; 3],
            origin_filter: None,
            selected: None,
            tile: 92.0,
            list_view: false,
            items,
            anchors,
        });
    }

    /// Replace the picked fill source (keeping its channels and opacity).
    fn apply_pick(&mut self, target: &(Vec<usize>, usize, bool, usize), choice: PickChoice) {
        let a = self.active();
        let (path, pos, in_mask, k) = target;
        let Some(e) = props_effect_mut(&mut self.stacks[a].layers, path, *pos, *in_mask) else { return };
        let EffectKind::Fill(f) = &mut e.kind else { return };
        let Some(src) = f.sources.get_mut(*k) else { return };
        let (channels, opacity) = match src {
            Source::Uniform { channels, opacity, .. }
            | Source::Bitmap { channels, opacity, .. }
            | Source::Procedural { channels, opacity, .. }
            | Source::Reference { channels, opacity, .. } => (*channels, *opacity),
            Source::Other(_) => return,
        };
        *src = match choice {
            PickChoice::Procedural { resource, outputs } => Source::Procedural {
                channels,
                procedural: shard_model::Procedural {
                    resource: Some(resource),
                    outputs,
                    ..Default::default()
                },
                opacity,
            },
            PickChoice::Texture { resource } => Source::Bitmap { channels, resource: Some(resource), opacity },
            PickChoice::Anchor { uid } => Source::Reference { channels, opacity, target: uid, levels: None },
        };
        self.mark_recompose();
        self.mask_reeval = true;
        self.status = "Fill source changed".into();
    }

    /// Preview texture of a shelf file / project resource. Loaded on background threads: returns
    /// `None` until it is ready (or when the asset has no embedded preview).
    fn preview_tex(&mut self, key: &str, make: impl FnOnce() -> crate::previews::PreviewSource) -> Option<egui::TextureHandle> {
        match self.thumbs.get(key) {
            Some(Thumb::Ready(t)) => return Some(t.clone()),
            Some(Thumb::Missing) => return None,
            Some(Thumb::Pending) => {
                self.previews.want(key, || unreachable!(), false);
                return None;
            }
            None => {}
        }
        self.thumbs.insert(key.to_string(), Thumb::Pending);
        self.previews.want(key, make, true);
        None
    }

    /// Preview of a fill source's resource (`file:<path>` from the shelf, or a project url).
    fn source_preview(&mut self, resource: &str) -> Option<egui::TextureHandle> {
        if let Some(p) = resource.strip_prefix("file:") {
            let path = PathBuf::from(p);
            return self.preview_tex(p, || crate::previews::PreviewSource::Shelf(path));
        }
        let project = self.project_path.clone()?;
        let key = format!("proj:{resource}");
        let res = resource.to_string();
        self.preview_tex(&key, || crate::previews::PreviewSource::Project { project, resource: res })
    }

    /// Hand finished previews to the GPU (a few per frame so scrolling stays fluid).
    fn pump_previews(&mut self, ctx: &egui::Context) {
        self.previews.next_frame();
        for (key, msg) in self.previews.poll(6) {
            match msg {
                crate::previews::Loaded::Skipped => {
                    self.thumbs.remove(&key);
                }
                crate::previews::Loaded::Ready(None) => {
                    self.thumbs.insert(key, Thumb::Missing);
                }
                crate::previews::Loaded::Ready(Some(p)) => {
                    let img = egui::ColorImage::from_rgba_unmultiplied([p.w, p.h], &p.rgba);
                    let tex = ctx.load_texture(format!("preview:{key}"), img, egui::TextureOptions::LINEAR);
                    self.thumbs.insert(key, Thumb::Ready(tex));
                }
            }
            ctx.request_repaint();
        }
        if self.thumbs.values().any(|t| matches!(t, Thumb::Pending)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        // keep GPU memory bounded on huge shelves: forget the ready ones, they reload on demand
        if self.thumbs.len() > 900 {
            self.thumbs.retain(|_, t| matches!(t, Thumb::Pending));
        }
    }

    /// Thumbnail of a picker item.
    fn pick_thumb(&mut self, it: &PickItem) -> Option<egui::TextureHandle> {
        match (&it.path, &it.resource) {
            (Some(p), _) => {
                let key = p.to_string_lossy().into_owned();
                let path = p.clone();
                self.preview_tex(&key, || crate::previews::PreviewSource::Shelf(path))
            }
            (None, Some(r)) => self.source_preview(r),
            _ => None,
        }
    }

    /// The picker window: RESOURCES (project + shelf, filters, grid / list) and ANCHOR POINTS.
    fn picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut pk) = self.picker.take() else { return };
        let mut keep = true;
        let mut chosen: Option<PickChoice> = None;
        egui::Window::new("Select a resource")
            .open(&mut keep)
            .default_size([840.0, 600.0])
            .min_size([600.0, 380.0])
            .max_size([1500.0, 1100.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                // every width below is derived from this one so the window can never grow by itself
                let full_w = ui.available_width();
                let t = w::tab_strip(ui, &[("RESOURCES", pk.tab == 0), ("ANCHOR POINTS", pk.tab == 1)], pk.tab);
                pk.tab = t.selected;
                ui.add_space(6.0);
                if pk.tab == 1 {
                    if pk.anchors.is_empty() {
                        ui.label(RichText::new("No anchor points in this texture set yet.").color(theme::TEXT_DIM));
                        ui.label(
                            RichText::new("Add one from a layer or mask effect (right-click > Add anchor point).")
                                .color(theme::TEXT_DIM)
                                .size(11.0),
                        );
                    }
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for (uid, name, owner) in &pk.anchors {
                            let (rect, resp) = ui.allocate_exact_size(vec2(full_w - 24.0, 34.0), Sense::click());
                            let p = ui.painter();
                            if resp.hovered() {
                                p.rect_filled(rect, 3.0, theme::WIDGET);
                            }
                            crate::icons::draw(p, Rect::from_center_size(pos2(rect.min.x + 18.0, rect.center().y), vec2(20.0, 20.0)), Icon::Anchor, Color32::WHITE);
                            p.text(pos2(rect.min.x + 40.0, rect.center().y - 7.0), Align2::LEFT_CENTER, name, FontId::proportional(12.5), Color32::WHITE);
                            p.text(pos2(rect.min.x + 40.0, rect.center().y + 8.0), Align2::LEFT_CENTER, format!("on '{owner}'"), FontId::proportional(10.5), theme::TEXT_DIM);
                            if resp.clicked() || resp.double_clicked() {
                                chosen = Some(PickChoice::Anchor { uid: *uid });
                            }
                        }
                    });
                    return;
                }
                // search + view options
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut pk.query)
                            .hint_text("Search: name or type (procedural, texture, alpha)")
                            .desired_width((full_w - 270.0).max(120.0)),
                    );
                    ui.selectable_value(&mut pk.list_view, false, "Grid");
                    ui.selectable_value(&mut pk.list_view, true, "List");
                    ui.add(egui::Slider::new(&mut pk.tile, 56.0..=150.0).show_value(false));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(full_w - 8.0);
                    for (i, k) in [PickKind::Procedural, PickKind::Texture, PickKind::Alpha].into_iter().enumerate() {
                        let n = pk.items.iter().filter(|it| it.kind == k).count();
                        ui.toggle_value(&mut pk.kinds[i], format!("{} ({n})", k.label()));
                    }
                    ui.separator();
                    let mut origins: Vec<String> = pk.items.iter().map(|i| i.origin.clone()).collect();
                    origins.sort();
                    origins.dedup();
                    if ui.selectable_label(pk.origin_filter.is_none(), "All sources").clicked() {
                        pk.origin_filter = None;
                    }
                    for o in origins {
                        if ui.selectable_label(pk.origin_filter.as_deref() == Some(o.as_str()), &o).clicked() {
                            pk.origin_filter = Some(o);
                        }
                    }
                });
                ui.separator();
                let q = pk.query.to_ascii_lowercase();
                let terms: Vec<&str> = q.split([' ', ',']).filter(|t| !t.is_empty()).collect();
                let shown: Vec<usize> = pk
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| pk.kinds[match it.kind { PickKind::Procedural => 0, PickKind::Texture => 1, PickKind::Alpha => 2 }])
                    .filter(|(_, it)| pk.origin_filter.as_ref().map_or(true, |o| *o == it.origin))
                    .filter(|(_, it)| {
                        let hay = format!("{} {}", it.name.to_ascii_lowercase(), it.kind.label().to_ascii_lowercase());
                        terms.iter().all(|t| hay.contains(t))
                    })
                    .map(|(i, _)| i)
                    .collect();
                // grid (left) + details (right): fixed widths, 40 px of slack for spacings and separator
                let detail_w = 220.0;
                let list_w = (full_w - detail_w - 40.0).max(260.0);
                let body_h = (ui.available_height() - 8.0).clamp(280.0, 900.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(vec2(list_w, body_h), egui::Layout::top_down(Align::Min), |ui| {
                        let tile = pk.tile;
                        let cols = if pk.list_view { 1 } else { (((list_w - 18.0) / (tile + 8.0)).floor() as usize).max(1) };
                        let row_h = if pk.list_view { 30.0 } else { tile + 34.0 };
                        let rows = shown.len().div_ceil(cols);
                        egui::ScrollArea::vertical()
                            .id_salt("pick_grid")
                            .max_height(body_h)
                            .auto_shrink([false, false])
                            .show_rows(ui, row_h, rows, |ui, range| {
                                ui.set_max_width(list_w - 16.0);
                                for r in range {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        for c in 0..cols {
                                            let Some(&idx) = shown.get(r * cols + c) else { break };
                                            let it = pk.items[idx].clone();
                                            let sel = pk.selected == Some(idx);
                                            let size = if pk.list_view { vec2(list_w - 24.0, 28.0) } else { vec2(tile, row_h - 6.0) };
                                            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                                            if !ui.is_rect_visible(rect) {
                                                continue;
                                            }
                                            let thumb = self.pick_thumb(&it);
                                            let p = ui.painter();
                                            if sel {
                                                p.rect_filled(rect, 4.0, theme::accent_soft());
                                            } else if resp.hovered() {
                                                p.rect_filled(rect, 4.0, theme::WIDGET);
                                            }
                                            let art = if pk.list_view {
                                                Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(24.0, 24.0))
                                            } else {
                                                Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(tile - 8.0, tile - 8.0))
                                            };
                                            match &thumb {
                                                Some(t) => {
                                                    // keep the aspect ratio inside the square
                                                    let ts = t.size_vec2();
                                                    let k = (art.width() / ts.x).min(art.height() / ts.y);
                                                    let r = Rect::from_center_size(art.center(), ts * k);
                                                    p.rect_filled(art, 3.0, theme::WELL);
                                                    p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                                }
                                                None => {
                                                    p.rect_filled(art, 3.0, it.kind.colour());
                                                    p.text(art.center(), Align2::CENTER_CENTER, it.kind.glyph(), FontId::proportional(if pk.list_view { 10.0 } else { 18.0 }), Color32::WHITE);
                                                }
                                            }
                                            if pk.list_view {
                                                p.text(pos2(art.max.x + 8.0, rect.center().y), Align2::LEFT_CENTER, &it.name, FontId::proportional(12.0), Color32::WHITE);
                                                p.text(pos2(rect.max.x - 8.0, rect.center().y), Align2::RIGHT_CENTER, &it.origin, FontId::proportional(10.5), theme::TEXT_DIM);
                                            } else {
                                                let mut name = it.name.clone();
                                                let max_chars = ((tile - 6.0) / 6.2) as usize;
                                                if name.chars().count() > max_chars {
                                                    name = format!("{}…", name.chars().take(max_chars.saturating_sub(1)).collect::<String>());
                                                }
                                                p.text(pos2(rect.center().x, art.max.y + 14.0), Align2::CENTER_CENTER, name, FontId::proportional(11.0), Color32::WHITE);
                                            }
                                            if resp.clicked() {
                                                pk.selected = Some(idx);
                                            }
                                            let used = resp.double_clicked();
                                            resp.on_hover_text(format!("{} — {}", it.name, it.origin));
                                            if used {
                                                chosen = Some(it.choice());
                                            }
                                        }
                                    });
                                }
                            });
                        if shown.is_empty() {
                            ui.label(RichText::new("Nothing matches. Add shelf folders in Edit > Settings > Libraries.").color(theme::TEXT_DIM));
                        }
                    });
                    ui.separator();
                    ui.allocate_ui_with_layout(vec2(detail_w, body_h), egui::Layout::top_down(Align::Min), |ui| {
                        ui.set_max_width(detail_w);
                        match pk.selected.and_then(|i| pk.items.get(i)).cloned() {
                            Some(it) => {
                                let side = detail_w - 12.0;
                                let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
                                let p = ui.painter();
                                p.rect_filled(rect, 6.0, theme::WELL);
                                match self.pick_thumb(&it) {
                                    Some(t) => {
                                        let ts = t.size_vec2();
                                        let k = ((rect.width() - 8.0) / ts.x).min((rect.height() - 8.0) / ts.y);
                                        let r = Rect::from_center_size(rect.center(), ts * k);
                                        p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                    }
                                    None => {
                                        p.text(rect.center(), Align2::CENTER_CENTER, it.kind.glyph(), FontId::proportional(40.0), theme::TEXT_DIM);
                                    }
                                }
                                ui.add_space(6.0);
                                ui.add(egui::Label::new(RichText::new(&it.name).strong().size(14.0)).wrap());
                                ui.label(RichText::new(it.kind.label()).color(theme::TEXT_DIM));
                                ui.add_space(4.0);
                                ui.label(RichText::new(format!("Source: {}", it.origin)).color(theme::TEXT_DIM).size(11.0));
                                if let Some(pth) = &it.path {
                                    ui.add(egui::Label::new(RichText::new(pth.display().to_string()).color(theme::TEXT_DIM).size(10.0)).wrap());
                                    if let Ok(m) = std::fs::metadata(pth) {
                                        ui.label(RichText::new(format!("{:.1} KB", m.len() as f64 / 1024.0)).color(theme::TEXT_DIM).size(10.5));
                                    }
                                    let supported = match it.kind {
                                        PickKind::Texture | PickKind::Alpha => pth.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png")),
                                        PickKind::Procedural => true,
                                    };
                                    if !supported {
                                        ui.colored_label(theme::WARNING, "Only PNG textures can be loaded for now.");
                                    }
                                    ui.add_space(8.0);
                                    if ui.add_enabled(supported, egui::Button::new("Use this resource")).clicked() {
                                        chosen = Some(it.choice());
                                    }
                                } else {
                                    ui.add_space(8.0);
                                    if ui.button("Use this resource").clicked() {
                                        chosen = Some(it.choice());
                                    }
                                }
                            }
                            None => {
                                ui.label(RichText::new("Select a resource").color(theme::TEXT_DIM));
                                ui.label(RichText::new("Double-click to use it right away.").color(theme::TEXT_DIM).size(10.5));
                            }
                        }
                    });
                });
            });
        if let Some(c) = chosen {
            self.apply_pick(&pk.target, c);
            keep = false;
        }
        if keep {
            self.picker = Some(pk);
        }
    }

    fn assets_tab(&mut self, ui: &mut egui::Ui) {
        use crate::library::AssetKind;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} asset(s), {} librar(ies)", self.assets.len(), self.config.libraries.len()))
                    .color(theme::TEXT_DIM),
            );
            if ui.small_button("Libraries...").clicked() {
                self.settings_open = true;
                self.settings_tab = 3;
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.asset_query).hint_text("Search").desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.asset_filter.is_none(), "All").clicked() {
                self.asset_filter = None;
            }
            for k in AssetKind::ALL {
                let n = self.assets.iter().filter(|a| a.kind == k).count();
                if n > 0 && ui.selectable_label(self.asset_filter == Some(k), format!("{} ({n})", k.label())).clicked() {
                    self.asset_filter = Some(k);
                }
            }
        });
        ui.add_space(6.0);
        if self.config.libraries.is_empty() {
            ui.label(
                RichText::new("No libraries configured. Add a folder in Edit > Settings > Libraries.")
                    .color(theme::TEXT_DIM),
            );
            return;
        }
        let q = self.asset_query.to_ascii_lowercase();
        let idx: Vec<usize> = self
            .assets
            .iter()
            .enumerate()
            .filter(|(_, a)| self.asset_filter.map_or(true, |k| a.kind == k))
            .filter(|(_, a)| q.is_empty() || a.name.to_ascii_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        let mut to_use = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 20.0, idx.len(), |ui, range| {
            for r in range {
                let a = &self.assets[idx[r]];
                let ext = a.path.extension().and_then(|x| x.to_str()).unwrap_or("");
                let resp = ui.selectable_label(false, format!("{}   .{}", a.name, ext));
                if resp.double_clicked() {
                    to_use = Some(idx[r]);
                }
                resp.on_hover_text(format!("{}\n{}\nDouble-click to use", a.path.display(), a.kind.label()));
            }
        });
        if let Some(i) = to_use {
            self.use_asset(i);
        }
    }

    fn preview_row(&mut self, ui: &mut egui::Ui) {
        let total = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(total, 96.0), Sense::hover());
        let p = ui.painter();
        let half = (total / 2.0).floor();
        let l = Rect::from_min_size(rect.min, vec2(half - 1.0, rect.height()));
        let r = Rect::from_min_size(pos2(rect.min.x + half + 1.0, rect.min.y), vec2(half - 1.0, rect.height()));
        p.rect_filled(l, 0.0, Color32::BLACK);
        p.rect_filled(r, 0.0, Color32::BLACK);
        // brush stroke preview
        let b = &self.scene.brush;
        let rad = (b.radius * 0.3).clamp(2.0, 14.0);
        let col = if self.scene.tool == Tool::Eraser {
            Color32::from_gray(200)
        } else {
            Color32::from_rgb(
                (b.color[0] * 255.0) as u8,
                (b.color[1] * 255.0) as u8,
                (b.color[2] * 255.0) as u8,
            )
        };
        for i in 0..70 {
            let t = i as f32 / 69.0;
            let x = l.min.x + 14.0 + t * (l.width() - 28.0);
            let y = l.center().y + (t * 7.0).sin() * l.height() * 0.22;
            p.circle_filled(pos2(x, y), rad, col.gamma_multiply(0.35 + 0.35 * b.hardness));
        }
        // sphere preview
        let c = r.center();
        let rr = r.height() * 0.40;
        p.circle_filled(c, rr, Color32::from_gray(150));
        p.circle_filled(c - vec2(rr * 0.25, rr * 0.28), rr * 0.45, Color32::from_gray(190));
    }

    fn brush_sections(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.scene.brush;
        let (open, reset, _) = w::section(ui, "size", "Size", true);
        if reset {
            b.radius = 14.0;
            b.spacing = 20.0;
        }
        if open {
            w::slider(ui, "Size", &mut b.radius, 1.0..=200.0, 0, true);
            w::slider(ui, "Minimum Size (%)", &mut 5.0, 0.0..=100.0, 0, false);
            w::slider(ui, "Spacing", &mut b.spacing, 1.0..=200.0, 0, true);
        }
        let (open, reset, _) = w::section(ui, "opacity", "Opacity", true);
        if reset {
            b.flow = 1.0;
            b.opacity = 1.0;
        }
        if open {
            let mut flow = b.flow * 100.0;
            if w::slider(ui, "Flow", &mut flow, 1.0..=100.0, 0, true) {
                b.flow = flow / 100.0;
            }
            w::slider(ui, "Minimum Flow (%)", &mut 5.0, 0.0..=100.0, 0, false);
            let mut op = b.opacity * 100.0;
            if w::slider(ui, "Stroke opacity", &mut op, 1.0..=100.0, 0, true) {
                b.opacity = op / 100.0;
            }
        }
        let (open, _, _) = w::section(ui, "orientation", "Orientation", true);
        if open {
            w::slider(ui, "Angle", &mut 0.0, 0.0..=360.0, 0, false);
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Follow Path");
                    ui.add_sized([ui.available_width(), 20.0], egui::Button::new("Off"));
                });
            });
        }
        let (open, _, _) = w::section(ui, "jitter", "Jitter", false);
        if open {
            for l in ["Size Jitter", "Flow Jitter", "Angle Jitter", "Position Jitter"] {
                w::slider(ui, l, &mut 0.0, 0.0..=1.0, 2, false);
            }
        }
        let (open, _, _) = w::section(ui, "projection", "Projection", false);
        if open {
            ui.add_enabled_ui(false, |ui| {
                ui.label("Alignment: Tangent | Wrap");
                ui.label("Backface culling: On (90)");
                ui.label("Size Space: Object");
            });
        }
    }

    fn alpha_section(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(false, |ui| resource_button(ui, "Alpha", "Shape (Default)"));
        for (id, label) in [("attr", "Attributes"), ("cs", "Color Spaces"), ("res", "Resolution")] {
            w::section(ui, id, label, false);
        }
        let (open, reset, _) = w::section(ui, "params", "Parameters", true);
        let b = &mut self.scene.brush;
        if reset {
            b.hardness = 0.5;
        }
        if open {
            w::slider(ui, "Hardness", &mut b.hardness, 0.0..=1.0, 2, true);
            w::slider(ui, "Roundness", &mut 1.0, 0.0..=1.0, 0, false);
            w::slider(ui, "Size X", &mut 1.0, 0.0..=1.0, 0, false);
            w::slider(ui, "Size Y", &mut 1.0, 0.0..=1.0, 0, false);
            ui.add_enabled(false, egui::Button::new("Restore defaults"));
        }
        w::section(ui, "advblend", "Advanced Blending", false);
    }

    fn material_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
            for (i, c) in ["color", "height", "rough", "metal", "nrm", "op", "emiss"].iter().enumerate() {
                let on = i == 0;
                let b = egui::Button::new(RichText::new(*c).size(10.5)).selected(on).min_size(vec2(0.0, 18.0));
                ui.add_enabled(false, b);
            }
        });
        ui.add_enabled_ui(false, |ui| resource_button(ui, "Material mode", "No Resource Selected"));
        ui.vertical_centered(|ui| ui.label(RichText::new("Or").color(theme::TEXT_DIM)));
        resource_button(ui, "Base color", "uniform color");
        ui.horizontal(|ui| {
            let wd = (ui.available_width() - 32.0).max(40.0);
            let (rect, _) = ui.allocate_exact_size(vec2(wd, 24.0), Sense::hover());
            let c = self.scene.brush.color;
            let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            ui.painter().rect_filled(rect, theme::RADIUS as f32, Color32::from_rgb(to8(c[0]), to8(c[1]), to8(c[2])));
            ui.painter().rect_stroke(rect, theme::RADIUS as f32, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            let (rect, _) = ui.allocate_exact_size(vec2(22.0, 24.0), Sense::hover());
            crate::icons::draw(ui.painter(), rect, Icon::Dropper, theme::TEXT_DIM);
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Color").color(theme::TEXT_DIM));
            egui::color_picker::color_edit_button_rgb(ui, &mut self.scene.brush.color);
        });
        for l in ["Height", "Roughness", "Metallic"] {
            ui.add_enabled_ui(false, |ui| resource_button(ui, l, "uniform color"));
        }
        ui.add_enabled_ui(false, |ui| resource_button(ui, "Normal", "uniform color"));
    }

    // ------------------------------------------------------------ viewport toolbar / overlay

    fn viewport_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            w::icon_button(ui, Icon::Brush, 24.0, false, false);
            let b = &mut self.scene.brush;
            const SLIDER: f32 = 230.0;
            inline_slider(ui, "Size", &mut b.radius, 1.0..=200.0, 0, true);
            if ui.available_width() > SLIDER {
                w::icon_button(ui, Icon::Pressure, 22.0, true, false);
                let mut flow = b.flow * 100.0;
                if inline_slider(ui, "Flow", &mut flow, 1.0..=100.0, 0, true) {
                    b.flow = flow / 100.0;
                }
            }
            if ui.available_width() > SLIDER + 20.0 {
                let mut op = b.opacity * 100.0;
                if inline_slider(ui, "Stroke opacity", &mut op, 1.0..=100.0, 0, true) {
                    b.opacity = op / 100.0;
                }
            }
            if ui.available_width() > SLIDER {
                inline_slider(ui, "Spacing", &mut b.spacing, 1.0..=200.0, 0, true);
            }
            // optional items only when there is room (Painter hides them in narrow viewports too)
            if ui.available_width() > 230.0 {
                w::icon_button(ui, Icon::Lazy, 22.0, false, false);
                inline_slider(ui, "Distance", &mut 8.0, 0.0..=100.0, 0, false);
            }
            if ui.available_width() > 150.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for icon in [Icon::Camera, Icon::Cube, Icon::Pause, Icon::Symmetry] {
                        w::icon_button(ui, icon, 24.0, false, false);
                    }
                });
            }
        });
    }

    /// The viewport's own controls, floating over the free area: Painter's brush toolbar along the top and
    /// the display controls (colour profile, post-processing, channel) below its right end.
    fn viewport_chrome(&mut self, ctx: &egui::Context) {
        let free = self.central;
        let card = |fill_alpha: bool| {
            egui::Frame::new()
                .fill(if fill_alpha { theme::card() } else { theme::PANEL })
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(theme::RADIUS_CARD)
                .inner_margin(egui::Margin::symmetric(8, 4))
        };
        // The display pill goes beside the toolbar when the viewport is wide enough for both (widescreen used to
        // push it under the toolbar, away from the top edge), under its right end otherwise.
        const BAR_MIN: f32 = 560.0;
        let pill_w = self.display_pill_w;
        let side_by_side = free.width() - 8.0 >= BAR_MIN + pill_w + 8.0;
        let (width, top) = if side_by_side {
            let width = (free.width() - 8.0 - pill_w - 8.0).min(1000.0);
            // centred on the viewport unless that would run into the pill
            let x = (free.center().x - width * 0.5).min(free.max.x - 4.0 - pill_w - 8.0 - width);
            (width, pos2(x, free.min.y + 2.0))
        } else {
            let width = (free.width() - 8.0).clamp(160.0, 1000.0);
            (width, pos2(free.center().x - width * 0.5, free.min.y + 2.0))
        };
        let bar = egui::Area::new(egui::Id::new("vp_toolbar"))
            .order(egui::Order::Middle)
            .fade_in(false)
            .fixed_pos(top)
            .show(ctx, |ui| {
                card(true).show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    self.viewport_toolbar(ui);
                });
            })
            .response
            .rect;
        // display controls: render mode spheres + the shading menu, post effects, channel
        let mut picked: Option<ViewKind> = None;
        let pos = if side_by_side {
            pos2(free.max.x - pill_w - 4.0, bar.min.y)
        } else {
            pos2((free.max.x - pill_w - 4.0).max(free.min.x + 2.0), bar.max.y + 8.0)
        };
        let mut menu_btn = Rect::NOTHING;
        let pill = egui::Area::new(egui::Id::new("vp_display")).order(egui::Order::Middle).fade_in(false).fixed_pos(pos).show(ctx, |ui| {
            card(true).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if w::sphere_button(ui, w::SphereKind::Raster, 26.0, !self.path_trace, true).on_hover_text("Real-time render (rasterized)").clicked() {
                        self.path_trace = false;
                    }
                    let pt_ok = self.path_trace_supported;
                    let tip = if pt_ok {
                        "Path tracing (Umbra): full ray traced render, refines while the view is still"
                    } else {
                        "Path tracing needs texture binding arrays, which this GPU / driver does not offer"
                    };
                    if w::sphere_button(ui, w::SphereKind::PathTrace, 26.0, self.path_trace, pt_ok).on_hover_text(tip).clicked() {
                        self.path_trace = true;
                    }
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 26.0), Sense::click());
                    let p = ui.painter();
                    if self.shading_open {
                        p.rect_filled(r, theme::RADIUS as f32, theme::accent_soft());
                    } else if resp.hovered() {
                        p.rect_filled(r, theme::RADIUS as f32, theme::WIDGET_HOVER);
                    }
                    crate::icons::draw(p, r.shrink2(vec2(2.0, 6.0)), Icon::ChevronDown, if self.shading_open { theme::accent_hover() } else { theme::TEXT });
                    if resp.on_hover_text("Viewport shading: environment, color, antialiasing").clicked() {
                        self.shading_open = !self.shading_open;
                    }
                    menu_btn = r;
                    ui.add_space(6.0);
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, theme::BORDER);
                    ui.add_space(6.0);
                    // post effects (tone mapping + colour correction) on / off: stays at hand
                    if w::icon_button(ui, Icon::PostFx, 26.0, self.post.enabled, true)
                        .on_hover_text(if self.post.enabled { "Post effects: on" } else { "Post effects: off" })
                        .clicked()
                    {
                        self.post.enabled = !self.post.enabled;
                    }
                    ui.add_space(6.0);
                    let cur = VIEW_CHOICES.iter().find(|(k, _)| *k == self.view).map(|(_, l)| *l).unwrap_or("Material");
                    egui::ComboBox::from_id_salt("chan").width(150.0).selected_text(cur).show_ui(ui, |ui| {
                        for (k, label) in VIEW_CHOICES {
                            if ui.selectable_label(self.view == k, label).clicked() {
                                picked = Some(k);
                            }
                        }
                    });
                });
            });
        });
        self.display_pill_w = pill.response.rect.width();
        if self.config.show_stats {
            let s = self.stats.shown();
            let usage = self.stats.usage();
            let pct = |v: Option<f32>| v.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "-".into());
            let ms = |v: Option<f32>, digits: usize| v.map(|m| format!("{m:.digits$} ms")).unwrap_or_else(|| "-".into());
            let lines = [
                format!("{} FPS   {} / frame", s.fps, ms(s.frame_ms, 1)),
                format!("Frame  CPU {}   GPU {}", ms(s.cpu_ms, 1), ms(s.gpu_ms, 2)),
                format!("Usage  CPU {}   GPU {}", pct(usage.cpu), pct(usage.gpu)),
            ];
            egui::Area::new(egui::Id::new("vp_stats"))
                .order(egui::Order::Middle)
                .fade_in(false)
                .fixed_pos(pos2(free.min.x + 10.0, bar.max.y + 8.0))
                .show(ctx, |ui| {
                    card(true).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        for (k, l) in lines.iter().enumerate() {
                            let col = if k == 0 { Color32::WHITE } else { theme::TEXT };
                            ui.add(egui::Label::new(RichText::new(l).font(FontId::monospace(11.0)).color(col)).extend());
                        }
                    });
                });
        }
        if let (true, Some(n)) = (self.path_trace, self.path_trace_samples) {
            let done = n >= crate::pathtrace::MAX_SAMPLES;
            egui::Area::new(egui::Id::new("vp_pt_samples"))
                .order(egui::Order::Middle)
                .fade_in(false)
                .fixed_pos(pos2(free.min.x + 10.0, free.max.y - 34.0))
                .show(ctx, |ui| {
                    card(true).show(ui, |ui| {
                        let mut text = if done {
                            format!("Path tracing  {n} samples (done)")
                        } else {
                            format!("Path tracing  {n} / {} samples", crate::pathtrace::MAX_SAMPLES)
                        };
                        if let Some(a) = self.path_trace_ai {
                            text.push_str(&format!("   AI denoised at {a}"));
                        }
                        ui.label(RichText::new(text).size(11.0).color(if done { theme::accent_hover() } else { theme::TEXT }));
                    });
                });
        }
        if let Some(k) = picked {
            self.set_view(k);
        }
        if self.shading_open {
            let anchor = pill.response.rect;
            self.shading_popover(ctx, anchor, menu_btn);
        }
    }

    /// The environment in use, as a picture with its name: clicking it opens the environment picker.
    fn hdri_banner(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let ctx = ui.ctx().clone();
        let width = ui.available_width();
        let (banner, resp) = ui.allocate_exact_size(vec2(width, (width * 0.36).round()), Sense::click());
        self.hdri_banner_rect = banner;
        ui.painter().rect_filled(banner, 8.0, theme::WELL);
        let cur_path = self.hdri_current.and_then(|i| self.hdri_files.get(i).cloned());
        match cur_path.as_ref().map(|p| self.hdri_thumb(&ctx, p)) {
            Some(Some(t)) => paint_hdri_thumb(ui, &t, banner, 8),
            Some(None) => egui::Spinner::new().size(18.0).paint_at(ui, Rect::from_center_size(banner.center(), vec2(18.0, 18.0))),
            None => {}
        }
        let name = cur_path
            .as_ref()
            .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.replace('_', " ")))
            .unwrap_or_else(|| self.env.name.clone());
        let band = Rect::from_min_max(pos2(banner.min.x, banner.max.y - 24.0), banner.max);
        ui.painter().rect_filled(band, egui::CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }, Color32::from_black_alpha(160));
        ui.painter().text(pos2(band.min.x + 9.0, band.center().y), Align2::LEFT_CENTER, &name, theme::semibold(11.5), Color32::WHITE);
        let open = self.hdri_picker_open;
        if resp.hovered() || open {
            ui.painter().rect_stroke(banner, 8.0, Stroke::new(if open { 2.0 } else { 1.0 }, if open { theme::accent() } else { theme::TEXT_DIM }), egui::StrokeKind::Inside);
            ui.painter().text(pos2(band.max.x - 9.0, band.center().y), Align2::RIGHT_CENTER, "Change", theme::semibold(11.0), theme::accent_hover());
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp.on_hover_text("Pick an environment")
    }

    /// Floating environment picker beside the shading menu (`menu`), level with the banner: a search field and every
    /// HDRI found (HDRI folder and libraries) as a grid of previews. Returns the one clicked.
    fn hdri_picker(&mut self, ctx: &egui::Context, menu: Rect, banner: Rect) -> Option<usize> {
        const W: f32 = 380.0;
        const COLS: usize = 3;
        const GAP: f32 = 8.0;
        const LABEL_H: f32 = 16.0;
        let x = (menu.min.x - 8.0 - W).max(self.central.min.x + 4.0);
        let pos = pos2(x, banner.min.y.max(menu.min.y));
        let max_h = (ctx.screen_rect().max.y - pos.y - 60.0).max(200.0);
        let frame = egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(12)
            .inner_margin(egui::Margin::same(14))
            .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) });
        let names: Vec<String> =
            self.hdri_files.iter().map(|p| p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").replace('_', " ")).collect();
        let mut pick = None;
        egui::Area::new(egui::Id::new("hdri_picker")).order(egui::Order::Foreground).fade_in(false).fixed_pos(pos).show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(W - 28.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Environment").font(theme::semibold(13.5)).color(Color32::WHITE));
                    ui.label(RichText::new(format!("{} HDRI", names.len())).size(11.0).color(theme::TEXT_DIM));
                });
                ui.add_space(8.0);
                // search by name (focused when the picker opens)
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.hdri_query)
                        .hint_text("Search HDRI...")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 5)),
                );
                if std::mem::take(&mut self.hdri_picker_focus) {
                    search.request_focus();
                }
                ui.add_space(8.0);
                let words: Vec<String> = self.hdri_query.to_lowercase().split_whitespace().map(str::to_string).collect();
                let shown: Vec<usize> = (0..names.len())
                    .filter(|&i| {
                        let n = names[i].to_lowercase();
                        words.iter().all(|w| n.contains(w.as_str()))
                    })
                    .collect();
                if names.is_empty() {
                    ui.label(RichText::new("No .exr found in Documents/HDRI or in the libraries.").size(11.0).color(theme::TEXT_DIM));
                    return;
                }
                egui::ScrollArea::vertical().id_salt("hdri_picker_grid").max_height(max_h).min_scrolled_height(max_h.min(560.0)).auto_shrink([false, true]).show(ui, |ui| {
                    let width = ui.available_width();
                    let cell_w = ((width - GAP * (COLS - 1) as f32) / COLS as f32).floor();
                    let cell = vec2(cell_w, (cell_w * 0.5).round());
                    if shown.is_empty() {
                        ui.label(RichText::new("No HDRI matches the search.").size(11.0).color(theme::TEXT_DIM));
                    }
                    for chunk in shown.chunks(COLS) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = GAP;
                            for &idx in chunk {
                                let path = self.hdri_files[idx].clone();
                                let (rect, resp) = ui.allocate_exact_size(vec2(cell.x, cell.y + LABEL_H), Sense::click());
                                let img = Rect::from_min_size(rect.min, cell);
                                ui.painter().rect_filled(img, 6.0, theme::WELL);
                                match self.hdri_thumb(ctx, &path) {
                                    Some(t) => paint_hdri_thumb(ui, &t, img, 6),
                                    None => egui::Spinner::new().size(12.0).paint_at(ui, Rect::from_center_size(img.center(), vec2(12.0, 12.0))),
                                }
                                let sel = self.hdri_current == Some(idx);
                                if sel {
                                    ui.painter().rect_stroke(img, 6.0, Stroke::new(2.0, theme::accent()), egui::StrokeKind::Inside);
                                } else if resp.hovered() {
                                    ui.painter().rect_stroke(img, 6.0, Stroke::new(1.0, theme::TEXT_DIM), egui::StrokeKind::Inside);
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                let label = names[idx].clone();
                                ui.painter().with_clip_rect(rect).text(
                                    pos2(rect.min.x + 1.0, img.max.y + 3.0),
                                    Align2::LEFT_TOP,
                                    &label,
                                    FontId::proportional(10.5),
                                    if sel { Color32::WHITE } else { theme::TEXT_DIM },
                                );
                                if resp.on_hover_text(&label).clicked() {
                                    pick = Some(idx);
                                }
                            }
                        });
                        ui.add_space(GAP);
                    }
                });
            });
        });
        pick
    }

    /// The post effects of the viewport (Painter's): a master switch, then each effect with its settings when on;
    /// those that need image passes are listed as not rendered yet (flagged when the project turns them on).
    fn post_effects_ui(&mut self, ui: &mut egui::Ui) {
        // header: the section's caption with the master switch
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("POST EFFECTS").font(theme::semibold(10.5)).color(theme::TEXT_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::switch(ui, self.post.enabled, true).on_hover_text("All post effects").clicked() {
                    self.post.enabled = !self.post.enabled;
                }
            });
        });
        ui.add_space(4.0);
        if !self.post.enabled {
            ui.label(RichText::new("Off: the shaded view goes to the screen as it is.").size(11.0).color(theme::TEXT_DISABLED));
            return;
        }
        let p = &mut self.post;
        effect_category(ui, "TONE & COLOR");
        effect_box(ui, "Tone mapping", "ACES filmic", &mut p.tone_mapping, |_| {});
        effect_box(ui, "Color correction", "", &mut p.color_correction, |ui| {
            w::form_slider(ui, "Brightness", &mut p.brightness, -1.0..=1.0, 2);
            w::form_slider(ui, "Contrast", &mut p.contrast, 0.0..=2.0, 2);
            w::form_slider(ui, "Gamma", &mut p.gamma, 0.2..=3.0, 2);
            w::form_slider(ui, "Saturation", &mut p.saturation, -1.0..=1.0, 2);
            w::form_slider(ui, "Temperature", &mut p.temperature, -1.0..=1.0, 2);
            w::form_slider(ui, "Magenta - green", &mut p.magenta_green, -1.0..=1.0, 2);
        });
        effect_category(ui, "LIGHT & LENS");
        effect_box(ui, "Bloom", "glow of bright and emissive areas", &mut p.bloom, |ui| {
            w::form_slider(ui, "Strength", &mut p.bloom_level, 0.0..=10.0, 2);
            w::form_slider(ui, "Threshold", &mut p.bloom_threshold, 0.0..=5.0, 2);
            w::form_slider(ui, "Falloff", &mut p.bloom_falloff, 0.0..=1.0, 2);
        });
        effect_box(ui, "Vignette", "", &mut p.vignette, |ui| {
            w::form_slider(ui, "Strength", &mut p.vignette_level, 0.0..=1.0, 2);
            w::form_slider(ui, "Radius", &mut p.vignette_radius, 0.0..=1.0, 2);
            w::form_slider(ui, "Feather", &mut p.vignette_feather, 0.0..=1.0, 2);
            w::form_slider(ui, "Ratio", &mut p.vignette_ratio, 0.5..=3.0, 2);
        });
        effect_category(ui, "FILM");
        effect_box(ui, "Film grain", "", &mut p.grain, |ui| {
            w::form_slider(ui, "Amount", &mut p.grain_level, 0.0..=0.1, 3);
        });
        effect_category(ui, "NOT RENDERED YET");
        egui::Frame::new()
            .fill(theme::WELL)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (_, name) in UNRENDERED_POST {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).size(11.5).color(theme::TEXT_DISABLED));
                        if self.post_unrendered.contains(&name) {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new("on in this project").size(10.5).color(theme::WARNING));
                            });
                        }
                    });
                }
            });
    }

    /// Blender-style "Viewport Shading" popover under the display pill: render mode, environment, colour,
    /// antialiasing. Closes on a click outside it or Escape.
    fn shading_popover(&mut self, ctx: &egui::Context, anchor: Rect, toggle: Rect) {
        const W: f32 = 312.0;
        let pos = pos2((anchor.max.x - W).max(self.central.min.x + 4.0), anchor.max.y + 6.0);
        let frame = egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(12)
            .inner_margin(egui::Margin::same(14))
            .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) });
        let max_h = (ctx.screen_rect().max.y - pos.y - 70.0).max(220.0);
        let area = egui::Area::new(egui::Id::new("vp_shading")).order(egui::Order::Foreground).fade_in(false).fixed_pos(pos).show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(W - 28.0);
                ui.label(RichText::new("Viewport Shading").font(theme::semibold(13.5)).color(Color32::WHITE));
                ui.add_space(2.0);
                egui::ScrollArea::vertical().id_salt("vp_shading_scroll").max_height(max_h).min_scrolled_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                    ui.set_width(W - 28.0 - 10.0);
                    w::caption(ui, "RENDER");
                    let pt_ok = self.path_trace_supported;
                    if let Some(k) = w::segmented(
                        ui,
                        &[("Real-time", true, ""), ("Path tracing", pt_ok, "Needs texture binding arrays (not offered by this GPU / driver)")],
                        self.path_trace as usize,
                    ) {
                        self.path_trace = k == 1;
                    }
                    w::caption(ui, "ENVIRONMENT");
                    if self.hdri_banner(ui).clicked() {
                        self.hdri_picker_open = !self.hdri_picker_open;
                        self.hdri_picker_focus = self.hdri_picker_open;
                    }
                    let mut deg = self.env_yaw.to_degrees().rem_euclid(360.0);
                    if w::form_slider(ui, "Rotation", &mut deg, 0.0..=360.0, 0) {
                        self.env_yaw = deg.to_radians();
                    }
                    w::form_slider(ui, "Exposure (EV)", &mut self.exposure_ev, -5.0..=5.0, 2);
                    if w::form_toggle(ui, "Background", self.show_background) {
                        self.show_background = !self.show_background;
                    }
                    w::caption(ui, "COLOR");
                    w::form_combo(ui, "vp_cs", "Color profile", "ACES - sRGB", &["ACES - sRGB"], false);
                    self.post_effects_ui(ui);
                    w::caption(ui, "ANTIALIASING");
                    let levels = [(1u32, "Off"), (2, "2x"), (4, "4x"), (8, "8x")];
                    let items: Vec<(&str, bool, &str)> =
                        levels.iter().map(|(n, l)| (*l, self.msaa_supported.contains(n), "Not supported by this GPU")).collect();
                    let want = self.config.viewport_msaa;
                    let cur = levels.iter().rposition(|(n, _)| *n <= want && self.msaa_supported.contains(n)).unwrap_or(0);
                    if let Some(k) = w::segmented(ui, &items, cur) {
                        self.config.viewport_msaa = levels[k].0;
                        self.config.save();
                        self.status = format!("Viewport antialiasing: {}", levels[k].1);
                    }
                    w::caption(ui, "SHADING");
                    w::form_slider(ui, "AO intensity", &mut self.ao_intensity, 0.0..=1.0, 2);
                });
            });
        });
        let rect = area.response.rect;
        let (pressed, origin, escape) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.press_origin(), i.key_pressed(egui::Key::Escape)));
        // the environment picker floats beside the menu, level with the banner
        if self.hdri_picker_open {
            if let Some(k) = self.hdri_picker(ctx, rect, self.hdri_banner_rect) {
                self.load_hdri(k);
                self.hdri_picker_open = false;
            }
        }
        if escape {
            // Esc closes the picker first, then the menu
            if self.hdri_picker_open {
                self.hdri_picker_open = false;
            } else {
                self.shading_open = false;
            }
            return;
        }
        // A click outside closes them. "Outside" is decided by the layer under the press (the menu's, the picker's,
        // a combo list's own popup layer...), not by this frame's rectangles: the frame can be laid out twice, and a
        // picker closed by the pick in the first pass would make the same press read as "outside" in the second
        // (choosing an HDRI closed the whole menu).
        let in_popup = ctx.memory(|m| m.any_popup_open());
        if let (true, false, Some(p)) = (pressed, in_popup, origin) {
            let layer = ctx.layer_id_at(p).map(|l| l.id);
            let on_menu = layer == Some(egui::Id::new("vp_shading"));
            let on_picker = layer == Some(egui::Id::new("hdri_picker"));
            let on_pill = layer == Some(egui::Id::new("vp_display"));
            if !on_menu && !on_picker && !on_pill && !toggle.contains(p) {
                self.shading_open = false;
                self.hdri_picker_open = false;
            } else if on_menu && self.hdri_picker_open && !self.hdri_banner_rect.contains(p) {
                self.hdri_picker_open = false;
            }
        }
    }

    // ------------------------------------------------------------ right dock

    fn texture_set_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            w::icon_button(ui, Icon::EyeOff, 20.0, false, false);
            w::icon_button(ui, Icon::Eye, 20.0, false, false);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                w::icon_button(ui, Icon::Menu, 20.0, false, false);
            });
        });
        let f = FontId::proportional(11.5);
        let n = self.scene.texs.len();
        let loading = self.loading_sets();
        if loading != 0 {
            ui.ctx().request_repaint(); // the spinners turn
        }
        egui::ScrollArea::vertical().id_salt("sets").auto_shrink([false, false]).show(ui, |ui| {
            for i in 0..n {
                let name = self.scene.mesh.set_names.get(i).cloned().unwrap_or_else(|| self.doc_name.clone());
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                if resp.clicked() {
                    self.selected_set = i;
                    self.status = format!("Active texture set: {name}");
                }
                let sel = self.selected_set == i;
                let fill = if sel {
                    theme::accent_soft()
                } else if resp.hovered() {
                    theme::WIDGET
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(row, theme::RADIUS as f32, fill);
                if sel {
                    ui.painter().rect_stroke(row, theme::RADIUS as f32, Stroke::new(1.0, theme::accent()), egui::StrokeKind::Inside);
                }
                crate::icons::draw(
                    ui.painter(),
                    Rect::from_center_size(pos2(row.min.x + 12.0, row.center().y), vec2(14.0, 14.0)),
                    Icon::Eye,
                    Color32::WHITE,
                );
                // the shader name goes first when the window is narrow, then the size
                let (show_shader, show_size) = (row.width() > 380.0, row.width() > 250.0);
                let mut right = row.max.x - 6.0;
                if show_shader {
                    ui.painter().text(pos2(right, row.center().y), Align2::RIGHT_CENTER, "Main shader", f.clone(), theme::TEXT_DIM);
                    right -= 74.0;
                }
                if show_size {
                    ui.painter().text(
                        pos2(right, row.center().y),
                        Align2::RIGHT_CENTER,
                        format!("{}x{}", self.scene.texs[i].w, self.scene.texs[i].h),
                        f.clone(),
                        theme::TEXT_DIM,
                    );
                    right -= 70.0;
                }
                // a set whose composition is on its way: grey name and a turning spinner after it
                let is_loading = (loading >> i) & 1 == 1;
                let clip = Rect::from_min_max(pos2(row.min.x + 26.0, row.min.y), pos2(right.max(row.min.x + 30.0), row.max.y));
                let text_rect = ui.painter().with_clip_rect(clip).text(
                    pos2(row.min.x + 26.0, row.center().y),
                    Align2::LEFT_CENTER,
                    &name,
                    f.clone(),
                    if is_loading { theme::TEXT_DIM } else { Color32::WHITE },
                );
                if is_loading {
                    let c = pos2((text_rect.max.x + 10.0).min(right - 6.0), row.center().y);
                    let t = ui.input(|inp| inp.time) as f32;
                    let r = 4.5;
                    let painter = ui.painter().with_clip_rect(clip.expand(12.0));
                    painter.circle_stroke(c, r, Stroke::new(1.5, theme::TEXT_DIM.gamma_multiply(0.35)));
                    // a quarter arc turning around the circle
                    let start = t * 6.0;
                    let pts: Vec<egui::Pos2> = (0..=10)
                        .map(|k| {
                            let a = start + k as f32 / 10.0 * std::f32::consts::FRAC_PI_2 * 1.5;
                            pos2(c.x + r * a.cos(), c.y + r * a.sin())
                        })
                        .collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5, theme::accent())));
                }
            }
        });
    }

    // ------------------------------------------------------------ dock windows

    /// Content of one window of the dock.
    fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Properties => self.properties_pane(ui),
            Pane::Assets => self.assets_tab(ui),
            Pane::TextureSetList => self.texture_set_list(ui),
            Pane::Layers => self.layers_panel(ui),
            Pane::TextureSetSettings => self.texture_set_settings(ui),
            Pane::DisplaySettings => self.display_settings_pane(ui),
            Pane::ShaderSettings => self.shader_settings_pane(ui),
            Pane::History => self.history_pane(ui),
            Pane::Log => self.log_pane(ui),
        }
    }

    /// Bring a window to front: select its tab, or re-open it when it was closed.
    fn show_pane(&mut self, pane: Pane) {
        dock::show(&mut self.dock, pane);
    }

    /// Persist the arrangement once a drag / resize has finished.
    fn save_layout_when_idle(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_released()) && !self.layout_saved.is_empty() {
            return;
        }
        if let Some(json) = dock::layout_json(&self.dock) {
            if json != self.layout_saved {
                dock::save_layout_json(&json);
                self.layout_saved = json;
            }
        }
    }

    fn texture_set_settings(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            w::title_row(ui, "GENERAL PROPERTIES", false);
            // texture size of the active set (enabled): paint of the set is discarded on change
            let set = self.active();
            if let Some(t) = self.scene.texs.get(set) {
                let cur = t.w;
                let mut size = cur;
                ui.horizontal(|ui| {
                    ui.label("Size");
                    egui::ComboBox::from_id_salt("tssize").width(90.0).selected_text(size.to_string()).show_ui(ui, |ui| {
                        for s in [128u32, 256, 512, 1024, 2048, 4096] {
                            ui.selectable_value(&mut size, s, s.to_string());
                        }
                    });
                });
                if size != cur || self.scene.texs[set].h != cur {
                    self.resize_set(set, size, size);
                }
            }
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let a = self.selected_set.min(self.stacks.len().saturating_sub(1));
                    ui.label(&self.stacks[a].name);
                });
                ui.horizontal(|ui| {
                    ui.label("Shader Instance");
                    egui::ComboBox::from_id_salt("tsshader").selected_text("Main shader").show_ui(ui, |_| {});
                });
            });
            w::separator(ui);
            w::title_row(ui, "CHANNELS", false);
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Channels");
                    ui.button("Add or remove channels");
                });
                for (l, v) in [
                    ("Normal mixing", "Replace"),
                    ("Height to normal method", "Smooth (Sobel)"),
                    ("Ambient occlusion mixing", "Replace"),
                    ("UV padding", "UV Space Neighbor"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(l);
                        egui::ComboBox::from_id_salt(l).selected_text(v).show_ui(ui, |_| {});
                    });
                }
            });
            w::separator(ui);
            w::title_row(ui, "MESH MAPS", false);
            ui.add_enabled(false, egui::Button::new("Bake Mesh Maps"));
        });
    }

    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        // toolbar: channel combo + icons
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            egui::ComboBox::from_id_salt("channel")
                .width(110.0)
                .selected_text(channel_label(self.active_channel))
                .show_ui(ui, |ui| {
                    for c in CHANNEL_CHOICES {
                        ui.selectable_value(&mut self.active_channel, c, channel_label(c));
                    }
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::icon_button(ui, Icon::Trash, 24.0, false, true).on_hover_text("Delete layer").clicked() {
                    self.delete_selected();
                }
                if w::icon_button(ui, Icon::Folder, 24.0, false, true).on_hover_text("Add group").clicked() {
                    self.layer_action(LayerAction::AddGroup);
                }
                w::icon_button(ui, Icon::Sphere, 24.0, false, false).on_hover_text("Add smart material");
                if w::icon_button(ui, Icon::Bucket, 24.0, false, true).on_hover_text("Add fill layer").clicked() {
                    self.add_fill_layer();
                }
                w::icon_button(ui, Icon::Brush, 24.0, false, false).on_hover_text("Add paint layer");
                if w::icon_button(ui, Icon::Mask, 24.0, false, true).on_hover_text("Add white mask").clicked() {
                    self.layer_action(LayerAction::AddMask(true));
                }
                w::icon_button(ui, Icon::Wand, 24.0, false, false).on_hover_text("Add effect");
            });
        });
        ui.add_space(4.0);
        let ch = self.active_channel;
        let mut changed = false;
        let mut mask_click: Option<Vec<usize>> = None;
        let mut mask_open = self.mask_open_ui.clone();
        let mut effect_sel = self.props_sel.clone();
        let a = self.active();
        let mut sel = self.stacks[a].selected.clone();
        let mut collapsed = std::mem::take(&mut self.stacks[a].collapsed);
        let mut action: Option<(LayerAction, Vec<usize>)> = None;
        let mut dropped: Option<(Vec<usize>, Vec<usize>, usize)> = None;
        let mut renaming = self.renaming.take();
        self.thumbs_wanted = std::time::Instant::now();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.stacks[a].layers.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("'{}' has no layers. Add a fill layer.", self.stacks[a].name))
                        .color(theme::TEXT_DIM),
                );
            } else {
                let mut path = Vec::new();
                let mut lc = LayerCtx {
                    ch,
                    set: a,
                    thumbs: &self.thumb_tex,
                    selected: &mut sel,
                    collapsed: &mut collapsed,
                    changed: &mut changed,
                    mask_click: &mut mask_click,
                    mask_open: &mut mask_open,
                    effect_sel: &mut effect_sel,
                    action: &mut action,
                    drop: &mut dropped,
                    renaming: &mut renaming,
                };
                layer_rows(ui, &mut self.stacks[a].layers, 0, &mut path, &mut lc);
            }
        });
        self.stacks[a].selected = sel;
        self.stacks[a].collapsed = collapsed;
        self.renaming = renaming;
        if let Some((from, parent, index)) = dropped {
            let st = &mut self.stacks[a];
            if let Some(np) = shard_model::edit::move_to(&mut st.layers, &from, &parent, index) {
                st.selected = Some(np);
                changed = true;
            }
        }
        if let Some((act, path)) = action {
            self.stacks[a].selected = Some(path);
            self.layer_action(act);
        }
        if changed {
            self.mark_recompose();
            self.mask_reeval = true;
        }
        self.mask_open_ui = mask_open;
        if effect_sel != self.props_sel {
            self.props_sel = effect_sel;
            self.pending_focus = Some(Pane::Properties);
        }
        if let Some(p) = mask_click {
            self.toggle_mask_preview(p);
        }
    }

    /// Diagnostic: show the properties of effect `idx` of the layer called `name` (mask or content).
    pub fn select_effect(&mut self, name: &str, idx: usize, in_mask: bool) -> Result<(), String> {
        let path = self.find_layer_path(name).ok_or("layer not found")?;
        self.props_sel = Some((path, idx, in_mask));
        Ok(())
    }

    /// Select the active texture set (headless checks / future shortcuts).
    pub fn set_active_set(&mut self, i: usize) {
        self.selected_set = i;
        self.sync_stacks();
        self.scene.active_set = self.active();
    }

    /// Same as the "+ Fill layer" button (used by headless checks).
    pub fn add_fill_layer_public(&mut self) {
        self.add_fill_layer();
        self.do_recompose(None);
    }

    /// Add a fill layer on top of the **active texture set's** stack.
    fn add_fill_layer(&mut self) {
        self.sync_stacks();
        let c = self.scene.brush.color;
        let a = self.active();
        let st = &mut self.stacks[a];
        let n = st.layers.len() + 1;
        st.layers.insert(0, new_fill_layer(&format!("Fill layer {n}"), c));
        st.session_layers += 1;
        st.selected = Some(vec![0]);
        self.mark_recompose();
    }

    /// Run `act` on the selected layer of the active set (context menu, toolbar and Painter's shortcuts:
    /// Ctrl+D duplicate, Ctrl+G group, Del delete, Ctrl+C / Ctrl+X / Ctrl+V clipboard).
    pub fn layer_action(&mut self, act: LayerAction) {
        use shard_model::edit as ed;
        let a = self.active();
        let st = &mut self.stacks[a];
        let sel = st.selected.clone();
        let mut done = true;
        match (act, sel) {
            (LayerAction::Duplicate, Some(p)) => st.selected = ed::duplicate(&mut st.layers, &p),
            (LayerAction::Group, Some(p)) => st.selected = ed::group(&mut st.layers, &p),
            (LayerAction::Ungroup, Some(p)) => st.selected = ed::ungroup(&mut st.layers, &p).or(Some(p)),
            (LayerAction::Delete, Some(_)) => {
                self.delete_selected();
                return;
            }
            (LayerAction::Copy, Some(p)) => {
                self.layer_clipboard = ed::layer_at(&st.layers, &p).map(ed::clone_fresh);
                self.status = "Layer copied".into();
                return;
            }
            (LayerAction::Cut, Some(p)) => {
                self.layer_clipboard = ed::remove(&mut st.layers, &p).map(|l| ed::clone_fresh(&l));
                st.selected = None;
            }
            (LayerAction::Paste, sel) => {
                let Some(l) = self.layer_clipboard.as_ref().map(ed::clone_fresh) else {
                    self.status = "Nothing to paste".into();
                    return;
                };
                // above the selection, or on top of the stack
                let (parent, idx) = match sel {
                    Some(p) => (p[..p.len() - 1].to_vec(), *p.last().unwrap_or(&0)),
                    None => (Vec::new(), 0),
                };
                st.selected = ed::insert(&mut st.layers, &parent, idx, l);
            }
            (LayerAction::AddGroup, sel) => {
                let g = ed::new_group("Group", Vec::new());
                let (parent, idx) = match sel {
                    Some(p) => (p[..p.len() - 1].to_vec(), *p.last().unwrap_or(&0)),
                    None => (Vec::new(), 0),
                };
                st.selected = ed::insert(&mut st.layers, &parent, idx, g);
            }
            (LayerAction::AddMask(white), Some(p)) => match ed::layer_at_mut(&mut st.layers, &p) {
                // a layer "without mask" is one with no mask object or with the empty white placeholder
                Some(l) if l.mask.as_ref().map(|m| m.effects.is_empty() && !m.explicit && m.initial != 0).unwrap_or(true) => {
                    ed::add_mask(l, white)
                }
                _ => {
                    self.status = "This layer already has a mask".into();
                    done = false
                }
            },
            (LayerAction::RemoveMask, Some(p)) => match ed::layer_at_mut(&mut st.layers, &p) {
                Some(l) if l.mask.is_some() => l.mask = None,
                _ => done = false,
            },
            (LayerAction::ToggleMask, Some(p)) => match ed::layer_at_mut(&mut st.layers, &p).and_then(|l| l.mask.as_mut()) {
                Some(m) => m.enabled = !m.enabled,
                None => done = false,
            },
            _ => done = false,
        }
        if done {
            self.mark_recompose();
            self.mask_reeval = true;
        }
    }

    fn delete_selected(&mut self) {
        let a = self.active();
        let st = &mut self.stacks[a];
        let Some(path) = st.selected.clone() else { return };
        if remove_at(&mut st.layers, &path) {
            if path.len() == 1 && path[0] < st.session_layers {
                st.session_layers -= 1;
            }
            st.selected = None;
            self.mark_recompose();
        }
    }
}

fn remove_at(layers: &mut Vec<Layer>, path: &[usize]) -> bool {
    match path {
        [i] if *i < layers.len() => {
            layers.remove(*i);
            true
        }
        [i, rest @ ..] if *i < layers.len() => remove_at(&mut layers[*i].children, rest),
        _ => false,
    }
}

// ---------------------------------------------------------------- small helpers

fn resource_button(ui: &mut egui::Ui, title: &str, sub: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, theme::WIDGET);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    p.text(pos2(rect.center().x, rect.min.y + 12.0), Align2::CENTER_CENTER, title, FontId::proportional(11.5), theme::TEXT);
    p.text(pos2(rect.center().x, rect.min.y + 26.0), Align2::CENTER_CENTER, sub, FontId::proportional(10.5), theme::TEXT_DIM);
}

/// Toolbar slider: `label [track] value` in ~200 px.
fn inline_slider(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, dec: usize, enabled: bool) -> bool {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(11.5), theme::TEXT);
    let lw = galley.size().x;
    // room for the value (up to 4 digits) and a gap before the next control
    let (rect, _) = ui.allocate_exact_size(vec2(lw + 6.0 + 56.0 + 8.0 + 30.0 + 12.0, 22.0), Sense::hover());
    let p = ui.painter().clone();
    let dim = if enabled { theme::TEXT } else { theme::TEXT_DISABLED };
    p.text(pos2(rect.min.x, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(11.5), dim);
    let (lo, hi) = (*range.start(), *range.end());
    let x0 = rect.min.x + lw + 6.0;
    let x1 = x0 + 56.0;
    let y = rect.center().y;
    let id = ui.id().with(("inl", label));
    let resp = ui.interact(
        Rect::from_min_max(pos2(x0 - 5.0, y - 9.0), pos2(x1 + 5.0, y + 9.0)),
        id,
        if enabled { Sense::click_and_drag() } else { Sense::hover() },
    );
    let mut changed = false;
    if enabled && (resp.dragged() || resp.clicked()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
            let nv = lo + t * (hi - lo);
            if (nv - *v).abs() > f32::EPSILON {
                *v = nv;
                changed = true;
            }
        }
    }
    let t = ((*v - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    let hx = x0 + t * (x1 - x0);
    let track = if enabled { theme::SLIDER_TRACK } else { theme::SLIDER_TRACK_OFF };
    let fill = if enabled { theme::accent() } else { theme::SLIDER_FILL_OFF };
    p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(2.0, track));
    p.line_segment([pos2(x0, y), pos2(hx, y)], Stroke::new(2.0, fill));
    p.circle_filled(pos2(hx, y), 4.5, theme::PANEL);
    p.circle_stroke(pos2(hx, y), 4.5, Stroke::new(1.5, fill));
    p.text(
        pos2(x1 + 8.0, y),
        Align2::LEFT_CENTER,
        format!("{:.*}", dec, *v),
        FontId::proportional(11.5),
        if enabled { theme::VALUE } else { theme::TEXT_DISABLED },
    );
    changed
}

fn channel_label(bit: u32) -> String {
    match bit {
        0 => "Base color".into(),
        1 => "Height".into(),
        4 => "Emissive".into(),
        7 => "Roughness".into(),
        13 => "Metallic".into(),
        22 => "Normal".into(),
        b => format!("Channel {b}"),
    }
}

fn new_fill_layer(name: &str, color: [f32; 3]) -> Layer {
    let all = ChannelSet(1);
    Layer {
        uid: shard_model::next_uid(),
        kind: LayerKind::Layer,
        name: name.to_string(),
        visible: true,
        blend: vec![],
        content: vec![Effect {
            uid: shard_model::next_uid(),
            visible: true,
            channels: all,
            label: None,
            blend: vec![],
            kind: EffectKind::Fill(Fill {
                projection: Projection::Uv,
                filtering: 2,
                uv: UvTransform::default(),
                sources: vec![Source::Uniform {
                    channels: all,
                    color: Some(color.map(shard_compose::srgb_to_linear)),
                    opacity: 1.0,
                }],
                projector: None,
            }),
        }],
        mask: None,
        children: vec![],
    }
}

fn short_blend(name: &str) -> &str {
    match name {
        "Normal" => "Norm",
        "Multiply" => "Mul",
        "Passthrough" => "Pthr",
        "LinearDodge" => "Add",
        "NormalMapCombine" => "NMC",
        "SoftLight" => "SLight",
        n => n,
    }
}

fn blend_of(l: &Layer, ch: u32) -> (u8, f32) {
    for b in &l.blend {
        if b.channels.contains(ch) {
            return (b.mode.0, b.opacity);
        }
    }
    (BlendMode::NORMAL.0, 1.0)
}

fn set_blend(l: &mut Layer, ch: u32, mode: u8, opacity: f32) {
    for b in &mut l.blend {
        if b.channels.contains(ch) {
            b.mode = BlendMode(mode);
            b.opacity = opacity;
            return;
        }
    }
    l.blend.push(ChannelBlend { mode: BlendMode(mode), channels: ChannelSet(1u128 << ch), opacity });
}

fn thumb_color(l: &Layer) -> Option<Color32> {
    for e in &l.content {
        if let EffectKind::Fill(f) = &e.kind {
            for s in &f.sources {
                if let Source::Uniform { channels, color: Some(c), .. } = s {
                    if channels.contains(0) {
                        let e = |v: f32| (shard_compose::linear_to_srgb(v) * 255.0 + 0.5) as u8;
                        return Some(Color32::from_rgb(e(c[0]), e(c[1]), e(c[2])));
                    }
                }
            }
        }
    }
    None
}

struct LayerCtx<'a> {
    ch: u32,
    /// Texture set shown and the pictures of its layers.
    set: usize,
    thumbs: &'a std::collections::HashMap<(usize, u64, bool), egui::TextureHandle>,
    selected: &'a mut Option<Vec<usize>>,
    collapsed: &'a mut HashSet<Vec<usize>>,
    changed: &'a mut bool,
    /// Path of the layer whose mask thumbnail was Alt+clicked.
    mask_click: &'a mut Option<Vec<usize>>,
    /// Layer whose mask content list is expanded (plain click on the mask thumbnail).
    mask_open: &'a mut Option<Vec<usize>>,
    /// Mask effect row clicked: (layer path, position) -> properties dock.
    effect_sel: &'a mut Option<(Vec<usize>, usize, bool)>,
    /// Context-menu action requested on a layer.
    action: &'a mut Option<(LayerAction, Vec<usize>)>,
    /// Drag and drop result: (from path, parent path, index in parent).
    drop: &'a mut Option<(Vec<usize>, Vec<usize>, usize)>,
    renaming: &'a mut Option<(Vec<usize>, String)>,
}

/// Operations on a layer of the stack (context menu and Painter's shortcuts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerAction {
    Duplicate,
    Group,
    Ungroup,
    Delete,
    Copy,
    Cut,
    Paste,
    AddGroup,
    AddMask(bool),
    RemoveMask,
    ToggleMask,
}

const ROW_H: f32 = 46.0;

/// Debug aid: log who switches the viewport (set `SHARD_TRACE_VIEW`).
fn trace_view(what: &str) {
    if std::env::var_os("SHARD_TRACE_VIEW").is_some() {
        eprintln!("[view] {what}\n{}", std::backtrace::Backtrace::force_capture());
    }
}

/// State of a preview texture.
enum Thumb {
    Pending,
    Ready(egui::TextureHandle),
    /// The asset has no preview (placeholder glyph).
    Missing,
}

/// Kinds of resources the picker offers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Procedural,
    Texture,
    Alpha,
}

impl PickKind {
    fn label(self) -> &'static str {
        match self {
            PickKind::Procedural => "Procedural",
            PickKind::Texture => "Texture",
            PickKind::Alpha => "Alpha",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            PickKind::Procedural => "fx",
            PickKind::Texture => "img",
            PickKind::Alpha => "a",
        }
    }
    fn colour(self) -> Color32 {
        match self {
            PickKind::Procedural => Color32::from_gray(66),
            PickKind::Texture => Color32::from_gray(54),
            PickKind::Alpha => Color32::from_gray(78),
        }
    }
}

#[derive(Clone)]
struct PickItem {
    name: String,
    kind: PickKind,
    /// Shelf file.
    path: Option<PathBuf>,
    /// Resource of this project (`/name?version=...`).
    resource: Option<String>,
    origin: String,
    outputs: Vec<shard_model::ProceduralOutput>,
}

impl PickItem {
    fn choice(&self) -> PickChoice {
        let res = match (&self.resource, &self.path) {
            (Some(r), _) => r.clone(),
            (None, Some(p)) => format!("file:{}", p.display()),
            (None, None) => String::new(),
        };
        match self.kind {
            PickKind::Procedural => PickChoice::Procedural { resource: res, outputs: self.outputs.clone() },
            PickKind::Texture | PickKind::Alpha => PickChoice::Texture { resource: res },
        }
    }
}

enum PickChoice {
    Procedural { resource: String, outputs: Vec<shard_model::ProceduralOutput> },
    Texture { resource: String },
    Anchor { uid: u64 },
}

/// State of the resource picker window.
struct Picker {
    /// (layer path, effect position, in mask, source index) of the slot that opened it.
    target: (Vec<usize>, usize, bool, usize),
    tab: usize,
    query: String,
    kinds: [bool; 3],
    origin_filter: Option<String>,
    selected: Option<usize>,
    tile: f32,
    list_view: bool,
    items: Vec<PickItem>,
    /// (uid, name, owner layer)
    anchors: Vec<(u64, String, String)>,
}

/// Every anchor point of a stack: (uid, name, owner layer).
fn collect_anchors(layers: &[Layer]) -> Vec<(u64, String, String)> {
    fn eff(es: &[Effect], owner: &str, out: &mut Vec<(u64, String, String)>) {
        for e in es {
            match &e.kind {
                EffectKind::Anchor { name } if e.uid != 0 => {
                    let n = name.clone().or_else(|| e.label.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Anchor point".into());
                    out.push((e.uid, n, owner.to_string()));
                }
                EffectKind::Group { children } => eff(children, owner, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    fn walk(ls: &[Layer], out: &mut Vec<(u64, String, String)>) {
        for l in ls {
            eff(&l.content, &l.name, out);
            if let Some(m) = &l.mask {
                eff(&m.effects, &l.name, out);
            }
            walk(&l.children, out);
        }
    }
    walk(layers, &mut out);
    out
}

/// What the properties tab shows for an effect.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelKind {
    Fill,
    Generator,
    Filter,
}

fn projection_label(p: Projection) -> &'static str {
    match p {
        Projection::Uv => "UV projection",
        Projection::Planar => "Planar projection",
        Projection::Triplanar => "Tri-planar projection",
        Projection::Spherical => "Spherical projection",
        Projection::Cylindrical => "Cylindrical projection",
        Projection::UvSetToUvSet => "UV set to UV set",
        Projection::Fill => "Fill (mesh data)",
        Projection::Warp => "Warp",
        Projection::Unknown(_) => "Unknown projection",
    }
}

/// "FILL" section of Painter's properties: projection, filtering, wrap, UV transformations, symmetry note.
fn draw_fill_section(ui: &mut egui::Ui, f: &mut Fill) -> bool {
    let mut changed = false;
    w::form_title(ui, "FILL", true, false);
    // evaluated: UV projection only; the others are shown as stored
    w::form_combo(ui, "fill_proj", "Projection", projection_label(f.projection), &[projection_label(f.projection)], false);
    let filt = match f.filtering {
        0 => "Nearest",
        1 => "Bilinear",
        _ => "Bilinear | HQ",
    };
    if let Some(k) = w::form_combo(ui, "fill_filter", "Filtering", filt, &["Nearest", "Bilinear", "Bilinear | HQ"], true) {
        f.filtering = k as i32;
        changed = true;
    }
    let wrap = match f.uv.wrap {
        0 => "Clamp to edge",
        1 => "Clamp to border",
        2 => "Mirror repeat",
        _ => "Repeat",
    };
    if let Some(k) = w::form_combo(ui, "fill_wrap", "UV Wrap", wrap, &["Clamp to edge", "Clamp to border", "Mirror repeat", "Repeat"], true) {
        f.uv.wrap = k as i32;
        changed = true;
    }
    ui.add_space(4.0);
    if w::form_sub(ui, "uv_transform", "UV transformations", true) {
        w::form_indent(ui, |ui| {
            w::form_combo(
                ui,
                "fill_scale_mode",
                "Scale",
                if f.uv.scale_mode == 0 { "Tiling" } else { "Physical size" },
                &["Tiling"],
                false,
            );
            let id = ui.id().with("tiling_lock");
            let mut lock = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, true));
            let mut sc = f.uv.scale.unwrap_or([1.0, 1.0]);
            let (mut x, mut y) = (sc[0], sc[1]);
            if w::form_pair(ui, "Tiling", &mut x, &mut y, 0.0..=20.0, 2, Some(&mut lock)) {
                sc = [x, y];
                f.uv.scale = Some(sc);
                changed = true;
            }
            ui.ctx().data_mut(|d| d.insert_persisted(id, lock));
            // stored in radians, shown in degrees like Painter's panel
            let mut rot = f.uv.rotation.unwrap_or(0.0).to_degrees().rem_euclid(360.0);
            if w::form_slider(ui, "Rotation", &mut rot, 0.0..=360.0, 0) {
                f.uv.rotation = Some(rot.to_radians());
                changed = true;
            }
            let off = f.uv.position.unwrap_or([0.0, 0.0]);
            let (mut x, mut y) = (off[0], off[1]);
            if w::form_pair(ui, "Offset", &mut x, &mut y, -1.0..=1.0, 2, None) {
                f.uv.position = Some([x, y]);
                changed = true;
            }
        });
    }
    w::form_title(ui, "SYMMETRY", false, false);
    let note = if matches!(f.projection, Projection::Uv) {
        "Fill layer symmetry options are not available with the current projection method: UV projection"
    } else {
        "Symmetry is not evaluated yet."
    };
    ui.horizontal(|ui| {
        ui.add_space(w::FORM_PAD);
        ui.add(egui::Label::new(RichText::new(note).color(theme::TEXT_DIM).size(11.0)).wrap());
    });
    changed
}

/// Resource slot like Painter's: thumbnail, kind on top, name below, remove button. Returns clicked.
fn resource_slot(ui: &mut egui::Ui, kind: &str, name: &str, thumb: Option<&egui::TextureHandle>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, 44.0), Sense::click());
    let rect = rect.shrink2(vec2(w::FORM_PAD, 0.0));
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if resp.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET });
    p.rect_stroke(rect, 4.0, Stroke::new(1.0, if resp.hovered() { theme::accent() } else { theme::BORDER }), egui::StrokeKind::Inside);
    let th = Rect::from_center_size(pos2(rect.min.x + 24.0, rect.center().y), vec2(30.0, 30.0));
    match thumb {
        Some(t) => {
            p.image(t.id(), th, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.circle_filled(th.center(), 14.0, Color32::from_gray(70));
            p.circle_stroke(th.center(), 14.0, Stroke::new(1.0, theme::TEXT_DISABLED));
        }
    }
    p.text(pos2(rect.min.x + 48.0, rect.center().y - 8.0), Align2::LEFT_CENTER, kind, FontId::proportional(12.0), Color32::WHITE);
    p.text(pos2(rect.min.x + 48.0, rect.center().y + 9.0), Align2::LEFT_CENTER, name, FontId::proportional(11.0), theme::TEXT_DIM);
    crate::icons::draw(p, Rect::from_center_size(pos2(rect.max.x - 14.0, rect.center().y), vec2(14.0, 14.0)), Icon::Close, theme::TEXT_DIM);
    resp.on_hover_text("Click to pick another resource or an anchor point").clicked()
}

/// The sources of a fill: GRAYSCALE for a mask fill, MATERIAL (one entry per channel) for layer content.
fn draw_fill_sources(
    ui: &mut egui::Ui,
    f: &mut Fill,
    in_mask: bool,
    iface: Option<&shard_formats::sbsar::Interface>,
    thumbs: &std::collections::HashMap<String, egui::TextureHandle>,
    ctx_res: (u32, u32),
    open_picker: &mut Option<usize>,
) -> bool {
    let mut changed = false;
    w::form_title(ui, if in_mask { "GRAYSCALE" } else { "MATERIAL" }, false, true);
    ui.add_space(4.0);
    let chan_name = |channels: &shard_model::ChannelSet| -> (u32, String) {
        let bit = (0..64u32).find(|b| channels.contains(*b)).unwrap_or(0);
        (bit, shard_formats::enums::channel_name(bit).unwrap_or_else(|| format!("channel {bit}")))
    };
    for (k, src) in f.sources.iter_mut().enumerate() {
        match src {
            Source::Uniform { channels, color, .. } => {
                let (bit, name) = chan_name(channels);
                let c = color.unwrap_or([0.0; 3]);
                if in_mask {
                    if resource_slot(ui, "grayscale", "Uniform color", None) {
                        *open_picker = Some(k);
                    }
                    ui.add_space(4.0);
                }
                w::form_row(ui, &name, |ui| {
                    if matches!(bit, 0 | 4) {
                        let mut srgb = c.map(shard_compose::linear_to_srgb);
                        if ui.color_edit_button_rgb(&mut srgb).changed() {
                            *color = Some(srgb.map(shard_compose::srgb_to_linear));
                            changed = true;
                        }
                    } else if bit == 22 {
                        // normal: raw vector colour, no gamma
                        let mut raw = c;
                        if ui.color_edit_button_rgb(&mut raw).changed() {
                            *color = Some(raw);
                            changed = true;
                        }
                    } else {
                        let mut v = c[0];
                        if ui.add(egui::DragValue::new(&mut v).range(0.0..=1.0).speed(0.005).max_decimals(3)).changed() {
                            *color = Some([v; 3]);
                            changed = true;
                        }
                    }
                    ui.label(RichText::new("Uniform").color(theme::TEXT_DIM).size(10.5));
                });
            }
            Source::Bitmap { channels, resource, .. } => {
                let (_, name) = chan_name(channels);
                let r = resource
                    .as_deref()
                    .map(|r| r.trim_start_matches("file:").split('?').next().unwrap_or(r).rsplit(['/', '\\']).next().unwrap_or(r).to_string());
                let th = resource.as_deref().and_then(|r| thumbs.get(r));
                if resource_slot(ui, if in_mask { "grayscale" } else { name.as_str() }, &format!("Texture: {}", r.unwrap_or_else(|| "none".into())), th) {
                    *open_picker = Some(k);
                }
            }
            Source::Reference { channels, target, .. } => {
                let (_, name) = chan_name(channels);
                if resource_slot(ui, if in_mask { "grayscale" } else { name.as_str() }, &format!("Anchor point #{target}"), None) {
                    *open_picker = Some(k);
                }
            }
            Source::Procedural { channels, procedural, .. } => {
                let (_, chan) = chan_name(channels);
                let title = iface
                    .map(|i| i.label.clone())
                    .or_else(|| procedural.graph_name().map(str::to_string))
                    .unwrap_or_else(|| "procedural".into());
                // resource slot like Painter's (click: pick another resource or an anchor point)
                let th = procedural.resource.as_deref().and_then(|r| thumbs.get(r));
                if resource_slot(ui, if in_mask { "grayscale" } else { chan.as_str() }, &title, th) {
                    *open_picker = Some(k);
                }
                ui.add_space(6.0);
                // Attributes (collapsed), Color Spaces and Resolution, like Painter's FILL panel
                if w::form_sub(ui, &format!("src_{k}_attr"), "Attributes", false) {
                    w::form_indent(ui, |ui| {
                        ui.label(RichText::new("No attributes for this resource.").color(theme::TEXT_DIM).size(10.5));
                    });
                }
                if w::form_sub(ui, &format!("src_{k}_cs"), "Color Spaces", true) {
                    w::form_indent(ui, |ui| {
                        const SPACES: [&str; 4] = ["auto (data)", "raw", "data", "data - signed normalized"];
                        ui.label(RichText::new("Output color spaces").color(theme::TEXT_DIM).size(11.0));
                        let ids: Vec<String> = if procedural.outputs.is_empty() {
                            vec!["output".to_string()]
                        } else {
                            procedural.outputs.iter().map(|o| o.id.clone()).collect()
                        };
                        for (oi, oid) in ids.iter().enumerate() {
                            let cur = procedural.outputs.get(oi).map_or(0, |o| o.color_space).clamp(0, 3) as usize;
                            if let Some(pick) = w::form_combo(ui, &format!("src_{k}_cs_{oi}"), oid, SPACES[cur], &SPACES, true) {
                                if let Some(o) = procedural.outputs.get_mut(oi) {
                                    o.color_space = pick as i32;
                                }
                            }
                        }
                    });
                }
                if w::form_sub(ui, &format!("src_{k}_res"), "Resolution", true) {
                    w::form_indent(ui, |ui| {
                        const MODES: [&str; 4] = ["Auto", "Texture Set", "UV Tile", "Custom"];
                        const FACTORS: [i32; 7] = [0, -3, -2, -1, 1, 2, 3];
                        let r = &mut procedural.resolution;
                        r.mode = r.mode.clamp(0, 3);
                        if let Some(i) = w::form_combo(ui, &format!("src_{k}_res_mode"), "Resolution", MODES[r.mode as usize], &MODES, true) {
                            r.mode = i as i32;
                            changed = true;
                        }
                        let custom = r.mode == 3;
                        let same = format!("Same as context ({})", if r.mode == 2 { "UV Tile" } else { "Texture Set" });
                        let names = [same.as_str(), "1/8", "1/4", "1/2", "x2", "x4", "x8"];
                        let cur = FACTORS.iter().position(|f| *f == r.log2_offset).unwrap_or(0);
                        if let Some(i) = w::form_combo(ui, &format!("src_{k}_res_factor"), "Factor", names[cur], &names, !custom) {
                            r.log2_offset = FACTORS[i];
                            changed = true;
                        }
                        if custom {
                            w::form_row(ui, "Size", |ui| {
                                let (mut cw, mut ch) = (r.manual[0], r.manual[1]);
                                let a = ui.add(egui::DragValue::new(&mut cw).range(16..=8192).speed(8.0));
                                ui.label("x");
                                let b = ui.add(egui::DragValue::new(&mut ch).range(16..=8192).speed(8.0));
                                if a.changed() || b.changed() {
                                    r.manual = [cw, ch];
                                    changed = true;
                                }
                            });
                        }
                        let (ow, oh) = r.target(ctx_res.0, ctx_res.1);
                        w::form_row(ui, "Output size", |ui| {
                            ui.label(RichText::new(format!("{ow}x{oh}")).color(theme::TEXT_DIM));
                        });
                    });
                }
                if w::form_sub(ui, &format!("src_{k}_params"), "Parameters", true) {
                    w::form_indent(ui, |ui| {
                        // seed: stored as `$randomseed`; 0 = "Random"
                        let seed = match procedural.tweaks.get("$randomseed") {
                            Some(shard_model::Tweak::Int(i)) => *i,
                            _ => 0,
                        };
                        let edit_id = ui.id().with(("seed_edit", k));
                        let mut editing = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(edit_id, false));
                        w::form_row(ui, "Seed", |ui| {
                            let wd = ui.available_width() - 34.0;
                            if editing {
                                let mut sv = seed;
                                if ui.add_sized([wd, 22.0], egui::DragValue::new(&mut sv).speed(1.0)).changed() {
                                    procedural.tweaks.insert("$randomseed".into(), shard_model::Tweak::Int(sv));
                                    changed = true;
                                }
                            } else {
                                let text = if seed == 0 { "Random".to_string() } else { format!("{seed}") };
                                if ui.add_sized([wd, 22.0], egui::Button::new(text)).clicked() {
                                    let t = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.subsec_nanos())
                                        .unwrap_or(1);
                                    procedural
                                        .tweaks
                                        .insert("$randomseed".into(), shard_model::Tweak::Int((t % 100_000) as i32 + 1));
                                    changed = true;
                                }
                            }
                            if w::icon_button(ui, Icon::Brush, 22.0, editing, true).on_hover_text("Type a seed").clicked() {
                                editing = !editing;
                            }
                        });
                        ui.ctx().data_mut(|d| d.insert_persisted(edit_id, editing));
                        if let Some(i) = iface {
                            changed |= draw_interface_params(ui, i, procedural);
                        }
                    });
                }
            }
            Source::Other(_) => {}
        }
    }
    changed
}

/// Parameters of a graph interface (groups, sliders, toggles, combos, colours, image pins) edited into
/// `proc_`'s tweaks, plus "Restore defaults". Returns true when something changed.
fn draw_interface_params(ui: &mut egui::Ui, iface: &shard_formats::sbsar::Interface, proc_: &mut shard_model::Procedural) -> bool {
    use shard_formats::sbsar::{DefaultValue, Widget};
    use shard_model::Tweak;
    let mut changed = false;
    // current numeric value of an input: tweak, else interface default
    let value_of = |proc_: &shard_model::Procedural, id: &str| -> f32 {
        match proc_.tweaks.get(id) {
            Some(Tweak::Float(f)) => *f,
            Some(Tweak::Int(i)) => *i as f32,
            Some(Tweak::Bool(b)) => *b as i32 as f32,
            _ => iface.values.iter().find(|(k, _)| k == id).and_then(|(_, v)| v.first().copied()).unwrap_or(0.0),
        }
    };
    let visible = |proc_: &shard_model::Procedural, expr: &Option<String>| -> bool {
        let Some(e) = expr else { return true };
        // `input["x"]`, `!input["x"]`, `input["x"]==2`, joined by && / ||
        let term = |t: &str| -> bool {
            let t = t.trim();
            let (neg, t) = match t.strip_prefix('!') {
                Some(r) => (true, r.trim()),
                None => (false, t),
            };
            let Some(a) = t.find("input[\"") else { return true };
            let rest = &t[a + 7..];
            let Some(b) = rest.find('"') else { return true };
            let id = &rest[..b];
            let tail = rest[b + 1..].trim_start_matches(']').trim();
            let v = value_of(proc_, id);
            let r = if let Some(n) = tail.strip_prefix("==") {
                n.trim().parse::<f32>().map_or(true, |n| (v - n).abs() < 1e-4)
            } else if let Some(n) = tail.strip_prefix("!=") {
                n.trim().parse::<f32>().map_or(true, |n| (v - n).abs() >= 1e-4)
            } else {
                v != 0.0
            };
            r != neg
        };
        e.split("||").any(|or| or.split("&&").all(term))
    };
    let mut groups: Vec<(String, String)> = vec![(String::new(), String::new())];
    groups.extend(iface.groups.iter().cloned());
    for (gid, glabel) in &groups {
        let params: Vec<&shard_formats::sbsar::Param> = iface
            .params
            .iter()
            .filter(|p| !matches!(p.widget, Widget::Image))
            .filter(|p| p.group.clone().unwrap_or_default() == *gid)
            .filter(|p| visible(proc_, &p.visible_if))
            .collect();
        if params.is_empty() {
            continue;
        }
        if !gid.is_empty() {
            ui.add_space(4.0);
            w::form_title(ui, &glabel.to_uppercase(), false, false);
        }
        for prm in params {
            let id = prm.identifier.as_str();
            match &prm.widget {
                Widget::Slider { min, max, step, .. } => {
                    let dec = if *step >= 1.0 { 0 } else if *step >= 0.1 { 1 } else { 2 };
                    let n = prm.comps.clamp(1, 4);
                    let is_int = matches!(prm.ty, 4 | 8);
                    let mut vals: Vec<f32> = match (proc_.tweaks.get(id), &prm.default) {
                        (Some(Tweak::Float(f)), _) => vec![*f],
                        (Some(Tweak::Int(i)), _) => vec![*i as f32],
                        (Some(Tweak::Float2(v)), _) => v.to_vec(),
                        (Some(Tweak::Float3(v)), _) => v.to_vec(),
                        (Some(Tweak::Float4(v)), _) => v.to_vec(),
                        (Some(Tweak::Int2(v)), _) => v.iter().map(|x| *x as f32).collect(),
                        (_, DefaultValue::Float(v)) => v.clone(),
                        (_, DefaultValue::Int(v)) => v.iter().map(|x| *x as f32).collect(),
                        _ => vec![0.0],
                    };
                    vals.resize(n, 0.0);
                    let mut any = false;
                    if n == 2 {
                        let (mut x, mut y) = (vals[0], vals[1]);
                        any |= w::form_pair(ui, &prm.label, &mut x, &mut y, *min..=*max, dec, None);
                        vals[0] = x;
                        vals[1] = y;
                    } else {
                        for (k, v) in vals.iter_mut().enumerate() {
                            let lab = if n > 1 { format!("{} {}", prm.label, ["X", "Y", "Z", "W"][k]) } else { prm.label.clone() };
                            any |= w::form_slider(ui, &lab, v, *min..=*max, dec);
                        }
                    }
                    if any {
                        let t = match (n, is_int) {
                            (1, true) => Tweak::Int(vals[0].round() as i32),
                            (1, false) => Tweak::Float(vals[0]),
                            (2, true) => Tweak::Int2([vals[0].round() as i32, vals[1].round() as i32]),
                            (2, false) => Tweak::Float2([vals[0], vals[1]]),
                            (3, _) => Tweak::Float3([vals[0], vals[1], vals[2]]),
                            _ => Tweak::Float4([vals[0], vals[1], vals[2], vals[3]]),
                        };
                        proc_.tweaks.insert(id.to_string(), t);
                        changed = true;
                    }
                }
                Widget::Toggle => {
                    let on = value_of(proc_, id) != 0.0;
                    if w::form_toggle(ui, &prm.label, on) {
                        proc_.tweaks.insert(id.to_string(), Tweak::Bool(!on));
                        changed = true;
                    }
                }
                Widget::Combo(items) => {
                    let cur = value_of(proc_, id).round() as i32;
                    let names: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
                    let text = items.iter().find(|(v, _)| *v == cur).map(|(_, t)| t.as_str()).unwrap_or("?");
                    if let Some(k) = w::form_combo(ui, &format!("prop_combo_{id}"), &prm.label, text, &names, true) {
                        proc_.tweaks.insert(id.to_string(), Tweak::Int(items[k].0));
                        changed = true;
                    }
                }
                Widget::Color => {
                    let cur: [f32; 3] = match proc_.tweaks.get(id) {
                        Some(Tweak::Float3(v)) => *v,
                        Some(Tweak::Float4(v)) => [v[0], v[1], v[2]],
                        Some(Tweak::Float(f)) => [*f; 3],
                        _ => match &prm.default {
                            DefaultValue::Float(v) if v.len() >= 3 => [v[0], v[1], v[2]],
                            DefaultValue::Float(v) if v.len() == 1 => [v[0]; 3],
                            _ => [0.5; 3],
                        },
                    };
                    // graph colours are sRGB fractions; the compositor reads them as given
                    let mut c = cur;
                    w::form_row(ui, &prm.label, |ui| {
                        if ui.color_edit_button_rgb(&mut c).changed() {
                            let t = if prm.ty == 0 {
                                Tweak::Float(c[0])
                            } else if prm.ty == 3 {
                                Tweak::Float4([c[0], c[1], c[2], 1.0])
                            } else {
                                Tweak::Float3(c)
                            };
                            proc_.tweaks.insert(id.to_string(), t);
                            changed = true;
                        }
                    });
                }
                Widget::Image => {}
            }
        }
    }
    // image inputs (mesh maps, anchors, bitmaps)
    let pins: Vec<&shard_formats::sbsar::Param> = iface.params.iter().filter(|p| matches!(p.widget, Widget::Image)).collect();
    let pins: Vec<&&shard_formats::sbsar::Param> = pins.iter().filter(|p| visible(proc_, &p.visible_if)).collect();
    if !pins.is_empty() {
        w::form_title(ui, "IMAGE INPUTS", false, false);
        for prm in pins {
            let what = match prm.group.as_deref() {
                Some("Mesh Data") => "Mesh map",
                _ => "Not connected",
            };
            w::form_row(ui, &prm.label, |ui| {
                ui.label(RichText::new(what).color(theme::TEXT_DIM).size(10.5));
            });
        }
    }
    ui.add_space(10.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        if ui.add_sized([120.0, 24.0], egui::Button::new("Restore defaults")).clicked() {
            for prm in &iface.params {
                if !matches!(prm.widget, Widget::Image) {
                    proc_.tweaks.remove(&prm.identifier);
                }
            }
            changed = true;
        }
    });
    changed
}

/// Layer at `path` (indices from the root list).
fn layer_at_mut<'a>(layers: &'a mut [Layer], path: &[usize]) -> Option<&'a mut Layer> {
    let (first, rest) = path.split_first()?;
    let l = layers.get_mut(*first)?;
    if rest.is_empty() {
        Some(l)
    } else {
        layer_at_mut(&mut l.children, rest)
    }
}

/// Effect `pos` of the layer's mask (`in_mask`) or content.
fn props_effect_mut<'a>(layers: &'a mut [Layer], path: &[usize], pos: usize, in_mask: bool) -> Option<&'a mut Effect> {
    let l = layer_at_mut(layers, path)?;
    if in_mask {
        l.mask.as_mut()?.effects.get_mut(pos)
    } else {
        l.content.get_mut(pos)
    }
}

/// The procedural graph an effect evaluates (generator fill or filter), if any.
fn effect_procedural_mut(e: &mut Effect) -> Option<&mut shard_model::Procedural> {
    match &mut e.kind {
        EffectKind::Fill(f) => f.sources.iter_mut().find_map(|s| match s {
            Source::Procedural { procedural, .. } => Some(procedural),
            _ => None,
        }),
        EffectKind::Filter { procedural } => procedural.as_mut(),
        _ => None,
    }
}

/// Display name of a mask effect (Painter lists the graph / tool name).
fn effect_label(e: &Effect) -> String {
    if let Some(l) = e.label.as_ref().filter(|l| !l.is_empty()) {
        return l.clone();
    }
    match &e.kind {
        EffectKind::Fill(f) => f
            .sources
            .iter()
            .find_map(|s| match s {
                Source::Procedural { procedural, .. } => procedural.graph_name().map(str::to_string),
                Source::Bitmap { .. } => Some("Bitmap".into()),
                Source::Reference { .. } => Some("Reference".into()),
                Source::Uniform { .. } => Some("Fill".into()),
                _ => None,
            })
            .unwrap_or_else(|| "Fill".into()),
        EffectKind::Paint(a) => format!("Paint ({})", a.strokes.len()),
        EffectKind::Eraser(a) => format!("Eraser ({})", a.strokes.len()),
        EffectKind::Smudge(a) => format!("Smudge ({})", a.strokes.len()),
        EffectKind::Group { children } => format!("Paint group ({})", children.len()),
        EffectKind::Filter { procedural } => {
            procedural.as_ref().and_then(|p| p.graph_name()).unwrap_or("Filter").to_string()
        }
        EffectKind::Levels(_) => "Levels".into(),
        EffectKind::Anchor { name } => name.clone().unwrap_or_else(|| "Anchor point".into()),
        EffectKind::Decal(h) => format!("Polygon fill ({})", h.len()),
        EffectKind::Other(s) => s.clone(),
    }
}

/// Rows listing the content of a mask: top of the list = last applied effect, each with visibility,
/// blend mode, opacity and a remove button (like Painter's mask stack).
fn mask_content_rows(ui: &mut egui::Ui, m: &mut shard_model::Mask, depth: usize, path: &[usize], lc: &mut LayerCtx) {
    const H: f32 = 28.0;
    let n = m.effects.len();
    let mut remove: Option<usize> = None;
    for pos in (0..n).rev() {
        // empty paint groups are placeholders in the file, not something a user sees
        if matches!(&m.effects[pos].kind, EffectKind::Group { children } if children.is_empty()) {
            continue;
        }
        let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::click());
        let p = ui.painter().clone();
        let picked = lc.effect_sel.as_ref().is_some_and(|(pp, k, m)| *m && pp.as_slice() == path && *k == pos);
        p.rect_filled(rect, 0.0, if picked { theme::accent_soft() } else { theme::WELL });
        if row.clicked() {
            *lc.effect_sel = Some((path.to_vec(), pos, true));
        }
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, theme::BORDER));
        let cy = rect.center().y;
        let mut x = rect.min.x + 18.0 + depth as f32 * 14.0;
        let id = ui.id().with(("mfx", path.to_vec(), pos));
        let eye = Rect::from_center_size(pos2(x + 8.0, cy), vec2(18.0, 18.0));
        let vis = m.effects[pos].visible;
        let er = ui.interact(eye, id.with("eye"), Sense::click());
        crate::icons::draw(&p, eye, if vis { Icon::Eye } else { Icon::EyeOff }, if vis { theme::TEXT } else { theme::TEXT_DISABLED });
        if er.clicked() {
            m.effects[pos].visible = !vis;
            lc_changed(lc.changed);
        }
        x += 24.0;
        let icon = match &m.effects[pos].kind {
            EffectKind::Paint(_) | EffectKind::Eraser(_) | EffectKind::Smudge(_) | EffectKind::Group { .. } => Icon::Brush,
            EffectKind::Fill(f) => {
                let generator = f.sources.iter().any(|s| match s {
                    Source::Procedural { procedural, .. } => procedural.graph_name().is_some_and(|n| n.starts_with("mg_")),
                    _ => false,
                });
                if generator { Icon::Wand } else { Icon::Bucket }
            }
            EffectKind::Filter { .. } | EffectKind::Levels(_) => Icon::Wand,
            EffectKind::Decal(_) => Icon::PolyFill,
            _ => Icon::Mask,
        };
        crate::icons::draw(&p, Rect::from_center_size(pos2(x + 7.0, cy), vec2(14.0, 14.0)), icon, Color32::from_gray(200));
        x += 22.0;
        let label = effect_label(&m.effects[pos]);
        let col = if m.effects[pos].visible { Color32::WHITE } else { theme::TEXT_DIM };
        p.text(pos2(x, cy), Align2::LEFT_CENTER, label, FontId::proportional(11.5), col);
        // blend, opacity, remove (right)
        let (mut mode, op) = m.effects[pos].blend.first().map(|b| (b.mode.0, b.opacity)).unwrap_or((2, 1.0));
        let mut opacity = op * 100.0;
        let before = (mode, opacity);
        let right = Rect::from_min_size(pos2(rect.max.x - 150.0, rect.min.y + 2.0), vec2(146.0, H - 4.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)));
        let vis_style = child.visuals_mut();
        for ws in [&mut vis_style.widgets.inactive, &mut vis_style.widgets.hovered, &mut vis_style.widgets.active, &mut vis_style.widgets.open] {
            ws.weak_bg_fill = Color32::TRANSPARENT;
            ws.bg_fill = Color32::TRANSPARENT;
            ws.bg_stroke = Stroke::NONE;
        }
        if child.small_button("x").on_hover_text("Remove from the mask").clicked() {
            remove = Some(pos);
        }
        child.add(egui::DragValue::new(&mut opacity).range(0.0..=100.0).speed(1.0).max_decimals(0));
        egui::ComboBox::from_id_salt(id.with("blend"))
            .width(58.0)
            .selected_text(RichText::new(short_blend(BLEND_NAMES[mode as usize & 31])).size(11.0))
            .show_ui(&mut child, |ui| {
                for (k, nme) in BLEND_NAMES.iter().enumerate() {
                    ui.selectable_value(&mut mode, k as u8, *nme);
                }
            });
        if mode != before.0 || (opacity - before.1).abs() > 0.01 {
            let e = &mut m.effects[pos];
            match e.blend.first_mut() {
                Some(b) => {
                    b.mode = BlendMode(mode);
                    b.opacity = opacity / 100.0;
                }
                None => e.blend.push(ChannelBlend {
                    mode: BlendMode(mode),
                    channels: ChannelSet(u128::MAX),
                    opacity: opacity / 100.0,
                }),
            }
            lc_changed(lc.changed);
        }
    }
    if let Some(pos) = remove {
        m.effects.remove(pos);
        lc_changed(lc.changed);
    }
}

/// Mark the layers panel as changed (SHARD_TRACE logs which control did it).
#[track_caller]
fn lc_changed(flag: &mut bool) {
    if std::env::var_os("SHARD_TRACE").is_some() && !*flag {
        eprintln!("[layers changed] by {}", std::panic::Location::caller());
    }
    *flag = true;
}

fn layer_rows(ui: &mut egui::Ui, layers: &mut [Layer], depth: usize, path: &mut Vec<usize>, lc: &mut LayerCtx) {
    for i in 0..layers.len() {
        path.push(i);
        let is_group = layers[i].kind == LayerKind::Group;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
        let sel = lc.selected.as_ref() == Some(path);
        let p = ui.painter().clone();
        let row_r = theme::RADIUS as f32;
        let row = rect.shrink2(vec2(0.0, 1.0));
        let fill = if sel {
            theme::accent_soft()
        } else if resp.hovered() {
            theme::WIDGET
        } else {
            Color32::TRANSPARENT
        };
        p.rect_filled(row, row_r, fill);
        if sel {
            p.rect_stroke(row, row_r, Stroke::new(1.0, theme::accent()), egui::StrokeKind::Inside);
        }
        // drag and drop: a drop on the top / bottom quarter goes before / after the row, on the middle of a group inside
        resp.dnd_set_drag_payload(path.clone());
        let place = |ptr_y: f32| -> (Vec<usize>, usize, u8) {
            let rel = ((ptr_y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
            let parent: Vec<usize> = path[..path.len() - 1].to_vec();
            let me = *path.last().unwrap_or(&0);
            if is_group && rel > 0.25 && rel < 0.75 {
                (path.clone(), 0, 2)
            } else if rel < 0.5 {
                (parent, me, 0)
            } else {
                (parent, me + 1, 1)
            }
        };
        if let Some(from) = resp.dnd_hover_payload::<Vec<usize>>() {
            if *from != *path {
                if let Some(ptr) = ui.input(|i| i.pointer.hover_pos()) {
                    let (_, _, kind) = place(ptr.y);
                    let c = theme::accent();
                    match kind {
                        0 => {
                            p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(2.0, c));
                        }
                        1 => {
                            p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(2.0, c));
                        }
                        _ => {
                            p.rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0, c), egui::StrokeKind::Inside);
                        }
                    }
                }
            }
        }
        if let Some(from) = resp.dnd_release_payload::<Vec<usize>>() {
            if *from != *path {
                if let Some(ptr) = ui.input(|i| i.pointer.latest_pos()) {
                    let (parent, index, _) = place(ptr.y);
                    *lc.drop = Some(((*from).clone(), parent, index));
                }
            }
        }
        if resp.secondary_clicked() {
            *lc.selected = Some(path.clone());
        }
        resp.context_menu(|ui| {
            let has_mask = layers[i].mask.is_some();
            let mut act: Option<LayerAction> = None;
            if ui.button("Duplicate   Ctrl+D").clicked() {
                act = Some(LayerAction::Duplicate);
            }
            if ui.button("Group   Ctrl+G").clicked() {
                act = Some(LayerAction::Group);
            }
            if is_group && ui.button("Ungroup").clicked() {
                act = Some(LayerAction::Ungroup);
            }
            if ui.button("Rename").clicked() {
                *lc.renaming = Some((path.clone(), layers[i].name.clone()));
                ui.close_menu();
            }
            ui.separator();
            if !has_mask {
                if ui.button("Add white mask").clicked() {
                    act = Some(LayerAction::AddMask(true));
                }
                if ui.button("Add black mask").clicked() {
                    act = Some(LayerAction::AddMask(false));
                }
            } else {
                let on = layers[i].mask.as_ref().map(|m| m.enabled).unwrap_or(true);
                if ui.button(if on { "Disable mask" } else { "Enable mask" }).clicked() {
                    act = Some(LayerAction::ToggleMask);
                }
                if ui.button("Remove mask").clicked() {
                    act = Some(LayerAction::RemoveMask);
                }
            }
            ui.separator();
            if ui.button("Copy   Ctrl+C").clicked() {
                act = Some(LayerAction::Copy);
            }
            if ui.button("Cut   Ctrl+X").clicked() {
                act = Some(LayerAction::Cut);
            }
            if ui.button("Paste   Ctrl+V").clicked() {
                act = Some(LayerAction::Paste);
            }
            ui.separator();
            if ui.button("Delete   Del").clicked() {
                act = Some(LayerAction::Delete);
            }
            if let Some(a) = act {
                *lc.action = Some((a, path.clone()));
                ui.close_menu();
            }
        });
        if resp.double_clicked() {
            *lc.renaming = Some((path.clone(), layers[i].name.clone()));
        }
        if resp.clicked() {
            *lc.selected = Some(path.clone());
            // a fill layer with a procedural / a filter layer shows its parameters
            let first = layers[i].content.iter().position(|e| match &e.kind {
                EffectKind::Fill(_) => true,
                EffectKind::Filter { procedural } => procedural.is_some(),
                _ => false,
            });
            *lc.effect_sel = first.map(|k| (path.clone(), k, false));
        }
        let indent = depth as f32 * 14.0;
        let mut x = rect.min.x + 6.0 + indent;
        let cy = rect.center().y;
        // eye
        let eye = Rect::from_center_size(pos2(x + 8.0, cy), vec2(18.0, 18.0));
        let er = ui.interact(eye, ui.id().with(("eye", path.clone())), Sense::click());
        let vis = layers[i].visible;
        crate::icons::draw(&p, eye, if vis { Icon::Eye } else { Icon::EyeOff }, if vis { Color32::WHITE } else { theme::TEXT_DIM });
        if er.clicked() {
            layers[i].visible = !vis;
            lc_changed(lc.changed);
        }
        x += 22.0;
        // group chevron
        if is_group {
            let cr = Rect::from_center_size(pos2(x + 6.0, cy), vec2(14.0, 14.0));
            let cresp = ui.interact(cr, ui.id().with(("chev", path.clone())), Sense::click());
            let closed = lc.collapsed.contains(path);
            crate::icons::draw(&p, cr, if closed { Icon::ChevronRight } else { Icon::ChevronDown }, theme::TEXT_DIM);
            if cresp.clicked() {
                if closed {
                    lc.collapsed.remove(path);
                } else {
                    lc.collapsed.insert(path.clone());
                }
            }
            x += 14.0;
        }
        // thumbnail: what the layer writes on the shown channel (a picture), else its flat colour / an icon
        let th = Rect::from_min_size(pos2(x, cy - 19.0), vec2(38.0, 38.0));
        let round = egui::CornerRadius::same(6);
        let picture = |tex: &egui::TextureHandle, r: Rect, ui: &egui::Ui| {
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), r.size())).corner_radius(round).paint_at(ui, r);
        };
        match (is_group, lc.thumbs.get(&(lc.set, layers[i].uid, false)), thumb_color(&layers[i])) {
            (true, _, _) => {
                p.rect_filled(th, 6.0, theme::WIDGET);
                crate::icons::draw(&p, th.shrink(8.0), Icon::Folder, theme::TEXT);
            }
            (false, Some(tex), _) => picture(tex, th, ui),
            (false, None, Some(c)) => {
                p.rect_filled(th, 6.0, c);
            }
            (false, None, None) => {
                p.rect_filled(th, 6.0, theme::WIDGET);
                crate::icons::draw(&p, th.shrink(8.0), Icon::Brush, theme::TEXT_DIM);
            }
        }
        p.rect_stroke(th, 6.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        x += 42.0;
        if layers[i].mask.as_ref().map(|m| m.explicit || (m.enabled && !m.effects.is_empty())).unwrap_or(false) {
            let m = Rect::from_min_size(pos2(x, cy - 16.0), vec2(32.0, 32.0));
            let enabled = layers[i].mask.as_ref().map(|m| m.enabled).unwrap_or(true);
            match lc.thumbs.get(&(lc.set, layers[i].uid, true)) {
                Some(tex) if enabled => picture(tex, m, ui),
                _ => {
                    let swatch = match layers[i].mask.as_ref() {
                        Some(mk) if mk.effects.is_empty() && mk.initial == 0 => Color32::from_gray(20),
                        _ => Color32::from_gray(210),
                    };
                    p.rect_filled(m, 6.0, if enabled { swatch } else { Color32::from_gray(60) });
                }
            }
            p.rect_stroke(m, 6.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            // Painter: Alt+click on a mask thumbnail previews the mask in the viewport
            let mr = ui.interact(m, ui.id().with(("maskthumb", path.clone())), Sense::click());
            let mresp = mr.on_hover_text("Click: show what the mask is made of. Alt+click: edit it in the viewport");
            if mresp.clicked() {
                if ui.input(|i| i.modifiers.alt) {
                    *lc.mask_click = Some(path.clone());
                } else {
                    *lc.mask_open = if lc.mask_open.as_ref() == Some(path) { None } else { Some(path.clone()) };
                }
            }
            if lc.mask_open.as_ref() == Some(path) {
                p.rect_stroke(m.expand(1.5), 7.0, Stroke::new(1.5, theme::accent()), egui::StrokeKind::Outside);
            }
            x += 36.0;
        }
        let name = if layers[i].name.is_empty() { "(unnamed)" } else { layers[i].name.as_str() };
        let name_col = if layers[i].visible { theme::TEXT } else { theme::TEXT_DISABLED };
        let renaming_here = lc.renaming.as_ref().map(|(rp, _)| rp == path).unwrap_or(false);
        if renaming_here {
            let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2((rect.max.x - 90.0 - x).max(60.0), 20.0));
            let mut commit = false;
            let mut cancel = false;
            if let Some((_, text)) = lc.renaming.as_mut() {
                let te = ui.put(r, egui::TextEdit::singleline(text).font(FontId::proportional(11.5)));
                if !te.has_focus() && !te.lost_focus() {
                    te.request_focus();
                }
                if te.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    } else {
                        commit = true;
                    }
                }
            }
            if commit {
                if let Some((_, text)) = lc.renaming.take() {
                    if text != layers[i].name {
                        layers[i].name = text;
                        lc_changed(lc.changed);
                    }
                }
            } else if cancel {
                *lc.renaming = None;
            }
        } else {
            // a name that does not fit ends in an ellipsis and shows in full on hover
            let room = (rect.max.x - 84.0 - x - 2.0).max(24.0);
            let mut job = egui::text::LayoutJob::single_section(
                name.to_string(),
                egui::TextFormat::simple(FontId::proportional(12.0), name_col),
            );
            job.wrap = egui::text::TextWrapping {
                max_width: room,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let galley = p.layout_job(job);
            let shortened = galley.text() != name;
            let at = pos2(x + 2.0, cy - galley.size().y * 0.5);
            p.galley(at, galley, name_col);
            if shortened {
                resp.clone().on_hover_text(name);
            }
        }

        // blend + opacity (stacked, right)
        let (mut mode, op) = blend_of(&layers[i], lc.ch);
        let mut opacity = op * 100.0;
        let before = (mode, opacity);
        let right = Rect::from_min_size(pos2(rect.max.x - 78.0, rect.min.y + 4.0), vec2(74.0, ROW_H - 8.0));
        {
            let mut child = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::top_down(Align::Max)));
            child.spacing_mut().item_spacing.y = 0.0;
            // compact controls: two of them have to fit the row (the theme's 24 px height overflowed it)
            child.spacing_mut().interact_size.y = 18.0;
            child.spacing_mut().button_padding.y = 1.0;
            let vis = child.visuals_mut();
            for ws in [&mut vis.widgets.inactive, &mut vis.widgets.hovered, &mut vis.widgets.active, &mut vis.widgets.open] {
                ws.weak_bg_fill = Color32::TRANSPARENT;
                ws.bg_fill = Color32::TRANSPARENT;
                ws.bg_stroke = Stroke::NONE;
            }
            egui::ComboBox::from_id_salt(("blend", path.clone()))
                .width(60.0)
                .selected_text(RichText::new(short_blend(BLEND_NAMES[mode as usize & 31])).size(11.0))
                .show_ui(&mut child, |ui| {
                    for (m, n) in BLEND_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut mode, m as u8, *n);
                    }
                });
            child.add(egui::DragValue::new(&mut opacity).range(0.0..=100.0).speed(1.0).max_decimals(0));
        }
        // a real change only: the opacity field clamps a stored 100.00001 % to 100 just by being drawn, which read
        // as an edit dropped Painter's cached picture and recomposed the set (at 4K) right after opening
        if mode != before.0 || (opacity - before.1).abs() > 0.01 {
            set_blend(&mut layers[i], lc.ch, mode, opacity / 100.0);
            lc_changed(lc.changed);
        }

        if lc.mask_open.as_ref() == Some(path) {
            if let Some(m) = layers[i].mask.as_mut() {
                mask_content_rows(ui, m, depth, path, lc);
            }
        }

        if is_group && !lc.collapsed.contains(path) {
            let mut kids = std::mem::take(&mut layers[i].children);
            layer_rows(ui, &mut kids, depth + 1, path, lc);
            layers[i].children = kids;
        }
        path.pop();
    }
}

// ---------------------------------------------------------------- settings window building blocks

/// A titled card of the settings window.
fn settings_card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_gray(33))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(theme::semibold(13.0)).color(Color32::WHITE));
            ui.add_space(8.0);
            add(ui);
        });
    ui.add_space(12.0);
}

/// One setting: name and description on the left, its control right-aligned.
fn settings_row(ui: &mut egui::Ui, label: &str, desc: &str, control: impl FnOnce(&mut egui::Ui)) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        let text_w = (w - 250.0).max(180.0);
        ui.vertical(|ui| {
            ui.set_width(text_w);
            ui.label(RichText::new(label).size(12.5).color(theme::TEXT));
            if !desc.is_empty() {
                ui.add(egui::Label::new(RichText::new(desc).size(11.0).color(theme::TEXT_DIM)).wrap());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

fn settings_divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme::BORDER);
    ui.add_space(4.0);
}

/// Small print under a card.
fn settings_note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(-4.0);
    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(theme::TEXT_DIM)).wrap());
    ui.add_space(12.0);
}

/// The shortcuts that work today (Substance 3D Painter's keys: ShardStroke uses no keys of its own).
fn settings_shortcuts(ui: &mut egui::Ui) {
    const GROUPS: [(&str, &[(&str, &str)]); 4] = [
        (
            "Edit",
            &[
                ("Ctrl+Z", "Undo"),
                ("Ctrl+Y", "Redo"),
                ("Ctrl+S", "Save"),
                ("Ctrl+Shift+S", "Save as"),
            ],
        ),
        (
            "Layers",
            &[
                ("Ctrl+D", "Duplicate"),
                ("Ctrl+G", "Group"),
                ("Del", "Delete"),
                ("Ctrl+C / Ctrl+X / Ctrl+V", "Copy / cut / paste"),
            ],
        ),
        ("Painting", &[("1", "Paint"), ("2", "Eraser"), ("[  ]", "Brush size")]),
        (
            "Viewport",
            &[
                ("Alt + LMB", "Orbit"),
                ("Alt + RMB / wheel", "Zoom"),
                ("Alt + MMB", "Pan"),
                ("Shift + RMB", "Rotate the environment"),
                ("C / Shift+C", "Next / previous channel"),
                ("B / Shift+B", "Next / previous mesh map"),
                ("M", "Material"),
                ("Alt+Q", "Isolate the texture set"),
            ],
        ),
    ];
    for (title, keys) in GROUPS {
        settings_card(ui, title, |ui| {
            for (k, (key, action)) in keys.iter().enumerate() {
                if k > 0 {
                    settings_divider(ui);
                }
                settings_row(ui, action, "", |ui| {
                    for part in key.split(" / ").collect::<Vec<_>>().into_iter().rev() {
                        let galley = ui.painter().layout_no_wrap(part.to_string(), theme::semibold(11.0), theme::TEXT);
                        let (r, _) = ui.allocate_exact_size(galley.size() + vec2(14.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(r, 5.0, theme::WIDGET);
                        ui.painter().rect_stroke(r, 5.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
                        ui.painter().galley(r.center() - galley.size() * 0.5, galley, theme::TEXT);
                    }
                });
            }
        });
    }
    settings_note(ui, "These are Substance 3D Painter's shortcuts. Changing them is not available yet.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use shard_model::{PaintAction, Stroke, StrokeKey};

    fn painted_layer(strokes: usize) -> Layer {
        let stroke = Stroke {
            keys: (0..64).map(|i| StrokeKey { pos: [i as f32 / 64.0, 0.5], time: i as f32, kind: 0 }).collect(),
            view: [0.0; 16],
            projection: [0.0; 16],
            seed: 1,
            view_type: 0,
            lazy_distance: 0.0,
            pressure: Vec::new(),
            duplicate_count: 0,
            duplicate_matrix: None,
        };
        let mut l = new_fill_layer("Painted", [0.2, 0.3, 0.4]);
        l.content.push(Effect {
            uid: shard_model::next_uid(),
            visible: true,
            channels: ChannelSet(1),
            label: None,
            blend: vec![],
            kind: EffectKind::Paint(PaintAction { strokes: vec![stroke; strokes].into(), ..Default::default() }),
        });
        l
    }

    fn state_with(layers: Vec<Layer>) -> (AppState, usize) {
        let mut st = AppState::new();
        st.sync_stacks();
        let a = st.active();
        st.stacks[a].layers = layers.clone();
        st.stacks[a].committed = layers;
        (st, a)
    }

    /// Drive `draw` with synthetic pointer input at 60 fps: press on `from`, move to `to`, hover there for a
    /// moment (egui_dock locks its drop target by time) and release.
    fn drag(st: &mut AppState, ctx: &egui::Context, from: egui::Pos2, to: egui::Pos2) {
        let mut time = 10.0f64;
        let mut frame = |st: &mut AppState, events: Vec<egui::Event>| {
            time += 1.0 / 60.0;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0))),
                time: Some(time),
                predicted_dt: 1.0 / 60.0,
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |c| st.draw(c));
        };
        let btn = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(st, vec![egui::Event::PointerMoved(from)]);
        frame(st, vec![btn(from, true)]);
        for i in 1..=24 {
            let t = i as f32 / 24.0;
            frame(st, vec![egui::Event::PointerMoved(from + (to - from) * t)]);
        }
        for _ in 0..20 {
            frame(st, vec![]);
        }
        frame(st, vec![btn(to, false)]);
        for _ in 0..4 {
            frame(st, vec![]);
        }
    }

    fn leaf_rect(st: &AppState, pane: Pane) -> Rect {
        st.dock
            .iter_all_nodes()
            .find(|(_, n)| n.tabs().is_some_and(|t| t.contains(&pane)))
            .and_then(|(_, n)| n.rect())
            .expect("pane is docked")
    }

    /// Where to press to grab the tab of `pane` (tabs sit side by side at the left of the bar).
    fn tab_grab(st: &AppState, pane: Pane) -> egui::Pos2 {
        let r = leaf_rect(st, pane);
        let dx = match pane {
            Pane::Assets => 190.0,
            Pane::TextureSetSettings => 150.0,
            _ => 24.0,
        };
        pos2(r.min.x + dx, r.min.y + 12.0)
    }

    fn settled(ctx: &egui::Context) -> AppState {
        let mut st = AppState::new();
        st.sync_stacks();
        st.dock = dock::default_layout();
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0))),
                ..Default::default()
            };
            let _ = ctx.run(raw, |c| st.draw(c));
        }
        st
    }

    /// Every window docks into another window's tab bar and floats when it is dropped over the body of one;
    /// the free area (where the fixed viewport shows) is never lost.
    #[test]
    fn every_window_can_be_docked_and_floated_and_the_free_area_stays() {
        for moved in Pane::DEFAULT {
            let target = if moved == Pane::Assets { Pane::Layers } else { Pane::Assets };
            // dock: drop on the tab bar of the other window
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut st = settled(&ctx);
            let to_leaf = leaf_rect(&st, target);
            let from = tab_grab(&st, moved);
            drag(&mut st, &ctx, from, pos2(to_leaf.min.x + 150.0, to_leaf.min.y + 12.0));
            assert_eq!(st.dock.iter_surfaces().count(), 1, "{moved:?}: no floating window expected");
            assert!(dock::spacer_rect(&st.dock).is_some(), "{moved:?}: the free area must stay");
            assert_eq!(leaf_rect(&st, moved), leaf_rect(&st, target), "{moved:?} should be a tab of {target:?}'s window");

            // float: drop over the body of another window (outside the drop buttons)
            if moved != Pane::Properties {
                let ctx = egui::Context::default();
                crate::theme::apply(&ctx);
                let mut st = settled(&ctx);
                let to_leaf = leaf_rect(&st, Pane::Properties);
                let to = pos2(to_leaf.min.x + 150.0, to_leaf.min.y + 130.0);
                let from = tab_grab(&st, moved);
                drag(&mut st, &ctx, from, to);
                assert_eq!(st.dock.iter_surfaces().count(), 2, "{moved:?} should float");
                assert!(dock::spacer_rect(&st.dock).is_some(), "{moved:?}: the free area must stay");
            }
        }
    }

    #[test]
    fn layer_undo_roundtrip_names_the_edit_and_shares_strokes() {
        let (mut st, a) = state_with(vec![painted_layer(2000), new_fill_layer("Base", [0.5; 3])]);
        let original = st.stacks[a].layers.clone();

        // hide a layer: one entry, named, and the snapshot shares the strokes with the live stack
        st.stacks[a].layers[1].visible = false;
        st.record_layer_change(a);
        assert_eq!(st.undo_labels(), vec!["Hide layer 'Base'".to_string()]);
        let shared = match st.undo_stack.last() {
            Some(UndoEntry::Layers { layers, .. }) => match (&layers[0].content[1].kind, &st.stacks[a].layers[0].content[1].kind) {
                (EffectKind::Paint(x), EffectKind::Paint(y)) => x.strokes.ptr_eq(&y.strokes),
                _ => false,
            },
            _ => false,
        };
        assert!(shared, "undo snapshot must share stroke storage, not copy it");

        st.undo();
        assert_eq!(st.stacks[a].layers, original);
        assert!(st.status.starts_with("Undo: Hide layer"));
        st.redo();
        assert!(!st.stacks[a].layers[1].visible);
        assert!(st.status.starts_with("Redo: Hide layer"));
    }

    #[test]
    fn project_round_trips_through_a_shard_file() {
        let spp = std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("screen.spp")).unwrap_or_default();
        if !spp.exists() {
            return; // corpus file not present on this machine
        }
        let mut a = AppState::new();
        a.open_document(&spp);
        assert!(!a.status.starts_with("Open failed"), "{}", a.status);
        let n0 = a.stacks[0].layers.len();
        a.add_fill_layer();
        let (w, h) = (a.scene.texs[0].w, a.scene.texs[0].h);
        let mut ov = vec![0u8; (w * h * 4) as usize];
        ov[0..4].copy_from_slice(&[255, 0, 0, 255]);
        a.scene.texs[0].set_overlay(w, h, &ov);
        let file = std::env::temp_dir().join(format!("shard_roundtrip_{}.shard", std::process::id()));
        a.save_project(&file).unwrap();

        let mut b = AppState::new();
        b.open_document(&file);
        assert!(!b.status.starts_with("Open failed"), "{}", b.status);
        assert_eq!(b.stacks[0].layers.len(), n0 + 1);
        assert_eq!(b.stacks[0].layers, a.stacks[0].layers);
        assert!(b.scene.texs[0].has_paint());
        assert_eq!(&b.scene.texs[0].overlay().0[0..4], &[255, 0, 0, 255]);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn layer_actions_edit_the_stack_and_undo() {
        let (mut st, a) = state_with(vec![new_fill_layer("One", [0.1; 3]), new_fill_layer("Two", [0.2; 3])]);
        st.stacks[a].selected = Some(vec![1]);
        let original = st.stacks[a].layers.clone();

        st.layer_action(LayerAction::Duplicate);
        st.record_layer_change(a);
        assert_eq!(st.stacks[a].layers.len(), 3);
        assert_eq!(st.stacks[a].layers[1].name, "Two");
        assert_ne!(st.stacks[a].layers[1].uid, st.stacks[a].layers[2].uid);
        assert_eq!(st.undo_labels().last().map(String::as_str), Some("Add layer 'Two'"));

        st.layer_action(LayerAction::Group);
        st.record_layer_change(a);
        assert_eq!(st.stacks[a].layers[1].kind, LayerKind::Group);

        st.layer_action(LayerAction::AddMask(false));
        st.record_layer_change(a);
        assert!(st.stacks[a].layers[1].mask.is_some());

        st.undo();
        st.undo();
        st.undo();
        assert_eq!(st.stacks[a].layers, original);
    }

    #[test]
    fn selection_follows_the_layer_across_undo() {
        let (mut st, a) = state_with(vec![new_fill_layer("One", [0.1; 3]), new_fill_layer("Two", [0.2; 3])]);
        // add a layer on top: "Two" (selected) moves from index 1 to index 2
        st.stacks[a].layers.insert(0, new_fill_layer("Top", [0.9; 3]));
        st.stacks[a].selected = Some(vec![2]);
        st.record_layer_change(a);
        assert_eq!(st.undo_labels(), vec!["Add layer 'Top'".to_string()]);
        st.undo();
        // back to two layers: "Two" is selected again at index 1
        assert_eq!(st.stacks[a].layers.len(), 2);
        assert_eq!(st.stacks[a].selected, Some(vec![1]));
    }

    #[test]
    fn autosave_writes_only_after_an_edit_and_is_offered_when_newer() {
        let dir = std::env::temp_dir().join(format!("shard_autosave_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("job.shard");
        let (mut st, a) = state_with(vec![new_fill_layer("One", [0.1; 3])]);
        st.shard_path = Some(file.clone());
        st.config.autosave_secs = 1;
        st.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(10);

        // nothing edited: no backup
        st.autosave_tick();
        assert!(!dir.join("job_autosave_0.shard").exists());

        // an edit: the first slot is written, the next one alternates
        st.stacks[a].layers.insert(0, new_fill_layer("Two", [0.2; 3]));
        st.record_layer_change(a);
        st.autosave_tick();
        for _ in 0..200 {
            if !st.autosave_busy.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let first = dir.join("job_autosave_0.shard");
        assert!(first.exists(), "autosave file written");
        let pf = shard_engine::persist::load(&first).unwrap();
        assert_eq!(pf.sets[0].layers.len(), 2);
        assert_eq!(st.autosave_slot, 1);

        // the project file itself is older than its backup: recovery is offered
        std::fs::write(&file, b"old").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        assert_eq!(AppState::newer_autosave(&file), Some(first));
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Diagnostic note on stderr (the in-app log arrives with the Log panel).
fn log_note(msg: &str) {
    eprintln!("[shard] {msg}");
}
