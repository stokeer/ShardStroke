//! ShardStroke: viewport + texture-space painting + layer panel.
//!
//! usage:
//!   shard-app [--obj MESH.obj] [--project FILE.(spp|spsm|spt)]
//!   shard-app --shot OUT.png [--size 1440x900] [--demo-stroke] [--obj ..] [--project ..]   (headless)
//!
//! Window controls (Painter-style): LMB paint, Alt+LMB orbit, Alt+RMB / wheel zoom, Alt+MMB pan,
//! `[` `]` brush size, `1`/`2` paint/eraser, Esc quit.

mod config;
mod dock;
mod env;
mod export;
mod library;
mod previews;
mod gui;
mod icons;
mod paint;
mod oidn;
mod pathtrace;
mod stats;
mod outline;
mod renderer;
mod scene;
mod theme;
mod ui;
mod widgets;

pub use shard_engine::{bitmaps, mesh, strokes};

use std::path::PathBuf;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use gui::Gui;
use paint::Tool;
use renderer::Renderer;
use ui::{AppState, RenderCmd};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

struct Args {
    obj: Option<PathBuf>,
    project: Option<PathBuf>,
    shot: Option<PathBuf>,
    save_texture: Option<PathBuf>,
    size: (u32, u32),
    demo_stroke: bool,
    /// `--cam yaw,pitch,dist` (radians, radians, distance)
    cam: Option<(f32, f32, f32)>,
    /// `--add-fill r,g,b`: add a fill layer on top (headless check of the compositor path)
    add_fill: Option<[f32; 3]>,
    /// `--save-project FILE`: save the session as a ShardStroke project after the other options are applied.
    save_project: Option<PathBuf>,
    /// `--set N`: active texture set
    set: Option<usize>,
    /// `--export DIR`: write `<set>_BCR.tga` / `<set>_NMO.tga` and exit (`--export-res N`, `--set N`)
    export: Option<PathBuf>,
    export_res: Option<u32>,
    /// `--texture-size N`: resize every texture set to N x N after opening (diagnostic)
    texture_size: Option<u32>,
    /// `--dump-masks DIR`: write every mask of the active set as PNG + index.txt and exit
    dump_masks: Option<PathBuf>,
    /// `--mask-paint NAME`: with `--dump-masks`, paint a test stroke into that layer's mask first
    mask_paint: Option<String>,
    /// `--target x,y,z`: camera orbit target (diagnostic close-ups).
    target: Option<(f32, f32, f32)>,
    /// `--painter-export PREFIX`: show Painter's exported textures (`<PREFIX><set>_BaseColor.png`...).
    painter_export: Option<String>,
    /// `--props NAME:IDX[:content]`: open the properties of an effect (mask by default).
    props: Option<String>,
    /// `--picker [anchors]`: open the resource picker on the first source of the props effect.
    picker: Option<String>,
    /// `--effect-toggle NAME`: hide each mask effect of that layer in turn and print timings.
    effect_toggle: Option<String>,
    /// `--probe-strokes`: ray-cast recorded stroke keys against the mesh (diagnostic, then exit)
    probe_strokes: bool,
    /// `--hdri NAME`: environment from the HDRI folder (file stem, case-insensitive substring)
    hdri: Option<String>,
    /// `--view NAME`: viewport channel/mesh-map selector (label of the combo box, case-insensitive)
    view: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        obj: None,
        project: None,
        shot: None,
        save_texture: None,
        size: (1440, 900),
        demo_stroke: false,
        cam: None,
        add_fill: None,
        save_project: None,
        set: None,
        probe_strokes: false,
        mask_paint: None,
        target: None,
        painter_export: None,
        props: None,
        picker: None,
        effect_toggle: None,
        dump_masks: None,
        texture_size: None,
        export: None,
        export_res: None,
        hdri: None,
        view: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--set" => a.set = it.next().and_then(|s| s.parse().ok()),
            "--view" => a.view = it.next(),
            "--hdri" => a.hdri = it.next(),
            "--probe-strokes" => a.probe_strokes = true,
            "--mask-paint" => a.mask_paint = it.next(),
            "--effect-toggle" => a.effect_toggle = it.next(),
            "--dump-masks" => a.dump_masks = it.next().map(PathBuf::from),
            "--texture-size" => a.texture_size = it.next().and_then(|s| s.parse().ok()),
            "--export" => a.export = it.next().map(PathBuf::from),
            "--save-project" => a.save_project = it.next().map(PathBuf::from),
            "--export-res" => a.export_res = it.next().and_then(|s| s.parse().ok()),
            "--painter-export" => a.painter_export = it.next(),
            "--props" => a.props = it.next(),
            "--picker" => a.picker = Some(it.next().unwrap_or_default()),
            "--target" => {
                if let Some(s) = it.next() {
                    let v: Vec<f32> = s.split(',').filter_map(|t| t.parse().ok()).collect();
                    if v.len() == 3 {
                        a.target = Some((v[0], v[1], v[2]));
                    }
                }
            }
            "--cam" => {
                if let Some(s) = it.next() {
                    let v: Vec<f32> = s.split(',').filter_map(|t| t.parse().ok()).collect();
                    if v.len() == 3 {
                        a.cam = Some((v[0], v[1], v[2]));
                    }
                }
            }
            "--add-fill" => {
                if let Some(s) = it.next() {
                    let v: Vec<f32> = s.split(',').filter_map(|t| t.parse().ok()).collect();
                    if v.len() == 3 {
                        a.add_fill = Some([v[0], v[1], v[2]]);
                    }
                }
            }
            "--obj" => a.obj = it.next().map(PathBuf::from),
            "--project" => a.project = it.next().map(PathBuf::from),
            "--shot" => a.shot = it.next().map(PathBuf::from),
            "--save-texture" => a.save_texture = it.next().map(PathBuf::from),
            "--demo-stroke" => a.demo_stroke = true,
            "--size" => {
                if let Some(s) = it.next() {
                    if let Some((w, h)) = s.split_once('x') {
                        a.size = (w.parse().unwrap_or(1440), h.parse().unwrap_or(900));
                    }
                }
            }
            other => eprintln!("ignored argument: {other}"),
        }
    }
    a
}

/// Working set / private bytes of this process in MB (Windows only; 0 elsewhere).
#[cfg(windows)]
fn process_mem_mb() -> (u64, u64) {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(h: isize, c: *mut Counters, cb: u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
    }
    unsafe {
        let mut c: Counters = std::mem::zeroed();
        c.cb = std::mem::size_of::<Counters>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return ((c.working_set >> 20) as u64, (c.pagefile >> 20) as u64);
        }
    }
    (0, 0)
}

#[cfg(not(windows))]
fn process_mem_mb() -> (u64, u64) {
    (0, 0)
}

