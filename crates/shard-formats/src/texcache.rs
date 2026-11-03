//! Painter's texture cache (`texture-cache/<sceneMaterialUID>/<stack>/<channel>` members of a project).
//!
//! Painter saves the final picture of every channel of every texture set with the project: the stack as it was
//! composed when the file was written. Each member is a Qt `QDataStream` map (`width`, `height`,
//! `internalFormat` = an OpenGL internal format, `bufferSize`, `compression` = `lz4`, `version`), then the
//! big-endian length of an LZ4 block and the block. Rows are stored bottom-up (OpenGL); [`decode`] returns them
//! top-down, like the rest of ShardStroke (`y = (1 - v) * h`). Format notes: `docs/FORMATS.md` 7.14.

/// Pixel format of a cached channel (OpenGL internal format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFormat {
    /// `GL_SRGB8_ALPHA8`: colour, sRGB-encoded, alpha = written by the stack.
    Srgb8Alpha8,
    /// `GL_RGBA8`.
    Rgba8,
    /// `GL_RG8`: a grey channel (roughness, metallic, opacity) and its coverage.
    Rg8,
    /// `GL_R8`.
    R8,
    /// `GL_RG16F`: a signed grey channel (height) and its coverage.
    Rg16f,
    /// `GL_R16F`.
    R16f,
    /// `GL_RGBA16F`: the normal channel (tangent space, 0..1 encoded) and its coverage.
    Rgba16f,
}

impl CacheFormat {
    pub fn from_gl(v: u32) -> Option<Self> {
        Some(match v {
            0x8C43 => Self::Srgb8Alpha8,
            0x8058 => Self::Rgba8,
            0x822B => Self::Rg8,
            0x8229 => Self::R8,
            0x822F => Self::Rg16f,
            0x822D => Self::R16f,
            0x881A => Self::Rgba16f,
            _ => return None,
        })
    }

    /// Components per pixel.
    pub fn comps(self) -> usize {
        match self {
            Self::Srgb8Alpha8 | Self::Rgba8 | Self::Rgba16f => 4,
            Self::Rg8 | Self::Rg16f => 2,
            Self::R8 | Self::R16f => 1,
        }
    }

    /// Bytes per component.
    pub fn comp_bytes(self) -> usize {
        match self {
            Self::Rg16f | Self::R16f | Self::Rgba16f => 2,
            _ => 1,
        }
    }

    pub fn is_srgb(self) -> bool {
        self == Self::Srgb8Alpha8
    }
}

/// One decoded channel: raw pixels, rows top-down.
#[derive(Debug, Clone)]
pub struct CacheImage {
    pub width: u32,
    pub height: u32,
    pub format: CacheFormat,
    pub pixels: Vec<u8>,
}

impl CacheImage {
    /// Component `c` of pixel `i` as a float (8-bit formats 0..1, still sRGB-encoded for colour).
    #[inline]
    pub fn get(&self, i: usize, c: usize) -> f32 {
        let n = self.format.comps();
        match self.format.comp_bytes() {
            1 => self.pixels[i * n + c] as f32 / 255.0,
            _ => {
                let o = (i * n + c) * 2;
                f16_to_f32(u16::from_le_bytes([self.pixels[o], self.pixels[o + 1]]))
            }
        }
    }

    /// The first `k` components as floats, box-averaged down to `size` x `size` (`size` divides the width).
    pub fn to_f32(&self, k: usize, size: u32) -> Vec<f32> {
        let k = k.min(self.format.comps());
        let size = size.clamp(1, self.width);
        let step = (self.width / size).max(1) as usize;
        let (w, h) = (self.width as usize, self.height as usize);
        let (ow, oh) = (w / step, h / step);
        let mut out = vec![0.0f32; ow * oh * k];
        let inv = 1.0 / (step * step) as f32;
        for oy in 0..oh {
            for ox in 0..ow {
                for c in 0..k {
                    let mut acc = 0.0;
                    for dy in 0..step {
                        for dx in 0..step {
                            acc += self.get((oy * step + dy) * w + ox * step + dx, c);
                        }
                    }
                    out[(oy * ow + ox) * k + c] = acc * inv;
                }
            }
        }
        out
    }
}

/// IEEE half to float.
pub fn f16_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) & 1) as u32;
    let e = ((h >> 10) & 0x1f) as u32;
    let m = (h & 0x3ff) as u32;
    let bits = if e == 0 {
        if m == 0 {
            s << 31
        } else {
            // subnormal: normalise
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (s << 31) | (e << 23) | ((m & 0x3ff) << 13)
        }
    } else if e == 31 {
        (s << 31) | (0xff << 23) | (m << 13)
    } else {
        (s << 31) | ((e + 127 - 15) << 23) | (m << 13)
    };
    f32::from_bits(bits)
}

