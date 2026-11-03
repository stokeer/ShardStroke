//! Umbra, the progressive path tracer of the viewport ("Path tracing" render mode): a compute shader that walks the mesh's
//! BVH on the GPU, shades with the same metal/roughness model as the real-time view (GGX + Lambert, the set's base
//! colour / roughness / metallic / normal / emissive textures), lights from the environment with importance
//! sampling and multiple importance sampling, and accumulates one sample per pixel per frame into the HDR target
//! (the post pass -- bloom, tone mapping, colour correction -- works on it like on the real-time picture).
//!
//! A denoiser always runs on top (see `DENOISE`): an edge-avoiding a-trous filter guided by the first hit's normal,
//! position and albedo, whose strength follows the noise left in each pixel -- strong on the first samples, nothing
//! once the picture has converged.
//!
//! Needs texture binding arrays with non-uniform indexing (one texture per set and channel); without them the mode
//! is not offered.

use crate::mesh::Mesh;
use wgpu::util::DeviceExt;

/// Samples after which the picture is considered converged (accumulation stops).
pub const MAX_SAMPLES: u32 = 4096;

const SHADER: &str = r#"
struct P {
  inv_vp: mat4x4<f32>,
  eye: vec4<f32>,
  // x = width, y = height, z = frame index, w = sample count so far
  size: vec4<f32>,
  // x = env yaw, y = env cdf width, z = env cdf height, w = env luminance total
  env: vec4<f32>,
  // x = emissive intensity, y = show background, z = isolated set (-1 = all), w = normal map Y sign
  misc: vec4<f32>,
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> nodes: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> order: array<u32>;
@group(0) @binding(3) var<storage, read> verts: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> idx: array<u32>;
@group(0) @binding(5) var env_tex: texture_2d<f32>;
@group(0) @binding(6) var lin: sampler;
@group(0) @binding(7) var<storage, read> marginal: array<f32>;
@group(0) @binding(8) var<storage, read> conditional: array<f32>;
@group(0) @binding(9) var<storage, read_write> accum: array<vec4<f32>>;
@group(0) @binding(10) var out_tex: texture_storage_2d<rgba16float, write>;
// denoiser guides of the pixel-centre primary hit: oct normal (snorm16 x2), distance (f32 bits), albedo (unorm8 x4),
// 1 = hit; and the moments of the demodulated luminance of the samples
@group(0) @binding(11) var<storage, read_write> feat: array<vec4<u32>>;
@group(0) @binding(12) var<storage, read_write> mom: array<vec2<f32>>;
@group(1) @binding(0) var base_tex: binding_array<texture_2d<f32>>;
@group(1) @binding(1) var aux_tex: binding_array<texture_2d<f32>>;
@group(1) @binding(2) var nrm_tex: binding_array<texture_2d<f32>>;
@group(1) @binding(3) var emi_tex: binding_array<texture_2d<f32>>;
@group(1) @binding(4) var mat_samp: sampler;

const PI: f32 = 3.14159265;

var<private> rng: u32;
fn pcg(v: u32) -> u32 {
  let s = v * 747796405u + 2891336453u;
  let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
  return (w >> 22u) ^ w;
}
fn rnd() -> f32 {
  rng = pcg(rng);
  return f32(rng >> 8u) / 16777216.0;
}

fn env_rot(d: vec3<f32>) -> vec3<f32> {
  let c = cos(p.env.x);
  let s = sin(p.env.x);
  return vec3<f32>(c * d.x + s * d.z, d.y, -s * d.x + c * d.z);
}
fn env_unrot(d: vec3<f32>) -> vec3<f32> {
  let c = cos(p.env.x);
  let s = sin(p.env.x);
  return vec3<f32>(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}
fn dir_to_uv(d: vec3<f32>) -> vec2<f32> {
  return vec2<f32>(atan2(d.z, d.x) / 6.2831853 + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}
fn uv_to_dir(uv: vec2<f32>) -> vec3<f32> {
  let phi = (uv.x - 0.5) * 6.2831853;
  let th = uv.y * PI;
  return vec3<f32>(sin(th) * cos(phi), cos(th), sin(th) * sin(phi));
}
fn env_radiance(world: vec3<f32>) -> vec3<f32> {
  return textureSampleLevel(env_tex, lin, dir_to_uv(env_rot(world)), 0.0).rgb;
}
fn lum(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }
fn oct_enc(n: vec3<f32>) -> vec2<f32> {
  let q = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
  if (n.z >= 0.0) { return q; }
  return (vec2<f32>(1.0) - abs(q.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), q >= vec2<f32>(0.0));
}

// importance sampling of the environment: a row from the marginal CDF, a column from that row's CDF
fn find(lo0: u32, n: u32, u: f32, cdf_row: bool) -> u32 {
  var lo = 0u;
  var hi = n - 1u;
  loop {
    if (lo >= hi) { break; }
    let mid = (lo + hi) / 2u;
    var v: f32;
    if (cdf_row) { v = conditional[lo0 + mid]; } else { v = marginal[mid]; }
    if (v < u) { lo = mid + 1u; } else { hi = mid; }
  }
  return lo;
}
fn env_pdf(world: vec3<f32>) -> f32 {
  let w = u32(p.env.y);
  let h = u32(p.env.z);
  let uv = dir_to_uv(env_rot(world));
  let x = min(u32(uv.x * f32(w)), w - 1u);
  let y = min(u32(uv.y * f32(h)), h - 1u);
  let row_prev = select(0.0, conditional[y * w + x - 1u], x > 0u);
  let cell = (conditional[y * w + x] - row_prev);
  let mrow_prev = select(0.0, marginal[y - 1u], y > 0u);
  let prow = marginal[y] - mrow_prev;
  let s = max(sin(uv.y * PI), 1e-4);
  return cell * prow * f32(w * h) / (2.0 * PI * PI * s);
}
// returns direction (world) in xyz, pdf in w
fn sample_env() -> vec4<f32> {
  let w = u32(p.env.y);
  let h = u32(p.env.z);
  let y = find(0u, h, rnd(), false);
  let x = find(y * w, w, rnd(), true);
  let uv = vec2<f32>((f32(x) + rnd()) / f32(w), (f32(y) + rnd()) / f32(h));
  let d = env_unrot(uv_to_dir(uv));
  return vec4<f32>(d, env_pdf(d));
}

struct Hit { t: f32, tri: u32, u: f32, v: f32 };

fn tri_pos(t: u32, k: u32) -> vec3<f32> { return verts[idx[3u * t + k] * 4u].xyz; }
fn tri_set(t: u32) -> u32 { return bitcast<u32>(verts[idx[3u * t] * 4u + 2u].w); }

fn ray_box(o: vec3<f32>, inv: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>, tmax: f32) -> bool {
  let t1 = (lo - o) * inv;
  let t2 = (hi - o) * inv;
  let tmin = max(max(min(t1.x, t2.x), min(t1.y, t2.y)), max(min(t1.z, t2.z), 0.0));
  let tm = min(min(max(t1.x, t2.x), max(t1.y, t2.y)), min(max(t1.z, t2.z), tmax));
  return tmin <= tm;
}

// nearest hit (two-sided); `any_hit` stops at the first one (shadow rays)
fn trace(o: vec3<f32>, d: vec3<f32>, tmax: f32, any_hit: bool) -> Hit {
  var h: Hit;
  h.t = tmax;
  h.tri = 0xffffffffu;
  let inv = 1.0 / d;
  var stack: array<u32, 64>;
  var sp = 1u;
  stack[0] = 0u;
  let iso = i32(p.misc.z);
  loop {
    if (sp == 0u) { break; }
    sp = sp - 1u;
    let ni = stack[sp];
    let a = nodes[2u * ni];
    let b = nodes[2u * ni + 1u];
    if (!ray_box(o, inv, a.xyz, b.xyz, h.t)) { continue; }
    let x = bitcast<u32>(a.w);
    let y = bitcast<u32>(b.w);
    if ((y & 0x80000000u) == 0u) {
      if (sp < 63u) {
        stack[sp] = x;
        stack[sp + 1u] = y;
        sp = sp + 2u;
      }
      continue;
    }
    let count = y & 0x7fffffffu;
    for (var k = 0u; k < count; k = k + 1u) {
      let t = order[x + k];
      if (iso >= 0 && tri_set(t) != u32(iso)) { continue; }
      let p0 = tri_pos(t, 0u);
      let e1 = tri_pos(t, 1u) - p0;
      let e2 = tri_pos(t, 2u) - p0;
      let pv = cross(d, e2);
      let det = dot(e1, pv);
      if (abs(det) < 1e-12) { continue; }
      let id = 1.0 / det;
      let tv = o - p0;
      let u = dot(tv, pv) * id;
      if (u < 0.0 || u > 1.0) { continue; }
      let q = cross(tv, e1);
      let v = dot(d, q) * id;
      if (v < 0.0 || u + v > 1.0) { continue; }
      let tt = dot(e2, q) * id;
      if (tt > 1e-4 && tt < h.t) {
        h.t = tt;
        h.tri = t;
        h.u = u;
        h.v = v;
        if (any_hit) { return h; }
      }
    }
  }
  return h;
}

struct Surf { pos: vec3<f32>, n: vec3<f32>, ng: vec3<f32>, base: vec3<f32>, rough: f32, metal: f32, emit: vec3<f32> };

fn surface(h: Hit, d: vec3<f32>) -> Surf {
  var s: Surf;
  let i0 = idx[3u * h.tri] * 4u;
  let i1 = idx[3u * h.tri + 1u] * 4u;
  let i2 = idx[3u * h.tri + 2u] * 4u;
  let w = vec3<f32>(1.0 - h.u - h.v, h.u, h.v);
  let p0 = verts[i0].xyz;
  let p1 = verts[i1].xyz;
  let p2 = verts[i2].xyz;
  s.pos = p0 * w.x + p1 * w.y + p2 * w.z;
  var ng = normalize(cross(p1 - p0, p2 - p0));
  var n = normalize(verts[i0 + 1u].xyz * w.x + verts[i1 + 1u].xyz * w.y + verts[i2 + 1u].xyz * w.z);
  let uv0 = vec2<f32>(verts[i0].w, verts[i0 + 1u].w);
  let uv1 = vec2<f32>(verts[i1].w, verts[i1 + 1u].w);
  let uv2 = vec2<f32>(verts[i2].w, verts[i2 + 1u].w);
  let uv = uv0 * w.x + uv1 * w.y + uv2 * w.z;
  let tuv = vec2<f32>(uv.x, 1.0 - uv.y);
  let mat_set = bitcast<u32>(verts[i0 + 2u].w);
  // two-sided: face the incoming ray
  if (dot(ng, d) > 0.0) { ng = -ng; }
  if (dot(n, ng) < 0.0) { n = -n; }
  s.ng = ng;
  s.base = textureSampleLevel(base_tex[mat_set], mat_samp, tuv, 0.0).rgb;
  let a = textureSampleLevel(aux_tex[mat_set], mat_samp, tuv, 0.0);
  s.rough = clamp(a.g, 0.04, 1.0);
  s.metal = a.b;
  s.emit = textureSampleLevel(emi_tex[mat_set], mat_samp, tuv, 0.0).rgb * p.misc.x;
  // normal map in the frame the raster view uses: Painter's stored tangent frame, else the triangle's UV frame
  let nt = textureSampleLevel(nrm_tex[mat_set], mat_samp, tuv, 0.0).rgb * 2.0 - 1.0;
  let tq = verts[i0 + 2u].xyz * w.x + verts[i1 + 2u].xyz * w.y + verts[i2 + 2u].xyz * w.z;
  let bq = verts[i0 + 3u].xyz * w.x + verts[i1 + 3u].xyz * w.y + verts[i2 + 3u].xyz * w.z;
  var mapped: vec3<f32>;
  if (dot(tq, tq) > 0.25) {
    mapped = normalize(tq) * nt.x - normalize(bq) * nt.y * p.misc.w + n * max(nt.z, 0.05);
  } else {
    let du1 = uv1 - uv0;
    let du2 = uv2 - uv0;
    let e1 = p1 - p0;
    let e2 = p2 - p0;
    let r = 1.0 / max(abs(du1.x * du2.y - du2.x * du1.y), 1e-12) * sign(du1.x * du2.y - du2.x * du1.y + 1e-20);
    let tt = normalize((e1 * du2.y - e2 * du1.y) * r + vec3<f32>(1e-6));
    let bb = normalize((e2 * du1.x - e1 * du2.x) * r + vec3<f32>(1e-6));
    mapped = tt * nt.x + bb * nt.y * p.misc.w + n * max(nt.z, 0.05);
  }
  s.n = normalize(mapped);
  if (dot(s.n, ng) < 0.0) { s.n = normalize(s.n - ng * dot(s.n, ng) * 1.01); }
  return s;
}

fn basis(n: vec3<f32>) -> mat3x3<f32> {
  let sg = select(-1.0, 1.0, n.z >= 0.0);
  let a = -1.0 / (sg + n.z);
  let b = n.x * n.y * a;
  let t = vec3<f32>(1.0 + sg * n.x * n.x * a, sg * b, -sg * n.x);
  let bt = vec3<f32>(b, sg + n.y * n.y * a, -n.y);
  return mat3x3<f32>(t, bt, n);
}

fn ggx_d(nh: f32, a2: f32) -> f32 {
  let k = nh * nh * (a2 - 1.0) + 1.0;
  return a2 / (PI * k * k);
}
fn smith_g1(nx: f32, a2: f32) -> f32 {
  return 2.0 * nx / (nx + sqrt(a2 + (1.0 - a2) * nx * nx));
}
fn fresnel(f0: vec3<f32>, vh: f32) -> vec3<f32> {
  return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vh, 5.0);
}

// BSDF value (already times cos) and pdf of `l` for the lobe mix with specular probability `ps`
fn eval(s: Surf, v: vec3<f32>, l: vec3<f32>, ps: f32) -> vec4<f32> {
  let nl = dot(s.n, l);
  let nv = max(dot(s.n, v), 1e-4);
  if (nl <= 0.0 || dot(s.ng, l) <= 0.0) { return vec4<f32>(0.0); }
  let h = normalize(v + l);
  let nh = max(dot(s.n, h), 0.0);
  let vh = max(dot(v, h), 0.0);
  let a = s.rough * s.rough;
  let a2 = max(a * a, 1e-6);
  let f0 = mix(vec3<f32>(0.04), s.base, s.metal);
  let f = fresnel(f0, vh);
  let d = ggx_d(nh, a2);
  let g = smith_g1(nv, a2) * smith_g1(nl, a2);
  let spec = f * d * g / (4.0 * nv * nl);
  let diff = (vec3<f32>(1.0) - f) * (1.0 - s.metal) * s.base / PI;
  let pdf_s = smith_g1(nv, a2) * d / (4.0 * nv);
  let pdf = ps * pdf_s + (1.0 - ps) * nl / PI;
  return vec4<f32>((spec + diff) * nl, pdf);
}

// visible-normal GGX sample (Heitz 2018) in the local frame where n = +z
fn sample_vndf(ve: vec3<f32>, a: f32) -> vec3<f32> {
  let vh = normalize(vec3<f32>(a * ve.x, a * ve.y, ve.z));
  let lensq = vh.x * vh.x + vh.y * vh.y;
  var t1 = vec3<f32>(1.0, 0.0, 0.0);
  if (lensq > 0.0) { t1 = vec3<f32>(-vh.y, vh.x, 0.0) / sqrt(lensq); }
  let t2 = cross(vh, t1);
  let r = sqrt(rnd());
  let phi = 2.0 * PI * rnd();
  let p1 = r * cos(phi);
  var p2 = r * sin(phi);
  let s = 0.5 * (1.0 + vh.z);
  p2 = (1.0 - s) * sqrt(max(0.0, 1.0 - p1 * p1)) + s * p2;
  let nh = p1 * t1 + p2 * t2 + sqrt(max(0.0, 1.0 - p1 * p1 - p2 * p2)) * vh;
  return normalize(vec3<f32>(a * nh.x, a * nh.y, max(0.0, nh.z)));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let w = u32(p.size.x);
  let hgt = u32(p.size.y);
  if (gid.x >= w || gid.y >= hgt) { return; }
  let pix = gid.y * w + gid.x;
  rng = pcg(pix * 9781u + u32(p.size.z) * 6271u + 1u);
  // denoiser guides: the hit of the ray through the pixel centre, once per accumulation
  if (p.size.w < 0.5) {
    let cx = (f32(gid.x) + 0.5) / f32(w) * 2.0 - 1.0;
    let cy = 1.0 - (f32(gid.y) + 0.5) / f32(hgt) * 2.0;
    let cf = p.inv_vp * vec4<f32>(cx, cy, 1.0, 1.0);
    let cn = p.inv_vp * vec4<f32>(cx, cy, 0.0, 1.0);
    let co = cn.xyz / cn.w;
    let cd = normalize(cf.xyz / cf.w - co);
    let ch = trace(co, cd, 1e30, false);
    var g = vec4<u32>(0u, 0u, pack4x8unorm(vec4<f32>(1.0)), 0u);
    if (ch.tri != 0xffffffffu) {
      let cs = surface(ch, cd);
      // what the lighting gets multiplied by: diffuse colour plus the specular reflectance at normal incidence
      let alb = (1.0 - cs.metal) * cs.base + mix(vec3<f32>(0.04), cs.base, cs.metal);
      g = vec4<u32>(pack2x16snorm(oct_enc(cs.n)), bitcast<u32>(ch.t), pack4x8unorm(vec4<f32>(clamp(alb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0)), 1u);
    }
    feat[pix] = g;
  }
  let guide_alb = max(unpack4x8unorm(feat[pix].z).rgb, vec3<f32>(0.02));
  // camera ray through a jittered point of the pixel (antialiasing for free)
  let jx = (f32(gid.x) + rnd()) / f32(w) * 2.0 - 1.0;
  let jy = 1.0 - (f32(gid.y) + rnd()) / f32(hgt) * 2.0;
  let far = p.inv_vp * vec4<f32>(jx, jy, 1.0, 1.0);
  let near = p.inv_vp * vec4<f32>(jx, jy, 0.0, 1.0);
  var o = near.xyz / near.w;
  var d = normalize(far.xyz / far.w - o);
  var thr = vec3<f32>(1.0);
  var col = vec3<f32>(0.0);
  var cover = 0.0;
  var last_pdf = 0.0;
  for (var bounce = 0u; bounce < 6u; bounce = bounce + 1u) {
    let h = trace(o, d, 1e30, false);
    if (h.tri == 0xffffffffu) {
      if (bounce == 0u) {
        if (p.misc.y > 0.5) {
          col = env_radiance(d);
          cover = 1.0;
        }
      } else {
        let le = env_radiance(d);
        var wgt = 1.0;
        if (last_pdf > 0.0) {
          let pe = env_pdf(d);
          wgt = last_pdf * last_pdf / (last_pdf * last_pdf + pe * pe);
        }
        col += thr * le * wgt;
      }
      break;
    }
    if (bounce == 0u) { cover = 1.0; }
    let s = surface(h, d);
    col += thr * s.emit;
    let v = -d;
    let f0 = mix(vec3<f32>(0.04), s.base, s.metal);
    let ps = mix(clamp(lum(fresnel(f0, max(dot(s.n, v), 0.0))) * 2.0, 0.15, 0.85), 1.0, s.metal);
    let start = s.pos + s.ng * 1e-4 * max(1.0, length(s.pos));
    // next event: a direction towards the environment, by its brightness
    let es = sample_env();
    if (es.w > 0.0) {
      let fe = eval(s, v, es.xyz, ps);
      if (fe.w > 0.0 && dot(fe.xyz, fe.xyz) > 0.0) {
        let sh = trace(start, es.xyz, 1e30, true);
        if (sh.tri == 0xffffffffu) {
          let wgt = es.w * es.w / (es.w * es.w + fe.w * fe.w);
          col += thr * fe.xyz * env_radiance(es.xyz) * wgt / es.w;
        }
      }
    }
    // continue along the BSDF
    let fr = basis(s.n);
    let ve = transpose(fr) * v;
    var l: vec3<f32>;
    if (rnd() < ps) {
      let a = max(s.rough * s.rough, 1e-3);
      let hl = sample_vndf(ve, a);
      l = fr * reflect(-ve, hl);
    } else {
      let r = sqrt(rnd());
      let phi = 2.0 * PI * rnd();
      l = fr * vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(0.0, 1.0 - r * r)));
    }
    let fb = eval(s, v, l, ps);
    if (fb.w <= 1e-6) { break; }
    thr *= fb.xyz / fb.w;
    last_pdf = fb.w;
    // russian roulette after a few bounces
    if (bounce >= 3u) {
      let q = clamp(max(thr.x, max(thr.y, thr.z)), 0.05, 0.95);
      if (rnd() > q) { break; }
      thr /= q;
    }
    o = start;
    d = l;
  }
  // fireflies: a single sample brighter than this is clamped
  let lc = lum(col);
  if (lc > 32.0) { col *= 32.0 / lc; }
  let prev = select(vec4<f32>(0.0), accum[pix], p.size.w > 0.5);
  let acc = prev + vec4<f32>(col * cover, cover);
  accum[pix] = acc;
  let il = lum(col * cover / guide_alb);
  mom[pix] = select(vec2<f32>(0.0), mom[pix], p.size.w > 0.5) + vec2<f32>(il, il * il);
  let n = p.size.w + 1.0;
  textureStore(out_tex, vec2<i32>(gid.xy), acc / n);
}
"#;

/// A-trous passes of the denoiser (taps 1, 2, 4, .. pixels apart).
const DENOISE_PASSES: u32 = 5;

/// Denoiser (SVGF's spatial filter without the temporal part, the accumulation does that): the accumulated colour is
/// divided by the albedo (texture detail stays out of the filter) and blurred by a 5x5 a-trous kernel whose weights
/// stop at other surfaces (normal, distance to the tangent plane) and at luminance steps larger than the noise the
/// pixel still carries (the variance of its mean, from the samples' moments; from the neighbours' spread on the first
/// samples). The variance of the mean falls with the sample count, so the filter fades out as the picture converges.
const DENOISE: &str = r#"
struct D {
  inv_vp: mat4x4<f32>,
  // x = width, y = height, z = samples, w = tap distance (pixels)
  size: vec4<f32>,
  // x = angle of one pixel (radians), y = last pass
  misc: vec4<f32>,
};
@group(0) @binding(0) var<uniform> d: D;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> feat: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> mom: array<vec2<f32>>;
@group(0) @binding(4) var src: texture_2d<f32>;
@group(0) @binding(5) var dst: texture_storage_2d<rgba16float, write>;

fn lum(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }
fn oct_dec(e: vec2<f32>) -> vec3<f32> {
  var n = vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y));
  if (n.z < 0.0) {
    let q = (vec2<f32>(1.0) - abs(n.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
    n = vec3<f32>(q, n.z);
  }
  return normalize(n);
}

struct G { hit: bool, n: vec3<f32>, pos: vec3<f32>, t: f32, alb: vec3<f32> };

fn guide(q: vec2<i32>) -> G {
  var g: G;
  let f = feat[u32(q.y) * u32(d.size.x) + u32(q.x)];
  g.hit = f.w != 0u;
  g.n = oct_dec(unpack2x16snorm(f.x));
  g.t = bitcast<f32>(f.y);
  g.alb = max(unpack4x8unorm(f.z).rgb, vec3<f32>(0.02));
  let c = vec2<f32>((f32(q.x) + 0.5) / d.size.x * 2.0 - 1.0, 1.0 - (f32(q.y) + 0.5) / d.size.y * 2.0);
  let far = d.inv_vp * vec4<f32>(c, 1.0, 1.0);
  let near = d.inv_vp * vec4<f32>(c, 0.0, 1.0);
  let o = near.xyz / near.w;
  g.pos = o + normalize(far.xyz / far.w - o) * g.t;
  return g;
}

fn inside(q: vec2<i32>) -> bool {
  return q.x >= 0 && q.y >= 0 && q.x < i32(d.size.x) && q.y < i32(d.size.y);
}

// same surface: similar normal and close to the tangent plane (tolerance: the footprint of the tap distance)
fn geo_w(gp: G, gq: G, dist: f32) -> f32 {
  let wn = pow(max(dot(gp.n, gq.n), 0.0), 64.0);
  let tol = gp.t * d.misc.x * max(dist, 1.0) + 1e-6;
  return wn * exp(-abs(dot(gp.n, gq.pos - gp.pos)) / tol);
}

fn kern(k: i32) -> f32 {
  let a = abs(k);
  return select(select(0.0625, 0.25, a == 1), 0.375, a == 0);
}

@compute @workgroup_size(8, 8)
fn prepare(@builtin(global_invocation_id) gid: vec3<u32>) {
  let p = vec2<i32>(gid.xy);
  if (!inside(p)) { return; }
  let i = gid.y * u32(d.size.x) + gid.x;
  let n = d.size.z;
  let gp = guide(p);
  if (!gp.hit) {
    textureStore(dst, p, vec4<f32>(accum[i].rgb / n, 0.0));
    return;
  }
  let c = accum[i].rgb / n / gp.alb;
  var v: f32;
  if (n >= 4.0) {
    let m = mom[i] / n;
    v = max(m.y - m.x * m.x, 0.0) / n;
  } else {
    // too few samples for their own spread: the spread of the means around, on the same surface
    var s1 = 0.0;
    var s2 = 0.0;
    var sw = 0.0;
    for (var y = -3; y <= 3; y = y + 1) {
      for (var x = -3; x <= 3; x = x + 1) {
        let q = p + vec2<i32>(x, y);
        if (!inside(q)) { continue; }
        let gq = guide(q);
        if (!gq.hit) { continue; }
        let w = geo_w(gp, gq, length(vec2<f32>(f32(x), f32(y))));
        let l = lum(accum[u32(q.y) * u32(d.size.x) + u32(q.x)].rgb / n / gq.alb);
        s1 += w * l;
        s2 += w * l * l;
        sw += w;
      }
    }
    let m1 = s1 / sw;
    v = max(s2 / sw - m1 * m1, 0.0);
  }
  textureStore(dst, p, vec4<f32>(c, v));
}

@compute @workgroup_size(8, 8)
fn atrous(@builtin(global_invocation_id) gid: vec3<u32>) {
  let p = vec2<i32>(gid.xy);
  if (!inside(p)) { return; }
  let i = gid.y * u32(d.size.x) + gid.x;
  let n = d.size.z;
  let last = d.misc.y > 0.5;
  let gp = guide(p);
  if (!gp.hit) {
    if (last) { textureStore(dst, p, accum[i] / n); } else { textureStore(dst, p, textureLoad(src, p, 0)); }
    return;
  }
  let cp = textureLoad(src, p, 0);
  // the variance a bit smoothed (a 3x3 gaussian): one pixel's estimate is noisy itself
  var vs = 0.0;
  let hi = vec2<i32>(i32(d.size.x) - 1, i32(d.size.y) - 1);
  for (var y = -1; y <= 1; y = y + 1) {
    for (var x = -1; x <= 1; x = x + 1) {
      let k = (0.5 - 0.25 * f32(abs(x))) * (0.5 - 0.25 * f32(abs(y)));
      vs += k * textureLoad(src, clamp(p + vec2<i32>(x, y), vec2<i32>(0), hi), 0).w;
    }
  }
  let sig = 4.0 * sqrt(vs) + 1e-4;
  let lp = lum(cp.rgb);
  let step = i32(d.size.w);
  let k0 = 0.375 * 0.375;
  var sc = cp.rgb * k0;
  var sv = cp.w * k0 * k0;
  var sw = k0;
  for (var y = -2; y <= 2; y = y + 1) {
    for (var x = -2; x <= 2; x = x + 1) {
      if (x == 0 && y == 0) { continue; }
      let q = p + vec2<i32>(x, y) * step;
      if (!inside(q)) { continue; }
      let gq = guide(q);
      if (!gq.hit) { continue; }
      let cq = textureLoad(src, q, 0);
      let w = kern(x) * kern(y) * geo_w(gp, gq, length(vec2<f32>(f32(x), f32(y))) * f32(step)) * exp(-abs(lp - lum(cq.rgb)) / sig);
      sc += w * cq.rgb;
      sv += w * w * cq.w;
      sw += w;
    }
  }
  let c = sc / sw;
  if (last) {
    textureStore(dst, p, vec4<f32>(c * gp.alb, accum[i].w / n));
  } else {
    textureStore(dst, p, vec4<f32>(c, sv / (sw * sw)));
  }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DParams {
    inv_vp: [[f32; 4]; 4],
    size: [f32; 4],
    misc: [f32; 4],
}

struct Denoiser {
    bgl: wgpu::BindGroupLayout,
    prepare: wgpu::ComputePipeline,
    atrous: wgpu::ComputePipeline,
    /// One `DParams` per pass, `stride` bytes apart (dynamic offsets).
    params: wgpu::Buffer,
    stride: u64,
    /// Ping-pong targets: demodulated colour and variance.
    ping: Option<([wgpu::TextureView; 2], (u32, u32))>,
}

impl Denoiser {
    fn new(device: &wgpu::Device) -> Self {
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let storage = |binding: u32| {
            entry(binding, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None })
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("denoise"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<DParams>() as u64),
                    },
                ),
                storage(1),
                storage(2),
                storage(3),
                entry(
                    4,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    5,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("denoise"), source: wgpu::ShaderSource::Wgsl(DENOISE.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("denoise"), bind_group_layouts: &[&bgl], push_constant_ranges: &[] });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let stride = (std::mem::size_of::<DParams>() as u64).div_ceil(align) * align;
        Denoiser {
            prepare: pipe("prepare"),
            atrous: pipe("atrous"),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("denoise_params"),
                size: stride * (DENOISE_PASSES as u64 + 1),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            stride,
            bgl,
            ping: None,
        }
    }

    /// Filter the accumulation (`samples` deep; `bufs`: colour, guides, moments) into `out`.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        bufs: [&wgpu::Buffer; 3],
        out: &wgpu::TextureView,
        size: (u32, u32),
        view_proj: glam::Mat4,
        samples: u32,
    ) {
        if self.ping.as_ref().is_none_or(|p| p.1 != size) {
            let make = || {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("denoise_ping"),
                        size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba16Float,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            self.ping = Some(([make(), make()], size));
        }
        let inv = view_proj.inverse();
        // angle between the rays of two neighbouring pixels at the centre of the view
        let ray = |x: f32| {
            let c = glam::Vec2::new(x / size.0 as f32 * 2.0 - 1.0, 0.0);
            (inv.project_point3(c.extend(1.0)) - inv.project_point3(c.extend(0.0))).normalize()
        };
        let pix = ray(size.0 as f32 * 0.5).angle_between(ray(size.0 as f32 * 0.5 + 1.0)).max(1e-6);
        let mut bytes = vec![0u8; self.params.size() as usize];
        for k in 0..=DENOISE_PASSES {
            let p = DParams {
                inv_vp: inv.to_cols_array_2d(),
                size: [size.0 as f32, size.1 as f32, samples as f32, if k == 0 { 0.0 } else { (1u32 << (k - 1)) as f32 }],
                misc: [pix, if k == DENOISE_PASSES { 1.0 } else { 0.0 }, 0.0, 0.0],
            };
            let at = (k as u64 * self.stride) as usize;
            bytes[at..at + std::mem::size_of::<DParams>()].copy_from_slice(bytemuck::bytes_of(&p));
        }
        queue.write_buffer(&self.params, 0, &bytes);
        let ping = &self.ping.as_ref().unwrap().0;
        let bind = |src: &wgpu::TextureView, dst: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("denoise"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.params,
                            offset: 0,
                            size: wgpu::BufferSize::new(std::mem::size_of::<DParams>() as u64),
                        }),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: bufs[0].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: bufs[1].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: bufs[2].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(dst) },
                ],
            })
        };
        // prepare -> ping 0; a-trous pass k reads ping k%2 and writes the other one, the last one writes `out`
        let mut groups = vec![bind(&ping[1], &ping[0])];
        for k in 0..DENOISE_PASSES as usize {
            let dst = if k + 1 == DENOISE_PASSES as usize { out } else { &ping[(k + 1) % 2] };
            groups.push(bind(&ping[k % 2], dst));
        }
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("denoise"), timestamp_writes: None });
        for (k, g) in groups.iter().enumerate() {
            pass.set_pipeline(if k == 0 { &self.prepare } else { &self.atrous });
            pass.set_bind_group(0, g, &[(k as u64 * self.stride) as u32]);
            pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        }
    }
}