fn trace(label: &str) {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t0 = *START.get_or_init(std::time::Instant::now);
    if std::env::var_os("SHARD_TRACE").is_some() {
        let (ws, commit) = process_mem_mb();
        eprintln!("[trace] {:7.1}s {label:34} working set {ws:5} MB  commit {commit:5} MB", t0.elapsed().as_secs_f32());
    }
}

fn init_state(args: &Args) -> Result<AppState, String> {
    trace("start");
    let mut st = AppState::new();
    // started without anything to open: the start screen comes up
    if args.obj.is_none() && args.project.is_none() && std::env::var_os("SHARD_NO_PROMPT").is_none() {
        st.open_start();
    }
    if let Some(p) = &args.obj {
        st.scene.load_mesh(p)?;
    }
    if let Some(p) = &args.project {
        if !p.exists() {
            return Err(format!("--project file not found: {}", p.display()));
        }
        st.open_document(p);
        trace("after open_document");
        if st.status.starts_with("Open failed") {
            return Err(st.status.clone());
        }
    } else if args.hdri.is_none() {
        st.use_default_environment();
    }
    if let Some(prefix) = &args.painter_export {
        st.wait_jobs();
        match st.apply_painter_export(prefix) {
            Ok(n) => eprintln!("painter export applied to {n} set(s)"),
            Err(e) => return Err(e),
        }
    }
    if let Some(spec) = &args.props {
        let parts: Vec<&str> = spec.split(':').collect();
        if parts.len() >= 2 {
            let idx = parts[1].parse().unwrap_or(0);
            st.wait_jobs();
            if let Err(e) = st.select_effect(parts[0], idx, parts.get(2) != Some(&"content")) {
                return Err(e);
            }
        }
    }
    if let Some(tab) = &args.picker {
        st.open_picker_on_props(if tab == "anchors" { 1 } else { 0 });
    }
    if let Some((x, y, z)) = args.target {
        st.scene.cam.target = glam::Vec3::new(x, y, z);
    }
    if let Some((y, p, d)) = args.cam {
        st.scene.cam.yaw = y;
        st.scene.cam.pitch = p;
        st.scene.cam.dist = d;
    }
    if let Some(sz) = args.texture_size {
        for i in 0..st.stacks.len() {
            st.resize_set(i, sz, sz);
        }
    }
    if let Some(i) = args.set {
        st.set_active_set(i);
    }
    if let Some(c) = args.add_fill {
        st.scene.brush.color = c;
        st.add_fill_layer_public();
    }
    if let Some(p) = &args.save_project {
        st.wait_jobs();
        st.save_project(p)?;
        eprintln!("saved {}", p.display());
    }
    if let Some(name) = &args.hdri {
        let needle = name.to_ascii_lowercase();
        let found = st.hdri_files.iter().position(|f| {
            f.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.to_ascii_lowercase().contains(&needle))
        });
        match found {
            Some(i) => st.load_hdri(i),
            None => return Err(format!("no HDRI matching '{name}'")),
        }
    }
    if let Some(name) = &args.view {
        match ui::VIEW_CHOICES.iter().find(|(_, l)| l.eq_ignore_ascii_case(name)) {
            Some((k, _)) => st.set_view(*k),
            None => return Err(format!("unknown --view '{name}'")),
        }
    }
    Ok(st)
}

// ---------------------------------------------------------------- headless

/// Hand the renderer work queued by the UI state (aux maps, channel views).
fn apply_cmds(r: &mut Renderer, st: &mut AppState) {
    for c in std::mem::take(&mut st.render_cmds) {
        match c {
            RenderCmd::SetAux(i, w, h, data) => r.set_aux(i, w, h, &data),
            RenderCmd::SetView(v) => r.set_view(v.as_deref()),
            RenderCmd::SetNormal(i, w, h, data) => r.set_normal(i, w, h, &data),
            RenderCmd::FaceNormals(on) => r.face_normals = on,
            RenderCmd::SetEmissive(i, w, h, data) => r.set_emissive(i, w, h, &data),
        }
    }
}

