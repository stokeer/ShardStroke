//! Bitmap resources of a project (`resources/<hash>.image`) for the compositor.
//!
//! The document references a bitmap by URL (`/Name?version=<sha1>.image`); the embedded resource is
//! stored as `resources/<sha1>.image`. Images are decoded on first use, reduced with a 2x2 box filter
//! until they fit `max_dim`, and cached.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use shard_compose::{BitmapProvider, Image};
use shard_formats::image::{self, PixelFormat};
use shard_formats::Container;

pub struct ProjectBitmaps {
    container: RefCell<Container>,
    cache: RefCell<HashMap<String, Option<Arc<Image>>>>,
    max_dim: u32,
    key: String,
}

/// Decoded bitmaps shared by every worker and every job (decoding big JP2 bakes is the slow part of
/// a mask preview): `(project, max_dim, resource)`.
type SharedKey = (String, u32, String);
static SHARED: std::sync::OnceLock<std::sync::Mutex<HashMap<SharedKey, Option<Arc<Image>>>>> =
    std::sync::OnceLock::new();

impl ProjectBitmaps {
    pub fn open(path: &Path, max_dim: u32) -> Option<Self> {
        Some(Self {
            container: RefCell::new(Container::open(path).ok()?),
            cache: RefCell::new(HashMap::new()),
            max_dim,
            key: path.to_string_lossy().into_owned(),
        })
    }