/// Sample counts of a still view at which the neural denoiser runs (each result replaces the previous one).
const AI_AT: [u32; 4] = [32, 256, 1024, MAX_SAMPLES];

/// Neural denoiser (Open Image Denoise, `oidn.rs`) on the accumulated picture: the accumulation and the guides are
/// copied to a staging buffer, mapped a frame later (once the copy was submitted), denoised on the worker thread
/// and uploaded to `tex`, which is then shown in place of the fast filter's picture.
struct Ai {
    worker: crate::oidn::Worker,
    staging: Option<(wgpu::Buffer, u64)>,
    /// A readback: accumulation run, samples, size and the map state (None until requested; 1 = mapped, 2 = failed).
    reading: Option<(u64, u32, (u32, u32), Option<std::sync::Arc<std::sync::atomic::AtomicU8>>)>,
    /// A job on the worker.
    busy: bool,
    /// Samples of the last picture sent for this accumulation.
    sent: u32,
    tex: Option<(wgpu::Texture, (u32, u32))>,
    /// Accumulation run and samples of the picture in `tex`.
    shown: Option<(u64, u32)>,
}

impl Ai {
    fn new(worker: crate::oidn::Worker) -> Self {
        Ai { worker, staging: None, reading: None, busy: false, sent: 0, tex: None, shown: None }
    }

