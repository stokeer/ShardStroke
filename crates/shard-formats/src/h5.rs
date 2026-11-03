//! Minimal read-only HDF5 reader.
//!
//! Scope: exactly what Painter container files use (observed): superblock v0, old-style groups
//! (symbol table + local heap + v1 B-tree), object header v1, datasets with contiguous, compact or
//! chunked layout (unfiltered, rank 1). Attributes are ignored. Anything else returns
//! [`Error::Unsupported`] instead of guessing.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Format(String),
    Unsupported(String),
    NotFound(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Format(s) => write!(f, "hdf5 format: {s}"),
            Error::Unsupported(s) => write!(f, "hdf5 unsupported: {s}"),
            Error::NotFound(s) => write!(f, "not found: {s}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

const UNDEF: u64 = u64::MAX;

#[derive(Debug, Clone)]
pub enum Layout {
    Compact(Vec<u8>),
    Contiguous { addr: u64, size: u64 },
    Chunked { btree: u64, chunk: Vec<u64>, elem: u64 },
}

#[derive(Debug, Clone)]
pub struct Dataset {
    pub path: String,
    pub dims: Vec<u64>,
    pub elem_size: u64,
    pub layout: Layout,
}

impl Dataset {
    pub fn byte_len(&self) -> u64 {
        self.dims.iter().product::<u64>() * self.elem_size
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Group(String),
    Dataset(Dataset),
}

pub struct H5File<R: Read + Seek> {
    r: R,
    so: usize, // size of offsets
    sl: usize, // size of lengths
    base: u64,
    root_header: u64,
}

impl H5File<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path)?)
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Cursor<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.p];
        self.p += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes(self.b[self.p..self.p + 2].try_into().unwrap());
        self.p += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        v
    }
    fn uint(&mut self, n: usize) -> u64 {
        let mut v = 0u64;
        for i in 0..n {
            v |= (self.b[self.p + i] as u64) << (8 * i);
        }
        self.p += n;
        v
    }
}

impl<R: Read + Seek> H5File<R> {
    pub fn new(mut r: R) -> Result<Self> {
        let mut sig_off = None;
        for off in [0u64, 512, 1024, 2048, 4096] {
            let mut sig = [0u8; 8];
            if r.seek(SeekFrom::Start(off)).is_ok() && r.read_exact(&mut sig).is_ok() && sig == *b"\x89HDF\r\n\x1a\n" {
                sig_off = Some(off);
                break;
            }
        }
        let off = sig_off.ok_or_else(|| Error::Format("no HDF5 signature".into()))?;
        r.seek(SeekFrom::Start(off))?;
        let mut head = [0u8; 24];
        r.read_exact(&mut head)?;
        let sb_ver = head[8];
        if sb_ver > 1 {
            return Err(Error::Unsupported(format!("superblock version {sb_ver}")));
        }
        let so = head[13] as usize;
        let sl = head[14] as usize;
        // after the 24-byte head: (v1: +4 bytes indexed K) then 4 addresses then root STE
        let mut rest = vec![0u8; 4 * so + 4 + 8 + 2 * so + 16 + 8];
        r.read_exact(&mut rest)?;
        let mut c = Cursor { b: &rest, p: 0 };
        if sb_ver == 1 {
            c.p += 4;
        }
        let base = c.uint(so);
        let _free = c.uint(so);
        let _eof = c.uint(so);
        let _drv = c.uint(so);
        // root symbol table entry: name offset, header addr
        let _name = c.uint(so);
        let root_header = c.uint(so);
        Ok(Self { r, so, sl, base: base + off, root_header })
    }

