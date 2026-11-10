use std::path::Path;

use shard_compose::{blend, compose};
use shard_model::import::import_file;
use shard_model::*;

fn fill_layer(name: &str, color: [f32; 3], bit: u32, blend_mode: BlendMode, opacity: f32) -> Layer {
    let mut ch = ChannelSet(0);
    ch.0 |= 1u128 << bit;
    Layer {
        uid: next_uid(),
        kind: LayerKind::Layer,
        name: name.into(),
        visible: true,
        blend: vec![ChannelBlend { mode: blend_mode, channels: ch, opacity }],
        content: vec![Effect {
            uid: 0,
            visible: true,
            channels: ChannelSet(u128::MAX),
            label: None,
            blend: vec![],
            kind: EffectKind::Fill(Fill {
                projection: Projection::Uv,
                filtering: 2,
                uv: UvTransform::default(),
                sources: vec![Source::Uniform { channels: ch, color: Some(color), opacity: 1.0 }],
                projector: None,
            }),
        }],
        mask: None,
        children: vec![],
    }
}

#[test]
fn blend_formulas() {
    assert_eq!(blend(BlendMode::MULTIPLY, 0.5, 0.5), 0.25);
    assert_eq!(blend(BlendMode::NORMAL, 0.1, 0.9), 0.9);
    let screen = BlendMode(8);
    assert!((blend(screen, 0.5, 0.5) - 0.75).abs() < 1e-6);
    let overlay = BlendMode(11);
    assert!((blend(overlay, 0.25, 0.5) - 0.25).abs() < 1e-6);
}

#[test]
fn stack_top_first_multiply_over_normal() {
    // top-first list: Multiply(0.5 grey, 50%) over Normal(red)
    let doc = Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: "DataDocument".into(),
        layers: vec![
            fill_layer("grey", [0.5, 0.5, 0.5], 0, BlendMode::MULTIPLY, 0.5),
            fill_layer("red", [1.0, 0.0, 0.0], 0, BlendMode::NORMAL, 1.0),
        ],
    };
    let c = compose(&doc, 2, 2, &[0]);
    let px = c.channels[&0].pixel(1, 1);
    // red = (1,0,0); multiply result (0.5,0,0); 50% -> (0.75, 0, 0)
    assert!((px[0] - 0.75).abs() < 1e-6, "{px:?}");
    assert!(px[1].abs() < 1e-6 && px[2].abs() < 1e-6);
    assert!(c.unsupported.is_empty());
}

#[test]
fn hidden_layer_and_disable_are_skipped() {
    let mut top = fill_layer("top", [0.0, 1.0, 0.0], 0, BlendMode::NORMAL, 1.0);
    top.visible = false;
    let doc = Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![top, fill_layer("base", [0.2, 0.2, 0.2], 0, BlendMode::NORMAL, 1.0)],
    };
    let c = compose(&doc, 1, 1, &[0]);
    assert!((c.channels[&0].pixel(0, 0)[0] - 0.2).abs() < 1e-6);
}

#[test]
fn board_project_composites_with_report() {
    let p = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("board.spp")).unwrap_or_default();
    if !p.exists() {
        return;
    }
    let d = import_file(p).unwrap();
    let c = compose(&d, 8, 8, &[0, 1, 7, 13, 22]);
    assert_eq!(c.channels.len(), 5);
    // pixels are finite and in a sane range for BaseColor
    for v in &c.channels[&0].data {
        assert!(v.is_finite());
    }
    eprintln!("unsupported constructs in Board.spp ({}):", c.unsupported.len());
    for u in c.unsupported.iter().take(12) {
        eprintln!("  - {u}");
    }
}

#[test]
fn writes_channel_follows_visible_sources() {
    let mut hidden = fill_layer("h", [0.5, 0.5, 0.5], 22, BlendMode::NORMAL, 1.0);
    hidden.visible = false;
    let doc = Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![hidden, fill_layer("height", [0.5, 0.5, 0.5], 1, BlendMode::NORMAL, 1.0)],
    };
    assert!(shard_compose::writes_channel(&doc, 1));
    assert!(!shard_compose::writes_channel(&doc, 22), "a hidden layer writes nothing");
    assert!(!shard_compose::writes_channel(&doc, 0));
}