/// Decompress one LZ4 block into a buffer of known size.
pub fn lz4_block(src: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(out_len);
    let mut i = 0usize;
    while i < src.len() {
        let token = src[i];
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            loop {
                let b = *src.get(i)?;
                i += 1;
                lit += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        out.extend_from_slice(src.get(i..i + lit)?);
        i += lit;
        if i >= src.len() {
            break; // the last sequence has literals only
        }
        let off = u16::from_le_bytes([*src.get(i)?, *src.get(i + 1)?]) as usize;
        i += 2;
        if off == 0 || off > out.len() {
            return None;
        }
        let mut ml = (token & 15) as usize;
        if ml == 15 {
            loop {
                let b = *src.get(i)?;
                i += 1;
                ml += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        ml += 4;
        let start = out.len() - off;
        if off >= ml {
            out.extend_from_within(start..start + ml);
        } else {
            for k in 0..ml {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
    (out.len() == out_len).then_some(out)
}

/// The header map of a cache member (only the value types it uses).
#[derive(Debug, Default, Clone)]
pub struct CacheHeader {
    pub width: u32,
    pub height: u32,
    pub internal_format: u32,
    pub buffer_size: u64,
    pub compression: String,
    /// Offset of the payload (after the map).
    pub payload: usize,
}

struct Rd<'a> {
    b: &'a [u8],
    o: usize,
}

impl Rd<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.o)?;
        self.o += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.b.get(self.o..self.o + 4)?;
        self.o += 4;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.b.get(self.o..self.o + 8)?;
        self.o += 8;
        Some(u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    }
    fn qstring(&mut self) -> Option<String> {
        let n = self.u32()?;
        if n == u32::MAX {
            return Some(String::new());
        }
        let s = self.b.get(self.o..self.o + n as usize)?;
        self.o += n as usize;
        let u: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&u))
    }
}

/// Parse the `QVariantMap` header of a cache member.
pub fn parse_header(b: &[u8]) -> Option<CacheHeader> {
    let mut r = Rd { b, o: 0 };
    let n = r.u32()?;
    if n > 64 {
        return None;
    }
    let mut h = CacheHeader::default();
    for _ in 0..n {
        let key = r.qstring()?;
        let ty = r.u32()?;
        let _null = r.u8()?;
        match ty {
            2 | 3 => {
                let v = r.u32()?;
                match key.as_str() {
                    "width" => h.width = v,
                    "height" => h.height = v,
                    "internalFormat" => h.internal_format = v,
                    "bufferSize" => h.buffer_size = v as u64,
                    _ => {}
                }
            }
            4 | 5 => {
                let v = r.u64()?;
                if key == "bufferSize" {
                    h.buffer_size = v;
                }
            }
            10 => {
                let v = r.qstring()?;
                if key == "compression" {
                    h.compression = v;
                }
            }
            1 => {
                r.u8()?;
            }
            _ => return None,
        }
    }
    h.payload = r.o;
    Some(h)
}

/// Decode a cache member (header, LZ4 payload, rows flipped to top-down).
pub fn decode(b: &[u8]) -> Option<CacheImage> {
    let h = parse_header(b)?;
    let format = CacheFormat::from_gl(h.internal_format)?;
    let raw = match h.compression.as_str() {
        "lz4" => {
            let mut r = Rd { b, o: h.payload };
            let clen = r.u32()? as usize;
            lz4_block(b.get(r.o..r.o + clen)?, h.buffer_size as usize)?
        }
        "" | "none" => b.get(h.payload..h.payload + h.buffer_size as usize)?.to_vec(),
        _ => return None,
    };
    let row = h.width as usize * format.comps() * format.comp_bytes();
    if raw.len() != row * h.height as usize {
        return None;
    }
    let mut pixels = Vec::with_capacity(raw.len());
    for y in (0..h.height as usize).rev() {
        pixels.extend_from_slice(&raw[y * row..(y + 1) * row]);
    }
    Some(CacheImage { width: h.width, height: h.height, format, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xbc00), -1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0), 0.0);
    }

    #[test]
    fn lz4_overlapping_match() {
        // literal "ab", then a match of 6 at offset 2 -> "abababab", then the final literal "c"
        let src = [0x22, b'a', b'b', 2, 0, 0x10, b'c'];
        assert_eq!(lz4_block(&src, 9).as_deref(), Some(&b"ababababc"[..]));
    }
}
