//! Native evaluation of compiled procedural graphs (`shard_formats::sbam`): the node network runs here instead of
//! in the external engine. Node kinds are added one by one, each measured against the external engine
//! (`docs/SBAM.md`); a graph with a kind not yet known is refused ([`run`] returns an error) so the caller falls back.

use std::collections::HashMap;
use std::rc::Rc;

use shard_formats::sbam::{eval_function, Node, Params, Sbam};

use crate::Image;

const NODE_BASE: usize = 0x34;
/// Output-size reference that ends every node's parameter area.
const OUTPUT_SIZE: u32 = 0x0a42_0001;

/// Interface values and image inputs of one evaluation.
pub struct Inputs<'a> {
    pub params: Params,
    /// Image inputs by interface `uid` (grey: `n` = 1).
    pub images: HashMap<u32, &'a Image>,
    pub w: u32,
    pub h: u32,
}

/// One parameter slot of a node: absent (default), an inline constant, or a function.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Default,
    Const(f32),
    Func(usize),
}

/// Evaluate the graph; returns the image of the last node (the graph's `output`). Lazy: only the nodes the output
/// depends on run, and a switch evaluates the branch it picks only (whole branches of the mask generators sit
/// behind switches that their defaults turn off).
pub fn run(g: &Sbam, inp: &Inputs) -> Result<Image, String> {
    let last = g.nodes.len().checked_sub(1).ok_or("no nodes")?;
    let mut ev = Lazy { g, inp, memo: vec![None; g.nodes.len()], stack: Vec::new() };
    ev.input(last as u32).map(|i| (*i).clone())
}

struct Lazy<'a> {
    g: &'a Sbam,
    inp: &'a Inputs<'a>,
    memo: Vec<Option<Rc<Image>>>,
    stack: Vec<usize>,
}

impl Lazy<'_> {
    fn input(&mut self, idx: u32) -> Result<Rc<Image>, String> {
        let k = idx as usize;
        if let Some(Some(v)) = self.memo.get(k) {
            return Ok(v.clone());
        }
        let nd = self.g.nodes.get(k).ok_or_else(|| format!("no node {k}"))?;
        if self.stack.contains(&k) {
            return Err(format!("cycle at node {k}"));
        }
        self.stack.push(k);
        let r = eval_node(self.g, nd, self, self.inp).map_err(|e| {
            if e.starts_with("node ") {
                e
            } else {
                format!("node {k} (class {:#04x}): {e}", nd.class)
            }
        });
        self.stack.pop();
        let v = Rc::new(r?.ok_or_else(|| format!("node {k} has no image"))?);
        self.memo[k] = Some(v.clone());
        Ok(v)
    }
}

/// The node's resolution: byte 1 of its type word holds log2 offsets (8 = the graph's size) for width and height.
fn node_size(nd: &Node, inp: &Inputs) -> (u32, u32) {
    let t = nd.words[0];
    let rel = |nib: u32| -> i32 { nib as i32 - 8 };
    let sz = |base: u32, r: i32| -> u32 { if r >= 0 { base << r } else { (base >> (-r)).max(1) } };
    (sz(inp.w, rel((t >> 12) & 0xF)), sz(inp.h, rel((t >> 8) & 0xF)))
}

/// The slots of a node whose flags hold 2 bits per parameter (00 default, 01 inline constant, 10 function);
/// constants follow the offset words in order, functions are reached by their offsets.
fn slots(nd: &Node, flags: u32, count: usize, offs_from: usize) -> Result<(Vec<Slot>, usize), String> {
    let kinds: Vec<u32> = (0..count).map(|i| (flags >> (2 * i)) & 3).collect();
    let nfunc = kinds.iter().filter(|k| **k == 2).count();
    // offsets: the output-size reference, then one per function
    let mut fo = nd.words.get(offs_from + 1..offs_from + 1 + nfunc).ok_or("truncated offsets")?.iter();
    let mut ci = offs_from + 1 + nfunc;
    let mut out = Vec::with_capacity(count);
    for k in kinds {
        out.push(match k {
            0 => Slot::Default,
            1 => {
                let v = f32::from_bits(*nd.words.get(ci).ok_or("truncated constant")?);
                ci += 1;
                Slot::Const(v)
            }
            2 => Slot::Func(NODE_BASE + *fo.next().unwrap() as usize),
            _ => return Err(format!("parameter kind 3 in flags {flags:#x}")),
        });
    }
    Ok((out, ci))
}