fn run_shot(args: &Args) -> Result<(), String> {
    let mut st = init_state(args)?;
    trace("state ready, waiting for the compositions");
    // background compositions finish before the picture is taken (SHARD_SHOT_NOWAIT=secs: take it while they run)
    match std::env::var("SHARD_SHOT_NOWAIT").ok().and_then(|v| v.parse::<f32>().ok()) {
        Some(secs) => {
            let t = std::time::Instant::now();
            while t.elapsed().as_secs_f32() < secs {
                st.poll_jobs_once();
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
        None => st.wait_jobs(),
    }
    trace("compositions done");
    let (w, h) = args.size;
    let instance = wgpu::Instance::default();
    let mut r = pollster::block_on(Renderer::new(&instance, None, None, &st.scene.mesh, &st.scene.texs, &st.env))?;
    st.msaa_supported = r.supported_samples().to_vec();
    st.path_trace_supported = r.path_trace_supported();
    st.gpu_label = r.adapter_label.clone();
    // the usage sampler feeds the status bar's video memory, as in the app
    st.stats.set_sampling(true);
    let mut gui = Gui::new(&r.device, r.format());
    let ppp = 1.0f32;
    gui.ctx.set_pixels_per_point(ppp);
    let raw = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / ppp, h as f32 / ppp))),
        ..Default::default()
    };
    // two passes so layout settles (panel sizes)
    let warm = gui.ctx.run(raw(), |c| st.draw(c));
    gui.apply_textures(&r.device, &r.queue, &warm);
    // the layer thumbnails are made on a worker thread: keep drawing frames until they are in
    let t0 = std::time::Instant::now();
    let mut out = gui.ctx.run(raw(), |c| st.draw(c));
    while !st.thumbs_settled() && t0.elapsed().as_secs() < 90 {
        gui.apply_textures(&r.device, &r.queue, &out);
        std::thread::sleep(std::time::Duration::from_millis(30));
        out = gui.ctx.run(raw(), |c| st.draw(c));
    }
    trace("thumbnails settled");
    // SHARD_SHOT_CLICKS="x,y;x,y": left clicks (in points) played on the interface before the picture, one frame
    // for the press and one for the release each, then the frames settle again (UI tests of popovers, pickers)
    if let Ok(spec) = std::env::var("SHARD_SHOT_CLICKS") {
        for pair in spec.split(';') {
            // "t:text": typed text
            if let Some(text) = pair.strip_prefix("t:") {
                let mut input = raw();
                input.events.push(egui::Event::Text(text.to_string()));
                out = gui.ctx.run(input, |c| st.draw(c));
                gui.apply_textures(&r.device, &r.queue, &out);
                out = gui.ctx.run(raw(), |c| st.draw(c));
                gui.apply_textures(&r.device, &r.queue, &out);
                continue;
            }
            // "r:x,y": a right click in the viewport (opens the quick panel there)
            if let Some(rest) = pair.strip_prefix("r:") {
                let v: Vec<f32> = rest.split(',').filter_map(|t| t.trim().parse().ok()).collect();
                if v.len() == 2 {
                    st.quick_click(egui::pos2(v[0], v[1]));
                    let mut input = raw();
                    input.events.push(egui::Event::PointerMoved(egui::pos2(v[0], v[1])));
                    out = gui.ctx.run(input, |c| st.draw(c));
                    gui.apply_textures(&r.device, &r.queue, &out);
                    out = gui.ctx.run(raw(), |c| st.draw(c));
                    gui.apply_textures(&r.device, &r.queue, &out);
                }
                continue;
            }
            // "x,y!": press and release in the same frame (a quick click while the app is busy)
            let same_frame = pair.trim_end().ends_with('!');
            let v: Vec<f32> = pair.trim_end_matches(['!', ' ']).split(',').filter_map(|t| t.trim().parse().ok()).collect();
            if v.len() != 2 {
                continue;
            }
            let at = egui::pos2(v[0], v[1]);
            let button = |pressed: bool| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let frames: Vec<Vec<egui::Event>> = if same_frame {
                vec![vec![egui::Event::PointerMoved(at), button(true), button(false)]]
            } else {
                vec![vec![egui::Event::PointerMoved(at), button(true)], vec![button(false)]]
            };
            for events in frames {
                let mut input = raw();
                input.events = events;
                out = gui.ctx.run(input, |c| st.draw(c));
                gui.apply_textures(&r.device, &r.queue, &out);
            }
            let t1 = std::time::Instant::now();
            out = gui.ctx.run(raw(), |c| st.draw(c));
            while !st.thumbs_settled() && t1.elapsed().as_secs() < 30 {
                gui.apply_textures(&r.device, &r.queue, &out);
                std::thread::sleep(std::time::Duration::from_millis(30));
                out = gui.ctx.run(raw(), |c| st.draw(c));
            }
            gui.apply_textures(&r.device, &r.queue, &out);
            eprintln!("[shot] clicked {at:?}: {}", st.status);
        }
    }
    // the scene fills the whole window; the brush works in its free area
    let aspect = w as f32 / h as f32;
    if args.demo_stroke {
        st.scene.brush.radius = 40.0;
        let vp_px = (w as f32, h as f32);
        let mut prev = None;
        for i in 0..=60 {
            let t = i as f32 / 60.0;
            let ndc = Vec2::new(-0.5 + t, 0.35 * (t * 6.0).sin());
            prev = st.scene.paint_stroke(aspect, vp_px, ndc, prev);
        }
    }
    trace("renderer created");
    apply_cmds(&mut r, &mut st);
    // `Renderer::new` already uploaded mesh and textures; only pending base/paint changes remain
    r.sync_textures(&mut st.scene.texs);
    trace("textures uploaded");
    let tex = r.offscreen_target((w, h));
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let vp = st.scene.cam.view_proj(aspect);
    r.post = st.post;
    // SHARD_SHOT_BLOOM=level,threshold (diagnostic): bloom forced on with these settings, 0 = off
    if let Ok(v) = std::env::var("SHARD_SHOT_BLOOM") {
        let f: Vec<f32> = v.split(',').filter_map(|t| t.parse().ok()).collect();
        r.post.enabled = true;
        r.post.bloom = f.first().is_some_and(|l| *l > 0.0);
        r.post.bloom_level = f.first().copied().unwrap_or(0.0);
        r.post.bloom_threshold = f.get(1).copied().unwrap_or(1.5);
    }
    r.emissive_intensity = st.emissive_intensity;
    r.ao_intensity = st.ao_intensity;
    r.loading_mask = st.loading_sets();
    r.hidden = st.scene.hidden_sets;
    r.isolate = st.scene.isolated;
    // SHARD_SHOT_HIDE=<mask>: hide these texture sets (bit per set), to check the Texture Set List's eye
    if let Some(m) = std::env::var("SHARD_SHOT_HIDE").ok().and_then(|v| v.parse::<u32>().ok()) {
        r.hidden = m;
    }
    let lin = |c: [u8; 3]| c.map(|v| shard_compose::srgb_to_linear(v as f32 / 255.0));
    r.loading_fill = lin(st.config.loading_fill);
    r.loading_outline = lin(st.config.loading_outline);
    r.env_yaw = st.env_yaw;
    r.exposure = st.exposure_ev.exp2();
    r.show_background = st.show_background;
    r.set_samples(st.config.viewport_msaa);
    // SHARD_SHOT_ISOLATE: draw only the `--set` texture set (Alt+Q) and print its bounds, to aim `--target`
    if let (Some(set), true) = (args.set, std::env::var_os("SHARD_SHOT_ISOLATE").is_some()) {
        r.isolate = Some(set);
        st.scene.isolated = Some(set);
        // the interface again, so the Texture Set List shows the isolation
        out = gui.ctx.run(raw(), |c| st.draw(c));
        gui.apply_textures(&r.device, &r.queue, &out);
        let m = &st.scene.mesh;
        let (lo, hi) = m.indices.iter().filter(|&&i| m.vertices[i as usize].mat as usize == set).fold(
            (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)),
            |(lo, hi), &i| {
                let p = glam::Vec3::from(m.vertices[i as usize].pos);
                (lo.min(p), hi.max(p))
            },
        );
        eprintln!("set {set} bounds {lo:?} .. {hi:?}");
    }
    // statistics overlay in a shot: some frames to measure, and a usage reading (sampled once a second)
    if st.config.show_stats {
        st.stats.set_sampling(true);
        r.measure_gpu = true;
        let t_end = std::time::Instant::now() + std::time::Duration::from_millis(2300);
        while std::time::Instant::now() < t_end {
            let t0 = std::time::Instant::now();
            r.render(&view, (w, h), None, vp, st.scene.cam.eye());
            r.device.poll(wgpu::Maintain::Wait);
            st.stats.frame(t0.elapsed(), r.gpu_ms());
        }
        out = gui.ctx.run(raw(), |c| st.draw(c));
        gui.apply_textures(&r.device, &r.queue, &out);
    }
    // SHARD_SHOT_PATHTRACE=n: the path-traced picture after n samples
    let traced: u32 = std::env::var("SHARD_SHOT_PATHTRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    r.path_trace = traced > 0 || st.path_trace;
    let t_pt = std::time::Instant::now();
    for _ in 1..traced {
        r.render(&view, (w, h), None, vp, st.scene.cam.eye());
    }
    r.ai_denoise = st.config.ai_denoise && std::env::var_os("SHARD_NO_OIDN").is_none();
    r.render(&view, (w, h), None, vp, st.scene.cam.eye());
    // the neural denoiser's picture when it started (from 32 samples)
    while traced > 0 && r.path_trace_ai_pending() {
        std::thread::sleep(std::time::Duration::from_millis(2));
        r.render(&view, (w, h), None, vp, st.scene.cam.eye());
    }
    if traced > 0 {
        r.device.poll(wgpu::Maintain::Wait);
        eprintln!(
            "[shot] path traced {:?} samples in {:.1}s (supported: {}, neural denoiser at {:?})",
            r.path_trace_samples(),
            t_pt.elapsed().as_secs_f32(),
            r.path_trace_supported(),
            r.path_trace_ai_samples()
        );
    }
    gui.paint(&r.device, &r.queue, &view, (w, h), None, out);
    let rgba = r.read_texture(&tex, (w, h));
    let outp = args.shot.as_ref().unwrap();
    paint::save_png(outp, w, h, &rgba)?;
    println!("wrote {} ({}x{})", outp.display(), w, h);
    if let Some(tp) = &args.save_texture {
        st.scene.texs[0].save_png(tp)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- windowed

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    gui: Gui,
    egui_state: egui_winit::State,
}

struct App {
    st: AppState,
    gfx: Option<Gfx>,
    mods: ModifiersState,
    cursor: Vec2,
    last_cursor: Vec2,
    painting: bool,
    left: bool,
    right: bool,
    middle: bool,
    prev_ndc: Option<Vec2>,
    /// Cursor where the right button went down over the viewport without a modifier: released there
    /// (no drag), it opens the quick panel.
    right_press: Option<Vec2>,
    /// When the window last gained or lost focus: Alt+Tab (switching windows) must not reach Tab (Hide UI).
    focus_changed: std::time::Instant,
}

impl App {
    /// Cursor (physical px) -> NDC of the whole window (the scene fills it), if the cursor is over the free
    /// area the windows leave open and nothing (a floating window, a menu) is above it.
    fn viewport_ndc(&self) -> Option<(Vec2, f32, (f32, f32))> {
        let g = self.gfx.as_ref()?;
        let ppp = g.window.scale_factor() as f32;
        let size = g.window.inner_size();
        let (w, h) = (size.width as f32, size.height as f32);
        let (cx, cy) = (self.cursor.x, self.cursor.y);
        let at = egui::pos2(cx / ppp, cy / ppp);
        if w < 1.0 || h < 1.0 || !self.st.central.contains(at) {
            return None;
        }
        // another egui layer (a floating window, a popup, the viewport's own controls) covers the scene here
        if g.gui.ctx.layer_id_at(at).is_some_and(|l| l.order != egui::Order::Background) {
            return None;
        }
        Some((Vec2::new(cx / w * 2.0 - 1.0, 1.0 - cy / h * 2.0), w / h, (w, h)))
    }

    /// True when the pointer should go to egui (panels, popups) instead of the 3D viewport.
    /// The central panel itself is an egui area, so `wants_pointer_input()` cannot be used here:
    /// the cursor is "over the viewport" when it is inside the central rect and no popup is open.
    fn pointer_over_ui(&self) -> bool {
        let Some(g) = &self.gfx else { return false };
        if self.viewport_ndc().is_none() {
            return true;
        }
        g.gui.ctx.memory(|m| m.any_popup_open())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("ShardStroke")
                    .with_window_icon(window_icon())
                    .with_inner_size(winit::dpi::LogicalSize::new(1440.0, 900.0)),
            )
            .expect("window"),
        );
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("surface");
        let size = window.inner_size();
        let renderer = pollster::block_on(Renderer::new(
            &instance,
            Some(&surface),
            Some(wgpu::TextureFormat::Bgra8UnormSrgb),
            &self.st.scene.mesh,
            &self.st.scene.texs,
            &self.st.env,
        ))
        .expect("renderer");
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: renderer.format(),
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&renderer.device, &config);
        self.st.msaa_supported = renderer.supported_samples().to_vec();
        // the renderer was created with the current mesh/textures/environment
        self.st.mesh_dirty = false;
        self.st.env_dirty = false;
        let gui = Gui::new(&renderer.device, renderer.format());
        let egui_state = egui_winit::State::new(
            gui.ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        self.gfx = Some(Gfx { window, surface, config, renderer, gui, egui_state });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        // egui first
        let mut consumed = false;
        if let Some(g) = &mut self.gfx {
            let r = g.egui_state.on_window_event(&g.window, &ev);
            consumed = r.consumed;
            if r.repaint {
                g.window.request_redraw();
            }
        }
        match &ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(_) => {
                self.focus_changed = std::time::Instant::now();
                // Alt may have been released in another window: the next key must not read it as held
                self.mods = ModifiersState::default();
            }
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gfx {
                    g.config.width = s.width.max(1);
                    g.config.height = s.height.max(1);
                    g.surface.configure(&g.renderer.device, &g.config);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = *state == ElementState::Pressed;
                let over_ui = self.pointer_over_ui();
                match button {
                    MouseButton::Left => {
                        let quick = down && !over_ui && !self.mods.alt_key() && {
                            let vp = self.viewport_ndc().map(|(ndc, aspect, _)| (ndc, aspect));
                            self.st.quick_takes_click(vp)
                        };
                        if down && !over_ui && !quick {
                            self.left = true;
                            if !self.mods.alt_key() {
                                if let Some((ndc, aspect, vp)) = self.viewport_ndc() {
                                    self.painting = true;
                                    self.prev_ndc = self.st.paint_begin(aspect, vp, ndc);
                                }
                            }
                        }
                        if !down {
                            self.left = false;
                            if self.painting {
                                self.st.end_stroke();
                            }
                            self.painting = false;
                            self.prev_ndc = None;
                        }
                    }
                    MouseButton::Right => {
                        self.right = down && !over_ui;
                        if down {
                            self.right_press = (!over_ui && self.mods.is_empty()).then_some(self.cursor);
                        } else if let Some(at) = self.right_press.take() {
                            if (self.cursor - at).length() < 5.0 && self.mods.is_empty() {
                                if let Some(g) = &self.gfx {
                                    let ppp = g.window.scale_factor() as f32;
                                    self.st.quick_click(egui::pos2(at.x / ppp, at.y / ppp));
                                    g.window.request_redraw();
                                }
                            }
                        }
                    }
                    MouseButton::Middle => self.middle = down && !over_ui,
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor = self.cursor;
                self.cursor = Vec2::new(position.x as f32, position.y as f32);
                let d = self.cursor - self.last_cursor;
                let cam = &mut self.st.scene.cam;
                if self.mods.shift_key() && self.right {
                    // Shift + right drag: rotate the environment (lighting, reflections, background)
                    self.st.env_yaw += d.x * 0.008;
                    self.st.status = format!("Environment rotation {:.0}°", self.st.env_yaw.to_degrees().rem_euclid(360.0));
                } else if self.mods.alt_key() && self.left {
                    cam.yaw -= d.x * 0.008;
                    cam.pitch = (cam.pitch + d.y * 0.008).clamp(-1.5, 1.5);
                } else if self.mods.alt_key() && self.right {
                    cam.dist = (cam.dist * (1.0 + d.y * 0.01)).clamp(0.3, 50.0);
                } else if self.mods.alt_key() && self.middle {
                    let s = cam.dist * 0.0015;
                    let right = Vec3::new(cam.yaw.cos(), 0.0, -cam.yaw.sin());
                    cam.target += (-right * d.x + Vec3::Y * d.y) * s;
                } else if self.painting && self.left {
                    if let Some((ndc, aspect, vp)) = self.viewport_ndc() {
                        self.prev_ndc = self.st.paint_move(aspect, vp, ndc, self.prev_ndc);
                    } else {
                        self.prev_ndc = None;
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } if !self.pointer_over_ui() => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.05,
                };
                let cam = &mut self.st.scene.cam;
                cam.dist = (cam.dist * (1.0 - y * 0.1)).clamp(0.3, 50.0);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !consumed => {
                if std::env::var_os("SHARD_TRACE_VIEW").is_some() {
                    eprintln!("[key] {:?} repeat={} ctrl={} alt={}", event.logical_key, event.repeat, self.mods.control_key(), self.mods.alt_key());
                }
                let over_vp = self.viewport_ndc().is_some();
                let b = &mut self.st.scene.brush;
                match event.logical_key.as_ref() {
                    // Painter shortcuts (docs/PAINTER_REFERENCE.md 12.19): C / Shift+C channel, B / Shift+B mesh map,
                    // M material
                    // Painter: Undo Ctrl+Z, Redo Ctrl+Y
                    Key::Character("z") | Key::Character("Z") if self.mods.control_key() => self.st.undo(),
                    Key::Character("y") | Key::Character("Y") if self.mods.control_key() => self.st.redo(),
                    // Painter: Duplicate Ctrl+D, Group Ctrl+G, Delete Del, Copy / Cut / Paste Ctrl+C / X / V
                    Key::Character("d") | Key::Character("D") if self.mods.control_key() && !self.mods.shift_key() => {
                        self.st.layer_action(ui::LayerAction::Duplicate)
                    }
                    Key::Character("g") | Key::Character("G") if self.mods.control_key() => {
                        self.st.layer_action(ui::LayerAction::Group)
                    }
                    Key::Named(NamedKey::Delete) => self.st.layer_action(ui::LayerAction::Delete),
                    Key::Named(NamedKey::F9) if !event.repeat => self.st.set_painting(),
                    Key::Named(NamedKey::F10) if !event.repeat => self.st.set_rendering(),
                    Key::Character("c") | Key::Character("C") if self.mods.control_key() && !self.mods.alt_key() => {
                        self.st.layer_action(ui::LayerAction::Copy)
                    }
                    Key::Character("x") | Key::Character("X") if self.mods.control_key() => {
                        self.st.layer_action(ui::LayerAction::Cut)
                    }
                    Key::Character("v") | Key::Character("V") if self.mods.control_key() && !self.mods.alt_key() => {
                        self.st.layer_action(ui::LayerAction::Paste)
                    }
                    // Painter: Save Ctrl+S, Save as Ctrl+Shift+S
                    // Painter: Export Textures Ctrl+Shift+E
                    Key::Character("e") | Key::Character("E") if self.mods.control_key() && self.mods.shift_key() => {
                        self.st.open_export()
                    }
                    Key::Character("s") | Key::Character("S") if self.mods.control_key() => {
                        self.st.save_current(self.mods.shift_key())
                    }
                    Key::Character("c") | Key::Character("C")
                        if !event.repeat
                            && !self.mods.control_key()
                            && !self.mods.alt_key()
                            && over_vp =>
                    {
                        let dir = if self.mods.shift_key() { -1 } else { 1 };
                        self.st.cycle_view(ui::ViewGroup::Channels, dir);
                    }
                    Key::Character("b") | Key::Character("B")
                        if !event.repeat
                            && !self.mods.control_key()
                            && !self.mods.alt_key()
                            && over_vp =>
                    {
                        let dir = if self.mods.shift_key() { -1 } else { 1 };
                        self.st.cycle_view(ui::ViewGroup::MeshMaps, dir);
                    }
                    Key::Character("m") | Key::Character("M")
                        if !event.repeat && !self.mods.control_key() && !self.mods.alt_key() && over_vp =>
                    {
                        self.st.set_view(ui::ViewKind::Material)
                    }
                    Key::Character("q") | Key::Character("Q") if self.mods.alt_key() => {
                        self.st.toggle_isolate();
                    }
                    Key::Character("[") => b.radius = (b.radius * 0.8).max(1.0),
                    Key::Character("]") => b.radius = (b.radius * 1.25).min(256.0),
                    Key::Character("1") => self.st.scene.tool = Tool::Paint,
                    Key::Character("2") => self.st.scene.tool = Tool::Eraser,
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.redraw(el),
            _ => {}
        }
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}

impl App {
    fn redraw(&mut self, el: &ActiveEventLoop) {
        let Some(g) = &mut self.gfx else { return };
        // SHARD_TRACE_FRAMES=ms: report every frame of the UI thread that takes longer than that, by part
        let frame_t0 = std::time::Instant::now();
        let raw = g.egui_state.take_egui_input(&g.window);
        let mut out = g.gui.ctx.run(raw, |c| self.st.draw(c));
        let t_draw = frame_t0.elapsed();
        g.egui_state.handle_platform_output(&g.window, std::mem::take(&mut out.platform_output));
        if self.st.quit {
            el.exit();
            return;
        }
        if self.st.mesh_dirty {
            g.renderer.set_mesh(&self.st.scene.mesh);
            g.renderer.set_textures(&self.st.scene.texs);
            self.st.mesh_dirty = false;
        }
        if self.st.env_dirty {
            g.renderer.set_environment(&self.st.env);
            self.st.env_dirty = false;
        }
        let t_a = std::time::Instant::now();
        apply_cmds(&mut g.renderer, &mut self.st);
        let t_cmds = t_a.elapsed();
        let t_b = std::time::Instant::now();
        g.renderer.sync_textures(&mut self.st.scene.texs);
        let t_sync = t_b.elapsed();
        let aspect = (g.config.width as f32 / g.config.height.max(1) as f32).max(0.01);
        let vp = self.st.scene.cam.view_proj(aspect);
        let eye = self.st.scene.cam.eye();
        let size = (g.config.width, g.config.height);
        match g.surface.get_current_texture() {
            Ok(frame) => {
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                // the scene is the background of the program: it fills the window, egui is laid over it
                g.renderer.post = self.st.post;
                g.renderer.emissive_intensity = self.st.emissive_intensity;
                g.renderer.ao_intensity = self.st.ao_intensity;
                g.renderer.loading_mask = self.st.loading_sets();
                g.renderer.hidden = self.st.scene.hidden_sets;
                g.renderer.isolate = self.st.scene.isolated;
                let lin = |c: [u8; 3]| c.map(|v| shard_compose::srgb_to_linear(v as f32 / 255.0));
                g.renderer.loading_fill = lin(self.st.config.loading_fill);
                g.renderer.loading_outline = lin(self.st.config.loading_outline);
                // the stripes of the sets still loading move: keep drawing frames until they are in
                if g.renderer.loading_mask != 0 {
                    g.window.request_redraw();
                }
                g.renderer.env_yaw = self.st.env_yaw;
                g.renderer.exposure = self.st.exposure_ev.exp2();
                g.renderer.show_background = self.st.show_background;
                g.renderer.set_samples(self.st.config.viewport_msaa);
                g.renderer.path_trace = self.st.path_trace;
                g.renderer.measure_gpu = self.st.config.show_stats;
                g.renderer.ai_denoise = self.st.config.ai_denoise;
                g.renderer.render(&view, size, None, vp, eye);
                // the path-traced picture refines one sample per frame until it converges
                self.st.path_trace_supported = g.renderer.path_trace_supported();
                if self.st.gpu_label.is_empty() {
                    self.st.gpu_label = g.renderer.adapter_label.clone();
                }
                self.st.path_trace_samples = g.renderer.path_trace_samples();
                self.st.path_trace_ai = g.renderer.path_trace_ai_samples();
                if g.renderer.path_trace_busy() {
                    g.window.request_redraw();
                }
                g.gui.paint(&g.renderer.device, &g.renderer.queue, &view, size, None, out);
                frame.present();
                // statistics overlay: the viewport keeps drawing while it is shown, so the FPS is the real rate
                let show = self.st.config.show_stats;
                // the sampler also feeds the video memory of the status bar: always on (once a second)
                self.st.stats.set_sampling(true);
                if show {
                    self.st.stats.frame(frame_t0.elapsed(), g.renderer.gpu_ms());
                    g.window.request_redraw();
                }
            }
            Err(_) => g.surface.configure(&g.renderer.device, &g.config),
        }
        if let Some(limit) = std::env::var("SHARD_TRACE_FRAMES").ok().and_then(|v| v.parse::<u128>().ok()) {
            let total = frame_t0.elapsed();
            if total.as_millis() >= limit {
                eprintln!(
                    "[frame] {:>6} ms  draw {:>6}  commands {:>6}  textures {:>6}  render+present {:>6}",
                    total.as_millis(),
                    t_draw.as_millis(),
                    t_cmds.as_millis(),
                    t_sync.as_millis(),
                    (total - t_draw - t_cmds - t_sync).as_millis()
                );
            }
        }
    }
}

/// The window and taskbar icon (the executable's own comes from `build.rs`).
fn window_icon() -> Option<winit::window::Icon> {
    let mut reader = png::Decoder::new(&include_bytes!("../assets/icon64.png")[..]).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    winit::window::Icon::from_rgba(buf, info.width, info.height).ok()
}

fn collect_strokes<'a>(es: &'a [shard_model::Effect], out: &mut Vec<&'a shard_model::Stroke>) {
    use shard_model::EffectKind;
    for e in es {
        match &e.kind {
            EffectKind::Paint(a) | EffectKind::Eraser(a) | EffectKind::Smudge(a) => out.extend(a.strokes.iter()),
            EffectKind::Group { children } => collect_strokes(children, out),
            _ => {}
        }
    }
}

fn collect_layer_strokes<'a>(ls: &'a [shard_model::Layer], out: &mut Vec<&'a shard_model::Stroke>) {
    for l in ls {
        collect_strokes(&l.content, out);
        if let Some(m) = &l.mask {
            collect_strokes(&m.effects, out);
        }
        collect_layer_strokes(&l.children, out);
    }
}

/// Diagnostic: cast recorded stroke keys through the stored camera and count the hits on triangles
/// of the texture set that owns the stroke's layer, for both vertical screen conventions.
/// Diagnostic (`--probe-strokes` with SHARD_PROBE_EDGE=1): artists wear paint along edges, so the right mapping of
/// the stroke keys to the mesh is the one under which the keys land on the baked curvature's edges. Mean
/// `|curvature - 0.5|` at the surface point of every key, for shifts of the screen mapping.
fn probe_stroke_edges(st: &AppState) {
    use glam::{Mat4, Vec4};
    use ui::Bake;
    let mut rows: Vec<(f32, f32, f32, f32, f64, usize)> = Vec::new();
    let list = |k: &str, d: &[f32]| -> Vec<f32> {
        std::env::var(k)
            .ok()
            .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect())
            .filter(|v: &Vec<f32>| !v.is_empty())
            .unwrap_or_else(|| d.to_vec())
    };
    let (ws, ys, xs) = (
        list("SHARD_PROBE_WIDE", &[1.0]),
        list("SHARD_PROBE_YOFF", &[-0.06, -0.045, -0.03, -0.015, 0.0]),
        list("SHARD_PROBE_XOFF", &[0.0]),
    );
    let yscales = list("SHARD_PROBE_YSCALE", &[0.94, 0.97, 1.0, 1.03, 1.06]);
    let xscales = list("SHARD_PROBE_XSCALE", &[1.0]);
    // translation of the scene the stroke cameras were recorded in, relative to ours (mesh units), and scale
    let (txs, tys, tzs, scs) = (
        list("SHARD_PROBE_TX", &[0.0]),
        list("SHARD_PROBE_TY", &[0.0]),
        list("SHARD_PROBE_TZ", &[0.0]),
        list("SHARD_PROBE_SCALE", &[1.0]),
    );
    let step_keys: usize = std::env::var("SHARD_PROBE_KEYSTEP").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    for &wide in &ws {
      for &ysc in &yscales {
       for &xsc in &xscales {
        for &yoff in &ys {
            for &xoff in &xs {
             for &tx in &txs {
              for &ty in &tys {
               for &tz in &tzs {
                for &scl in &scs {
                let shift = glam::Vec3::new(tx, ty, tz);
                let (mut acc, mut n) = (0.0f64, 0usize);
                for (set, stack) in st.stacks.iter().enumerate() {
                    let Some(curv) = st.bake_image(set, Bake::Curvature) else { continue };
                    let mut strokes = Vec::new();
                    collect_layer_strokes(&stack.layers, &mut strokes);
                    for s in strokes.iter().step_by(3) {
                        if s.view_type == 0 {
                            continue;
                        }
                        let inv = (Mat4::from_cols_array(&s.projection) * Mat4::from_cols_array(&s.view)).inverse();
                        for k in s.keys.iter().step_by(step_keys) {
                            let (x, y) = (2.0 * (k.pos[0] * xsc + xoff) - wide, 2.0 * (k.pos[1] * ysc + yoff) - 1.0);
                            let nr = inv * Vec4::new(x, y, -1.0, 1.0);
                            let fr = inv * Vec4::new(x, y, 1.0, 1.0);
                            let (nr, fr) = (nr.truncate() / nr.w, fr.truncate() / fr.w);
                            let dir = (fr - nr).normalize_or_zero();
                            if dir == glam::Vec3::ZERO {
                                continue;
                            }
                            // the recorded scene is ours scaled by `scl` and shifted by `shift`
                            let nr = nr * scl + shift;
                            let own = |t: usize| st.scene.mesh.tri_mat.get(t).copied() == Some(set as u32);
                            if let Some(h) = st.scene.mesh.raycast_where(nr, dir, |_| true) {
                                if !own(h.tri as usize) {
                                    continue;
                                }
                                let (px, py) = (
                                    (h.uv.x.rem_euclid(1.0) * curv.w as f32) as u32,
                                    ((1.0 - h.uv.y).rem_euclid(1.0) * curv.h as f32) as u32,
                                );
                                let v = curv.pixel(px.min(curv.w - 1), py.min(curv.h - 1))[0];
                                acc += (v - 0.5).abs() as f64;
                                n += 1;
                            }
                        }
                    }
                }
                if txs.len() * tys.len() * tzs.len() * scs.len() > 1 {
                    println!("  shift ({:+.3}, {:+.3}, {:+.3}) scale {:.3} yoff {:+.3} xoff {:+.3}: {:.4}  ({} keys)", tx, ty, tz, scl, yoff, xoff, acc / n.max(1) as f64, n);
                }
                rows.push((wide, xoff, yoff, ysc + 1000.0 * xsc, acc / n.max(1) as f64, n));
                }
               }
              }
             }
            }
        }
       }
      }
    }
    rows.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap());
    println!("edge-ness (mean |curvature - 0.5|) of the key hits, best first (wide, xoff, yoff):");
    for r in rows.iter().take(8) {
        println!(
            "  wide {:.2} xoff {:+.3} yoff {:+.3} yscale {:.2} xscale {:.2}: {:.4}  ({} keys)",
            r.0, r.1, r.2, r.3 % 1000.0, (r.3 / 1000.0).floor().max(1.0), r.4, r.5
        );
    }
    if let Some(r) = rows.iter().find(|r| r.0 == 1.0 && r.1 == 0.0 && r.2 == 0.0) {
        println!("  current (1.00, 0, 0): {:.4}", r.4);
    }
    println!("  worst: {:.4}", rows.last().map(|r| r.4).unwrap_or(0.0));
}

