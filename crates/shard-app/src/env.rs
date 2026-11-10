//! Image-based lighting data: an equirectangular HDR environment with a mip chain (specular) and
//! 9 spherical-harmonic coefficients (diffuse irradiance).
//!
//! Everything is kept as half floats (RGBA16F bits); float conversion happens per texel while
//! building the mip chain, so no full-size f32 copy is ever allocated.
//!
//! Direction mapping (Y up): `u = atan2(d.z, d.x) / 2pi + 0.5`, `v = acos(d.y) / pi` (v = 0 at the top).

use std::path::Path;

use shard_formats::image::{self, f16_to_f32, PixelFormat};
use shard_formats::Container;

pub struct Environment {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Mip chain, RGBA16F as raw half-float bits; level 0 is `width x height`.
    pub mips: Vec<Vec<u16>>,
    /// Real SH9 radiance coefficients (L00, L1-1, L10, L11, L2-2, L2-1, L20, L21, L22), RGB each.
    pub sh: [[f32; 3]; 9],
}

impl Environment {
    /// Soft studio-like gradient used when a project has no environment image.
    pub fn procedural() -> Self {
        let (w, h) = (128u32, 64u32);
        let mut px: Vec<u16> = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32; // 0 top, 1 bottom
            let up = 1.0 - 2.0 * v; // +1 top, -1 bottom
            let sky = 0.9 + 0.9 * up.max(0.0);
            let ground = 0.18 + 0.12 * (1.0 + up).clamp(0.0, 1.0);
            let l = if up >= 0.0 { sky } else { ground };
            for _ in 0..w {
                px.extend_from_slice(&[f32_to_f16(l), f32_to_f16(l * 0.98), f32_to_f16(l * 0.95), f32_to_f16(1.0)]);
            }
        }
        Self::from_f16("Procedural studio", w, h, px)
    }

    /// Environment from the project's HDR image (the first RGBA16F 2:1 image), if any.
    pub fn from_project(path: &Path) -> Option<Self> {
        let mut c = Container::open(path).ok()?;
        let names: Vec<String> = c
            .members()
            .filter(|d| d.path.starts_with("resources/") && d.path.ends_with(".image") && !d.path.contains(".alg_meta"))
            .map(|d| d.path.clone())
            .collect();
        for n in names {
            let head = c.read_prefix(&n, 1024).ok()?;
            let Ok(info) = image::info(&head) else { continue };
            if info.format == PixelFormat::Rgba16F && info.compression == "lz4" && info.width == 2 * info.height {
                let bytes = c.read(&n).ok()?;
                let img = image::decode(&bytes).ok()?;
                drop(bytes);
                let bits = img.rgba16f_bits()?;
                let short = n.rsplit('/').next().unwrap_or(&n);
                return Some(Self::from_f16(
                    &format!("project HDRI {}", &short[..8.min(short.len())]),
                    img.width,
                    img.height,
                    bits,
                ));
            }
        }
        None
    }

    /// Environment from an OpenEXR equirectangular image, box-downsampled while reading to at most
    /// 2048 texels wide (an 8k EXR never materialises as a full float image).
    pub fn from_exr(path: &Path) -> Result<Self, String> {
        let (tw, th, rgb) = read_exr_reduced(path, 2048)?;
        let mut px: Vec<u16> = Vec::with_capacity(tw * th * 4);
        for c in rgb.chunks_exact(3) {
            px.extend_from_slice(&[f32_to_f16(c[0]), f32_to_f16(c[1]), f32_to_f16(c[2]), f32_to_f16(1.0)]);
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("hdri").to_string();
        Ok(Self::from_f16(&name, tw as u32, th as u32, px))
    }

    /// Build the lighting data from a half-float equirect image: SH9 irradiance from a small box
    /// copy, and a **GGX-prefiltered** specular pyramid (mip `k` holds roughness `k / (N - 1)`).
    fn from_f16(name: &str, w: u32, h: u32, level0: Vec<u16>) -> Self {
        // linear RGB copy of the source
        let mut rgb: Vec<f32> = Vec::with_capacity((w * h * 3) as usize);
        for px in level0.chunks_exact(4) {
            rgb.extend_from_slice(&[f16_to_f32(px[0]), f16_to_f32(px[1]), f16_to_f32(px[2])]);
        }
        drop(level0);
        // base level: at most 2048x1024 (box filter), always 2:1. Sharp reflections (glass, polished
        // metal) show this level magnified, so it must not be coarse: a 512 base turns the sun into a
        // square and bilinear magnification draws creases along the texel grid.
        let base_w = w.min(2048).max(8);
        let base_h = (base_w / 2).max(4);
        let base = downsample(&rgb, w, h, base_w, base_h);
        drop(rgb);
        let levels = {
            let mut n = 1;
            while (base_w >> n) >= 8 {
                n += 1;
            }
            n.min(7)
        };
        let mut mips: Vec<Vec<u16>> = Vec::with_capacity(levels as usize);
        mips.push(pack(&base));
        for k in 1..levels {
            let (ow, oh) = ((base_w >> k).max(1), (base_h >> k).max(1));
            // a lobe at roughness r is smooth: filter at a modest size (cost = out x src texels) and
            // scale up to the mip size
            let (cw, ch) = (ow.min(128), oh.min(64));
            let sw = (cw * 2).clamp(16, 64);
            let src = downsample(&base, base_w, base_h, sw, (sw / 2).max(8));
            let r = k as f32 / (levels - 1).max(1) as f32;
            let small = ggx_prefilter(&src, sw, (sw / 2).max(8), cw, ch, r);
            mips.push(pack(&upsample_bilinear(&small, cw, ch, ow, oh)));
        }
        // SH9 from a 128-wide box copy of the unfiltered environment
        let (sw, sh_h) = (base_w.min(128), (base_w.min(128) / 2).max(4));
        let small = downsample(&base, base_w, base_h, sw, sh_h);
        let mut sh = [[0f32; 3]; 9];
        let mut wsum = 0f32;
        for y in 0..sh_h {
            let theta = (y as f32 + 0.5) / sh_h as f32 * std::f32::consts::PI;
            let dw = theta.sin();
            for x in 0..sw {
                let phi = ((x as f32 + 0.5) / sw as f32 - 0.5) * std::f32::consts::TAU;
                let d = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
                let i = ((y * sw + x) * 3) as usize;
                let rgb = [small[i], small[i + 1], small[i + 2]];
                let basis = sh_basis(d);
                for k in 0..9 {
                    for c in 0..3 {
                        sh[k][c] += rgb[c] * basis[k] * dw;
                    }
                }
                wsum += dw;
            }
        }
        let norm = 4.0 * std::f32::consts::PI / wsum.max(1e-6);
        for k in sh.iter_mut() {
            for c in k.iter_mut() {
                *c *= norm;
            }
        }
        Self { name: name.to_string(), width: base_w, height: base_h, mips, sh }
    }

    pub fn mip_count(&self) -> u32 {
        self.mips.len() as u32
    }
}