#[test]
fn normal_assembly_height_slope_and_base_length() {
    use shard_compose::{assemble_normal, Image, NormalMix};
    let mix = NormalMix { height_strength: 1.0e-3, flip_y: false, reoriented: false };
    let (w, h) = (8u32, 8u32);
    // a height that rises to the right
    let mut ramp = Vec::new();
    for _ in 0..h {
        for x in 0..w {
            ramp.push(x as f32 / w as f32);
        }
    }
    let height = Image { w, h, n: 1, data: ramp };
    let n = assemble_normal(None, Some(&height), None, w, h, &mix);
    let mid = n.pixel(4, 4);
    assert!(mid[0] < 0.5, "a rising height tilts the normal to the left: {mid:?}");
    assert!((mid[1] - 0.5).abs() < 1e-6 && mid[2] > 0.5);
    // flat height and no normal channel: flat normal
    let flat = Image::filled(w, h, &[0.3]);
    let n = assemble_normal(None, Some(&flat), None, w, h, &mix);
    assert_eq!(n.pixel(2, 2), &[0.5, 0.5, 1.0]);
    // a base normal map that was averaged down (vector shorter than 1) keeps its length under a flat detail
    let base = Image::filled(w, h, &[0.55, 0.5, 0.95]);
    let n = assemble_normal(Some(&base), None, None, w, h, &mix);
    for (a, b) in n.pixel(3, 3).iter().zip([0.55, 0.5, 0.95]) {
        assert!((a - b).abs() < 1e-5, "{a} vs {b}");
    }
}

#[test]
fn projector_maps_surface_like_painter() {
    use shard_compose::{project_sample, Image, ProjSpec, Surface, UvXform};
    // 64x1 image whose value is the horizontal coordinate
    let img = Image { w: 64, h: 2, n: 1, data: (0..128).map(|i| ((i % 64) as f32 + 0.5) / 64.0).collect() };
    let proj = Projector { position: [0.0; 3], euler: [0.0; 3], size: [1.0; 3], wrap: 1 };
    let spec = ProjSpec { kind: Projection::Triplanar, projector: Some(proj), repeat: true };
    let xf = UvXform::new(&UvTransform::default());
    // one texel on a +z facing surface at x = 0.5 (mesh units): u = x / 2 + 0.5 = 0.75
    let s = Surface {
        w: 1,
        h: 1,
        pos: vec![[0.5, 0.2, 0.9]],
        nrm: vec![[0.0, 0.0, 1.0]],
        covered: vec![true],
        lo: [-1.0; 3],
        hi: [1.0; 3],
    };
    let v = project_sample(&img, 0, 1, &s, 0, &xf, &spec).unwrap();
    assert!((v - 0.75).abs() < 0.02, "+z face: u follows +x: {v}");
    // the same point on a -z facing surface is mirrored: u = -x / 2 + 0.5 = 0.25
    let s2 = Surface { nrm: vec![[0.0, 0.0, -1.0]], ..s };
    let v2 = project_sample(&img, 0, 1, &s2, 0, &xf, &spec).unwrap();
    assert!((v2 - 0.25).abs() < 0.02, "-z face is seen from behind: {v2}");
    // no projector, no result: the caller falls back to the UV mapping
    let none = ProjSpec { projector: None, ..spec };
    assert!(project_sample(&img, 0, 1, &s2, 0, &xf, &none).is_none());
}

/// A mask made of one uniform fill, so the layer's coverage is exactly `value`.
fn masked_fill_layer(name: &str, color: [f32; 3], value: f32) -> Layer {
    let mut l = fill_layer(name, color, 0, BlendMode::NORMAL, 1.0);
    let ch = ChannelSet(u128::MAX);
    l.mask = Some(Mask {
        enabled: true,
        initial: 0,
        explicit: true,
        effects: vec![Effect {
            uid: 0,
            visible: true,
            channels: ch,
            label: None,
            blend: vec![],
            kind: EffectKind::Fill(Fill {
                projection: Projection::Uv,
                filtering: 2,
                uv: UvTransform::default(),
                sources: vec![Source::Uniform { channels: ch, color: Some([value, value, value]), opacity: 1.0 }],
                projector: None,
            }),
        }],
    });
    l
}

/// Mesh maps that only carry an identity, which is what lets evaluated masks be kept between compositions.
struct Identity(u64);

impl shard_compose::MeshMaps for Identity {
    fn mesh_map(&self, _: shard_compose::MeshMap) -> Option<std::sync::Arc<shard_compose::Image>> {
        None
    }

    fn cache_token(&self) -> Option<u64> {
        Some(self.0)
    }
}

