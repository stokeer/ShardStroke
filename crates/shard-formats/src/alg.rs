//! Decoder for Painter's "Alg" reflective serialization (variants A and B).
//!
//! Layout is documented in `docs/FORMATS.md` section 7. Variant A (`ver = 1`): class table inline at
//! first use. Variant B (`ver = 0`): every property carries name + type.

use serde_json::{json, Map, Value as J};

pub const MAGIC: u32 = 0x1B7C_2FDD;
const NEWCLASS: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f32),
    Str(String),
    /// Qt QVariant (big-endian payload) `UInt`.
    QUint(u32),
    Bytes(Vec<u8>),
    /// Fixed-size value of a type whose layout is not interpreted yet (type id, bytes).
    Raw(u32, Vec<u8>),
    List(Vec<Value>),
    Object(Object),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Object {
    pub class: String,
    pub props: Vec<(String, Value)>,
}

impl Object {
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.props.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
}

impl Value {
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Uint(u) => i64::try_from(*u).ok(),
            Value::QUint(u) => Some(*u as i64),
            _ => None,
        }
    }
    /// Unsigned value, the full 64-bit range: identifiers (`uid`, `sceneMaterialUID`) are random u64s and half
    /// of them are above `i64::MAX`, where [`Self::as_i64`] gives `None`.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Uint(u) => Some(*u),
            Value::Int(i) => Some(*i as u64),
            Value::QUint(u) => Some(*u as u64),
            _ => None,
        }
    }
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn to_json(&self) -> J {
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => json!(b),
            Value::Int(i) => json!(i),
            Value::Uint(u) => json!(u),
            Value::Float(f) => json!(f),
            Value::Str(s) => json!(s),
            Value::QUint(u) => json!(u),
            Value::Bytes(b) => json!(format!("hex:{}", hex(b))),
            Value::Raw(_, b) => json!(format!("raw:{}", hex(b))),
            Value::List(l) => J::Array(l.iter().map(Value::to_json).collect()),
            Value::Object(o) => {
                let mut m = Map::new();
                m.insert("_class".into(), json!(o.class));
                for (k, v) in &o.props {
                    m.insert(k.clone(), v.to_json());
                }
                J::Object(m)
            }
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Debug)]
pub struct AlgError(pub String);
impl std::fmt::Display for AlgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "alg: {}", self.0)
    }
}
impl std::error::Error for AlgError {}
type R<T> = Result<T, AlgError>;

fn err<T>(s: impl Into<String>) -> R<T> {
    Err(AlgError(s.into()))
}

pub fn is_alg(b: &[u8]) -> bool {
    b.len() >= 12 && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == MAGIC
}

/// Decode an Alg blob (variant chosen from the header version field).
pub fn decode(b: &[u8]) -> R<Value> {
    if !is_alg(b) {
        return err("not an Alg blob");
    }
    let ver = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let mut d = Dec { b, p: 0, classes: Vec::new() };
    let v = match ver {
        1 => d.run_a()?,
        0 => d.run_b()?,
        x => return err(format!("unknown version {x}")),
    };
    if d.p != b.len() {
        return err(format!("trailing bytes: parsed {}/{}", d.p, b.len()));
    }
    Ok(v)
}

struct Dec<'a> {
    b: &'a [u8],
    p: usize,
    classes: Vec<(String, Vec<(String, u32)>)>,
}

/// Byte sizes of fixed-size types (variant A numbering; fitted on the starter_assets corpus).
fn fixed_a(t: u32) -> Option<usize> {
    Some(match t {
        0x02 => 8,
        0x03 => 12,
        0x04 => 16,
        0x05 => 4,
        0x06 => 8,
        0x0B => 4,
        0x0D => 36,
        0x0E => 64,
        0x0F => 8,
        0x15 => 8,
        0x16 => 16,
        _ => return None,
    })
}

/// Variant B fixed sizes.
fn fixed_b(t: u32) -> Option<usize> {
    Some(match t {
        0x02 => 8,
        0x04 => 16,
        0x05 => 4,
        0x06 => 8,
        0x0D => 36,
        0x0E => 64,
        0x15 => 8,
        _ => return None,
    })
}

