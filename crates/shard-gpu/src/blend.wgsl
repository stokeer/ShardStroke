// One layer composited onto a GPU-resident accumulator: the same math as `composite_px` / `blend` in
// shard-compose (checked texel by texel against it in the crate tests).

struct Params {
    npx: u32,
    n: u32,
    mode: u32,
    flags: u32,      // 1: content is a constant, 2: mask is a constant, 4: coverage buffer present
    op: f32,
    mask_c: f32,
    _pad0: f32,
    _pad1: f32,
    const_s: vec4<f32>,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> acc: array<f32>;
@group(0) @binding(2) var<storage, read_write> cover: array<f32>;
@group(0) @binding(3) var<storage, read> content: array<f32>;
@group(0) @binding(4) var<storage, read> mask: array<f32>;

// mode ids: the order of `MODES` in lib.rs
fn blend(mode: u32, b: f32, s: f32) -> f32 {
    switch mode {
        case 3u: { return b * s; }
        case 4u: { if (s <= 1e-6) { return 1.0; } return min(b / s, 1.0); }
        case 5u: { if (b <= 1e-6) { return 1.0; } return min(s / b, 1.0); }
        case 6u: { return min(b, s); }
        case 7u: { return max(b, s); }
        case 8u: { return min(b + s, 1.0); }
        case 9u: { return max(b - s, 0.0); }
        case 10u: { return max(s - b, 0.0); }
        case 11u: { return abs(b - s); }
        case 12u: { return b + s - 2.0 * b * s; }
        case 13u: { return 1.0 - (1.0 - b) * (1.0 - s); }
        case 14u: { if (b < 0.5) { return 2.0 * b * s; } return 1.0 - 2.0 * (1.0 - b) * (1.0 - s); }
        case 15u: { if (s < 0.5) { return 2.0 * b * s; } return 1.0 - 2.0 * (1.0 - b) * (1.0 - s); }
        case 16u: {
            if (s < 0.5) { return b - (1.0 - 2.0 * s) * b * (1.0 - b); }
            return b + (2.0 * s - 1.0) * (sqrt(b) - b);
        }
        case 17u: { if (s <= 1e-6) { return 0.0; } return 1.0 - min((1.0 - b) / s, 1.0); }
        case 18u: { if (s >= 1.0 - 1e-6) { return 1.0; } return min(b / (1.0 - s), 1.0); }
        case 19u: { return max(b + s - 1.0, 0.0); }
        case 20u: { return clamp(b + 2.0 * s - 1.0, 0.0, 1.0); }
        case 21u: {
            if (s < 0.5) {
                let s2 = 2.0 * s;
                if (s2 <= 1e-6) { return 0.0; }
                return 1.0 - min((1.0 - b) / s2, 1.0);
            }
            let s2 = 2.0 * (s - 0.5);
            if (s2 >= 1.0 - 1e-6) { return 1.0; }
            return min(b / (1.0 - s2), 1.0);
        }
        case 22u: { if (s < 0.5) { return min(b, 2.0 * s); } return max(b, 2.0 * (s - 0.5)); }
        default: { return s; }
    }
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x + gid.y * 65535u * 256u;
    if (i >= P.npx) { return; }
    var m = P.mask_c;
    if ((P.flags & 2u) == 0u) { m = mask[i]; }
    let a = clamp(P.op * m, 0.0, 1.0);
    if (a <= 0.0) { return; }
    let has_cover = (P.flags & 4u) != 0u;
    var pa = 1.0;
    if (has_cover) { pa = cover[i]; }
    var ar = pa;
    for (var c = 0u; c < P.n; c = c + 1u) {
        var s: f32;
        if ((P.flags & 1u) != 0u) { s = P.const_s[min(c, 3u)]; } else { s = content[i * P.n + c]; }
        let d = acc[i * P.n + c];
        if (pa >= 1.0) {
            acc[i * P.n + c] = d + (blend(P.mode, d, s) - d) * a;
            ar = 1.0;
        } else {
            let r = a + pa * (1.0 - a);
            if (r > 0.0) {
                let mixed = (1.0 - pa) * s + pa * blend(P.mode, d, s);
                acc[i * P.n + c] = d + (mixed - d) * (a / r);
                ar = r;
            }
        }
    }
    if (has_cover) { cover[i] = ar; }
}
