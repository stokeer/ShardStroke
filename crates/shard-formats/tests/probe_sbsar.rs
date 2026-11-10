//! Diagnostic: dump the xml interface of every .sbsar of a project (SHARD_PROBE_SPP, SHARD_OUT_DIR).
use shard_formats::Container;

#[test]
fn dump_interfaces() {
    let (Ok(p), Ok(out)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_OUT_DIR")) else { return };
    let mut c = Container::open(&p).unwrap();
    let names: Vec<String> = c.members().map(|d| d.path.clone()).filter(|n| n.ends_with(".sbsar")).collect();
    println!("sbsar members: {}", names.len());
    for n in names {
        let b = c.read(&n).unwrap();
        match shard_formats::sbsar::extract_content(&b) {
            Ok(files) => {
                for (name, data) in files {
                    if name.ends_with(".xml") {
                        let f = std::path::Path::new(&out).join(name.rsplit('/').next().unwrap());
                        std::fs::write(f, data).unwrap();
                    }
                }
            }
            Err(e) => println!("{n}: {e}"),
        }
    }
}

#[test]
fn parse_dumped_xmls() {
    let Ok(out) = std::env::var("SHARD_OUT_DIR") else { return };
    let mut n = 0;
    for e in std::fs::read_dir(&out).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map_or(true, |x| x != "xml") {
            continue;
        }
        let i = shard_formats::sbsar::Interface::parse(&std::fs::read_to_string(&p).unwrap()).unwrap();
        println!("{:30} '{}' params={} groups={}", p.file_name().unwrap().to_string_lossy(), i.label, i.params.len(), i.groups.len());
        n += 1;
    }
    assert!(n > 0);
}
