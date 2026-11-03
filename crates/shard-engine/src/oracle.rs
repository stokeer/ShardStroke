//! Optional external Substance renderer.
//!
//! Procedural graphs (`.sbsar`) are compiled and their noise sources are not documented. When the user has
//! Substance 3D Designer installed, its command-line renderer (`sbsrender`) can evaluate any graph exactly.
//! This module runs it as a black box: graph bytes + parameter values + image inputs in, picture out. It is
//! the personal-use backend described in docs/FEATURES.md ("Motor procedural"); the native evaluation in
//! `shard-compose` is the fallback and is validated against this one (`tools/oracle.py`).
//!
//! Results are cached on disk by a hash of everything that influences them, so a slider that returns to an
//! earlier value, or a project reopened later, costs nothing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use shard_compose::Image;

const CANDIDATES: [&str; 2] = [
    r"C:\Program Files\Adobe\Adobe Substance 3D Designer\sbsrender.exe",
    r"C:\Program Files\Adobe\Adobe Substance 3D Painter\sbsrender.exe",
];

/// What to render.
pub struct Request {
    /// The `.sbsar` archive bytes.
    pub sbsar: Vec<u8>,
    /// Output identifier (`output`, `basecolor`...); `None` renders the first output.
    pub output: Option<String>,
    /// Numeric inputs: `("balance", "0.3")`, `("$outputsize", "11,11")` (log2), `("$randomseed", "5")`.
    pub values: Vec<(String, String)>,
    /// Image inputs: identifier and picture.
    pub inputs: Vec<(String, Arc<Image>)>,
    /// Side of the output as a power of two (log2).
    pub size_log2: u32,
    /// Graph name, for diagnostics.
    pub label: String,
}

pub struct Oracle {
    exe: PathBuf,
    cache: PathBuf,
    /// Renders already decoded, by request key: a compose asks for the same graph output several times (mask,
    /// content, every channel) and decoding the 16-bit PNG from the disk cache each time dominated it.
    mem: std::sync::Mutex<MemCache>,
}

/// Decoded renders kept in memory, least recently used out first.
#[derive(Default)]
struct MemCache {
    map: std::collections::HashMap<u64, (Arc<Image>, u64)>,
    bytes: usize,
    tick: u64,
}

/// Budget of the in-memory render cache (bytes of float data).
const MEM_BUDGET: usize = 512 << 20;

impl MemCache {
    fn get(&mut self, key: u64) -> Option<Arc<Image>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&key).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }

    fn put(&mut self, key: u64, img: Arc<Image>) {
        let size = img.data.len() * 4;
        if size > MEM_BUDGET / 2 {
            return;
        }
        self.tick += 1;
        if let Some(old) = self.map.insert(key, (img, self.tick)) {
            self.bytes -= old.0.data.len() * 4;
        }
        self.bytes += size;
        while self.bytes > MEM_BUDGET {
            let Some((&oldest, _)) = self.map.iter().min_by_key(|(_, v)| v.1) else { break };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.0.data.len() * 4;
            }
        }
    }
}

/// Hash of an image input by identity: the baked mesh maps are the same `Arc` for every request of a compose
/// and hashing a million floats per request added up. The weak pointer keeps the address from being reused
/// by another image while the entry exists.
fn image_hash_shared(img: &Arc<Image>) -> u64 {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<usize, (std::sync::Weak<Image>, u64)>>> =
        std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let ptr = Arc::as_ptr(img) as usize;
    if let Ok(m) = memo.lock() {
        if let Some((w, h)) = m.get(&ptr) {
            if w.upgrade().is_some_and(|x| Arc::ptr_eq(&x, img)) {
                return *h;
            }
        }
    }
    let h = image_hash(img);
    if let Ok(mut m) = memo.lock() {
        if m.len() > 256 {
            m.retain(|_, (w, _)| w.strong_count() > 0);
        }
        m.insert(ptr, (Arc::downgrade(img), h));
    }
    h
}

static ORACLE: OnceLock<Option<Oracle>> = OnceLock::new();
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Turn the external renderer on or off at run time (Settings > General).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Path of the renderer when it is installed (regardless of the on/off switch).
pub fn installed_path() -> Option<PathBuf> {
    ORACLE.get_or_init(init).as_ref().map(|o| o.exe.clone())
}