#[test]
fn masks_are_kept_between_compositions_and_follow_edits() {
    let doc = |value: f32| Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![
            masked_fill_layer("top", [1.0, 1.0, 1.0], value),
            fill_layer("base", [0.0, 0.0, 0.0], 0, BlendMode::NORMAL, 1.0),
        ],
    };
    let run = |d: &Document, token: u64| {
        let maps = Identity(token);
        let opts = shard_compose::Options {
            maps: &maps,
            allow_approx: true,
            ..Default::default()
        };
        shard_compose::compose_with(d, 4, 4, &[0], &opts).channels[&0].pixel(1, 1)[0]
    };
    // distinct token per test run so parallel tests of this binary do not share entries
    let token = 0x5eed_0001;
    let a = run(&doc(0.5), token);
    assert!((a - 0.5).abs() < 1e-3, "{a}");
    // the same definition is served from the store, and the result is the same
    let a2 = run(&doc(0.5), token);
    assert!((a - a2).abs() < 1e-6);
    // an edited mask must not be served the old value
    let b = run(&doc(1.0), token);
    assert!((b - 1.0).abs() < 1e-3, "{b}");
    // a different texture set / project (token) is a different entry
    let c = run(&doc(0.25), token + 1);
    assert!((c - 0.25).abs() < 1e-3, "{c}");
}

#[test]
fn checkpoints_resume_unchanged_prefix_and_follow_edits() {
    // a checkpoint after every layer, so editing one resumes from the layers below it
    std::env::set_var("SHARD_CKPT_MS", "0");
    let stack = |mid: f32, top: f32| Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![
            fill_layer("top", [top, top, top], 0, BlendMode::MULTIPLY, 0.5),
            masked_fill_layer("mid", [mid, 0.2, 0.9], 0.75),
            fill_layer("base", [0.1, 0.4, 0.3], 0, BlendMode::NORMAL, 1.0),
        ],
    };
    let cached = |d: &Document| {
        let maps = Identity(0xC0DE_0002);
        let opts = shard_compose::Options { maps: &maps, allow_approx: true, ..Default::default() };
        shard_compose::compose_with(d, 4, 4, &[0], &opts).channels[&0].pixel(2, 2).to_vec()
    };
    let plain = |d: &Document| {
        let opts = shard_compose::Options { allow_approx: true, ..Default::default() };
        shard_compose::compose_with(d, 4, 4, &[0], &opts).channels[&0].pixel(2, 2).to_vec()
    };
    for (mid, top) in [(0.6, 0.5), (0.6, 0.5), (0.6, 0.9), (0.3, 0.9), (0.3, 0.9), (0.6, 0.5)] {
        let d = stack(mid, top);
        let (a, b) = (cached(&d), plain(&d));
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 1e-4, "mid {mid} top {top}: {a:?} vs {b:?}");
        }
    }
}

#[test]
fn replace_group_flattens_a_channel_its_children_do_not_write() {
    // Normal channel (22): a layer writes a tilted normal, a group above it replaces it. The group's only child
    // writes base colour, so the group's own Normal channel is empty (flat) and replaces what is below.
    let below = fill_layer("tilted", [0.8, 0.2, 0.5], 22, BlendMode::NORMAL, 1.0);
    let child = fill_layer("colour", [0.3, 0.3, 0.3], 0, BlendMode::NORMAL, 1.0);
    let group = |mode: BlendMode| Layer {
        uid: next_uid(),
        kind: LayerKind::Group,
        name: "group".into(),
        visible: true,
        blend: vec![ChannelBlend { mode, channels: ChannelSet(1u128 << 22), opacity: 1.0 }],
        content: vec![],
        mask: None,
        children: vec![child.clone()],
    };
    let doc = |mode| Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![group(mode), below.clone()],
    };
    let flat = compose(&doc(BlendMode(1)), 2, 2, &[22]).channels[&22].pixel(1, 1).to_vec();
    assert!((flat[0] - 0.5).abs() < 1e-4 && (flat[1] - 0.5).abs() < 1e-4 && (flat[2] - 1.0).abs() < 1e-4, "{flat:?}");
    // a Normal-blended group leaves the normal below alone
    let kept = compose(&doc(BlendMode::NORMAL), 2, 2, &[22]).channels[&22].pixel(1, 1).to_vec();
    assert!((kept[0] - 0.8).abs() < 1e-4 && (kept[1] - 0.2).abs() < 1e-4, "{kept:?}");
}

