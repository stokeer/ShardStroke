//! Painter `.image` resources: a Qt `QDataStream` (big-endian) `QVariantMap` header followed by the
//! pixel payload.
//!
//! ```text
//! u32 count, then count x { QString key (u32 byte-len + UTF-16BE), QVariant value }
//! keys: width, height, pitch (bytes per row), format (BGRA8 | LUM8 | RGBA16F | LUM16),
//!       compression ("lz4" | "jp2"), dataSizeRaw, dataSizeLZ4, ICCdataSize, colorSpace, alphaFormat,
//!       flags, version ("v2"), tilesCount (jp2 only)
//! payload (lz4): [ICC profile bytes (ICCdataSize)] + one LZ4 *block* decoding to dataSizeRaw bytes
//! ```
//! `jp2` (JPEG 2000, tiled) is recognised but not decoded.

use std::collections::HashMap;

#[derive(Debug)]
pub enum ImageError {
    Format(String),
    Unsupported(String),
}
impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Format(s) => write!(f, "image format: {s}"),
            ImageError::Unsupported(s) => write!(f, "image unsupported: {s}"),
        }
    }
}
impl std::error::Error for ImageError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PixelFormat {
    Lum8,
    Lum16,
    /// 16-bit unsigned RGBA (JPEG 2000 mesh maps: normal, position...).
    Rgba16,
    Bgra8,
    Rgba16F,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct PainterImage {
    pub width: u32,
    pub height: u32,
    /// Bytes per row in `data`.
    pub pitch: u32,
    pub format: PixelFormat,
    pub color_space: i32,
    pub alpha_format: i32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
enum Var {
    Bool(bool),
    Int(i64),
    Str(String),
    Bytes,
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ImageError> {
        let s = self.b.get(self.p..self.p + n).ok_or_else(|| ImageError::Format("truncated header".into()))?;
        self.p += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, ImageError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn qstring(&mut self) -> Result<String, ImageError> {
        let n = self.u32()? as usize;
        if n == 0xFFFF_FFFF {
            return Ok(String::new());
        }
        let s = self.take(n)?;
        let u: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        Ok(String::from_utf16_lossy(&u))
    }
    fn variant(&mut self) -> Result<Var, ImageError> {
        let t = self.u32()?;
        let _null = self.take(1)?;
        Ok(match t {
            1 => Var::Bool(self.take(1)?[0] != 0),
            2 => Var::Int(i32::from_be_bytes(self.take(4)?.try_into().unwrap()) as i64),
            3 => Var::Int(u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as i64),
            4 => Var::Int(i64::from_be_bytes(self.take(8)?.try_into().unwrap())),
            5 => Var::Int(u64::from_be_bytes(self.take(8)?.try_into().unwrap()) as i64),
            6 => {
                self.take(8)?;
                Var::Int(0)
            }
            10 => Var::Str(self.qstring()?),
            12 => {
                let n = self.u32()?;
                if n != 0xFFFF_FFFF {
                    self.take(n as usize)?;
                }
                Var::Bytes
            }
            t => return Err(ImageError::Format(format!("unsupported QVariant type {t}"))),
        })
    }
}

fn int(m: &HashMap<String, Var>, k: &str) -> Option<i64> {
    match m.get(k) {
        Some(Var::Int(i)) => Some(*i),
        Some(Var::Bool(b)) => Some(*b as i64),
        _ => None,
    }
}

fn string<'a>(m: &'a HashMap<String, Var>, k: &str) -> Option<&'a str> {
    match m.get(k) {
        Some(Var::Str(s)) => Some(s),
        _ => None,
    }
}

/// Image properties from the header only (no pixel decoding).
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub compression: String,
}