/// Size of the render cache in bytes.
pub fn cache_bytes() -> u64 {
    let Some(o) = ORACLE.get_or_init(init).as_ref() else { return 0 };
    std::fs::read_dir(&o.cache)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Delete cached renders and inputs (graphs are kept: they are small and reused).
pub fn clear_cache() {
    let Some(o) = ORACLE.get_or_init(init).as_ref() else { return };
    if let Ok(rd) = std::fs::read_dir(&o.cache) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with('r') || n.starts_with('i') || n.starts_with('t') {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn init() -> Option<Oracle> {
    if std::env::var_os("SHARD_NO_ORACLE").is_some() {
        return None;
    }
    let exe = std::env::var_os("SBSRENDER")
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| CANDIDATES.iter().map(PathBuf::from).find(|p| p.exists()))?;
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShardStroke")
        .join("cache")
        .join("oracle");
    std::fs::create_dir_all(&base).ok()?;
    Some(Oracle { exe, cache: base, mem: Default::default() })
}

/// The renderer, when installed and not disabled (`SHARD_NO_ORACLE=1`).
pub fn get() -> Option<&'static Oracle> {
    if !is_enabled() {
        return None;
    }
    ORACLE.get_or_init(init).as_ref()
}

fn hash_of(parts: &[&[u8]]) -> u64 {
    let mut h = DefaultHasher::new();
    for p in parts {
        p.len().hash(&mut h);
        p.hash(&mut h);
    }
    h.finish()
}

fn image_hash(img: &Image) -> u64 {
    let mut h = DefaultHasher::new();
    (img.w, img.h, img.n).hash(&mut h);
    // sampling every 7th value is plenty to tell different pictures apart and keeps this cheap
    for v in img.data.iter().step_by(7) {
        v.to_bits().hash(&mut h);
    }
    h.finish()
}

fn write_png16(path: &Path, img: &Image) -> Result<(), String> {
    let (color, n) = match img.n {
        1 => (png::ColorType::Grayscale, 1),
        2 => (png::ColorType::GrayscaleAlpha, 2),
        3 => (png::ColorType::Rgb, 3),
        _ => (png::ColorType::Rgba, 4),
    };
    let mut bytes = Vec::with_capacity((img.w * img.h) as usize * n * 2);
    for px in img.data.chunks_exact(img.n) {
        for c in 0..n {
            let v = (px[c.min(img.n - 1)].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            bytes.extend_from_slice(&v.to_be_bytes());
        }
    }
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.w, img.h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Sixteen);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&bytes).map_err(|e| e.to_string())
}

/// 8 or 16-bit PNG as floats 0..1: gray stays one component, colour keeps RGB (alpha dropped).
pub fn read_png(path: &Path) -> Result<Image, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let ch = info.color_type.samples();
    let bytes_per = if info.bit_depth == png::BitDepth::Sixteen { 2 } else { 1 };
    let (w, h) = (info.width, info.height);
    let n = if ch <= 2 { 1 } else { 3 };
    let mut data = Vec::with_capacity((w * h) as usize * n);
    let val = |i: usize| -> f32 {
        if bytes_per == 2 {
            u16::from_be_bytes([buf[i], buf[i + 1]]) as f32 / 65535.0
        } else {
            buf[i] as f32 / 255.0
        }
    };
    for p in 0..(w * h) as usize {
        let base = p * ch * bytes_per;
        for c in 0..n {
            data.push(val(base + c * bytes_per));
        }
    }
    Ok(Image { w, h, n, data })
}

impl Oracle {
    fn graph_file(&self, sbsar: &[u8]) -> Option<PathBuf> {
        let key = hash_of(&[sbsar]);
        let p = self.cache.join(format!("g{key:016x}.sbsar"));
        if !p.exists() {
            let tmp = self.cache.join(format!("g{key:016x}.{}.tmp", std::process::id()));
            std::fs::write(&tmp, sbsar).ok()?;
            std::fs::rename(&tmp, &p).ok()?;
        }
        Some(p)
    }