/// Diagnostic (SHARD_PROBE_EDGE=2): the vertical shift that puts the keys of each stroke on edges, per stroke
/// (a histogram) and per block of strokes in recording order (sessions of one window geometry).
fn probe_stroke_offsets(st: &AppState) {
    use glam::{Mat4, Vec4};
    use ui::Bake;
    let offs: Vec<f32> = (-12..=6).map(|i| i as f32 * 0.01).collect();
    let mut all: Vec<(u64, usize, Vec<f64>, bool)> = Vec::new(); // (stroke order key, keys, score per offset, ortho)
    for (set, stack) in st.stacks.iter().enumerate() {
        let Some(curv) = st.bake_image(set, Bake::Curvature) else { continue };
        let mut strokes = Vec::new();
        collect_layer_strokes(&stack.layers, &mut strokes);
        for s in strokes {
            if s.view_type == 0 || s.keys.len() < 12 {
                continue;
            }
            let inv = (Mat4::from_cols_array(&s.projection) * Mat4::from_cols_array(&s.view)).inverse();
            let mut score = vec![0.0f64; offs.len()];
            let mut n = 0usize;
            for (oi, off) in offs.iter().enumerate() {
                let (mut acc, mut cnt) = (0.0f64, 0usize);
                for k in s.keys.iter().step_by(2) {
                    let (x, y) = (2.0 * k.pos[0] - 1.0, 2.0 * (k.pos[1] + off) - 1.0);
                    let nr = inv * Vec4::new(x, y, -1.0, 1.0);
                    let fr = inv * Vec4::new(x, y, 1.0, 1.0);
                    let (nr, fr) = (nr.truncate() / nr.w, fr.truncate() / fr.w);
                    let dir = (fr - nr).normalize_or_zero();
                    if dir == glam::Vec3::ZERO {
                        continue;
                    }
                    if let Some(h) = st.scene.mesh.raycast(nr, dir) {
                        if st.scene.mesh.tri_mat.get(h.tri as usize).copied() != Some(set as u32) {
                            continue;
                        }
                        let (px, py) = (
                            (h.uv.x.rem_euclid(1.0) * curv.w as f32) as u32,
                            ((1.0 - h.uv.y).rem_euclid(1.0) * curv.h as f32) as u32,
                        );
                        acc += (curv.pixel(px.min(curv.w - 1), py.min(curv.h - 1))[0] - 0.5).abs() as f64;
                        cnt += 1;
                    }
                }
                score[oi] = acc / cnt.max(1) as f64;
                n = n.max(cnt);
            }
            if n >= 10 {
                // recording order: the first key's seed is random, use the stroke's seed-independent camera as key
                all.push((s.seed as u64, n, score, s.projection[11].abs() < 1e-6));
            }
        }
    }
    // per kind of camera: the orthographic views and the perspective ones are told apart (they could differ)
    for (label, ortho) in [("orthographic", true), ("perspective", false)] {
        let sel: Vec<&(u64, usize, Vec<f64>, bool)> = all.iter().filter(|r| r.3 == ortho).collect();
        println!("{label}: {} strokes scored", sel.len());
        let mut hist = vec![0usize; offs.len()];
        let mut sum = vec![0.0f64; offs.len()];
        for (_, _, sc, _) in &sel {
            let best = sc.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap_or(0);
            hist[best] += 1;
            for (i, v) in sc.iter().enumerate() {
                sum[i] += v;
            }
        }
        println!("best vertical shift per stroke (histogram):");
        for (i, o) in offs.iter().enumerate() {
            println!("  {:+.2}: {:4} {}   mean score {:.4}", o, hist[i], "#".repeat(hist[i] / 4), sum[i] / sel.len().max(1) as f64);
        }
    }
}

