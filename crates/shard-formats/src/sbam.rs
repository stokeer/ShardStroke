//! `SBAM` (`.sbsasm` inside a `.sbsar`): the compiled procedural graph. Layout notes in `docs/SBAM.md`.
//!
//! * [`Sbam::parse`]: header and node table (each node's words, its class byte).
//! * [`eval_function`]: the stack-machine bytecode the node parameters are written in.
//!
//! Read from the data file only (word comparison between graphs of known behaviour).

use std::collections::HashMap;

/// Where the node offsets are relative to.
const NODE_BASE: usize = 0x34;
/// First entry of the node offset table.
const NODE_TABLE: usize = 0x38;

#[derive(Debug, Clone)]
pub struct Node {
    /// File offset of the node's first word.
    pub offset: usize,
    /// Low byte of the first word: the node class (`0x02` blend, `0x16` blur, `0x1e` levels, `0x20` image input,
    /// `0x28` pixel processor, `0x04` FX-map...).
    pub class: u8,
    /// The node's words, up to the next node.
    pub words: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Sbam {
    pub version: u32,
    pub asm_uid: u32,
    pub nodes: Vec<Node>,
    words: Vec<u32>,
}

impl Sbam {
    pub fn parse(b: &[u8]) -> Result<Sbam, String> {
        if b.len() < NODE_TABLE + 4 || &b[..4] != b"SBAM" {
            return Err("not an SBAM graph".into());
        }
        let words: Vec<u32> = b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let w = |o: usize| words.get(o / 4).copied();
        let size = w(0x10).unwrap_or(0) as usize;
        if size > b.len() {
            return Err(format!("truncated: {} of {size} bytes", b.len()));
        }
        // The node table: a run of growing offsets (relative to 0x34), one flag word, then the number of entries
        // minus one, and its first entry points at a node word. It sits right after the header in small graphs, after
        // a data block in some version-5 ones (~32 KB in `3D_perlin_noise`) and after the nodes in others
        // (`alpha_dirt_03`).
        let looks_like_node = |x: u32| matches!((x >> 16) & 0xF, 8 | 9);
        let n = words.len();
        let run_at = |t: usize| -> Option<Vec<usize>> {
            let first = words[t];
            if first == 0 || first as usize + NODE_BASE >= b.len() || !w(NODE_BASE + first as usize).is_some_and(looks_like_node) {
                return None;
            }
            let mut r = 1;
            while t + r < n && words[t + r] > words[t + r - 1] && (words[t + r] as usize) + NODE_BASE < b.len() {
                r += 1;
            }
            // the flag word may itself continue the run (`0x1c0` after `0x240` does not, `0x1c0` after `0x100` would)
            let len = if t + r + 1 < n && words[t + r + 1] as usize + 1 == r {
                r
            } else if r >= 2 && t + r < n && words[t + r] as usize + 1 == r - 1 {
                r - 1
            } else {
                return None;
            };
            (len >= 2).then(|| words[t..t + len].iter().map(|v| NODE_BASE + *v as usize).collect())
        };
        let (table, offs) = (NODE_TABLE / 4..n.saturating_sub(2))
            .find_map(|t| run_at(t).filter(|o| o.iter().all(|x| *x < b.len())).map(|o| (t * 4, o)))
            .ok_or("no node table")?;
        let end_of_nodes = (NODE_BASE + w(0x2c).unwrap_or(0) as usize).min(b.len());
        let nodes = offs
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let last_end = if table > s { table } else { end_of_nodes.max(s) };
                let e = offs.get(i + 1).copied().unwrap_or(last_end).min(b.len());
                let ws = words[s / 4..e / 4].to_vec();
                Node { offset: s, class: ws.first().map_or(0, |x| (*x & 0xFF) as u8), words: ws }
            })
            .collect();
        Ok(Sbam { version: w(4).unwrap_or(0), asm_uid: w(0x0c).unwrap_or(0), nodes, words })
    }

    /// The graph's words from a file offset (for [`eval_function`]).
    pub fn words_at(&self, offset: usize) -> &[u32] {
        &self.words[(offset / 4).min(self.words.len())..]
    }
}

/// Values the functions read: interface parameters by `uid`.
pub type Params = HashMap<u32, f32>;

const PUSH_CONST: u32 = 0x0900;
const PUSH_PARAM: u32 = 0x0902;
/// Integer and boolean interface parameters (read as floats here).
const PUSH_INT_PARAM: u32 = 0x0a02;
const PUSH_BOOL_PARAM: u32 = 0x0842;
const IMMEDIATE: u32 = 0x0000_0d00;
const ADD: u32 = 0x0912;
const SUB: u32 = 0x0913;
const MUL: u32 = 0x0914;
const DIV: u32 = 0x0915;
const MIN: u32 = 0x0930;
const MAX: u32 = 0x0931;
const NEG: u32 = 0x0523;
const EQ: u32 = 0x085d;
const SELECT: u32 = 0x0d09;