impl<'a> Dec<'a> {
    fn need(&self, n: usize) -> R<()> {
        if self.p + n > self.b.len() {
            return err(format!("eof need {n} @{:#x}", self.p));
        }
        Ok(())
    }
    fn u8(&mut self) -> R<u8> {
        self.need(1)?;
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> R<u16> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.b[self.p], self.b[self.p + 1]]);
        self.p += 2;
        Ok(v)
    }
    fn u32(&mut self) -> R<u32> {
        self.need(4)?;
        let v = u32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        Ok(v)
    }
    fn i32(&mut self) -> R<i32> {
        Ok(self.u32()? as i32)
    }
    fn u64(&mut self) -> R<u64> {
        self.need(8)?;
        let v = u64::from_le_bytes(self.b[self.p..self.p + 8].try_into().unwrap());
        self.p += 8;
        Ok(v)
    }
    fn f32(&mut self) -> R<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn raw(&mut self, n: usize) -> R<&'a [u8]> {
        self.need(n)?;
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn bytes(&mut self) -> R<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > (1 << 26) {
            return err(format!("bad length {n} @{:#x}", self.p - 4));
        }
        self.raw(n)
    }
    fn name(&mut self) -> R<String> {
        let b = self.bytes()?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }

    // ---- variant A ----
    fn run_a(&mut self) -> R<Value> {
        self.u32()?; // magic
        for _ in 0..5 {
            self.u32()?; // ver, 5, 18, nclasses, size
        }
        let idx = self.class_marker()?;
        self.object_a(idx)
    }

    fn class_marker(&mut self) -> R<usize> {
        let m = self.u32()?;
        if m == NEWCLASS {
            let name = self.name()?;
            let n = self.u32()? as usize;
            if n > 256 {
                return err(format!("nprops {n}"));
            }
            let mut props = Vec::with_capacity(n);
            for _ in 0..n {
                let pn = self.name()?;
                let pt = self.u32()?;
                props.push((pn, pt));
            }
            self.classes.push((name, props));
            return Ok(self.classes.len() - 1);
        }
        if (m as usize) >= self.classes.len() {
            return err(format!("unknown class idx {m} @{:#x}", self.p - 4));
        }
        Ok(m as usize)
    }

    fn object_a(&mut self, idx: usize) -> R<Value> {
        let (cname, props) = self.classes[idx].clone();
        let mut out = Object { class: cname, props: Vec::with_capacity(props.len()) };
        for (pn, pt) in props {
            let v = self.value_a(pt)?;
            out.props.push((pn, v));
        }
        Ok(Value::Object(out))
    }

    fn value_a(&mut self, t: u32) -> R<Value> {
        Ok(match t {
            0x01 => Value::Float(self.f32()?),
            0x09 => Value::Int(self.i32()? as i64),
            0x0A => Value::Bool(self.u8()? != 0),
            0x0C => Value::Uint(self.u64()?),
            0x10 => decode_bytes(self.bytes()?),
            0x11 => {
                let n = self.u32()? as usize;
                if n > (1 << 20) {
                    return err("list too long");
                }
                if n == 0 {
                    return Ok(Value::List(vec![]));
                }
                let idx = self.class_marker()?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.object_a(idx)?);
                }
                Value::List(v)
            }
            0x13 => {
                let n = self.u32()? as usize;
                if n > (1 << 20) {
                    return err("list too long");
                }
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    let _end = self.u32()?;
                    let idx = self.class_marker()?;
                    v.push(self.object_a(idx)?);
                }
                Value::List(v)
            }
            0x12 | 0x14 => {
                let end = self.u32()?;
                if end == 0 {
                    return Ok(Value::Null);
                }
                let idx = self.class_marker()?;
                self.object_a(idx)?
            }
            t => match fixed_a(t) {
                Some(n) => {
                    let b = self.raw(n)?.to_vec();
                    fixed_value(t, b)
                }
                None => return err(format!("unknown type {t:#x} @{:#x}", self.p)),
            },
        })
    }

    // ---- variant B ----
    fn run_b(&mut self) -> R<Value> {
        self.u32()?; // magic
        self.u32()?; // ver 0
        self.u32()?; // 2
        let t = self.u8()? as u32;
        self.value_b(t)
    }

    fn value_b(&mut self, t: u32) -> R<Value> {
        Ok(match t {
            0x01 => Value::Float(self.f32()?),
            0x03 => {
                let b = self.raw(12)?.to_vec();
                fixed_value(0x03, b)
            }
            0x09 => Value::Int(self.u64()? as i64),
            0x0A => Value::Bool(self.u8()? != 0),
            0x0B => {
                let b = self.raw(4)?;
                Value::QUint(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            }
            0x0C | 0x0F => Value::Uint(self.u64()?),
            0x10 => decode_bytes(self.bytes()?),
            0x11 | 0x13 => {
                let _end = self.u32()?;
                let n = self.u32()? as usize;
                if n > (1 << 22) {
                    return err("count too large");
                }
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    let et = self.u8()? as u32;
                    v.push(self.value_b(et)?);
                }
                Value::List(v)
            }
            0x12 | 0x14 => {
                let flag = self.u8()?;
                if flag != 0 {
                    return Ok(Value::Null);
                }
                let _end = self.u32()?;
                let class = self.name()?;
                let n = self.u16()? as usize;
                let mut o = Object { class, props: Vec::with_capacity(n) };
                for _ in 0..n {
                    let k = self.name()?;
                    let pt = self.u8()? as u32;
                    let v = self.value_b(pt)?;
                    o.props.push((k, v));
                }
                Value::Object(o)
            }
            t => match fixed_b(t) {
                Some(n) => {
                    let b = self.raw(n)?.to_vec();
                    fixed_value(t, b)
                }
                None => return err(format!("unknown type {t:#x} @{:#x}", self.p - 1)),
            },
        })
    }
}

fn fixed_value(t: u32, b: Vec<u8>) -> Value {
    Value::Raw(t, b)
}

/// QVariant (big-endian) UInt / QString, ASCII text, else raw bytes.
fn decode_bytes(bs: &[u8]) -> Value {
    if bs.len() >= 5 && bs[4] == 0 {
        let t = u32::from_be_bytes([bs[0], bs[1], bs[2], bs[3]]);
        if t == 3 && bs.len() == 9 {
            return Value::QUint(u32::from_be_bytes([bs[5], bs[6], bs[7], bs[8]]));
        }
        if t == 10 && bs.len() >= 9 {
            let n = u32::from_be_bytes([bs[5], bs[6], bs[7], bs[8]]) as usize;
            if 9 + n == bs.len() && n % 2 == 0 {
                let u: Vec<u16> = bs[9..].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                return Value::Str(String::from_utf16_lossy(&u));
            }
        }
    }
    match std::str::from_utf8(bs) {
        Ok(s) if s.chars().all(|c| (' '..='~').contains(&c)) => Value::Str(s.to_string()),
        _ => Value::Bytes(bs.to_vec()),
    }
}
