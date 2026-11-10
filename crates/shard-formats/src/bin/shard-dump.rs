//! `shard-dump FILE [member]`: list container members / decode one member to JSON.
//! With `--all`, decode every Alg member and report OK/FAIL counts.
use shard_formats::{alg, Container};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: shard-dump FILE [member | --all]");
        std::process::exit(2);
    }
    let mut c = match Container::open(&args[0]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    // `shard-dump FILE member --raw OUT`: the bytes of the member as they are stored
    if let (Some(member), Some("--raw"), Some(out)) = (args.get(1), args.get(2).map(String::as_str), args.get(3)) {
        match c.read(member) {
            Ok(b) => {
                std::fs::write(out, &b).expect("write");
                println!("wrote {} bytes to {out}", b.len());
            }
            Err(e) => {
                eprintln!("read failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    match args.get(1).map(String::as_str) {
        None => {
            for d in c.members() {
                println!("{:>10}  {}", d.byte_len(), d.path);
            }
        }
        Some("--all") => {
            let paths: Vec<(String, u64)> = c.members().map(|d| (d.path.clone(), d.byte_len())).collect();
            let (mut ok, mut bad, mut other) = (0, 0, 0);
            for (p, n) in paths {
                if n > 64_000_000 {
                    continue;
                }
                let b = match c.read(&p) {
                    Ok(b) => b,
                    Err(e) => {
                        println!("READ-FAIL {p}: {e}");
                        bad += 1;
                        continue;
                    }
                };
                if !alg::is_alg(&b) {
                    other += 1;
                    continue;
                }
                match alg::decode(&b) {
                    Ok(_) => {
                        ok += 1;
                        println!("OK        {n:>9}  {p}");
                    }
                    Err(e) => {
                        bad += 1;
                        println!("FAIL      {n:>9}  {p}: {e}");
                    }
                }
            }
            println!("alg-ok={ok} alg-bad={bad} non-alg={other}");
        }
        Some(m) => match c.decode(m) {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v.to_json()).unwrap()),
            Err(e) => {
                eprintln!("decode failed: {e}");
                std::process::exit(1);
            }
        },
    }
}