    fn reset(&mut self) {
        self.sent = 0;
        self.shown = None;
    }

    /// Hand a mapped readback to the worker and take its finished pictures. False when the worker is gone.
    fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, run: u64) -> bool {
        use std::sync::atomic::Ordering;
        if let (Some((r, samples, size, state)), Some((staging, bytes))) = (&mut self.reading, &self.staging) {
            match state {
                None => {
                    let s = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
                    let s2 = s.clone();
                    staging.slice(..*bytes).map_async(wgpu::MapMode::Read, move |res| s2.store(if res.is_ok() { 1 } else { 2 }, Ordering::Release));
                    *state = Some(s);
                }
                Some(s) => {
                    device.poll(wgpu::Maintain::Poll);
                    match s.load(Ordering::Acquire) {
                        1 => {
                            let px = (size.0 * size.1) as usize;
                            let job = {
                                let data = staging.slice(..*bytes).get_mapped_range();
                                crate::oidn::Job {
                                    key: *r,
                                    samples: *samples,
                                    size: *size,
                                    accum: bytemuck::cast_slice(&data[..px * 16]).to_vec(),
                                    feat: bytemuck::cast_slice(&data[px * 16..px * 32]).to_vec(),
                                }
                            };
                            staging.unmap();
                            if job.key == run {
                                if !self.worker.submit(job) {
                                    return false;
                                }
                                self.busy = true;
                            }
                            self.reading = None;
                        }
                        2 => self.reading = None,
                        _ => {}
                    }
                }
            }
        }
        while let Some(res) = self.worker.poll() {
            self.busy = false;
            let d = match res {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[oidn] {e}");
                    continue;
                }
            };
            if d.key != run {
                continue;
            }
            if self.tex.as_ref().is_none_or(|t| t.1 != d.size) {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("oidn"),
                    size: wgpu::Extent3d { width: d.size.0, height: d.size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                self.tex = Some((tex, d.size));
            }
            queue.write_texture(
                self.tex.as_ref().unwrap().0.as_image_copy(),
                bytemuck::cast_slice(&d.rgba),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(d.size.0 * 8), rows_per_image: None },
                wgpu::Extent3d { width: d.size.0, height: d.size.1, depth_or_array_layers: 1 },
            );
            self.shown = Some((run, d.samples));
        }
        true
    }

    /// Copy the accumulation out when the view has stayed still up to the next of `AI_AT` (and nothing is on the way).
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder, accum: &wgpu::Buffer, feat: &wgpu::Buffer, run: u64, samples: u32, size: (u32, u32)) {
        let due = AI_AT.iter().any(|&a| samples >= a && self.sent < a);
        if !due || self.reading.is_some() || self.busy {
            return;
        }
        let px = (size.0 * size.1) as u64;
        let bytes = px * 32;
        if self.staging.as_ref().is_none_or(|s| s.1 != bytes) {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("oidn_staging"),
                size: bytes.max(32),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.staging = Some((buf, bytes));
        }
        let staging = &self.staging.as_ref().unwrap().0;
        enc.copy_buffer_to_buffer(accum, 0, staging, 0, px * 16);
        enc.copy_buffer_to_buffer(feat, 0, staging, px * 16, px * 16);
        self.reading = Some((run, samples, size, None));
        self.sent = samples;
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inv_vp: [[f32; 4]; 4],
    eye: [f32; 4],
    size: [f32; 4],
    env: [f32; 4],
    misc: [f32; 4],
}

