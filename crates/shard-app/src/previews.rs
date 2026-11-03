//! Previews of shelf assets and project resources, like the thumbnails of Painter's shelf.
//!
//! Painter keeps them inside the files: smart materials / masks carry a WebP in their
//! `.alg_meta/preset.bin` (`previews/__DEFAULT_PREVIEW__`), shelf procedurals have a sidecar
//! `.alg_meta/<name>.sbsar` with the same blob, compiled graphs may embed an `icon*.png`, and
//! project resources keep theirs in `resources/.alg_meta/<hash>.sbsar`. Decoding happens on two
//! background threads so scrolling a large shelf never stalls the UI.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use shard_formats::Container;

/// Largest side of a decoded preview.
pub const MAX_SIDE: u32 = 160;

pub struct Preview {
    pub w: usize,
    pub h: usize,
    /// Straight (unmultiplied) RGBA8.
    pub rgba: Vec<u8>,
}

#[derive(Clone)]
pub enum PreviewSource {
    Shelf(PathBuf),
    /// A resource embedded in a project (`/name?version=<sha1>.…`).
    Project { project: PathBuf, resource: String },
}

pub enum Loaded {
    Ready(Option<Preview>),
    /// Dropped from the queue because it was no longer on screen: ask again when it is.
    Skipped,
}

/// First WebP file inside a blob (`RIFF <size> WEBP`).
fn find_webp(blob: &[u8]) -> Option<&[u8]> {
    let mut from = 0;
    while from + 12 <= blob.len() {
        let p = blob[from..].windows(4).position(|w| w == b"RIFF")? + from;
        if p + 12 <= blob.len() && &blob[p + 8..p + 12] == b"WEBP" {
            let size = u32::from_le_bytes([blob[p + 4], blob[p + 5], blob[p + 6], blob[p + 7]]) as usize + 8;
            return blob.get(p..(p + size).min(blob.len()));
        }
        from = p + 4;
    }
    None
}

/// Box-reduce straight RGBA8 so the longest side is at most `max`.
fn fit(w: usize, h: usize, rgba: Vec<u8>, max: u32) -> Preview {
    let max = max as usize;
    if w <= max && h <= max {
        return Preview { w, h, rgba };
    }
    let s = (w.max(h) as f32 / max as f32).max(1.0);
    let (tw, th) = (((w as f32 / s).round() as usize).max(1), ((h as f32 / s).round() as usize).max(1));
    let mut out = vec![0u8; tw * th * 4];
    for y in 0..th {
        let (y0, y1) = (y * h / th, ((y + 1) * h / th).max(y * h / th + 1).min(h));
        for x in 0..tw {
            let (x0, x1) = (x * w / tw, ((x + 1) * w / tw).max(x * w / tw + 1).min(w));
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for yy in (y0..y1).step_by(((y1 - y0) / 4).max(1)) {
                for xx in (x0..x1).step_by(((x1 - x0) / 4).max(1)) {
                    let i = (yy * w + xx) * 4;
                    // weight colour by alpha so transparent texels do not darken the edge
                    let a = rgba[i + 3] as u32;
                    acc[0] += rgba[i] as u32 * a;
                    acc[1] += rgba[i + 1] as u32 * a;
                    acc[2] += rgba[i + 2] as u32 * a;
                    acc[3] += a;
                    n += 1;
                }
            }
            let o = (y * tw + x) * 4;
            if acc[3] > 0 {
                out[o] = (acc[0] / acc[3]) as u8;
                out[o + 1] = (acc[1] / acc[3]) as u8;
                out[o + 2] = (acc[2] / acc[3]) as u8;
                out[o + 3] = (acc[3] / n.max(1)) as u8;
            }
        }
    }
    Preview { w: tw, h: th, rgba: out }
}

fn decode_webp(bytes: &[u8], max: u32) -> Option<Preview> {
    let mut d = image_webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
    let (w, h) = d.dimensions();
    let alpha = d.has_alpha();
    let mut buf = vec![0u8; d.output_buffer_size()?];
    d.read_image(&mut buf).ok()?;
    let rgba = if alpha {
        buf
    } else {
        buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
    };
    Some(fit(w as usize, h as usize, rgba, max))
}

fn decode_png(bytes: &[u8], max: u32) -> Option<Preview> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    if r.info().width as u64 * r.info().height as u64 > 64_000_000 {
        return None;
    }
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
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
    Some(fit(info.width as usize, info.height as usize, rgba, max))
}

fn hash_of(resource: &str) -> Option<String> {
    let v = resource.split("version=").nth(1)?;
    let h: String = v.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (!h.is_empty()).then_some(h)
}

fn shelf(path: &Path, max: u32) -> Option<Preview> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "spsm" | "spmsk" | "sppr" | "spexp" | "spt" => {
            let mut c = Container::open(path).ok()?;
            let blob = c.read(".alg_meta/preset.bin").ok()?;
            decode_webp(find_webp(&blob)?, max)
        }
        "sbsar" => {
            if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
                if let Ok(side) = std::fs::read(dir.join(".alg_meta").join(name)) {
                    if let Some(p) = find_webp(&side).and_then(|w| decode_webp(w, max)) {
                        return Some(p);
                    }
                }
            }
            let bytes = std::fs::read(path).ok()?;
            shard_formats::sbsar::extract_icon(&bytes).and_then(|p| decode_png(&p, max))
        }
        "png" => decode_png(&std::fs::read(path).ok()?, max),
        _ => None,
    }
}