/// Evaluate one parameter function (stack machine, `docs/SBAM.md`). Returns the value and the number of words it
/// took, or the first word it cannot read.
pub fn eval_function(words: &[u32], params: &Params) -> Result<(f32, usize), String> {
    let f = f32::from_bits;
    let mut st: Vec<f32> = Vec::new();
    let mut imm: Option<f32> = None;
    let mut i = 0;
    let pop = |st: &mut Vec<f32>, i: usize| st.pop().ok_or_else(|| format!("empty stack at word {i}"));
    while i < words.len() {
        let w = words[i];
        let (hi, lo) = (w >> 16, w & 0xFFFF);
        if hi == PUSH_CONST {
            st.push(f(*words.get(i + 1).ok_or("truncated constant")?));
            i += 2;
            continue;
        }
        if hi == PUSH_PARAM || hi == PUSH_INT_PARAM || hi == PUSH_BOOL_PARAM {
            let uid = *words.get(i + 1).ok_or("truncated parameter")?;
            st.push(*params.get(&uid).ok_or_else(|| format!("unknown parameter uid {uid}"))?);
            i += 2;
            continue;
        }
        if w == IMMEDIATE {
            imm = Some(f(*words.get(i + 1).ok_or("truncated immediate")?));
            i += 2;
            continue;
        }
        if (hi == 0 && lo < 0x100) || w == 0x0a42_0001 {
            // end of the function (an explicit count word, or the node's output-size reference right after it)
            return Ok((pop(&mut st, i)?, if w == 0x0a42_0001 { i } else { i + 1 }));
        }
        i += 1;
        if hi < 0x100 && lo < 0x100 {
            // conversion (float cast): no effect on a float
            continue;
        }
        let (op, low) = if hi >= 0x100 { (hi, false) } else { (lo, true) };
        if op == NEG {
            let a = pop(&mut st, i)?;
            st.push(-a);
            imm = None;
            continue;
        }
        if op == SELECT {
            // cond, a (stack) and b (immediate or stack): cond != 0 ? a : b
            let b = match imm.take() {
                Some(b) => b,
                None => pop(&mut st, i)?,
            };
            let a = pop(&mut st, i)?;
            let c = pop(&mut st, i)?;
            st.push(if c != 0.0 { a } else { b });
            continue;
        }
        let (a, b) = match imm.take() {
            Some(b) => (pop(&mut st, i)?, b),
            None => {
                let b = pop(&mut st, i)?;
                (pop(&mut st, i)?, b)
            }
        };
        st.push(match op {
            ADD => a + b,
            // the two encodings of the subtraction take their operands the other way round
            SUB if low => b - a,
            SUB => a - b,
            MUL => a * b,
            DIV => a / b,
            MIN => a.min(b),
            MAX => a.max(b),
            EQ => (a == b) as i32 as f32,
            _ => return Err(format!("unknown operation {w:#010x} at word {}", i - 1)),
        });
    }
    Err("function without end".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: u32 = 1001;
    const L: u32 = 1002;

    fn k(v: f32) -> u32 {
        v.to_bits()
    }

    /// clamp(C / 2, 0, 0.5), written by hand with the opcodes of the table.
    #[test]
    fn clamp_of_a_scaled_parameter() {
        let prog = [0x0902_0001, C, 0x0d00, k(2.0), 0x0000_0915, 0x0900_0002, k(0.5), 0x0003_0930, 0x0900_0004, k(0.0), 0x0005_0931, 6];
        for (c, want) in [(0.4f32, 0.2f32), (-0.3, 0.0), (1.6, 0.5)] {
            let p = Params::from([(C, c)]);
            let (v, used) = eval_function(&prog, &p).unwrap();
            assert!((v - want).abs() < 1e-7, "C {c}: {v} vs {want}");
            assert_eq!(used, prog.len());
        }
    }

    /// The two subtraction encodings and the negation: (L + 1) - (-(min(C, 0) / 2)) and 1 - C / 2.
    #[test]
    fn subtraction_order_and_negation() {
        let high = [0x0902_0001, L, 0x0d00, k(1.0), 0x0000_0912, 0x0902_0002, C, 0x0d00, k(0.0), 0x0000_0930, 0x0900_0003, k(2.0), 0x0004_0915, 0x0523_0005, 0x0913_0006, 7];
        let p = Params::from([(C, -0.5), (L, 0.25)]);
        assert!((eval_function(&high, &p).unwrap().0 - (1.25 - 0.25)).abs() < 1e-7);
        let low = [0x0902_0001, C, 0x0d00, k(2.0), 0x0000_0915, 0x0900_0002, k(1.0), 0x0003_0913, 4];
        let p = Params::from([(C, 0.4)]);
        assert!((eval_function(&low, &p).unwrap().0 - 0.8).abs() < 1e-7);
    }

    #[test]
    fn unknown_parameter_and_operation_are_errors() {
        assert!(eval_function(&[0x0902_0001, 77, 2], &Params::new()).is_err());
        assert!(eval_function(&[0x0900_0001, k(1.0), 0x0900_0002, k(1.0), 0x0003_0999, 4], &Params::new()).is_err());
    }

    /// `SHARD_SBAM_DIR=<folder of extracted .sbsasm + .xml> cargo test -p shard-formats sbam_corpus -- --ignored
    /// --nocapture`: every graph parses into nodes, and the levels functions of `contrast_luminosity` give the
    /// measured semantics.
    #[test]
    #[ignore]
    fn sbam_corpus() {
        let Some(dir) = std::env::var_os("SHARD_SBAM_DIR") else { return };
        let mut graphs = 0;
        let mut by_version = std::collections::BTreeMap::new();
        let mut census = std::collections::BTreeMap::new();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("sbsasm") {
                continue;
            }
            let b = std::fs::read(&p).unwrap();
            let version = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) >> 16;
            let e = by_version.entry(version).or_insert((0, 0, Vec::new()));
            match Sbam::parse(&b) {
                Ok(g) if !g.nodes.is_empty() => {
                    for nd in &g.nodes {
                        let c = census.entry(nd.class).or_insert((0usize, std::collections::BTreeSet::new()));
                        c.0 += 1;
                        c.1.insert(p.file_stem().unwrap().to_string_lossy().into_owned());
                    }
                    e.0 += 1;
                    graphs += 1;
                }
                _ => {
                    e.1 += 1;
                    e.2.push(p.file_stem().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        for (v, (ok, bad, names)) in &by_version {
            eprintln!("version {v}: {ok} parsed, {bad} not ({:?})", &names[..names.len().min(4)]);
        }
        eprintln!("{graphs} graphs parsed");
        let mut rows: Vec<_> = census.iter().collect();
        rows.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
        for (class, (n, gs)) in rows {
            let smallest = gs.iter().min_by_key(|g| std::fs::metadata(std::path::Path::new(&dir).join(format!("{g}.sbsasm"))).map(|m| m.len()).unwrap_or(u64::MAX));
            eprintln!("class {class:#04x}: {n} nodes in {} graphs, smallest {smallest:?}", gs.len());
        }
        let path = std::path::Path::new(&dir).join("contrast_luminosity.sbsasm");
        let Ok(b) = std::fs::read(&path) else { return };
        let xml = std::fs::read_to_string(path.with_extension("xml")).unwrap();
        let uid = |id: &str| -> u32 {
            let at = xml.find(&format!("identifier=\"{id}\"")).unwrap();
            let head = &xml[..at];
            let u = head.rfind("uid=\"").unwrap() + 5;
            head[u..u + head[u..].find('"').unwrap()].parse().unwrap()
        };
        let g = Sbam::parse(&b).unwrap();
        let lv = g.nodes.iter().find(|n| n.class == 0x1e).expect("levels node");
        // the levels node's four function offsets follow its class, atomic id, input and output offset words
        let starts: Vec<usize> = lv.words[4..8].iter().map(|o| NODE_BASE + *o as usize).collect();
        for c in [-0.9f32, -0.5, 0.0, 0.17, 0.4, 0.9] {
            for l in [-0.7f32, 0.0, 0.3] {
                let p = Params::from([(uid("Contrast"), c), (uid("Luminosity"), l)]);
                let got: Vec<f32> = starts.iter().map(|s| eval_function(g.words_at(*s), &p).unwrap().0).collect();
                let (ilo, ihi) = if c > 0.0 { (c / 2.0, 1.0 - c / 2.0) } else { (0.0, 1.0) };
                let (olo, ohi) = if c < 0.0 { (-c / 2.0, 1.0 + c / 2.0) } else { (0.0, 1.0) };
                let want = [ilo, ihi, (olo + l).clamp(0.0, 1.0), (ohi + l).clamp(0.0, 1.0)];
                for (g, w) in got.iter().zip(want) {
                    assert!((g - w).abs() < 1e-6, "C {c} L {l}: {got:?} vs {want:?}");
                }
            }
        }
        eprintln!("contrast_luminosity: levels functions match");
    }
}