/// Linear RGB of an OpenEXR image, box-downsampled while reading to at most `max_w` texels wide (the full-size
/// image never materialises): `(width, height, rgb)`.
fn read_exr_reduced(path: &Path, max_w: usize) -> Result<(usize, usize, Vec<f32>), String> {
    struct Acc {
        factor: usize,
        tw: usize,
        th: usize,
        sum: Vec<f32>,
        cnt: Vec<u16>,
    }
    let img = exr::prelude::read_first_rgba_layer_from_file(
        path,
        move |res, _| {
            let factor = res.width().div_ceil(max_w.max(1)).max(1);
            let (tw, th) = ((res.width() / factor).max(1), (res.height() / factor).max(1));
            Acc { factor, tw, th, sum: vec![0.0; tw * th * 3], cnt: vec![0; tw * th] }
        },
        |acc: &mut Acc, pos, (r, g, b, _a): (f32, f32, f32, f32)| {
            let (x, y) = (pos.x() / acc.factor, pos.y() / acc.factor);
            if x < acc.tw && y < acc.th {
                let i = y * acc.tw + x;
                let clean = |v: f32| if v.is_finite() { v.clamp(0.0, 60000.0) } else { 0.0 };
                acc.sum[i * 3] += clean(r);
                acc.sum[i * 3 + 1] += clean(g);
                acc.sum[i * 3 + 2] += clean(b);
                acc.cnt[i] = acc.cnt[i].saturating_add(1);
            }
        },
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let acc = img.layer_data.channel_data.pixels;
    let mut rgb = acc.sum;
    for (i, c) in rgb.chunks_exact_mut(3).enumerate() {
        let n = acc.cnt[i].max(1) as f32;
        c.iter_mut().for_each(|v| *v /= n);
    }
    Ok((acc.tw, acc.th, rgb))
}

/// Small preview of an equirectangular EXR, `width` texels wide, as sRGB RGBA8: exposed so that its log-average
/// luminance lands on mid grey, then the ACES filmic curve (an HDRI of any brightness reads as a picture).
pub fn exr_preview(path: &Path, width: usize) -> Result<(u32, u32, Vec<u8>), String> {
    let (w, h, rgb) = read_exr_reduced(path, width)?;
    let lum = |c: &[f32]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let n = (w * h).max(1) as f32;
    let log_avg = (rgb.chunks_exact(3).map(|c| (lum(c) + 1e-4).ln()).sum::<f32>() / n).exp();
    let k = (0.18 / log_avg.max(1e-6)).clamp(1e-3, 1e3);
    let aces = |x: f32| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(w * h * 4);
    for c in rgb.chunks_exact(3) {
        for v in c {
            out.push((shard_compose::linear_to_srgb(aces(v * k)) * 255.0 + 0.5) as u8);
        }
        out.push(255);
    }
    Ok((w as u32, h as u32, out))
}

fn sh_basis(d: [f32; 3]) -> [f32; 9] {
    let (x, y, z) = (d[0], d[1], d[2]);
    [
        0.282095,
        0.488603 * y,
        0.488603 * z,
        0.488603 * x,
        1.092548 * x * y,
        1.092548 * y * z,
        0.315392 * (3.0 * z * z - 1.0),
        1.092548 * x * z,
        0.546274 * (x * x - y * y),
    ]
}

/// f32 -> IEEE half bits (round to nearest, ties away; HDR values clamped to the half range).
pub fn f32_to_f16(v: f32) -> u16 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(-65504.0, 65504.0) };
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32 - 127 + 15;
    let man = b & 0x7F_FFFF;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exp >= 31 {
        return sign | 0x7BFF;
    }
    let r = (man + 0x1000) >> 13;
    sign | ((exp as u16) << 10).wrapping_add(r as u16)
}