/// Parse only the QVariantMap header (needs the first ~512 bytes of the resource).
pub fn info(bytes: &[u8]) -> Result<ImageInfo, ImageError> {
    let mut r = R { b: bytes, p: 0 };
    let n = r.u32()?;
    if n > 64 {
        return Err(ImageError::Format(format!("implausible entry count {n}")));
    }
    let mut m = HashMap::new();
    for _ in 0..n {
        let k = r.qstring()?;
        let v = r.variant()?;
        m.insert(k, v);
    }
    let format = match string(&m, "format").unwrap_or("") {
        "LUM8" => PixelFormat::Lum8,
        "LUM16" => PixelFormat::Lum16,
        "RGBA16" => PixelFormat::Rgba16,
        "BGRA8" => PixelFormat::Bgra8,
        "RGBA16F" => PixelFormat::Rgba16F,
        other => PixelFormat::Other(other.to_string()),
    };
    Ok(ImageInfo {
        width: int(&m, "width").ok_or_else(|| ImageError::Format("no width".into()))? as u32,
        height: int(&m, "height").ok_or_else(|| ImageError::Format("no height".into()))? as u32,
        format,
        compression: string(&m, "compression").unwrap_or("").to_string(),
    })
}

/// Decode a `.image` resource into raw pixel rows.
pub fn decode(bytes: &[u8]) -> Result<PainterImage, ImageError> {
    let mut r = R { b: bytes, p: 0 };
    let n = r.u32()?;
    if n > 64 {
        return Err(ImageError::Format(format!("implausible entry count {n}")));
    }
    let mut m = HashMap::new();
    for _ in 0..n {
        let k = r.qstring()?;
        let v = r.variant()?;
        m.insert(k, v);
    }
    let width = int(&m, "width").ok_or_else(|| ImageError::Format("no width".into()))? as u32;
    let height = int(&m, "height").ok_or_else(|| ImageError::Format("no height".into()))? as u32;
    let pitch = int(&m, "pitch").unwrap_or(0) as u32;
    let raw_size = int(&m, "dataSizeRaw").unwrap_or(0) as usize;
    let icc = int(&m, "ICCdataSize").unwrap_or(0) as usize;
    let format = match string(&m, "format").unwrap_or("") {
        "LUM8" => PixelFormat::Lum8,
        "LUM16" => PixelFormat::Lum16,
        "RGBA16" => PixelFormat::Rgba16,
        "BGRA8" => PixelFormat::Bgra8,
        "RGBA16F" => PixelFormat::Rgba16F,
        other => PixelFormat::Other(other.to_string()),
    };
    match string(&m, "compression").unwrap_or("") {
        "lz4" => {}
        "jp2" => {
            let tiles = int(&m, "tilesCount").unwrap_or(0) as usize;
            let start = r.p + icc;
            let data = decode_jp2_tiles(bytes, start, tiles, width, height, &format)?;
            let bpp = if format == PixelFormat::Rgba16 { 8 } else { 2 };
            let pitch = width * bpp;
            return Ok(PainterImage {
                width,
                height,
                pitch,
                format,
                color_space: int(&m, "colorSpace").unwrap_or(0) as i32,
                alpha_format: int(&m, "alphaFormat").unwrap_or(0) as i32,
                data,
            });
        }
        other => return Err(ImageError::Unsupported(format!("compression {other}"))),
    }
    let lz4 = int(&m, "dataSizeLZ4").unwrap_or(0) as usize;
    let start = r.p + icc;
    let payload = bytes
        .get(start..start + lz4)
        .ok_or_else(|| ImageError::Format("payload out of range".into()))?;
    let data = lz4_flex::block::decompress(payload, raw_size)
        .map_err(|e| ImageError::Format(format!("lz4: {e}")))?;
    if data.len() < (pitch as usize) * (height as usize) {
        return Err(ImageError::Format("decoded size smaller than pitch*height".into()));
    }
    Ok(PainterImage {
        width,
        height,
        pitch,
        format,
        color_space: int(&m, "colorSpace").unwrap_or(0) as i32,
        alpha_format: int(&m, "alphaFormat").unwrap_or(0) as i32,
        data,
    })
}