fn probe_strokes(st: &AppState) {
    use glam::{Mat4, Vec4};
    if std::env::var_os("SHARD_PROBE_RANGE").is_some() {
        let (mut us, mut vs): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
        for stack in &st.stacks {
            let mut strokes = Vec::new();
            collect_layer_strokes(&stack.layers, &mut strokes);
            for s in strokes {
                if s.view_type == 0 {
                    continue;
                }
                for k in &s.keys {
                    us.push(k.pos[0]);
                    vs.push(k.pos[1]);
                }
            }
        }
        us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        vs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |v: &Vec<f32>, f: f32| v[((v.len() - 1) as f32 * f) as usize];
        for (n, v) in [("u", &us), ("v", &vs)] {
            println!(
                "{n}: n={} min {:.3} p1 {:.3} p10 {:.3} p50 {:.3} p90 {:.3} p99 {:.3} max {:.3}",
                v.len(), v[0], q(v, 0.01), q(v, 0.1), q(v, 0.5), q(v, 0.9), q(v, 0.99), v[v.len() - 1]
            );
        }
        return;
    }
    match std::env::var("SHARD_PROBE_EDGE").ok().as_deref() {
        Some("2") => {
            probe_stroke_offsets(st);
            return;
        }
        Some(_) => {
            probe_stroke_edges(st);
            return;
        }
        None => {}
    }
    // x_ndc = 2 * u - aspect (positions are normalised by the viewport height), y_ndc = 2 * v - 1
    println!("estimated aspect of the session: {:.3}", st.stroke_aspect);
    for aspect in [0.6f32, 0.7, 0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.78, 2.0] {
        let (mut own, mut any, mut total) = (0usize, 0usize, 0usize);
        for (set, stack) in st.stacks.iter().enumerate() {
            let mut strokes = Vec::new();
            collect_layer_strokes(&stack.layers, &mut strokes);
            for s in strokes.iter().step_by(2) {
                if s.view_type == 0 {
                    continue;
                }
                let v = Mat4::from_cols_array(&s.view);
                let p = Mat4::from_cols_array(&s.projection);
                let inv = (p * v).inverse();
                for k in s.keys.iter().step_by(15) {
                    let (x, y) = (2.0 * k.pos[0] - aspect, 2.0 * k.pos[1] - 1.0);
                    // the projection is a square-pixel one (u runs 0..aspect): only y is bounded by the unit square
                    if y.abs() > 1.0 {
                        continue;
                    }
                    let n = inv * Vec4::new(x, y, -1.0, 1.0);
                    let f = inv * Vec4::new(x, y, 1.0, 1.0);
                    let (n, f) = (n.truncate() / n.w, f.truncate() / f.w);
                    let dir = (f - n).normalize();
                    total += 1;
                    if let Some(h) = st.scene.mesh.raycast(n, dir) {
                        any += 1;
                        if st.scene.mesh.tri_mat.get(h.tri as usize).copied() == Some(set as u32) {
                            own += 1;
                        }
                    }
                }
            }
        }
        println!(
            "aspect={aspect:.2}: keys on screen {total}, hit {any} ({:.1}%), own-set {own} ({:.1}%)",
            100.0 * any as f32 / total.max(1) as f32,
            100.0 * own as f32 / total.max(1) as f32
        );
    }
}

