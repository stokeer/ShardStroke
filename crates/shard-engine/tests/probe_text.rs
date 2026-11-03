//! Renders a text stencil to a PNG in the scratchpad (SHARD_TEXT_OUT) for a visual check.
use shard_model::TextStencil;

#[test]
fn render_k1() {
    let Ok(out) = std::env::var("SHARD_TEXT_OUT") else { return };
    let t = TextStencil {
        text: "K1\n".into(),
        font: "resource://system_fonts/arialbd?version=x.ttf".into(),
        size: 0.5,
        auto_size: true,
        padding: [0.0078125; 2],
        offset: [0.0; 2],
        h_align: 0,
        v_align: 1,
        letter_spacing: 0.0,
        line_spacing: 1.2,
        foreground: [1.0; 3],
        background: [0.0; 3],
        background_opacity: 0.0,
    };
    let img = shard_engine::text::render_text(&t, 256).expect("rendered");
    let bytes: Vec<u8> = img.alpha.iter().map(|a| (a * 255.0) as u8).collect();
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, 256, 256);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&bytes).unwrap();
    }
    std::fs::write(out, buf).unwrap();
    println!("sum alpha {}", img.alpha.iter().sum::<f32>());
}

/// SHARD_PROBE_SPP + SHARD_GRAPH: image inputs, outputs and numeric parameters of a graph used in the project.
#[test]
fn graph_interface() {
    let (Ok(p), Ok(name)) = (std::env::var("SHARD_PROBE_SPP"), std::env::var("SHARD_GRAPH")) else { return };
    use shard_compose::BitmapProvider as _;
    let stacks = shard_model::import::import_project_stacks(std::path::Path::new(&p)).unwrap();
    let bm = shard_engine::bitmaps::ProjectBitmaps::open(std::path::Path::new(&p), 256).unwrap();
    fn find(es: &[shard_model::Effect], name: &str, out: &mut Option<String>) {
        for e in es {
            match &e.kind {
                shard_model::EffectKind::Fill(f) => {
                    for s in &f.sources {
                        if let shard_model::Source::Procedural { procedural, .. } = s {
                            if procedural.graph_name() == Some(name) {
                                *out = procedural.resource.clone();
                            }
                        }
                    }
                }
                shard_model::EffectKind::Group { children } => find(children, name, out),
                _ => {}
            }
        }
    }
    fn walk(ls: &[shard_model::Layer], name: &str, out: &mut Option<String>) {
        for l in ls {
            find(&l.content, name, out);
            if let Some(m) = &l.mask {
                find(&m.effects, name, out);
            }
            walk(&l.children, name, out);
        }
    }
    let mut res = None;
    for s in &stacks {
        walk(&s.layers, &name, &mut res);
    }
    let res = res.expect("graph not used");
    let bytes = bm.sbsar_bytes(&res).expect("no archive");
    let iface = shard_formats::sbsar::Interface::from_sbsar(&bytes).unwrap();
    println!("resource {res}");
    println!("images {:?}", iface.images);
    println!("outputs {:?}", iface.outputs);
    println!("params {:?}", iface.params.iter().map(|q| q.identifier.clone()).collect::<Vec<_>>());
    println!("values {:?}", iface.values);
}