/// Tiled JPEG 2000 payload: `tiles` records of `(x0, y0, x1, y1, size, 0)` (u32 LE) followed by a JP2
/// file of `size` bytes holding one tile; records are padded to 8 bytes relative to the table start.
/// Returns little-endian u16 rows with `components` interleaved samples per pixel.
fn decode_jp2_tiles(
    b: &[u8],
    start: usize,
    tiles: usize,
    width: u32,
    height: u32,
    format: &PixelFormat,
) -> Result<Vec<u8>, ImageError> {
    let ncomp: usize = match format {
        PixelFormat::Lum16 => 1,
        PixelFormat::Rgba16 => 4,
        other => return Err(ImageError::Unsupported(format!("jp2 pixel format {other:?}"))),
    };
    let le = |o: usize| -> Result<u32, ImageError> {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            .ok_or_else(|| ImageError::Format("jp2 tile table out of range".into()))
    };
    let mut out = vec![0u8; (width * height) as usize * ncomp * 2];
    let mut p = start;
    for _ in 0..tiles {
        let (x0, y0, x1, y1, size) = (le(p)?, le(p + 4)?, le(p + 8)?, le(p + 12)?, le(p + 16)? as usize);
        let data = p + 24;
        let jp2 = b.get(data..data + size).ok_or_else(|| ImageError::Format("jp2 tile out of range".into()))?;
        p = start + ((p - start + 24 + size + 7) & !7);
        let img = openjpeg::Image::from_bytes(jp2)
            .map_err(|e| ImageError::Unsupported(format!("jp2 tile: {e}")))?;
        let comps = img.components();
        let (tw, th) = (x1 - x0, y1 - y0);
        // RGBA16 tiles may store only RGB (alpha is implied opaque)
        let have = comps.len().min(ncomp);
        if have == 0 || (ncomp == 4 && have < 3) || comps[0].width() != tw || comps[0].height() != th {
            return Err(ImageError::Format(format!(
                "jp2 tile layout mismatch: {} component(s) {:?}, expected {ncomp} of {tw}x{th}",
                comps.len(),
                comps.iter().map(|c| (c.width(), c.height())).collect::<Vec<_>>()
            )));
        }
        for row in 0..th {
            let dy = y0 + row;
            if dy >= height {
                break;
            }
            for col in 0..tw {
                let dx = x0 + col;
                if dx >= width {
                    break;
                }
                for c in 0..ncomp {
                    let v = if c < have {
                        comps[c].data()[(row * tw + col) as usize].clamp(0, 65535) as u16
                    } else {
                        65535
                    };
                    let o = (((dy * width + dx) as usize) * ncomp + c) * 2;
                    out[o..o + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
    }
    Ok(out)
}

/// IEEE half-float bits to f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) & 1) as u32;
    let e = ((h >> 10) & 0x1F) as u32;
    let m = (h & 0x3FF) as u32;
    let bits = if e == 0 {
        if m == 0 {
            s << 31
        } else {
            // subnormal
            let mut e2 = 127 - 15 + 1;
            let mut m2 = m;
            while m2 & 0x400 == 0 {
                m2 <<= 1;
                e2 -= 1;
            }
            (s << 31) | ((e2 as u32) << 23) | ((m2 & 0x3FF) << 13)
        }
    } else if e == 31 {
        (s << 31) | (0xFF << 23) | (m << 13)
    } else {
        (s << 31) | ((e + 127 - 15) << 23) | (m << 13)
    };
    f32::from_bits(bits)
}

impl PainterImage {
    /// Row `y` as a byte slice (without padding).
    fn row(&self, y: u32, bytes_per_px: usize) -> &[u8] {
        let s = (y as usize) * self.pitch as usize;
        &self.data[s..s + self.width as usize * bytes_per_px]
    }

    /// Raw half-float bits of an `RGBA16F` image, tightly packed (no row padding), without any
    /// float conversion. `None` for other formats.
    pub fn rgba16f_bits(&self) -> Option<Vec<u16>> {
        if self.format != PixelFormat::Rgba16F {
            return None;
        }
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            out.extend(self.row(y, 8).chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])));
        }
        Some(out)
    }

    /// Convert to tightly packed linear RGBA f32 (HDR values preserved; 8-bit formats are /255).
    pub fn to_rgba_f32(&self) -> Result<Vec<f32>, ImageError> {
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        match self.format {
            PixelFormat::Rgba16F => {
                for y in 0..self.height {
                    for p in self.row(y, 8).chunks_exact(8) {
                        for c in 0..4 {
                            out.push(f16_to_f32(u16::from_le_bytes([p[2 * c], p[2 * c + 1]])));
                        }
                    }
                }
            }
            PixelFormat::Bgra8 => {
                for y in 0..self.height {
                    for p in self.row(y, 4).chunks_exact(4) {
                        out.extend_from_slice(&[
                            p[2] as f32 / 255.0,
                            p[1] as f32 / 255.0,
                            p[0] as f32 / 255.0,
                            p[3] as f32 / 255.0,
                        ]);
                    }
                }
            }
            PixelFormat::Lum8 => {
                for y in 0..self.height {
                    for &l in self.row(y, 1) {
                        let v = l as f32 / 255.0;
                        out.extend_from_slice(&[v, v, v, 1.0]);
                    }
                }
            }
            PixelFormat::Lum16 => {
                for y in 0..self.height {
                    for p in self.row(y, 2).chunks_exact(2) {
                        let v = u16::from_le_bytes([p[0], p[1]]) as f32 / 65535.0;
                        out.extend_from_slice(&[v, v, v, 1.0]);
                    }
                }
            }
            PixelFormat::Rgba16 => {
                for y in 0..self.height {
                    for p in self.row(y, 8).chunks_exact(8) {
                        for c in 0..4 {
                            out.push(u16::from_le_bytes([p[2 * c], p[2 * c + 1]]) as f32 / 65535.0);
                        }
                    }
                }
            }
            PixelFormat::Other(ref s) => return Err(ImageError::Unsupported(format!("pixel format {s}"))),
        }
        Ok(out)
    }

    /// Convert to tightly packed RGBA8 (HDR values are clamped to [0,1]).
    pub fn to_rgba8(&self) -> Result<Vec<u8>, ImageError> {
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        match self.format {
            PixelFormat::Lum8 => {
                for y in 0..self.height {
                    for &l in self.row(y, 1) {
                        out.extend_from_slice(&[l, l, l, 255]);
                    }
                }
            }
            PixelFormat::Lum16 => {
                for y in 0..self.height {
                    for p in self.row(y, 2).chunks_exact(2) {
                        let g = (u16::from_le_bytes([p[0], p[1]]) >> 8) as u8;
                        out.extend_from_slice(&[g, g, g, 255]);
                    }
                }
            }
            PixelFormat::Rgba16 => {
                for y in 0..self.height {
                    for p in self.row(y, 8).chunks_exact(8) {
                        out.extend_from_slice(&[p[1], p[3], p[5], p[7]]);
                    }
                }
            }
            PixelFormat::Bgra8 => {
                for y in 0..self.height {
                    for p in self.row(y, 4).chunks_exact(4) {
                        out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                    }
                }
            }
            PixelFormat::Rgba16F => {
                for y in 0..self.height {
                    for p in self.row(y, 8).chunks_exact(8) {
                        for c in 0..4 {
                            let v = f16_to_f32(u16::from_le_bytes([p[2 * c], p[2 * c + 1]]));
                            out.push((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                        }
                    }
                }
            }
            PixelFormat::Other(ref s) => return Err(ImageError::Unsupported(format!("pixel format {s}"))),
        }
        Ok(out)
    }
}
