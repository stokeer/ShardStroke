//! `sbsar_probe FILE.spp MEMBER OUT`: save a project member and report how the sbsar reader sees it.
use shard_formats::Container;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut c = Container::open(&a[0]).expect("open");
    let bytes = c.read(&a[1]).expect("member");
    std::fs::write(&a[2], &bytes).unwrap();
    println!("{} bytes, head {:02x?}", bytes.len(), &bytes[..32.min(bytes.len())]);
    match shard_formats::sbsar::Interface::from_sbsar(&bytes) {
        Ok(i) => println!("interface ok: {} params, outputs {:?}", i.params.len(), i.outputs),
        Err(e) => println!("interface failed: {e}"),
    }
    match sevenz_rust2::ArchiveReader::new(std::io::Cursor::new(&bytes), sevenz_rust2::Password::empty()) {
        Ok(mut r) => {
            let res = r.for_each_entries(|e, _| {
                println!("entry {} ({} bytes)", e.name(), e.size());
                Ok(true)
            });
            println!("for_each: {res:?}");
        }
        Err(e) => println!("7z open failed: {e}"),
    }
}