/// Run the calling thread below normal priority: the compositing workers must never take the core the
/// interface thread needs (the window felt frozen while a heavy project composed on every core).
#[cfg(windows)]
pub(crate) fn lower_thread_priority() {
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
    // SAFETY: both calls only act on the calling thread's own pseudo handle
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
pub(crate) fn lower_thread_priority() {}

/// The parallel loops of the compositor: all cores but two (SHARD_THREADS=n overrides), below normal priority.
fn configure_threads() {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let n = std::env::var("SHARD_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| cores.saturating_sub(2).max(2));
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .start_handler(|_| lower_thread_priority())
        .build_global();
}

fn main() {
    configure_threads();
    let args = parse_args();
    if let Some(dir) = &args.export {
        match init_state(&args) {
            Ok(st) => {
                let t = std::time::Instant::now();
                match export::export_all(&st, dir, args.export_res, args.set) {
                    Ok(files) => {
                        for f in &files {
                            println!("wrote {}", f.display());
                        }
                        println!("exported {} file(s) in {:.1}s", files.len(), t.elapsed().as_secs_f32());
                    }
                    Err(e) => eprintln!("export failed: {e}"),
                }
            }
            Err(e) => eprintln!("error: {e}"),
        }
        return;
    }
    if let Some(name) = &args.effect_toggle {
        match init_state(&args) {
            Ok(mut st) => {
                st.wait_jobs();
                if let Err(e) = st.simulate_effect_toggle(name) {
                    eprintln!("effect toggle failed: {e}");
                }
            }
            Err(e) => eprintln!("error: {e}"),
        }
        return;
    }
    if let Some(dir) = &args.dump_masks {
        match init_state(&args) {
            Ok(mut st) => {
                if let Some(name) = &args.mask_paint {
                    match st.simulate_mask_paint(name) {
                        Ok(()) => println!("painted a test stroke into the mask of '{name}'"),
                        Err(e) => eprintln!("mask paint failed: {e}"),
                    }
                }
                match st.dump_masks(dir, args.export_res.unwrap_or(512)) {
                    Ok(n) => println!("dumped {n} mask(s) to {}", dir.display()),
                    Err(e) => eprintln!("dump failed: {e}"),
                }
            }
            Err(e) => eprintln!("error: {e}"),
        }
        return;
    }
    if args.probe_strokes {
        match init_state(&args) {
            Ok(st) => probe_strokes(&st),
            Err(e) => eprintln!("error: {e}"),
        }
        return;
    }
    if args.shot.is_some() {
        if let Err(e) = run_shot(&args) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    let mut args = args;
    // a project with a newer autosave: the question is asked inside the program once it is up
    let mut ask = None;
    if let Some(p) = args.project.clone() {
        if std::env::var_os("SHARD_NO_PROMPT").is_none() && ui::AppState::newer_autosave(&p).is_some() {
            ask = args.project.take();
        }
    }
    let mut st = match init_state(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if let Some(p) = ask {
        st.ask_recovery(p);
    }
    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        st,
        gfx: None,
        mods: ModifiersState::default(),
        focus_changed: std::time::Instant::now(),
        cursor: Vec2::ZERO,
        last_cursor: Vec2::ZERO,
        painting: false,
        left: false,
        right: false,
        right_press: None,
        middle: false,
        prev_ndc: None,
    };
    el.run_app(&mut app).expect("run");
}