fn pack(rgb: &[f32]) -> Vec<u16> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for p in rgb.chunks_exact(3) {
        out.extend_from_slice(&[f32_to_f16(p[0]), f32_to_f16(p[1]), f32_to_f16(p[2]), f32_to_f16(1.0)]);
    }
    out
}

/// Bilinear enlargement of an equirect image (wraps horizontally, clamps vertically).
fn upsample_bilinear(src: &[f32], sw: u32, sh: u32, tw: u32, th: u32) -> Vec<f32> {
    if (sw, sh) == (tw, th) {
        return src.to_vec();
    }
    let mut out = vec![0f32; (tw * th * 3) as usize];
    for y in 0..th {
        let fy = ((y as f32 + 0.5) * sh as f32 / th as f32 - 0.5).max(0.0);
        let (y0, ty) = ((fy.floor() as u32).min(sh - 1), fy - fy.floor());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..tw {
            let fx = (x as f32 + 0.5) * sw as f32 / tw as f32 - 0.5;
            let x0f = fx.floor();
            let tx = fx - x0f;
            let x0 = (x0f as i64).rem_euclid(sw as i64) as u32;
            let x1 = (x0 + 1) % sw;
            for c in 0..3 {
                let at = |xx: u32, yy: u32| src[((yy * sw + xx) * 3) as usize + c];
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                out[((y * tw + x) * 3) as usize + c] = top + (bot - top) * ty;
            }
        }
    }
    out
}