#[test]
fn masks_with_anchors_are_kept_and_follow_the_anchor() {
    let eff = |kind: EffectKind, blend: Vec<ChannelBlend>, uid: u64| Effect {
        uid,
        visible: true,
        channels: ChannelSet(u128::MAX),
        label: None,
        blend,
        kind,
    };
    let uniform = |v: f32| {
        EffectKind::Fill(Fill {
            projection: Projection::Uv,
            filtering: 2,
            uv: UvTransform::default(),
            sources: vec![Source::Uniform { channels: ChannelSet(u128::MAX), color: Some([v, v, v]), opacity: 1.0 }],
            projector: None,
        })
    };
    let reference = |target: u64| {
        EffectKind::Fill(Fill {
            projection: Projection::Uv,
            filtering: 2,
            uv: UvTransform::default(),
            sources: vec![Source::Reference { channels: ChannelSet(u128::MAX), opacity: 1.0, target, levels: None }],
            projector: None,
        })
    };
    let multiply = vec![ChannelBlend { mode: BlendMode(4), channels: ChannelSet(u128::MAX), opacity: 1.0 }];
    let stack = |anchor_value: f32| {
        // bottom layer: its mask holds a value and publishes it as anchor 77 (no content: it writes nothing)
        let mut holder = fill_layer("holder", [0.0, 0.0, 0.0], 5, BlendMode::NORMAL, 1.0);
        holder.mask = Some(Mask {
            enabled: true,
            initial: 0,
            explicit: true,
            effects: vec![eff(uniform(anchor_value), vec![], 1), eff(EffectKind::Anchor { name: None }, vec![], 77)],
        });
        // top layer: white fill whose mask is 1 multiplied by anchor 77
        let mut reader = fill_layer("reader", [1.0, 1.0, 1.0], 0, BlendMode::NORMAL, 1.0);
        reader.mask = Some(Mask {
            enabled: true,
            initial: 0,
            explicit: true,
            effects: vec![eff(uniform(1.0), vec![], 2), eff(reference(77), multiply.clone(), 3)],
        });
        Document {
            kind: DocKind::Project,
            name: "t".into(),
            root_class: String::new(),
            layers: vec![reader, holder, fill_layer("base", [0.0, 0.0, 0.0], 0, BlendMode::NORMAL, 1.0)],
        }
    };
    let run = |d: &Document, token: u64| {
        let maps = Identity(token);
        let opts = shard_compose::Options { maps: &maps, allow_approx: true, ..Default::default() };
        shard_compose::compose_with(d, 4, 4, &[0], &opts).channels[&0].pixel(1, 1)[0]
    };
    let token = 0xA4C4_0001;
    let a = run(&stack(0.7), token);
    assert!((a - 0.7).abs() < 1e-3, "{a}");
    // from the store: same value, the anchor is published again for the reader
    let a2 = run(&stack(0.7), token);
    assert!((a2 - 0.7).abs() < 1e-3, "{a2}");
    // the holder changes: the reader (same definition) must follow its anchor
    let b = run(&stack(0.3), token);
    assert!((b - 0.3).abs() < 1e-3, "{b}");
    let b2 = run(&stack(0.3), token);
    assert!((b2 - 0.3).abs() < 1e-3, "{b2}");
}

fn group_of(name: &str, children: Vec<Layer>, blend: Vec<ChannelBlend>, content: Vec<Effect>) -> Layer {
    Layer {
        uid: next_uid(),
        kind: LayerKind::Group,
        name: name.into(),
        visible: true,
        blend,
        content,
        mask: None,
        children,
    }
}