/// Inputs of one path-traced frame.
pub struct FrameInput<'a> {
    pub view_proj: glam::Mat4,
    pub eye: glam::Vec3,
    pub size: (u32, u32),
    pub env_yaw: f32,
    pub emissive_intensity: f32,
    pub show_background: bool,
    pub isolate: Option<usize>,
    /// One `(base, aux, normal, emissive)` view per texture set.
    pub sets: Vec<[&'a wgpu::TextureView; 4]>,
    pub env: &'a wgpu::TextureView,
    pub out: &'a wgpu::TextureView,
    /// The texture of `out` (the neural denoiser's picture is copied into it).
    pub out_tex: &'a wgpu::Texture,
    /// Bumped whenever textures or the environment change (restarts the accumulation).
    pub scene_gen: u64,
}

pub struct PathTracer {
    scene_bgl: wgpu::BindGroupLayout,
    nodes: wgpu::Buffer,
    order: wgpu::Buffer,
    verts: wgpu::Buffer,
    idx: wgpu::Buffer,
    params: wgpu::Buffer,
    lin: wgpu::Sampler,
    mat_samp: wgpu::Sampler,
    marginal: wgpu::Buffer,
    conditional: wgpu::Buffer,
    env_dims: (u32, u32, f32),
    /// Accumulated colour, denoiser guides and sample moments, for a size.
    accum: Option<([wgpu::Buffer; 3], (u32, u32))>,
    /// None with `SHARD_NO_DENOISE` (the raw accumulation, to compare).
    denoiser: Option<Denoiser>,
    /// Neural denoiser on a still view (None when off or not installed); `ai_failed`: the library did not load.
    ai: Option<Ai>,
    ai_failed: bool,
    /// Counts the accumulations (a new one each time the samples restart from 0).
    run: u64,
    /// Pipeline and texture layout for a number of sets.
    pipe: Option<(usize, wgpu::BindGroupLayout, wgpu::ComputePipeline)>,
    samples: u32,
    key: u64,
}