    /// Does the project blend in ACEScg? `colorManagementWorkflow` 1 is OCIO; with the ACES config its working
    /// space is ACEScg (a test tank), while 2 (Adobe colour engine) works in linear sRGB. Read once per file;
    /// `SHARD_WORKING=srgb|aces` overrides it.
    pub fn aces_working(&self) -> bool {
        match std::env::var("SHARD_WORKING").ok().as_deref() {
            Some("srgb") => return false,
            Some("aces") => return true,
            _ => {}
        }
        static FLAGS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, bool>>> = std::sync::OnceLock::new();
        let flags = FLAGS.get_or_init(Default::default);
        if let Some(v) = flags.lock().ok().and_then(|m| m.get(&self.key).copied()) {
            return v;
        }
        let read = || -> Option<bool> {
            let root = self.container.borrow_mut().decode("paint/document.bin").ok()?;
            let o = root.as_object()?;
            if o.get("colorManagementWorkflow")?.as_i64()? != 1 {
                return Some(false);
            }
            let cfg = o
                .get("colorManagementOCIO")
                .and_then(|v| v.as_object())
                .and_then(|c| c.get("colorConfigFilepath"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            Some(cfg.contains("aces"))
        };
        let v = read().unwrap_or(false);
        if let Ok(mut m) = flags.lock() {
            m.insert(self.key.clone(), v);
        }
        v
    }

    /// Project file and size class of this provider (what its decoded bitmaps depend on).
    pub fn identity(&self) -> (&str, u32) {
        (&self.key, self.max_dim)
    }

    /// Decode several resources at once into the shared cache (each worker opens its own container: this
    /// provider is single-threaded). Baked mesh maps are big JP2 / PNG files, a second or more in sequence.
    pub fn preload(&self, resources: &[String]) {
        let shared = SHARED.get_or_init(Default::default);
        let todo: Vec<&String> = {
            let Ok(m) = shared.lock() else { return };
            resources
                .iter()
                .filter(|r| !m.contains_key(&(self.key.clone(), self.max_dim, (*r).clone())) && !self.cache.borrow().contains_key(*r))
                .collect()
        };
        if todo.len() < 2 {
            return;
        }
        let path = std::path::PathBuf::from(&self.key);
        let max_dim = self.max_dim;
        std::thread::scope(|s| {
            for r in todo {
                let path = &path;
                s.spawn(move || {
                    if let Some(p) = ProjectBitmaps::open(path, max_dim) {
                        let _ = p.bitmap(r);
                    }
                });
            }
        });
    }

    /// Every `.image` resource URL listed in the project's `resources.toc` (UTF-16BE strings).
    pub fn resource_urls(&self) -> Vec<String> {
        let Ok(bytes) = self.container.borrow_mut().read("resources.toc") else { return Vec::new() };
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16_lossy(&units);
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        for ch in text.chars().chain(std::iter::once('\0')) {
            if ch == ' ' || ch.is_ascii_graphic() {
                cur.push(ch);
            } else {
                if cur.starts_with('/') && cur.contains("?version=") && !out.contains(&cur) {
                    out.push(cur.clone());
                }
                cur.clear();
            }
        }
        out
    }

    /// Parameter interface of a procedural resource (the `.sbsar` embedded in the project).
    pub fn sbsar_interface(&self, resource: &str) -> Option<shard_formats::sbsar::Interface> {
        if let Some(path) = resource.strip_prefix("file:") {
            let bytes = std::fs::read(path).ok()?;
            return shard_formats::sbsar::Interface::from_sbsar(&bytes).ok();
        }
        let member = Self::member_for(resource)?;
        let member = format!("{}.sbsar", member.trim_end_matches(".image"));
        let bytes = self.container.borrow_mut().read(&member).ok()?;
        shard_formats::sbsar::Interface::from_sbsar(&bytes).ok()
    }

    fn member_for(resource: &str) -> Option<String> {
        let v = resource.split("version=").nth(1)?;
        let hash: String = v.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        (!hash.is_empty()).then(|| format!("resources/{hash}.image"))
    }

    /// Decode `resource` at up to `max_dim` without touching any cache (one-off, large uploads).
    pub fn bitmap_full(&self, resource: &str, max_dim: u32) -> Option<Image> {
        self.load_with(resource, max_dim)
    }

    fn load(&self, resource: &str) -> Option<Image> {
        self.load_with(resource, self.max_dim)
    }

    /// The `.sbsar` archive of a procedural resource (project member or shelf file).
    pub fn sbsar_bytes(&self, resource: &str) -> Option<Vec<u8>> {
        if let Some(path) = resource.strip_prefix("file:") {
            return std::fs::read(path).ok();
        }
        let member = Self::member_for(resource)?;
        let member = format!("{}.sbsar", member.trim_end_matches(".image"));
        self.container.borrow_mut().read(&member).ok()
    }

    /// Bitmap `index` embedded in the compiled graph of a procedural resource (`<resource>#e<index>`).
    fn load_embedded(&self, resource: &str, index: usize, max_dim: u32) -> Option<Image> {
        let bytes = if let Some(path) = resource.strip_prefix("file:") {
            std::fs::read(path).ok()?
        } else {
            let member = Self::member_for(resource)?;
            let member = format!("{}.sbsar", member.trim_end_matches(".image"));
            self.container.borrow_mut().read(&member).ok()?
        };
        let imgs = shard_formats::sbsar::embedded_images(&bytes);
        let e = imgs.get(index)?;
        let mut out = Image { w: e.w, h: e.h, n: e.comps, data: e.data.iter().map(|b| *b as f32 / 255.0).collect() };
        while out.w > max_dim || out.h > max_dim {
            out = halve(&out);
        }
        Some(out)
    }

    fn load_with(&self, resource: &str, max_dim: u32) -> Option<Image> {
        if let Some((base, idx)) = resource.rsplit_once("#e") {
            if let Ok(i) = idx.parse::<usize>() {
                return self.load_embedded(base, i, max_dim);
            }
        }
        if let Some(path) = resource.strip_prefix("file:") {
            return load_png_file(path, max_dim);
        }
        let member = Self::member_for(resource)?;
        // some resources (Painter-baked mesh maps) are stored under the bare hash, without `.image`
        let bare = member.trim_end_matches(".image").to_string();
        let bytes = {
            let mut c = self.container.borrow_mut();
            c.read(&member).or_else(|_| c.read(&bare)).ok()?
        };
        let img = image::decode(&bytes).ok()?;
        drop(bytes);
        let rgba = img.to_rgba_f32().ok()?;
        if matches!(img.format, PixelFormat::Lum16) {
            if let Ok(mut w) = WIDE.get_or_init(Default::default).lock() {
                w.insert(resource.to_string());
            }
        }
        let n = if img.format == PixelFormat::Lum8 { 1 } else { 3 };
        let mut data = Vec::with_capacity((img.width * img.height) as usize * n);
        for px in rgba.chunks_exact(4) {
            data.extend_from_slice(&px[..n]);
        }
        let mut out = Image { w: img.width, h: img.height, n, data };
        while out.w > max_dim || out.h > max_dim {
            out = halve(&out);
        }
        Some(out)
    }
}

/// Resources decoded from a 16-bit grey image (Painter's own bakes).
static WIDE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();

impl BitmapProvider for ProjectBitmaps {
    /// A 16-bit grey bitmap (Painter's baked curvature, ...) filling a colour channel is sRGB-encoded: a test pistol's and
    /// a test tank's `Curvalture` (the baked curvature in Overlay) read raw lit the whole set (base colour 11.7 ->
    /// 4.7, 16.7 -> 4.2); the 8-bit imported maps of a test vehicle are raw.
    fn colour_encoded(&self, resource: &str) -> bool {
        WIDE.get().and_then(|w| w.lock().ok()).is_some_and(|w| w.contains(resource))
    }

    fn bitmap(&self, resource: &str) -> Option<Arc<Image>> {
        if let Some(hit) = self.cache.borrow().get(resource) {
            return hit.clone();
        }
        let shared = SHARED.get_or_init(Default::default);
        let k = (self.key.clone(), self.max_dim, resource.to_string());
        if let Some(hit) = shared.lock().ok().and_then(|m| m.get(&k).cloned()) {
            self.cache.borrow_mut().insert(resource.to_string(), hit.clone());
            return hit;
        }
        let loaded = self.load(resource).map(Arc::new);
        if let Ok(mut m) = shared.lock() {
            m.insert(k, loaded.clone());
        }
        self.cache.borrow_mut().insert(resource.to_string(), loaded.clone());
        loaded
    }
}

fn halve(i: &Image) -> Image {
    let (w, h) = ((i.w / 2).max(1), (i.h / 2).max(1));
    let mut data = Vec::with_capacity((w * h) as usize * i.n);
    for y in 0..h {
        for x in 0..w {
            for c in 0..i.n {
                let mut acc = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(i.w - 1);
                    let sy = (y * 2 + dy).min(i.h - 1);
                    acc += i.data[((sy * i.w + sx) as usize) * i.n + c];
                }
                data.push(acc * 0.25);
            }
        }
    }
    Image { w, h, n: i.n, data }
}

/// A PNG from the shelf as a float image: grayscale stays one component, colour becomes linear RGB.
fn load_png_file(path: &str, max_dim: u32) -> Option<Image> {
    let f = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    let ch = info.color_type.samples();
    let (w, h) = (info.width, info.height);
    let gray = ch <= 2;
    let n = if gray { 1 } else { 3 };
    let mut data = Vec::with_capacity((w * h) as usize * n);
    for p in buf[..info.buffer_size()].chunks_exact(ch) {
        if gray {
            data.push(p[0] as f32 / 255.0);
        } else {
            for c in 0..3 {
                data.push(shard_compose::srgb_to_linear(p[c] as f32 / 255.0));
            }
        }
    }
    let mut out = Image { w, h, n, data };
    while out.w > max_dim || out.h > max_dim {
        out = halve(&out);
    }
    Some(out)
}