    fn read_at(&mut self, addr: u64, len: usize) -> Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        self.r.seek(SeekFrom::Start(self.base + addr))?;
        self.r.read_exact(&mut v)?;
        Ok(v)
    }

    /// Parse an object header (v1) into (message type, bytes) pairs.
    fn header_messages(&mut self, addr: u64) -> Result<Vec<(u16, Vec<u8>)>> {
        let h = self.read_at(addr, 16)?;
        if h[0] != 1 {
            return Err(Error::Unsupported(format!("object header version {}", h[0])));
        }
        let nmsgs = u16::from_le_bytes([h[2], h[3]]) as usize;
        let hsize = u32::from_le_bytes([h[8], h[9], h[10], h[11]]) as usize;
        let mut blocks = vec![(addr + 16, hsize)];
        let mut out = Vec::new();
        let mut bi = 0;
        while bi < blocks.len() && out.len() < nmsgs + 64 {
            let (baddr, blen) = blocks[bi];
            bi += 1;
            let data = self.read_at(baddr, blen)?;
            let mut p = 0usize;
            while p + 8 <= data.len() {
                let t = u16::from_le_bytes([data[p], data[p + 1]]);
                let sz = u16::from_le_bytes([data[p + 2], data[p + 3]]) as usize;
                p += 8;
                if p + sz > data.len() {
                    break;
                }
                let body = data[p..p + sz].to_vec();
                p += sz;
                if t == 0x10 {
                    let mut c = Cursor { b: &body, p: 0 };
                    let a = c.uint(self.so);
                    let l = c.uint(self.sl);
                    blocks.push((a, l as usize));
                } else if t != 0 {
                    out.push((t, body));
                }
            }
        }
        Ok(out)
    }

    fn heap_name(&mut self, heap_addr: u64, off: u64) -> Result<String> {
        let h = self.read_at(heap_addr, 8 + 2 * self.sl + self.so)?;
        if &h[0..4] != b"HEAP" {
            return Err(Error::Format("bad local heap".into()));
        }
        let mut c = Cursor { b: &h, p: 8 };
        let _size = c.uint(self.sl);
        let _free = c.uint(self.sl);
        let data = c.uint(self.so);
        let mut out = Vec::new();
        let mut a = data + off;
        loop {
            let chunk = self.read_at(a, 64).or_else(|_| self.read_at(a, 1))?;
            if let Some(z) = chunk.iter().position(|&b| b == 0) {
                out.extend_from_slice(&chunk[..z]);
                break;
            }
            out.extend_from_slice(&chunk);
            a += chunk.len() as u64;
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Collect (name offset, object header addr) of all symbol-table entries below a group B-tree.
    fn group_entries(&mut self, tree: u64, out: &mut Vec<(u64, u64)>) -> Result<()> {
        let head = self.read_at(tree, 8)?;
        if &head[0..4] != b"TREE" {
            return Err(Error::Format("bad group B-tree".into()));
        }
        let level = head[5];
        let used = u16::from_le_bytes([head[6], head[7]]) as usize;
        let body = self.read_at(tree + 8, 2 * self.so + (2 * used + 1) * self.sl.max(self.so))?;
        let mut c = Cursor { b: &body, p: 2 * self.so };
        let mut kids = Vec::new();
        let _k0 = c.uint(self.sl);
        for _ in 0..used {
            kids.push(c.uint(self.so));
            let _key = c.uint(self.sl);
        }
        for k in kids {
            if level > 0 {
                self.group_entries(k, out)?;
            } else {
                let sn = self.read_at(k, 8)?;
                if &sn[0..4] != b"SNOD" {
                    return Err(Error::Format("bad SNOD".into()));
                }
                let n = u16::from_le_bytes([sn[6], sn[7]]) as usize;
                let esz = 2 * self.so + 8 + 16;
                let eb = self.read_at(k + 8, n * esz)?;
                let mut c = Cursor { b: &eb, p: 0 };
                for _ in 0..n {
                    let name = c.uint(self.so);
                    let hdr = c.uint(self.so);
                    c.p += 4 + 4 + 16;
                    out.push((name, hdr));
                }
            }
        }
        Ok(())
    }

    fn classify(&mut self, path: String, hdr: u64) -> Result<Node> {
        let msgs = self.header_messages(hdr)?;
        if msgs.iter().any(|(t, _)| *t == 0x11) {
            return Ok(Node::Group(path));
        }
        let mut dims = Vec::new();
        let mut elem = 1u64;
        let mut layout = None;
        for (t, b) in &msgs {
            match *t {
                0x01 => {
                    let ver = b[0];
                    let rank = b[1] as usize;
                    let start = if ver == 1 { 8 } else { 4 };
                    let mut c = Cursor { b, p: start };
                    for _ in 0..rank {
                        dims.push(c.uint(self.sl));
                    }
                }
                0x03 => {
                    elem = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as u64;
                }
                0x08 => {
                    let ver = b[0];
                    let mut c = Cursor { b, p: 1 };
                    if ver == 3 {
                        let class = c.u8();
                        layout = Some(match class {
                            0 => {
                                let n = c.u16() as usize;
                                Layout::Compact(b[c.p..c.p + n].to_vec())
                            }
                            1 => {
                                let addr = c.uint(self.so);
                                let size = c.uint(self.sl);
                                Layout::Contiguous { addr, size }
                            }
                            2 => {
                                let dim = c.u8() as usize;
                                let btree = c.uint(self.so);
                                let mut chunk = Vec::new();
                                for _ in 0..dim {
                                    chunk.push(c.u32() as u64);
                                }
                                let e = chunk.pop().unwrap_or(1);
                                Layout::Chunked { btree, chunk, elem: e }
                            }
                            x => return Err(Error::Unsupported(format!("layout class {x}"))),
                        });
                    } else {
                        // v1/v2: dimensionality, class, reserved(5), [addr], dims...
                        let dim = c.u8() as usize;
                        let class = c.u8();
                        c.p += 5;
                        layout = Some(match class {
                            1 => {
                                let addr = c.uint(self.so);
                                let size: u64 = dims.iter().product::<u64>() * elem;
                                Layout::Contiguous { addr, size }
                            }
                            2 => {
                                let btree = c.uint(self.so);
                                let mut chunk = Vec::new();
                                for _ in 0..dim {
                                    chunk.push(c.u32() as u64);
                                }
                                let e = chunk.pop().unwrap_or(1);
                                Layout::Chunked { btree, chunk, elem: e }
                            }
                            x => return Err(Error::Unsupported(format!("layout v{ver} class {x}"))),
                        });
                    }
                }
                _ => {}
            }
        }
        match layout {
            Some(layout) => Ok(Node::Dataset(Dataset { path, dims, elem_size: elem, layout })),
            None => Ok(Node::Group(path)), // empty/unknown object: treat as group
        }
    }

    /// List the immediate children of a group (`""` or `"/"` = root).
    pub fn list(&mut self, group_path: &str) -> Result<Vec<(String, Node)>> {
        let mut hdr = self.root_header;
        for part in group_path.split('/').filter(|s| !s.is_empty()) {
            let kids = self.children(hdr)?;
            hdr = kids
                .into_iter()
                .find(|(n, _)| n == part)
                .map(|(_, h)| h)
                .ok_or_else(|| Error::NotFound(group_path.to_string()))?;
        }
        let kids = self.children(hdr)?;
        let prefix = group_path.trim_matches('/');
        let mut out = Vec::new();
        for (name, h) in kids {
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            out.push((name, self.classify(path, h)?));
        }
        Ok(out)
    }

    fn children(&mut self, group_hdr: u64) -> Result<Vec<(String, u64)>> {
        let msgs = self.header_messages(group_hdr)?;
        let st = msgs
            .iter()
            .find(|(t, _)| *t == 0x11)
            .ok_or_else(|| Error::Format("not a group".into()))?;
        let mut c = Cursor { b: &st.1, p: 0 };
        let tree = c.uint(self.so);
        let heap = c.uint(self.so);
        let mut ents = Vec::new();
        self.group_entries(tree, &mut ents)?;
        let mut out = Vec::new();
        for (noff, hdr) in ents {
            out.push((self.heap_name(heap, noff)?, hdr));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Recursively list every dataset in the file.
    pub fn datasets(&mut self) -> Result<Vec<Dataset>> {
        let mut out = Vec::new();
        let mut stack = vec![String::new()];
        while let Some(g) = stack.pop() {
            for (_, n) in self.list(&g)? {
                match n {
                    Node::Group(p) => stack.push(p),
                    Node::Dataset(d) => out.push(d),
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    pub fn dataset(&mut self, path: &str) -> Result<Dataset> {
        let (dir, name) = match path.rsplit_once('/') {
            Some((d, n)) => (d, n),
            None => ("", path),
        };
        for (n, node) in self.list(dir)? {
            if n == name {
                if let Node::Dataset(d) = node {
                    return Ok(d);
                }
            }
        }
        Err(Error::NotFound(path.to_string()))
    }

    /// Read at most the first `n` bytes of a dataset (cheap for contiguous layouts).
    pub fn read_prefix(&mut self, ds: &Dataset, n: usize) -> Result<Vec<u8>> {
        let total = ds.byte_len() as usize;
        let n = n.min(total);
        match &ds.layout {
            Layout::Contiguous { addr, .. } if *addr != UNDEF => self.read_at(*addr, n),
            Layout::Compact(d) => Ok(d[..n.min(d.len())].to_vec()),
            _ => {
                let mut v = self.read(ds)?;
                v.truncate(n);
                Ok(v)
            }
        }
    }

    /// Read the raw bytes of a dataset.
    pub fn read(&mut self, ds: &Dataset) -> Result<Vec<u8>> {
        let total = ds.byte_len() as usize;
        match &ds.layout {
            Layout::Compact(d) => Ok(d.clone()),
            Layout::Contiguous { addr, .. } => {
                if *addr == UNDEF || total == 0 {
                    return Ok(vec![0; total]);
                }
                self.read_at(*addr, total)
            }
            Layout::Chunked { btree, chunk, elem } => {
                if ds.dims.len() != 1 || chunk.len() != 1 {
                    return Err(Error::Unsupported("chunked datasets with rank != 1".into()));
                }
                let mut out = vec![0u8; total];
                if *btree != UNDEF {
                    self.read_chunks(*btree, chunk[0] * *elem, *elem, &mut out)?;
                }
                Ok(out)
            }
        }
    }

    fn read_chunks(&mut self, tree: u64, chunk_bytes: u64, elem: u64, out: &mut [u8]) -> Result<()> {
        let head = self.read_at(tree, 8)?;
        if &head[0..4] != b"TREE" {
            return Err(Error::Format("bad chunk B-tree".into()));
        }
        let level = head[5];
        let used = u16::from_le_bytes([head[6], head[7]]) as usize;
        // key: size u32, filter mask u32, offsets: (rank+1) u64 ; child: O
        let key_sz = 8 + 2 * 8; // rank 1 -> offsets for dim0 and the element dim
        let body = self.read_at(tree + 8 + 2 * self.so as u64, used * (key_sz + self.so) + key_sz)?;
        let mut c = Cursor { b: &body, p: 0 };
        for _ in 0..used {
            let size = c.u32() as u64;
            let mask = c.u32();
            let off = c.uint(8);
            let _e = c.uint(8);
            let child = c.uint(self.so);
            if level > 0 {
                self.read_chunks(child, chunk_bytes, elem, out)?;
            } else {
                if mask != 0 {
                    return Err(Error::Unsupported("filtered chunk".into()));
                }
                let data = self.read_at(child, size as usize)?;
                let start = (off * elem) as usize;
                if start < out.len() {
                    let n = data.len().min(out.len() - start);
                    out[start..start + n].copy_from_slice(&data[..n]);
                }
            }
        }
        Ok(())
    }
}