impl PathTracer {
    /// Whether the device can run it (texture binding arrays indexed per triangle).
    pub fn supported(features: wgpu::Features) -> bool {
        features.contains(
            wgpu::Features::TEXTURE_BINDING_ARRAY | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        )
    }

    pub fn new(device: &wgpu::Device, mesh: &Mesh) -> Self {
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let scene_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pt_scene"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                storage(7, true),
                storage(8, true),
                storage(9, false),
                storage(11, false),
                storage(12, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let lin = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let mat_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let empty = |label: &str| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: &[0u8; 16], usage: wgpu::BufferUsages::STORAGE });
        let mut pt = PathTracer {
            scene_bgl,
            nodes: empty("pt_nodes"),
            order: empty("pt_order"),
            verts: empty("pt_verts"),
            idx: empty("pt_idx"),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pt_params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            lin,
            mat_samp,
            marginal: empty("pt_marginal"),
            conditional: empty("pt_conditional"),
            env_dims: (1, 1, 1.0),
            accum: None,
            denoiser: if std::env::var_os("SHARD_NO_DENOISE").is_some() { None } else { Some(Denoiser::new(device)) },
            ai: None,
            ai_failed: false,
            run: 0,
            pipe: None,
            samples: 0,
            key: 0,
        };
        pt.set_mesh(device, mesh);
        pt
    }

    /// Geometry and BVH of the mesh.
    pub fn set_mesh(&mut self, device: &wgpu::Device, mesh: &Mesh) {
        let (nodes, order) = mesh.gpu_bvh();
        let mut verts: Vec<[f32; 4]> = Vec::with_capacity(mesh.vertices.len() * 4);
        for v in &mesh.vertices {
            verts.push([v.pos[0], v.pos[1], v.pos[2], v.uv[0]]);
            verts.push([v.normal[0], v.normal[1], v.normal[2], v.uv[1]]);
            verts.push([v.tangent[0], v.tangent[1], v.tangent[2], f32::from_bits(v.mat)]);
            verts.push([v.bitangent[0], v.bitangent[1], v.bitangent[2], 0.0]);
        }
        let buf = |label: &str, bytes: &[u8]| {
            let contents: Vec<u8> = if bytes.is_empty() { vec![0u8; 16] } else { bytes.to_vec() };
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: &contents, usage: wgpu::BufferUsages::STORAGE })
        };
        self.nodes = buf("pt_nodes", bytemuck::cast_slice(&nodes));
        self.order = buf("pt_order", bytemuck::cast_slice(order));
        self.verts = buf("pt_verts", bytemuck::cast_slice(&verts));
        self.idx = buf("pt_idx", bytemuck::cast_slice(&mesh.indices));
        self.samples = 0;
    }

    /// Importance-sampling tables of the environment from a small linear RGB copy of it (`w x h`).
    pub fn set_environment(&mut self, device: &wgpu::Device, w: u32, h: u32, rgb: &[f32]) {
        let (w, h) = (w.max(1), h.max(1));
        let mut conditional = vec![0f32; (w * h) as usize];
        let mut rows = vec![0f32; h as usize];
        for y in 0..h as usize {
            let s = ((y as f32 + 0.5) / h as f32 * std::f32::consts::PI).sin();
            let mut acc = 0f32;
            for x in 0..w as usize {
                let i = y * w as usize + x;
                let l = (0.2126 * rgb[i * 3] + 0.7152 * rgb[i * 3 + 1] + 0.0722 * rgb[i * 3 + 2]).max(0.0) * s + 1e-6;
                acc += l;
                conditional[i] = acc;
            }
            for x in 0..w as usize {
                conditional[y * w as usize + x] /= acc;
            }
            rows[y] = acc;
        }
        let total: f32 = rows.iter().sum();
        let mut marginal = vec![0f32; h as usize];
        let mut acc = 0f32;
        for y in 0..h as usize {
            acc += rows[y] / total;
            marginal[y] = acc;
        }
        self.marginal = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("pt_marginal"), contents: bytemuck::cast_slice(&marginal), usage: wgpu::BufferUsages::STORAGE });
        self.conditional =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("pt_conditional"), contents: bytemuck::cast_slice(&conditional), usage: wgpu::BufferUsages::STORAGE });
        self.env_dims = (w, h, total);
        self.samples = 0;
    }

    /// Samples accumulated in the picture shown.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Restart the accumulation (an edit that does not change any input the tracer can see by itself).
    pub fn reset(&mut self) {
        self.samples = 0;
    }

    fn pipeline(&mut self, device: &wgpu::Device, n_sets: usize) {
        if self.pipe.as_ref().is_some_and(|p| p.0 == n_sets) {
            return;
        }
        let count = std::num::NonZeroU32::new(n_sets.max(1) as u32);
        let arr = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count,
        };
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pt_textures"),
            entries: &[
                arr(0),
                arr(1),
                arr(2),
                arr(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("pathtrace"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pathtrace"),
            bind_group_layouts: &[&self.scene_bgl, &tex_bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("pathtrace"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        self.pipe = Some((n_sets, tex_bgl, pipeline));
    }

    /// Add one sample per pixel to the picture in `f.out` (restarting when the view or the scene changed).
    /// Returns false once the picture has converged (nothing was drawn).
    /// `ts`: a timestamp to write at the start of the pass (statistics).
    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        f: &FrameInput,
        ts: Option<(&wgpu::QuerySet, u32)>,
    ) -> bool {
        if f.sets.is_empty() {
            return false;
        }
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for v in f.view_proj.to_cols_array() {
                v.to_bits().hash(&mut h);
            }
            (f.size, f.env_yaw.to_bits(), f.emissive_intensity.to_bits(), f.show_background, f.isolate, f.scene_gen).hash(&mut h);
            h.finish()
        };
        if key != self.key {
            self.key = key;
            self.samples = 0;
        }
        if let Some(ai) = &mut self.ai {
            if !ai.poll(device, queue, self.run) {
                self.ai = None;
                self.ai_failed = true;
            }
        }
        if self.samples >= MAX_SAMPLES {
            self.ai_finish(device, enc, f);
            return false;
        }
        let px = (f.size.0 * f.size.1) as u64;
        if self.accum.as_ref().is_none_or(|a| a.1 != f.size) {
            let buf = |label: &str, bytes: u64| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: px.max(1) * bytes,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })
            };
            self.accum = Some(([buf("pt_accum", 16), buf("pt_feat", 16), buf("pt_mom", 8)], f.size));
            self.samples = 0;
        }
        if self.samples == 0 {
            // a new accumulation: what the neural denoiser made of the previous one is stale
            self.run += 1;
            if let Some(ai) = &mut self.ai {
                ai.reset();
            }
        }
        self.pipeline(device, f.sets.len());
        let params = Params {
            inv_vp: f.view_proj.inverse().to_cols_array_2d(),
            eye: [f.eye.x, f.eye.y, f.eye.z, 0.0],
            size: [f.size.0 as f32, f.size.1 as f32, self.samples as f32 + (self.key & 0xffff) as f32 * 0.0, self.samples as f32],
            env: [f.env_yaw, self.env_dims.0 as f32, self.env_dims.1 as f32, self.env_dims.2],
            misc: [
                f.emissive_intensity,
                if f.show_background { 1.0 } else { 0.0 },
                f.isolate.map(|i| i as f32).unwrap_or(-1.0),
                if std::env::var_os("SHARD_NORMAL_DIRECTX").is_some() { -1.0 } else { 1.0 },
            ],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let [accum, feat, mom] = &self.accum.as_ref().unwrap().0;
        let scene = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pt_scene"),
            layout: &self.scene_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.nodes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.order.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.verts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: self.idx.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(f.env) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.lin) },
                wgpu::BindGroupEntry { binding: 7, resource: self.marginal.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: self.conditional.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 9, resource: accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(f.out) },
                wgpu::BindGroupEntry { binding: 11, resource: feat.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 12, resource: mom.as_entire_binding() },
            ],
        });
        let (_, tex_bgl, pipeline) = self.pipe.as_ref().unwrap();
        let col = |k: usize| f.sets.iter().map(|s| s[k]).collect::<Vec<_>>();
        let (b, a, n, e) = (col(0), col(1), col(2), col(3));
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pt_textures"),
            layout: tex_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureViewArray(&b) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureViewArray(&a) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureViewArray(&n) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureViewArray(&e) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.mat_samp) },
            ],
        });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("pathtrace"),
                timestamp_writes: ts.map(|(q, i)| wgpu::ComputePassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: Some(i),
                    end_of_pass_write_index: None,
                }),
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &scene, &[]);
            pass.set_bind_group(1, &textures, &[]);
            pass.dispatch_workgroups(f.size.0.div_ceil(8), f.size.1.div_ceil(8), 1);
        }
        self.samples += 1;
        if let Some(dn) = &mut self.denoiser {
            dn.run(device, queue, enc, [accum, feat, mom], f.out, f.size, f.view_proj, self.samples);
        }
        self.ai_finish(device, enc, f);
        true
    }

    /// Start the neural denoiser when the still view reached one of `AI_AT`, and show its latest picture of this
    /// accumulation over the fast filter's.
    fn ai_finish(&mut self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder, f: &FrameInput) {
        let (Some(ai), Some(([accum, feat, _], size))) = (&mut self.ai, &self.accum) else { return };
        if *size != f.size {
            return;
        }
        ai.start(device, enc, accum, feat, self.run, self.samples, *size);
        if let Some((tex, tsize)) = &ai.tex {
            if ai.shown.is_some_and(|s| s.0 == self.run) && *tsize == f.size {
                enc.copy_texture_to_texture(
                    tex.as_image_copy(),
                    f.out_tex.as_image_copy(),
                    wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                );
            }
        }
    }

    /// Turn the neural denoiser on or off (it stays off when the library is not installed).
    pub fn set_ai_denoise(&mut self, on: bool) {
        if on == self.ai.is_some() || (on && self.ai_failed) {
            return;
        }
        self.ai = if on {
            let w = crate::oidn::Worker::spawn();
            self.ai_failed = w.is_none();
            w.map(Ai::new)
        } else {
            None
        };
    }

    /// Samples of the neural denoiser's picture on screen (None: the fast filter's).
    pub fn ai_samples(&self) -> Option<u32> {
        self.ai.as_ref().and_then(|a| a.shown.filter(|s| s.0 == self.run).map(|s| s.1))
    }

    /// Whether more frames would change the picture: samples to add, or the neural denoiser at work.
    pub fn busy(&self) -> bool {
        self.samples < MAX_SAMPLES || self.ai_pending()
    }

    /// The neural denoiser is reading back or denoising a picture.
    pub fn ai_pending(&self) -> bool {
        self.ai.as_ref().is_some_and(|a| a.reading.is_some() || a.busy)
    }
}