fn value(g: &Sbam, s: Slot, default: f32, p: &Params) -> Result<f32, String> {
    Ok(match s {
        Slot::Default => default,
        Slot::Const(v) => v,
        Slot::Func(o) => eval_function(g.words_at(o), p)?.0,
    })
}

fn eval_node(g: &Sbam, nd: &Node, ev: &mut Lazy, inp: &Inputs) -> Result<Option<Image>, String> {
    let w = |i: usize| nd.words.get(i).copied().ok_or_else(|| format!("truncated at word {i}"));
    let (cw, ch) = node_size(nd, inp);
    match nd.class {
        // image input: [type][uid]
        0x20 | 0x21 => {
            let uid = w(1)?;
            Ok(match inp.images.get(&uid) {
                Some(img) => Some(fit(&resize(&Rc::new((*img).clone()), cw, ch), nd.class & 1 == 1)),
                None => None,
            })
        }
        // uniform colour: [type][grey | rgba]
        0x0c => Ok(Some(Image::filled(cw, ch, &[f32::from_bits(w(1)?)]))),
        0x0d => Ok(Some(Image::filled(cw, ch, &[f32::from_bits(w(1)?), f32::from_bits(w(2)?), f32::from_bits(w(3)?)]))),
        // levels: [type][flags][input][output-size offset][function offsets...][constants...]
        0x1e | 0x1f => {
            let src = &*resize(&ev.input(w(2)?)?, cw, ch);
            let (s, _) = slots(nd, w(1)?, 5, 3)?;
            let v = |i: usize, d: f32| value(g, s[i], d, &inp.params);
            let (ilo, ihi, mid, olo, ohi) = (v(0, 0.0)?, v(1, 1.0)?, v(2, 0.5)?, v(3, 0.0)?, v(4, 1.0)?);
            if (mid - 0.5).abs() > 1e-6 {
                return Err("levels with a mid-tone not measured yet".into());
            }
            let span = if (ihi - ilo).abs() < 1e-9 { 1e-9 } else { ihi - ilo };
            let mut out = src.clone();
            for x in out.data.iter_mut() {
                *x = olo + (ohi - olo) * ((*x - ilo) / span).clamp(0.0, 1.0);
            }
            Ok(Some(out))
        }
        // sharpen: [type][input][output-size offset][intensity function offset] (version-dependent: the input
        // index is the first small word)
        0x1a | 0x1b => {
            let src = &*resize(&ev.input(w(1)?)?, cw, ch);
            let k = match nd.words.iter().position(|x| *x == 0x0902_0001 || (*x >> 16) == 0x0902) {
                Some(at) => eval_function(&nd.words[at..], &inp.params)?.0,
                None => 1.0,
            };
            Ok(Some(sharpen(src, k)))
        }
        // directional blur: [type][flags: intensity, angle (turns)][input][output-size offset][...]; the length in
        // texels is 8.3 x intensity at 1024 (fitted on `highpass` noise responses, Radius 0.4 - 3.2: 0.35-0.87 of
        // 8-bit error against the external engine; Radius 0.2 still off, docs/SBAM.md)
        0x16 | 0x17 => {
            let src = &*resize(&ev.input(w(2)?)?, cw, ch);
            let (s, _) = slots(nd, w(1)?, 2, 3)?;
            let intensity = value(g, s[0], 0.0, &inp.params)?;
            let turns = value(g, s[1], 0.0, &inp.params)?;
            Ok(Some(directional_blur(src, intensity * 8.3 * src.w as f32 / 1024.0, turns)))
        }
        // blend: [type][flags: mode in the low nibble, opacity kind in bits 4-5][background][foreground][...]
        0x02 | 0x03 => {
            let flags = w(1)?;
            let op_kind = (flags >> 4) & 3;
            let opacity = match op_kind {
                0 => 1.0,
                1 => f32::from_bits(w(5)?),
                2 => eval_function(g.words_at(NODE_BASE + w(5)? as usize), &inp.params)?.0,
                _ => return Err(format!("blend flags {flags:#x}")),
            };
            let mode = flags & 0xF;
            if mode == 7 {
                // switch: only the picked input is evaluated
                let pick = if opacity >= 0.5 { w(3)? } else { w(2)? };
                return Ok(Some((*resize(&ev.input(pick)?, cw, ch)).clone()));
            }
            let (bg, fg) = (resize(&ev.input(w(2)?)?, cw, ch), resize(&ev.input(w(3)?)?, cw, ch));
            let (bg, fg) = (&*bg, &*fg);
            let n = bg.n.max(fg.n);
            let (bg, fg) = (widen(bg, n), widen(fg, n));
            let mut out = bg.clone();
            for (o, f) in out.data.iter_mut().zip(&fg.data) {
                *o = match mode {
                    0 => *o + (f - *o) * opacity,                          // copy
                    4 => (*o + (2.0 * f - 1.0) * opacity).clamp(0.0, 1.0), // add-sub
                    m => return Err(format!("blend mode {m} not measured yet")),
                };
            }
            Ok(Some(out))
        }
        c => Err(format!("node class {c:#04x} not implemented")),
    }
    .map(|v| {
        let _ = OUTPUT_SIZE;
        v
    })
}