#[test]
fn isolated_group_blends_its_layers_against_emptiness() {
    // base grey 0.8; an isolated (Normal) group holds an Overlay layer of 0.3. Against the empty group the
    // layer is a plain copy (0.3); blended against the backdrop it would be Overlay(0.8, 0.3) = 0.72.
    let base = fill_layer("base", [0.8, 0.8, 0.8], 0, BlendMode::NORMAL, 1.0);
    let overlay = fill_layer("overlay", [0.3, 0.3, 0.3], 0, BlendMode(11), 1.0);
    let normal = vec![ChannelBlend { mode: BlendMode::NORMAL, channels: ChannelSet(1), opacity: 1.0 }];
    let isolated = Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![group_of("g", vec![overlay.clone()], normal, vec![]), base.clone()],
    };
    let v = compose(&isolated, 2, 2, &[0]).channels[&0].pixel(1, 1)[0];
    assert!((v - 0.3).abs() < 1e-4, "{v}");
    // a group channel without an entry is isolated too (Normal)
    let unlisted = Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![group_of("g", vec![overlay.clone()], vec![], vec![]), base.clone()],
    };
    let v = compose(&unlisted, 2, 2, &[0]).channels[&0].pixel(1, 1)[0];
    assert!((v - 0.3).abs() < 1e-4, "{v}");
    // an explicit Passthrough group blends against the backdrop like a loose layer: Overlay(0.8, 0.3)
    let passthrough = vec![ChannelBlend { mode: BlendMode::PASSTHROUGH, channels: ChannelSet(1), opacity: 1.0 }];
    let pass = Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![group_of("g", vec![overlay], passthrough, vec![]), base],
    };
    let v = compose(&pass, 2, 2, &[0]).channels[&0].pixel(1, 1)[0];
    assert!((v - 0.72).abs() < 1e-3, "{v}");
}

#[test]
fn levels_in_a_group_adjust_its_result_for_the_listed_channels() {
    let levels = |channel: u32| Effect {
        uid: 0,
        visible: true,
        channels: ChannelSet(1u128 << channel),
        label: None,
        blend: vec![],
        kind: EffectKind::Levels(Levels { in_min: [0.5; 3], ..Levels::default() }),
    };
    let child = fill_layer("c", [0.75, 0.75, 0.75], 0, BlendMode::NORMAL, 1.0);
    let doc = |channel: u32| Document {
        kind: DocKind::Project,
        name: "t".into(),
        root_class: String::new(),
        layers: vec![group_of("g", vec![child.clone()], vec![], vec![levels(channel)])],
    };
    // listed channel: (0.75 - 0.5) / 0.5 = 0.5
    let v = compose(&doc(0), 2, 2, &[0]).channels[&0].pixel(0, 0)[0];
    assert!((v - 0.5).abs() < 1e-4, "{v}");
    // another channel listed: untouched
    let v = compose(&doc(7), 2, 2, &[0]).channels[&0].pixel(0, 0)[0];
    assert!((v - 0.75).abs() < 1e-4, "{v}");
}

#[test]
fn thumbnails_show_each_layer_alone_and_its_mask() {
    // top-first: a masked red layer over a plain grey one, plus a group that only organises them
    let masked = masked_fill_layer("masked", [1.0, 0.0, 0.0], 0.25);
    let grey = fill_layer("grey", [0.5, 0.5, 0.5], 0, BlendMode::NORMAL, 1.0);
    let group = group_of("group", vec![grey.clone()], vec![], vec![]);
    let doc = Document { kind: DocKind::Project, name: "t".into(), root_class: String::new(), layers: vec![masked, group] };
    let t = shard_compose::thumbnails(&doc, 4, 4, 0, &shard_compose::Options::default());
    // contents: the red layer [0] and the grey layer inside the group [1, 0]; the group has none
    let paths: Vec<Vec<usize>> = t.contents.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(paths, vec![vec![0], vec![1, 0]]);
    // each shows its own colour, without the mask or what is below
    assert_eq!(t.contents[0].1.pixel(2, 2), &[1.0, 0.0, 0.0]);
    assert_eq!(t.contents[1].1.pixel(2, 2), &[0.5, 0.5, 0.5]);
    // the explicit mask picture is its value everywhere
    assert_eq!(t.masks.len(), 1);
    assert_eq!(t.masks[0].0, vec![0]);
    assert!(t.masks[0].1.iter().all(|v| (*v - 0.25).abs() < 1e-6));
}

#[test]
fn soft_light_is_photoshops_over_the_whole_range() {
    // Painter's Soft Light is sqrt(b) for the whole range (the W3C cubic below 0.25 gives 0.2133 here): fitted on
    // a test vehicle `highpass` layer of the layer-by-layer export
    let v = blend(BlendMode(12), 0.1, 0.789);
    let want = 0.1 + (2.0 * 0.789 - 1.0) * (0.1f32.sqrt() - 0.1);
    assert!((v - want).abs() < 1e-5, "{v} vs {want}");
    // below 0.5 the two formulas agree
    let lo = blend(BlendMode(12), 0.3, 0.2);
    assert!((lo - (0.3 - 0.6 * 0.3 * 0.7)).abs() < 1e-6);
}