/// Area-average resample of a linear RGB image (target not larger than the source in each axis).
fn downsample(src: &[f32], sw: u32, sh: u32, tw: u32, th: u32) -> Vec<f32> {
    if (sw, sh) == (tw, th) {
        return src.to_vec();
    }
    let mut out = vec![0f32; (tw * th * 3) as usize];
    for y in 0..th {
        let (y0, y1) = ((y * sh / th) as usize, (((y + 1) * sh).div_ceil(th)).min(sh) as usize);
        for x in 0..tw {
            let (x0, x1) = ((x * sw / tw) as usize, (((x + 1) * sw).div_ceil(tw)).min(sw) as usize);
            let mut acc = [0f32; 3];
            let mut n = 0f32;
            for yy in y0..y1.max(y0 + 1) {
                for xx in x0..x1.max(x0 + 1) {
                    let i = (yy.min(sh as usize - 1) * sw as usize + xx.min(sw as usize - 1)) * 3;
                    acc[0] += src[i];
                    acc[1] += src[i + 1];
                    acc[2] += src[i + 2];
                    n += 1.0;
                }
            }
            let o = ((y * tw + x) * 3) as usize;
            out[o] = acc[0] / n;
            out[o + 1] = acc[1] / n;
            out[o + 2] = acc[2] / n;
        }
    }
    out
}

fn dir_of(x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
    let phi = ((x as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU;
    [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()]
}

/// GGX-convolved environment for `N = V = R` at `roughness` (alpha = roughness^2): every output
/// texel is the lobe-weighted mean of the source texels (weights D(h) * N.L * texel solid angle).
fn ggx_prefilter(src: &[f32], sw: u32, sh: u32, ow: u32, oh: u32, roughness: f32) -> Vec<f32> {
    let a = (roughness * roughness).max(1e-3);
    let a2 = a * a;
    // source directions and solid-angle weights
    let mut sd: Vec<[f32; 4]> = Vec::with_capacity((sw * sh) as usize);
    for y in 0..sh {
        for x in 0..sw {
            let d = dir_of(x, y, sw, sh);
            let theta = (y as f32 + 0.5) / sh as f32 * std::f32::consts::PI;
            sd.push([d[0], d[1], d[2], theta.sin()]);
        }
    }
    let mut out = vec![0f32; (ow * oh * 3) as usize];
    for y in 0..oh {
        for x in 0..ow {
            let r = dir_of(x, y, ow, oh);
            let (mut acc, mut sum) = ([0f32; 3], 0f32);
            for (i, l) in sd.iter().enumerate() {
                let ndl = r[0] * l[0] + r[1] * l[1] + r[2] * l[2];
                if ndl <= 0.0 {
                    continue;
                }
                let ndh2 = (1.0 + ndl) * 0.5; // (N.H)^2 when N == V
                let denom = ndh2 * (a2 - 1.0) + 1.0;
                let d = a2 / (std::f32::consts::PI * denom * denom);
                let w = d * ndl * l[3];
                acc[0] += src[i * 3] * w;
                acc[1] += src[i * 3 + 1] * w;
                acc[2] += src[i * 3 + 2] * w;
                sum += w;
            }
            let o = ((y * ow + x) * 3) as usize;
            let inv = 1.0 / sum.max(1e-12);
            out[o] = acc[0] * inv;
            out[o + 1] = acc[1] * inv;
            out[o + 2] = acc[2] * inv;
        }
    }
    out
}