/// Mean of samples spread evenly along a segment of `length` texels at `turns` of a full turn, bilinear,
/// wrapping.
fn directional_blur(src: &Image, length: f32, turns: f32) -> Image {
    if length <= 0.0 {
        return src.clone();
    }
    let (w, h, n) = (src.w as i64, src.h as i64, src.n);
    let samples = ((length * 2.0).ceil() as usize).clamp(4, 256);
    let a = turns * std::f32::consts::TAU;
    let (ux, uy) = (a.cos(), a.sin());
    let taps: Vec<(f32, f32)> = (0..samples).map(|k| ((k as f32 + 0.5) / samples as f32 - 0.5) * length).map(|t| (ux * t, uy * t)).collect();
    let mut out = src.clone();
    let at = |x: i64, y: i64, c: usize| src.data[((y.rem_euclid(h) * w + x.rem_euclid(w)) as usize) * n + c];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let mut acc = 0.0;
                for (dx, dy) in &taps {
                    let (sx, sy) = (x as f32 + dx, y as f32 + dy);
                    let (ix, iy) = (sx.floor(), sy.floor());
                    let (fx, fy) = (sx - ix, sy - iy);
                    let (ix, iy) = (ix as i64, iy as i64);
                    let top = at(ix, iy, c) * (1.0 - fx) + at(ix + 1, iy, c) * fx;
                    let bot = at(ix, iy + 1, c) * (1.0 - fx) + at(ix + 1, iy + 1, c) * fx;
                    acc += top * (1.0 - fy) + bot * fy;
                }
                out.data[((y * w + x) as usize) * n + c] = acc / samples as f32;
            }
        }
    }
    out
}

/// The image at `w` x `h` (bilinear, wrapping) when it is not already that size.
fn resize(img: &Rc<Image>, w: u32, h: u32) -> Rc<Image> {
    if img.w == w && img.h == h {
        return img.clone();
    }
    let n = img.n;
    let mut data = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            for c in 0..n {
                data.push(crate::sample(img, u, v, c, n));
            }
        }
    }
    Rc::new(Image { w, h, n, data })
}

/// Grey or colour copy of an input image.
fn fit(img: &Image, colour: bool) -> Image {
    widen(img, if colour { 3 } else { 1 })
}