fn project(project: &Path, resource: &str, max: u32) -> Option<Preview> {
    let hash = hash_of(resource)?;
    let mut c = Container::open(project).ok()?;
    for member in [format!("resources/.alg_meta/{hash}.sbsar"), format!("resources/.alg_meta/{hash}.image")] {
        if let Ok(blob) = c.read(&member) {
            if let Some(p) = find_webp(&blob).and_then(|w| decode_webp(w, max)) {
                return Some(p);
            }
        }
    }
    if let Ok(bytes) = c.read(&format!("resources/{hash}.sbsar")) {
        if let Some(p) = shard_formats::sbsar::extract_icon(&bytes).and_then(|p| decode_png(&p, max)) {
            return Some(p);
        }
    }
    // bitmap resource: its own pixels
    let bytes = c.read(&format!("resources/{hash}.image")).ok()?;
    let img = shard_formats::image::decode(&bytes).ok()?;
    let f = img.to_rgba_f32().ok()?;
    let gray = img.format == shard_formats::image::PixelFormat::Lum8;
    let rgba: Vec<u8> = f
        .chunks_exact(4)
        .flat_map(|p| {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            if gray {
                [q(p[0]), q(p[0]), q(p[0]), 255]
            } else {
                [q(p[0]), q(p[1]), q(p[2]), 255]
            }
        })
        .collect();
    Some(fit(img.width as usize, img.height as usize, rgba, max))
}

pub fn load(src: &PreviewSource, max: u32) -> Option<Preview> {
    match src {
        PreviewSource::Shelf(p) => shelf(p, max),
        PreviewSource::Project { project: p, resource } => project(p, resource, max),
    }
}

struct Queue {
    stack: Vec<(String, PreviewSource)>,
    /// Frame at which each queued key was last asked for.
    wanted: HashMap<String, u64>,
}

/// Background loader: newest request first, stale (scrolled-away) requests are skipped.
pub struct PreviewLoader {
    q: Arc<(Mutex<Queue>, Condvar)>,
    frame: Arc<AtomicU64>,
    rx: Receiver<(String, Loaded)>,
}

impl PreviewLoader {
    pub fn new() -> Self {
        let q = Arc::new((Mutex::new(Queue { stack: Vec::new(), wanted: HashMap::new() }), Condvar::new()));
        let frame = Arc::new(AtomicU64::new(0));
        let (tx, rx): (Sender<(String, Loaded)>, _) = channel();
        for _ in 0..2 {
            let (q, frame, tx) = (q.clone(), frame.clone(), tx.clone());
            std::thread::spawn(move || loop {
                crate::lower_thread_priority();
                let job = {
                    let (m, cv) = &*q;
                    let mut g = m.lock().unwrap();
                    loop {
                        if let Some(j) = g.stack.pop() {
                            let seen = g.wanted.remove(&j.0).unwrap_or(0);
                            break (j, seen);
                        }
                        g = cv.wait(g).unwrap();
                    }
                };
                let ((key, src), seen) = job;
                let msg = if frame.load(Ordering::Relaxed).saturating_sub(seen) > 3 {
                    Loaded::Skipped
                } else {
                    Loaded::Ready(load(&src, MAX_SIDE))
                };
                if tx.send((key, msg)).is_err() {
                    return;
                }
            });
        }
        Self { q, frame, rx }
    }

    /// Call once per UI frame.
    pub fn next_frame(&self) {
        self.frame.fetch_add(1, Ordering::Relaxed);
    }

    /// Ask for `key`; `first` = it was never requested before (otherwise only its "still wanted"
    /// stamp is refreshed).
    pub fn want(&self, key: &str, src: impl FnOnce() -> PreviewSource, first: bool) {
        let now = self.frame.load(Ordering::Relaxed);
        let (m, cv) = &*self.q;
        let mut g = m.lock().unwrap();
        if first {
            g.stack.push((key.to_string(), src()));
            cv.notify_one();
        }
        g.wanted.insert(key.to_string(), now);
    }

    pub fn poll(&self, max: usize) -> Vec<(String, Loaded)> {
        let mut out = Vec::new();
        while out.len() < max {
            match self.rx.try_recv() {
                Ok(x) => out.push(x),
                Err(_) => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHARD_PREVIEW_FILES=<path>;<path>...: print what the loader finds for each file.
    #[test]
    fn probe_previews() {
        let Ok(list) = std::env::var("SHARD_PREVIEW_FILES") else { return };
        for f in list.split(';').filter(|s| !s.is_empty()) {
            let t = std::time::Instant::now();
            match load(&PreviewSource::Shelf(PathBuf::from(f)), MAX_SIDE) {
                Some(p) => {
                    let n = (p.w * p.h) as f64;
                    let avg: Vec<f64> = (0..4).map(|c| p.rgba.chunks_exact(4).map(|q| q[c] as f64).sum::<f64>() / n).collect();
                    println!("{f}: {}x{} avg {:.0?} in {:?}", p.w, p.h, avg, t.elapsed());
                }
                None => println!("{f}: no preview ({:?})", t.elapsed()),
            }
        }
    }
}