    /// Render a graph; `None` when it fails (the caller falls back to its own evaluation).
    pub fn render(&self, req: &Request) -> Option<Image> {
        self.render_shared(req).map(|a| Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()))
    }

    /// [`Self::render`] without copying: renders are cached decoded, in memory, within a budget.
    pub fn render_shared(&self, req: &Request) -> Option<Arc<Image>> {
        let key = self.request_key(req);
        if let Some(hit) = self.mem.lock().ok().and_then(|mut m| m.get(key)) {
            return Some(hit);
        }
        let img = Arc::new(self.render_disk(req, key)?);
        if let Ok(mut m) = self.mem.lock() {
            m.put(key, img.clone());
        }
        Some(img)
    }

    /// Hash of everything that influences the render (the name of its file in the disk cache).
    fn request_key(&self, req: &Request) -> u64 {
        let output = req.output.as_deref();
        let mut parts: Vec<Vec<u8>> = vec![req.sbsar.len().to_be_bytes().to_vec()];
        parts.push(hash_of(&[&req.sbsar]).to_be_bytes().to_vec());
        parts.push(output.unwrap_or("").as_bytes().to_vec());
        parts.push(req.size_log2.to_be_bytes().to_vec());
        let mut values = req.values.clone();
        values.sort();
        for (k, v) in &values {
            parts.push(format!("{k}={v}").into_bytes());
        }
        for (k, img) in &req.inputs {
            parts.push(format!("{k}:{:016x}", image_hash_shared(img)).into_bytes());
        }
        let refs: Vec<&[u8]> = parts.iter().map(|v| v.as_slice()).collect();
        hash_of(&refs)
    }

    fn render_disk(&self, req: &Request, key: u64) -> Option<Image> {
        let output = req.output.as_deref();
        let mut values = req.values.clone();
        values.sort();
        let out = self.cache.join(format!("r{key:016x}.png"));
        if let Ok(img) = read_png(&out) {
            if std::env::var_os("SHARD_TRACE_ORACLE").is_some() {
                eprintln!(
                    "[oracle] cached {} {} -> {} inputs={:?}",
                    req.label,
                    output.unwrap_or("output"),
                    req.size_log2,
                    req.inputs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>()
                );
            }
            return Some(img);
        }
        let graph = self.graph_file(&req.sbsar)?;
        // image inputs go to files next to the result
        let mut entry_args: Vec<String> = Vec::new();
        for (k, img) in &req.inputs {
            let p = self.cache.join(format!("i{:016x}.png", image_hash_shared(img) ^ hash_of(&[k.as_bytes()])));
            if !p.exists() {
                let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
                write_png16(&tmp, img).ok()?;
                std::fs::rename(&tmp, &p).ok()?;
            }
            entry_args.push("--set-entry".into());
            entry_args.push(format!("{k}@{}", p.display()));
        }
        let tmp_name = format!("t{key:016x}_{}", std::process::id());
        let mut cmd = std::process::Command::new(&self.exe);
        // below normal priority, without a console window: the renders run while the user works (a file dialog
        // listing a big folder was left "Working on it..." behind several of them)
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
        }
        cmd.args(["--quiet", "render", "--input"])
            .arg(&graph)
            .arg("--output-path")
            .arg(&self.cache)
            .args(["--output-name", &tmp_name, "--output-format", "png", "--output-bit-depth", "16", "--no-report"]);
        if let Some(o) = output {
            cmd.args(["--input-graph-output", o]);
        }
        if !values.iter().any(|(k, _)| k == "$outputsize") {
            cmd.args(["--set-value", &format!("$outputsize@{0},{0}", req.size_log2)]);
        }
        for (k, v) in &values {
            cmd.args(["--set-value", &format!("{k}@{v}")]);
        }
        cmd.args(&entry_args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let t0 = std::time::Instant::now();
        let status = cmd.output().ok()?;
        let produced = self.cache.join(format!("{tmp_name}.png"));
        if !produced.exists() && std::env::var_os("SHARD_TRACE_RENDER").is_some() {
            eprintln!(
                "[oracle] {} {} failed: {:?}
  stdout: {}
  stderr: {}
  values: {:?}",
                req.label,
                output.unwrap_or("output"),
                status.status,
                String::from_utf8_lossy(&status.stdout).trim(),
                String::from_utf8_lossy(&status.stderr).trim(),
                values
            );
        }
        if std::env::var_os("SHARD_TRACE").is_some() {
            eprintln!(
                "[oracle] {} {} -> {} in {:?} (ok={})",
                req.label,
                output.unwrap_or("output"),
                req.size_log2,
                t0.elapsed(),
                status.status.success() && produced.exists()
            );
        }
        if !produced.exists() {
            return None;
        }
        let _ = std::fs::rename(&produced, &out);
        read_png(&out).ok()
    }
}