fn widen(img: &Image, n: usize) -> Image {
    if img.n == n {
        return img.clone();
    }
    let npx = (img.w * img.h) as usize;
    let mut data = Vec::with_capacity(npx * n);
    for i in 0..npx {
        for c in 0..n {
            data.push(if n == 1 && img.n >= 3 {
                0.2126 * img.data[i * img.n] + 0.7152 * img.data[i * img.n + 1] + 0.0722 * img.data[i * img.n + 2]
            } else {
                img.data[i * img.n + c.min(img.n - 1)]
            });
        }
    }
    Image { w: img.w, h: img.h, n, data }
}

fn sharpen(src: &Image, k: f32) -> Image {
    let (w, h, n) = (src.w as usize, src.h as usize, src.n);
    let at = |x: usize, y: usize, c: usize| src.data[(y * w + x) * n + c];
    let mut out = src.clone();
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let v = at(x, y, c);
                let lap = 4.0 * v - at((x + w - 1) % w, y, c) - at((x + 1) % w, y, c) - at(x, (y + h - 1) % h, c) - at(x, (y + 1) % h, c);
                out.data[(y * w + x) * n + c] = (v + k * lap).clamp(0.0, 1.0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(w: u32, h: u32) -> Image {
        let mut seed = 0x1234_5678u32;
        let data = (0..w * h)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as f32 / (1u32 << 24) as f32
            })
            .collect();
        Image { w, h, n: 1, data }
    }

    fn uid(xml: &str, id: &str) -> u32 {
        let at = xml.find(&format!("identifier=\"{id}\"")).unwrap();
        let head = &xml[..at];
        let u = head.rfind("uid=\"").unwrap() + 5;
        head[u..u + head[u..].find('"').unwrap()].parse().unwrap()
    }

    /// `SHARD_SBAM_DIR=<extracted graphs> cargo test -p shard-compose graph_vs_native -- --ignored --nocapture`:
    /// the compiled graphs of the filters already measured, run node by node, against the native versions.
    #[test]
    #[ignore]
    fn graph_vs_native() {
        let Some(dir) = std::env::var_os("SHARD_SBAM_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let load = |g: &str| {
            let b = std::fs::read(dir.join(format!("{g}.sbsasm"))).unwrap();
            (Sbam::parse(&b).unwrap(), std::fs::read_to_string(dir.join(format!("{g}.xml"))).unwrap())
        };
        let (w, h) = (128, 128);
        let src = noise(w, h);
        // contrast_luminosity
        let (g, xml) = load("contrast_luminosity");
        for (c, l) in [(0.17f32, -0.06f32), (-0.5, 0.3), (0.4, -0.4)] {
            let params = Params::from([(uid(&xml, "Contrast"), c), (uid(&xml, "Luminosity"), l)]);
            let out = run(&g, &Inputs { params, images: HashMap::from([(uid(&xml, "input"), &src)]), w, h }).unwrap();
            let (ilo, ihi) = if c > 0.0 { (c / 2.0, 1.0 - c / 2.0) } else { (0.0, 1.0) };
            let (olo, ohi) = if c < 0.0 { (-c / 2.0, 1.0 + c / 2.0) } else { (0.0, 1.0) };
            let (olo, ohi) = ((olo + l).clamp(0.0, 1.0), (ohi + l).clamp(0.0, 1.0));
            let worst = out.data.iter().zip(&src.data).map(|(o, x)| (o - (olo + (ohi - olo) * ((x - ilo) / (ihi - ilo)).clamp(0.0, 1.0))).abs()).fold(0.0f32, f32::max);
            eprintln!("contrast_luminosity C {c} L {l}: worst {worst:e}");
            assert!(worst < 1e-5);
        }
        // sharpen
        let (g, xml) = load("sharpen");
        let params = Params::from([(uid(&xml, "intensity"), 0.35)]);
        let out = run(&g, &Inputs { params, images: HashMap::from([(uid(&xml, "input"), &src)]), w, h }).unwrap();
        let want = sharpen(&src, 0.35);
        let worst = out.data.iter().zip(&want.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        eprintln!("sharpen: worst {worst:e}");
        assert!(worst < 1e-6);
    }

    /// `SHARD_SBAM_DIR=... SHARD_FILTER_PROBE_DIR=<sbsrender noise responses> cargo test -p shard-compose
    /// graph_highpass -- --ignored --nocapture`: `highpass` run from its compiled graph against the external engine.
    #[test]
    #[ignore]
    fn graph_highpass() {
        let (Some(dir), Some(probe)) = (std::env::var_os("SHARD_SBAM_DIR"), std::env::var_os("SHARD_FILTER_PROBE_DIR")) else { return };
        let dir = std::path::PathBuf::from(dir);
        let probe = std::path::PathBuf::from(probe);
        let g = Sbam::parse(&std::fs::read(dir.join("highpass.sbsasm")).unwrap()).unwrap();
        let xml = std::fs::read_to_string(dir.join("highpass.xml")).unwrap();
        let rd = |p: std::path::PathBuf| -> Vec<f32> { std::fs::read(p).unwrap().chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect() };
        for r in ["0.4", "1.6", "3.2"] {
            let x = rd(probe.join(format!("highpass_noise_Radius{r}_1024x1024_in.f32")));
            let want = rd(probe.join(format!("highpass_noise_Radius{r}_1024x1024_out.f32")));
            let src = Image { w: 1024, h: 1024, n: 1, data: x.iter().map(|v| (v * 255.0).round() / 255.0).collect() };
            let params = Params::from([(uid(&xml, "Radius"), r.parse::<f32>().unwrap())]);
            let out = run(&g, &Inputs { params, images: HashMap::from([(uid(&xml, "input"), &src)]), w: 1024, h: 1024 }).unwrap();
            let mae = out.data.iter().zip(&want).map(|(a, b)| (a - b).abs()).sum::<f32>() / want.len() as f32 * 255.0;
            eprintln!("highpass Radius {r}: MAE {mae:.3} of 255 against the external engine");
            assert!(mae < 1.5);
        }
    }

    /// Interface defaults of a graph's `.xml` (`uid` -> first component).
    fn defaults(xml: &str) -> Params {
        let mut p = Params::new();
        for part in xml.split("<input ").skip(1) {
            let head = &part[..part.find('>').unwrap_or(part.len())];
            let attr = |k: &str| head.find(&format!("{k}=\"")).map(|a| &head[a + k.len() + 2..]).map(|t| &t[..t.find('"').unwrap()]);
            if let (Some(u), Some(d)) = (attr("uid"), attr("default")) {
                if let (Ok(u), Some(Ok(v))) = (u.parse::<u32>(), d.split(',').next().map(|x| x.trim().parse::<f32>())) {
                    p.insert(u, v);
                }
            }
        }
        p
    }

    /// `SHARD_SBAM_DIR=... SHARD_GRAPH=name cargo test -p shard-compose graph_try -- --ignored --nocapture`: run a
    /// graph at its defaults with noise on every image input and report the first node it cannot evaluate.
    #[test]
    #[ignore]
    fn graph_try() {
        let (Some(dir), Ok(name)) = (std::env::var_os("SHARD_SBAM_DIR"), std::env::var("SHARD_GRAPH")) else { return };
        let dir = std::path::PathBuf::from(dir);
        let g = Sbam::parse(&std::fs::read(dir.join(format!("{name}.sbsasm"))).unwrap()).unwrap();
        let xml = std::fs::read_to_string(dir.join(format!("{name}.xml"))).unwrap();
        let src = noise(256, 256);
        let mut images = HashMap::new();
        for part in xml.split("<input ").skip(1) {
            let head = &part[..part.find('>').unwrap_or(part.len())];
            if head.contains("type=\"5\"") || head.contains("type=\"1\"") {
                if let Some(a) = head.find("uid=\"") {
                    let t = &head[a + 5..];
                    images.insert(t[..t.find('"').unwrap()].parse::<u32>().unwrap(), &src);
                }
            }
        }
        match run(&g, &Inputs { params: defaults(&xml), images, w: 256, h: 256 }) {
            Ok(img) => eprintln!("{name}: ran, {}x{} n={} mean {:.4}", img.w, img.h, img.n, img.data.iter().sum::<f32>() / img.data.len() as f32),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
