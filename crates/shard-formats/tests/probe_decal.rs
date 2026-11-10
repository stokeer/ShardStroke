//! Diagnostic: print the raw properties of the first objects of class SHARD_CLASS (SHARD_PROBE_SPP).
use shard_formats::alg::{Object, Value};
use shard_formats::Container;

fn show(v: &Value, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    match v {
        Value::Object(o) => {
            out.push(format!("{pad}<{}>", o.class));
            for (k, x) in &o.props {
                match x {
                    Value::Object(_) | Value::List(_) => {
                        out.push(format!("{pad} .{k}:"));
                        show(x, depth + 2, out);
                    }
                    Value::Raw(t, b) => out.push(format!("{pad} .{k} = Raw(type {t}, {} bytes) {:02x?}", b.len(), &b[..b.len().min(48)])),
                    Value::Bytes(b) => out.push(format!("{pad} .{k} = Bytes({}) {:02x?}", b.len(), &b[..b.len().min(32)])),
                    other => out.push(format!("{pad} .{k} = {other:?}")),
                }
            }
        }
        Value::List(l) => {
            out.push(format!("{pad}[list of {}]", l.len()));
            for x in l.iter().take(2) {
                show(x, depth + 1, out);
            }
        }
        other => out.push(format!("{pad}{other:?}")),
    }
}

fn find<'a>(v: &'a Value, class: &str, out: &mut Vec<&'a Object>) {
    match v {
        Value::Object(o) => {
            if o.class == class {
                out.push(o);
            }
            for (_, x) in &o.props {
                find(x, class, out);
            }
        }
        Value::List(l) => l.iter().for_each(|x| find(x, class, out)),
        _ => {}
    }
}

#[test]
fn dump_class() {
    let (Ok(p), Ok(class)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_CLASS")) else { return };
    let mut c = Container::open(&p).unwrap();
    let root = c.decode("paint/document.bin").unwrap();
    let mut hits = Vec::new();
    find(&root, &class, &mut hits);
    if let Ok(label) = std::env::var("SHARD_LABEL") {
        hits.retain(|o| matches!(o.get("label"), Some(Value::Str(l)) if *l == label));
    }
    println!("{} objects of class {class}", hits.len());
    for o in hits.iter().take(2) {
        let mut lines = Vec::new();
        show(&Value::Object((*o).clone()), 0, &mut lines);
        for l in lines.iter().take(400) {
            println!("{l}");
        }
    }
}

/// SHARD_HIST=1: distinct scalar-property tuples (minus uid/identifier) of every SHARD_CLASS object.
#[test]
fn histogram_class() {
    let (Ok(p), Ok(class), Ok(_)) = (
        std::env::var("SHARD_PROBE_SPP"),
        std::env::var("SHARD_CLASS"),
        std::env::var("SHARD_HIST"),
    ) else {
        return;
    };
    let mut c = Container::open(&p).unwrap();
    let root = c.decode("paint/document.bin").unwrap();
    let mut hits = Vec::new();
    find(&root, &class, &mut hits);
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for o in hits {
        let mut key = Vec::new();
        for (k, x) in &o.props {
            if matches!(k.as_str(), "uid" | "identifier") {
                continue;
            }
            match x {
                Value::Object(_) | Value::List(_) => {}
                Value::Raw(t, b) => key.push(format!("{k}=Raw({t}){:02x?}", &b[..b.len().min(8)])),
                other => key.push(format!("{k}={other:?}")),
            }
        }
        *counts.entry(key.join(" ")).or_default() += 1;
    }
    for (k, n) in counts {
        println!("{n:5}  {k}");
    }
}
