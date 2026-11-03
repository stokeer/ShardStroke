//! wgpu renderer: textured mesh with simple lit shading. One GPU texture (own resolution) and bind
//! group per texture set; triangles are sorted by set so each set is a single draw range.

use crate::mesh::{Mesh, Vertex};
use crate::paint::{PaintTexture, Rect4};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    inv_vp: [[f32; 4]; 4],
    eye: [f32; 4],
    light: [f32; 4],
    /// SH9 radiance coefficients (rgb in xyz).
    sh: [[f32; 4]; 9],
    /// x = exposure, y = max mip level of the environment, z = 1 for unlit channel view, w = environment yaw.
    params: [f32; 4],
    /// x = sign applied to the normal map Y axis (1 = +Y up), y = face normals.
    params2: [f32; 4],
    /// x = bits of the texture sets still loading (u32 bit pattern), y = seconds (animates their stripes),
    /// z = emissive intensity.
    params3: [f32; 4],
    /// Linear colour of the loading stripes (rgb).
    params4: [f32; 4],
    /// Post effects, see [`PostFx::uniforms`].
    post: [[f32; 4]; 6],
}

/// Post effects of the shaded view, after Painter's (`editor/posteffects.ini`, Display Settings > Post effects).
/// The per-pixel ones are evaluated in the fragment shader; bloom, glare, sharpen, depth of field, chromatic
/// aberration, lens flare and colour grading need image passes and are not rendered yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostFx {
    /// Master switch ("Activate post effects").
    pub enabled: bool,
    /// ACES filmic tone mapping (off: the linear value is clamped).
    pub tone_mapping: bool,
    /// Colour correction on display values: brightness / saturation / temperature / magenta-green are offsets
    /// around 0, contrast (around mid grey) and gamma factors around 1.
    pub color_correction: bool,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub magenta_green: f32,
    /// Darkening towards the frame edges: strength, radius where it starts (1 = the frame edge), feather, and the
    /// width / height ratio of its ellipse.
    pub vignette: bool,
    pub vignette_level: f32,
    pub vignette_radius: f32,
    pub vignette_feather: f32,
    pub vignette_ratio: f32,
    /// Film grain: noise amplitude.
    pub grain: bool,
    pub grain_level: f32,
    /// Bloom: glow around what is brighter than `bloom_threshold` (linear, before exposure); `bloom_level` is
    /// Painter's level (strength), `bloom_falloff` the weight of each wider blur level.
    pub bloom: bool,
    pub bloom_level: f32,
    pub bloom_threshold: f32,
    pub bloom_falloff: f32,
}

impl Default for PostFx {
    /// Painter's defaults: post effects on with every effect off.
    fn default() -> Self {
        PostFx {
            enabled: true,
            tone_mapping: false,
            color_correction: false,
            brightness: 0.0,
            contrast: 1.0,
            gamma: 1.0,
            saturation: 0.0,
            temperature: 0.0,
            magenta_green: 0.0,
            vignette: false,
            vignette_level: 1.0,
            vignette_radius: 0.5,
            vignette_feather: 0.5,
            vignette_ratio: 1.5,
            grain: false,
            grain_level: 0.01,
            bloom: false,
            bloom_level: 1.0,
            bloom_threshold: 1.5,
            bloom_falloff: 0.8,
        }
    }
}

impl PostFx {
    /// Shader block: [on, tone, cc, vignette], [brightness, contrast, gamma, saturation],
    /// [temperature, magenta-green, grain, vignette level], [vignette radius, feather, ratio, time],
    /// [viewport x, y, w, h] in pixels, [bloom strength (0 = off), threshold, -, falloff].
    fn uniforms(&self, viewport: [f32; 4], time: f32) -> [[f32; 4]; 6] {
        // Painter's bloom level is a strength of a few units (a test tank: 3.92); 0.05 per unit is a first scale,
        // not measured against Painter yet
        let bloom = if self.enabled && self.bloom { self.bloom_level.max(0.0) * 0.05 } else { 0.0 };
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        [
            [b(self.enabled), b(self.tone_mapping), b(self.color_correction), b(self.vignette)],
            [self.brightness, self.contrast, self.gamma.max(0.05), self.saturation],
            [self.temperature, self.magenta_green, if self.grain { self.grain_level } else { 0.0 }, self.vignette_level],
            [self.vignette_radius, self.vignette_feather.max(1e-3), self.vignette_ratio.max(0.05), time],
            viewport,
            [bloom, self.bloom_threshold.max(0.0), 0.0, self.bloom_falloff.clamp(0.0, 1.0)],
        ]
    }
}

struct SetGpu {
    texture: wgpu::Texture,
    /// Linear RGBA8 auxiliary map: R = ambient occlusion (1 = open), G = roughness, B = metallic.
    /// 1x1 default: open, roughness 0.55, dielectric.
    aux: wgpu::Texture,
    /// Tangent-space normal map (baked mesh normal); 1x1 flat by default.
    nrm: wgpu::Texture,
    /// Emissive colour (sRGB RGBA8); 1x1 black by default.
    emi: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bg_pipeline: wgpu::RenderPipeline,
    /// Draw the environment image behind the mesh.
    pub show_background: bool,
    /// Rotation of the environment around the vertical axis, radians (Shift + right mouse drag).
    pub env_yaw: f32,
    /// Bits of the texture sets whose composition is still on its way (drawn with moving stripes).
    pub loading_mask: u32,
    started: std::time::Instant,
    /// Linear colours of the loading stripes and outline.
    pub loading_fill: [f32; 3],
    pub loading_outline: [f32; 3],
    outline: Option<crate::outline::Outline>,
    /// Shade with the geometric face normal (the normal map was baked against it).
    pub face_normals: bool,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    /// `(first index, index count)` per texture set.
    ranges: Vec<(u32, u32)>,
    sets: Vec<SetGpu>,
    depth: Option<(wgpu::TextureView, (u32, u32))>,
    /// Multisampled colour target the scene is drawn into and resolved from (None without MSAA).
    msaa: Option<(wgpu::TextureView, (u32, u32))>,
    /// Samples per pixel of the shaded view.
    samples: u32,
    /// Sample counts the adapter supports (ascending, 1 included).
    supported_samples: Vec<u32>,
    /// Kept to rebuild the scene pipelines when the sample count changes.
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// HDR scene target, bloom chain and post pass.
    post_pipes: PostPipes,
    targets: Option<Targets>,
    /// Path-traced picture instead of the rasterised one ("Path tracing" render mode).
    pub path_trace: bool,
    /// The path tracer (None when the device has no texture binding arrays).
    pt: Option<crate::pathtrace::PathTracer>,
    /// Bumped when anything the path tracer sees changes (textures, environment, mesh).
    scene_gen: u64,
    /// GPU time of the viewport's passes (statistics overlay; None without timestamp queries).
    timer: Option<GpuTimer>,
    /// Measure it (only while the statistics are shown).
    pub measure_gpu: bool,
    /// Neural denoiser (Open Image Denoise) on the path-traced picture once the view stops.
    pub ai_denoise: bool,
    env_view: Option<wgpu::TextureView>,
    mip_bgl: wgpu::BindGroupLayout,
    mip_sampler: wgpu::Sampler,
    mip_srgb: wgpu::RenderPipeline,
    mip_linear: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    env_bgl: wgpu::BindGroupLayout,
    env_sampler: wgpu::Sampler,
    env_bind_group: Option<wgpu::BindGroup>,
    sh: [[f32; 4]; 9],
    env_max_lod: f32,
    pub exposure: f32,
    /// `ao_intensity` of the project's shader (1 = the baked occlusion as it is).
    pub ao_intensity: f32,
    /// Post effects of the shaded view.
    pub post: PostFx,
    /// `emissive_intensity` of the project's shader (scales the emissive channel).
    pub emissive_intensity: f32,
    /// Draw only this texture set (Alt+Q isolate).
    pub isolate: Option<usize>,
    /// Per-set images shown unlit instead of the shaded material (viewport channel selector).
    view: Option<Vec<SetGpu>>,
}

const SHADER: &str = r#"
struct U {
  vp: mat4x4<f32>,
  inv_vp: mat4x4<f32>,
  eye: vec4<f32>,
  light: vec4<f32>,
  sh: array<vec4<f32>, 9>,
  params: vec4<f32>,
  params2: vec4<f32>,
  params3: vec4<f32>,
  params4: vec4<f32>,
  post: array<vec4<f32>, 5>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var aux: texture_2d<f32>;
@group(0) @binding(4) var nrm: texture_2d<f32>;
@group(0) @binding(5) var emi: texture_2d<f32>;
@group(1) @binding(0) var env: texture_2d<f32>;
@group(1) @binding(1) var env_samp: sampler;

// rotate a world direction by the environment yaw (about +Y)
fn env_rot(d: vec3<f32>) -> vec3<f32> {
  let c = cos(u.params.w);
  let s = sin(u.params.w);
  return vec3<f32>(c * d.x + s * d.z, d.y, -s * d.x + c * d.z);
}

fn dir_to_uv(d: vec3<f32>) -> vec2<f32> {
  let uu = atan2(d.z, d.x) / 6.2831853 + 0.5;
  let vv = acos(clamp(d.y, -1.0, 1.0)) / 3.1415927;
  return vec2<f32>(uu, vv);
}

// diffuse irradiance from SH9 (Ramamoorthi & Hanrahan)
fn irradiance(n: vec3<f32>) -> vec3<f32> {
  let c1 = 0.429043; let c2 = 0.511664; let c3 = 0.743125; let c4 = 0.886227; let c5 = 0.247708;
  let l00 = u.sh[0].xyz; let l1m1 = u.sh[1].xyz; let l10 = u.sh[2].xyz; let l11 = u.sh[3].xyz;
  let l2m2 = u.sh[4].xyz; let l2m1 = u.sh[5].xyz; let l20 = u.sh[6].xyz; let l21 = u.sh[7].xyz;
  let l22 = u.sh[8].xyz;
  return c1 * l22 * (n.x * n.x - n.y * n.y) + c3 * l20 * n.z * n.z + c4 * l00 - c5 * l20
    + 2.0 * c1 * (l2m2 * n.x * n.y + l21 * n.x * n.z + l2m1 * n.y * n.z)
    + 2.0 * c2 * (l11 * n.x + l1m1 * n.y + l10 * n.z);
}

// The scene is drawn in linear HDR (exposure, bloom, tone mapping and the other post effects come in the post
// pass, `POST_SHADER`): alpha is the coverage, the clear colour (0, 0, 0, 0) marks the empty background.
struct VIn {
  @location(0) pos: vec3<f32>,
  @location(1) n: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) t: vec3<f32>,
  @location(4) b: vec3<f32>,
};
struct VOut {
  @builtin(position) p: vec4<f32>,
  @location(0) wp: vec3<f32>,
  @location(1) n: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) t: vec3<f32>,
  @location(4) b: vec3<f32>,
  @location(5) @interpolate(flat) tset: u32,
};

@vertex fn vs(v: VIn, @builtin(instance_index) inst: u32) -> VOut {
  var o: VOut;
  o.tset = inst;
  o.p = u.vp * vec4<f32>(v.pos, 1.0);
  o.wp = v.pos;
  o.n = v.n;
  o.uv = v.uv;
  o.t = v.t;
  o.b = v.b;
  return o;
}

struct BgOut {
  @builtin(position) p: vec4<f32>,
  @location(0) ndc: vec2<f32>,
};

@vertex fn vs_bg(@builtin(vertex_index) vi: u32) -> BgOut {
  var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o: BgOut;
  o.p = vec4<f32>(pos[vi], 1.0, 1.0);
  o.ndc = pos[vi];
  return o;
}

@fragment fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
  let p = u.inv_vp * vec4<f32>(i.ndc, 1.0, 1.0);
  let d = normalize(p.xyz / p.w - u.eye.xyz);
  let c = textureSampleLevel(env, env_samp, dir_to_uv(env_rot(d)), 0.0).rgb;
  return vec4<f32>(c, 1.0);
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
  let tuv = vec2<f32>(i.uv.x, 1.0 - i.uv.y);
  let base = textureSample(tex, samp, tuv).rgb;
  if (u.params.z > 0.5) {
    return vec4<f32>(base, 1.0);
  }
  let a = textureSample(aux, samp, tuv);
  // the shader's `ao_intensity` setting of the project (Painter: occlusion = mix(1, baked AO, intensity))
  let ao = mix(1.0, a.r, clamp(u.light.w, 0.0, 1.0));
  let roughness = clamp(a.g, 0.05, 1.0);
  let metallic = a.b;
  let v = normalize(u.eye.xyz - i.wp);
  // Back faces are culled and winding follows the authored normals, so the normal is used as is.
  var ng = normalize(i.n);
  if (u.params2.y > 0.5) {
    // geometric (face) normal, the one the normal map was baked against; sign follows the authored normal
    var fnr = normalize(cross(dpdx(i.wp), dpdy(i.wp)));
    if (dot(fnr, ng) < 0.0) { fnr = -fnr; }
    ng = fnr;
  }
  // tangent frame from screen-space derivatives (no stored tangents), then the baked normal map
  let dp1 = dpdx(i.wp);
  let dp2 = dpdy(i.wp);
  let duv1 = dpdx(i.uv);
  let duv2 = dpdy(i.uv);
  let dp2perp = cross(dp2, ng);
  let dp1perp = cross(ng, dp1);
  let tt = dp2perp * duv1.x + dp1perp * duv2.x;
  let bb = dp2perp * duv1.y + dp1perp * duv2.y;
  let invmax = inverseSqrt(max(max(dot(tt, tt), dot(bb, bb)), 1e-20));
  let nt = textureSample(nrm, samp, tuv).rgb * 2.0 - 1.0;
  var n = normalize(tt * invmax * nt.x + bb * invmax * nt.y * u.params2.x + ng * max(nt.z, 0.05));
  if (dot(i.t, i.t) > 0.25 && u.params2.y < 0.5) {
    // the frame Painter stored (interpolated per vertex, like the one the normal map was baked in)
    // measured against the face normals of flat faces (SHARD_NORMAL_PROBE): with Painter's stored bitangent
    // the baked green channel points the other way
    n = normalize(normalize(i.t) * nt.x - normalize(i.b) * nt.y * u.params2.x + ng * max(nt.z, 0.05));
  }
  let f0 = mix(vec3<f32>(0.04), base, metallic);
  let ndv = clamp(dot(n, v), 0.0, 1.0);
  // split-sum environment BRDF (Karis, "Physically Based Shading on Mobile")
  let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
  let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
  let rr = roughness * c0 + c1;
  let a004 = min(rr.x * rr.x, exp2(-9.28 * ndv)) * rr.x + rr.y;
  let ab = vec2<f32>(-1.04, 1.04) * a004 + rr.zw;
  // grazing (F90) term fades out for surfaces whose F0 is nearly black: a pure metal with a black base colour
  // reflects nothing ("vantablack"), as in Painter's viewport; dielectrics (F0 = 0.04) keep the full term
  let f90 = clamp(dot(f0, vec3<f32>(50.0 / 3.0)), 0.0, 1.0);
  let spec_albedo = f0 * ab.x + vec3<f32>(ab.y * f90);
  // diffuse: energy left after the specular reflection; metals have none
  let kd = (vec3<f32>(1.0) - spec_albedo) * (1.0 - metallic);
  let diffuse = base * kd * max(irradiance(env_rot(n)), vec3<f32>(0.0)) / 3.1415927 * ao;
  let r = reflect(-v, n);
  // the pyramid holds GGX-prefiltered radiance: mip k = roughness k / (levels - 1)
  let spec_rad = textureSampleLevel(env, env_samp, dir_to_uv(env_rot(r)), roughness * u.params.y).rgb;
  // specular occlusion from the baked AO, stronger for rough surfaces
  let spec_occ = clamp(pow(ndv + ao, exp2(-16.0 * roughness - 1.0)) - 1.0 + ao, 0.0, 1.0);
  let spec = spec_rad * spec_albedo * spec_occ;
  // emission, scaled by the shader's `emissive_intensity`; above the bloom threshold it glows
  let emissive = textureSample(emi, samp, tuv).rgb * u.params3.z;
  var col = diffuse + spec + emissive;
  // a texture set whose composition is still on its way: moving diagonal stripes over it
  let loading = bitcast<u32>(u.params3.x);
  if (i.tset < 32u && ((loading >> i.tset) & 1u) == 1u) {
    let s = fract((i.p.x + i.p.y) / 26.0 - u.params3.y * 0.8);
    let band = smoothstep(0.0, 0.06, s) - smoothstep(0.5, 0.56, s);
    col = mix(col, u.params4.rgb, 0.25 + 0.6 * band);
  }
  return vec4<f32>(col, 1.0);
}
"#;

/// Final pass: the HDR scene (premultiplied by coverage) through exposure, bloom and the post effects, over the
/// viewport background.
const POST_SHADER: &str = r#"
struct U {
  vp: mat4x4<f32>,
  inv_vp: mat4x4<f32>,
  eye: vec4<f32>,
  light: vec4<f32>,
  sh: array<vec4<f32>, 9>,
  params: vec4<f32>,
  params2: vec4<f32>,
  params3: vec4<f32>,
  params4: vec4<f32>,
  post: array<vec4<f32>, 6>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var bloom: texture_2d<f32>;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
  var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  return vec4<f32>(pos[i], 0.0, 1.0);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
  let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn hash12(p: vec2<f32>) -> f32 {
  var q = fract(p * vec2<f32>(0.1031, 0.1030));
  q += dot(q, q.yx + 33.33);
  return fract((q.x + q.y) * q.x);
}

// What Painter does to the shaded view (`PostFx`): the exposure, then -- when post effects are on -- the tone
// mapping (the projects of the corpus leave it off: the linear value goes straight to the sRGB output), the colour
// correction on display values (contrast around mid grey, the corpus' only active effect), the vignette and the
// film grain. `fp` = framebuffer position of the fragment.
fn post(x: vec3<f32>, fp: vec2<f32>) -> vec3<f32> {
  var c = x * u.params.x;
  let on = u.post[0].x > 0.5;
  if (on && u.post[0].y > 0.5) {
    c = aces(c);
  } else {
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
  }
  if (!on) {
    return c;
  }
  if (u.post[0].z > 0.5) {
    let k = u.post[1];
    let t = u.post[2];
    var d = pow(c, vec3<f32>(1.0 / 2.2));
    d = d * vec3<f32>(1.0 + 0.1 * t.x, 1.0 - 0.1 * t.y, 1.0 - 0.1 * t.x);
    d = d + vec3<f32>(k.x);
    d = (d - 0.5) * k.y + 0.5;
    let l = dot(d, vec3<f32>(0.2126, 0.7152, 0.0722));
    d = mix(vec3<f32>(l), d, 1.0 + k.w);
    d = pow(max(d, vec3<f32>(0.0)), vec3<f32>(1.0 / k.z));
    c = pow(clamp(d, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2.2));
  }
  let vp = u.post[4];
  if (u.post[0].w > 0.5 && vp.z > 0.0 && vp.w > 0.0) {
    let q = (fp - vp.xy) / vp.zw - 0.5;
    let e = length(vec2<f32>(q.x * (vp.z / vp.w) / u.post[3].z, q.y)) * 2.0;
    let r = u.post[3].x * 2.0;
    let v = smoothstep(r, r + u.post[3].y * 2.0, e) * u.post[2].w;
    c = c * (1.0 - clamp(v, 0.0, 1.0));
  }
  if (u.post[2].z > 0.0) {
    let n = hash12(fp + vec2<f32>(fract(u.post[3].w * 7.13) * 512.0, 0.0)) - 0.5;
    c = clamp(c + vec3<f32>(n * u.post[2].z * 2.0), vec3<f32>(0.0), vec3<f32>(1.0));
  }
  return c;
}


@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  // Painter's viewport background with the environment hidden: sRGB 36 (measured on its screenshots)
  let bg = vec3<f32>(0.0176);
  let s = textureLoad(hdr, vec2<i32>(p.xy), 0);
  let a = clamp(s.a, 0.0, 1.0);
  if (u.params.z > 0.5) {
    // channel view: the unlit values as they are
    if (a <= 0.0) { return vec4<f32>(bg, 1.0); }
    return vec4<f32>(mix(bg, s.rgb / a, a), 1.0);
  }
  var glow = vec3<f32>(0.0);
  if (u.post[0].x > 0.5 && u.post[5].x > 0.0) {
    let dim = vec2<f32>(textureDimensions(hdr));
    glow = textureSampleLevel(bloom, smp, p.xy / dim, 0.0).rgb * u.post[5].x;
  }
  var surface = vec3<f32>(0.0);
  if (a > 0.0) {
    surface = post(s.rgb / a + glow, p.xy);
  }
  // around the mesh the glow lands on the background
  let around = bg + clamp(glow * u.params.x, vec3<f32>(0.0), vec3<f32>(1.0));
  return vec4<f32>(mix(around, surface, a), 1.0);
}
"#;

/// Bloom chain: `mode` 0 = bright-pass + downsample from the scene, 1 = downsample, 2 = upsample (tent filter,
/// added onto the next finer level with the falloff weight).
const BLOOM_SHADER: &str = r#"
struct U {
  vp: mat4x4<f32>,
  inv_vp: mat4x4<f32>,
  eye: vec4<f32>,
  light: vec4<f32>,
  sh: array<vec4<f32>, 9>,
  params: vec4<f32>,
  params2: vec4<f32>,
  params3: vec4<f32>,
  params4: vec4<f32>,
  post: array<vec4<f32>, 6>,
};
struct L { texel: vec2<f32>, mode: f32, scale: f32 };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var<uniform> l: L;
struct O { @builtin(position) p: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> O {
  var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o: O;
  o.p = vec4<f32>(pos[i], 0.0, 1.0);
  o.uv = vec2<f32>(pos[i].x * 0.5 + 0.5, 0.5 - pos[i].y * 0.5);
  return o;
}
fn box4(uv: vec2<f32>) -> vec3<f32> {
  let t = l.texel;
  return (textureSampleLevel(src, smp, uv + vec2<f32>(-t.x, -t.y), 0.0).rgb
        + textureSampleLevel(src, smp, uv + vec2<f32>(t.x, -t.y), 0.0).rgb
        + textureSampleLevel(src, smp, uv + vec2<f32>(-t.x, t.y), 0.0).rgb
        + textureSampleLevel(src, smp, uv + vec2<f32>(t.x, t.y), 0.0).rgb) * 0.25;
}
@fragment fn fs(i: O) -> @location(0) vec4<f32> {
  if (l.mode < 0.5) {
    let c = box4(i.uv);
    // soft-knee bright pass
    let br = max(c.r, max(c.g, c.b));
    let t = u.post[5].y;
    let knee = max(t * 0.5, 1e-4);
    var soft = clamp(br - t + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-4);
    let w = max(soft, br - t) / max(br, 1e-4);
    return vec4<f32>(c * w, 1.0);
  }
  if (l.mode < 1.5) {
    return vec4<f32>(box4(i.uv), 1.0);
  }
  // 3x3 tent
  let t = l.texel;
  var c = textureSampleLevel(src, smp, i.uv, 0.0).rgb * 4.0;
  c += (textureSampleLevel(src, smp, i.uv + vec2<f32>(-t.x, 0.0), 0.0).rgb
      + textureSampleLevel(src, smp, i.uv + vec2<f32>(t.x, 0.0), 0.0).rgb
      + textureSampleLevel(src, smp, i.uv + vec2<f32>(0.0, -t.y), 0.0).rgb
      + textureSampleLevel(src, smp, i.uv + vec2<f32>(0.0, t.y), 0.0).rgb) * 2.0;
  c += textureSampleLevel(src, smp, i.uv + vec2<f32>(-t.x, -t.y), 0.0).rgb
     + textureSampleLevel(src, smp, i.uv + vec2<f32>(t.x, -t.y), 0.0).rgb
     + textureSampleLevel(src, smp, i.uv + vec2<f32>(-t.x, t.y), 0.0).rgb
     + textureSampleLevel(src, smp, i.uv + vec2<f32>(t.x, t.y), 0.0).rgb;
  return vec4<f32>(c / 16.0 * u.post[5].w, 1.0);
}
"#;

/// Box-filter downsample of one mip level into the next (a bilinear tap at the centre of 2x2 texels).
const MIP_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct O { @builtin(position) p: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> O {
  var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o: O;
  let p = pos[i];
  o.p = vec4<f32>(p, 0.0, 1.0);
  o.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
  return o;
}
@fragment fn fs(i: O) -> @location(0) vec4<f32> {
  return textureSampleLevel(src, smp, i.uv, 0.0);
}
"#;

/// Number of mip levels of a `w x h` texture.
fn mip_levels(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

/// Sample counts the device can render the shaded view with (colour target and depth), ascending; 1 = no MSAA.
/// Without the adapter-specific format features only the counts WebGPU guarantees (1, 4) are valid.
fn msaa_supported(adapter: &wgpu::Adapter, format: wgpu::TextureFormat, specific: bool) -> Vec<u32> {
    let ok = |n: u32| {
        (specific || n == 4)
            && adapter.get_texture_format_features(format).flags.sample_count_supported(n)
            && adapter.get_texture_format_features(wgpu::TextureFormat::Depth32Float).flags.sample_count_supported(n)
    };
    [1, 2, 4, 8].into_iter().filter(|&n| n == 1 || ok(n)).collect()
}

/// Counter of texture uploads (part of the path tracer's accumulation key).
static SCENE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// IEEE half bits -> f32.
fn image_f16(h: u16) -> f32 {
    shard_formats::image::f16_to_f32(h)
}

/// Format of the scene target: linear HDR (bloom and tone mapping need the values above 1).
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The supported sample count closest to `want` from below.
fn pick_samples(supported: &[u32], want: u32) -> u32 {
    supported.iter().copied().filter(|&n| n <= want.max(1)).max().unwrap_or(1)
}

/// The shaded-mesh and environment-background pipelines for `samples` samples per pixel (rebuilt when the
/// viewport's antialiasing changes).
fn scene_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
    let scene = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 2 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 36, shader_location: 3 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 48, shader_location: 4 },
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            front_face: wgpu::FrontFace::Ccw,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
        multiview: None,
        cache: None,
    });
    let background = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("background"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_bg"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_bg"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
        multiview: None,
        cache: None,
    });
    (scene, background)
}

/// Two timestamps around the viewport's passes, read back without waiting (the value arrives a frame or two late).
struct GpuTimer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// 0 idle, 1 being mapped, 2 ready to read.
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    period_ns: f32,
    last_ms: Option<f32>,
}

impl GpuTimer {
    fn new(device: &wgpu::Device, period_ns: f32) -> Self {
        GpuTimer {
            set: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("gpu_timer"), ty: wgpu::QueryType::Timestamp, count: 2 }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu_timer_resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            read: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu_timer_read"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Default::default(),
            period_ns,
            last_ms: None,
        }
    }

    /// Take a finished reading, if any.
    fn collect(&mut self) {
        use std::sync::atomic::Ordering;
        if self.state.load(Ordering::Acquire) != 2 {
            return;
        }
        {
            let data = self.read.slice(..).get_mapped_range();
            let t: &[u64] = bytemuck::cast_slice(&data);
            if t[1] > t[0] {
                self.last_ms = Some((t[1] - t[0]) as f32 * self.period_ns / 1.0e6);
            }
        }
        self.read.unmap();
        self.state.store(0, Ordering::Release);
    }

    fn idle(&self) -> bool {
        self.state.load(std::sync::atomic::Ordering::Acquire) == 0
    }
}

/// Pipelines of the bloom chain and the final post pass.
struct PostPipes {
    post_bgl: wgpu::BindGroupLayout,
    post: wgpu::RenderPipeline,
    bloom_bgl: wgpu::BindGroupLayout,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl PostPipes {
    fn new(device: &wgpu::Device, out_format: wgpu::TextureFormat) -> Self {
        let tex = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let ubo = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let smp = wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post_bgl"),
            entries: &[ubo(0), tex(1), smp, tex(3)],
        });
        let bloom_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom_bgl"),
            entries: &[ubo(0), tex(1), smp, ubo(3)],
        });
        let pipe = |label: &str, src: &str, bgl: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, additive: bool| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[bgl], push_constant_ranges: &[] });
            let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: additive.then_some(wgpu::BlendState { color: add, alpha: add }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        PostPipes {
            post: pipe("post", POST_SHADER, &post_bgl, out_format, false),
            bloom_down: pipe("bloom_down", BLOOM_SHADER, &bloom_bgl, HDR_FORMAT, false),
            bloom_up: pipe("bloom_up", BLOOM_SHADER, &bloom_bgl, HDR_FORMAT, true),
            post_bgl,
            bloom_bgl,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
        }
    }
}

/// Size-dependent targets: the resolved HDR scene, the bloom chain (half size and down) with the bind group of
/// every bloom pass, and the post pass's bind group.
struct Targets {
    size: (u32, u32),
    hdr: wgpu::TextureView,
    hdr_tex: wgpu::Texture,
    /// Bloom levels: view and size.
    levels: Vec<(wgpu::TextureView, (u32, u32))>,
    /// `(bind group, target level, additive)` in drawing order.
    passes: Vec<(wgpu::BindGroup, usize, bool)>,
    post_bind: wgpu::BindGroup,
}

impl Targets {
    fn new(device: &wgpu::Device, pipes: &PostPipes, uniforms: &wgpu::Buffer, size: (u32, u32)) -> Self {
        let make_tex = |w: u32, h: u32, label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                // copy destination: the neural denoiser's picture of the path tracer
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let make = |w: u32, h: u32, label: &str| make_tex(w, h, label).create_view(&wgpu::TextureViewDescriptor::default());
        let hdr_tex = make_tex(size.0, size.1, "hdr");
        let hdr = hdr_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut levels = Vec::new();
        let (mut w, mut h) = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        while levels.len() < 6 && w.min(h) >= 8 {
            levels.push((make(w, h, "bloom"), (w, h)));
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        if levels.is_empty() {
            levels.push((make(1, 1, "bloom"), (1, 1)));
        }
        let pass = |src: &wgpu::TextureView, src_size: (u32, u32), mode: f32| {
            let l: [f32; 4] = [1.0 / src_size.0.max(1) as f32, 1.0 / src_size.1.max(1) as f32, mode, 1.0];
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bloom_level"),
                contents: bytemuck::cast_slice(&l),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom_pass"),
                layout: &pipes.bloom_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&pipes.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: buf.as_entire_binding() },
                ],
            })
        };
        let mut passes = vec![(pass(&hdr, size, 0.0), 0, false)];
        for i in 1..levels.len() {
            passes.push((pass(&levels[i - 1].0, levels[i - 1].1, 1.0), i, false));
        }
        for i in (1..levels.len()).rev() {
            passes.push((pass(&levels[i].0, levels[i].1, 2.0), i - 1, true));
        }
        let post_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &pipes.post_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&hdr) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&pipes.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&levels[0].0) },
            ],
        });
        Targets { size, hdr, hdr_tex, levels, passes, post_bind }
    }
}

impl Renderer {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        format_hint: Option<wgpu::TextureFormat>,
        mesh: &Mesh,
        texs: &[PaintTexture],
        env: &crate::env::Environment,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no suitable GPU adapter")?;
        // MSAA other than 1x / 4x (the counts WebGPU guarantees) needs the adapter-specific format features; without
        // them a 2x / 8x target is a validation error (it crashed the app when 8x was picked)
        let specific = adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        // the path tracer reads every set's textures through binding arrays indexed per triangle
        let arrays = wgpu::Features::TEXTURE_BINDING_ARRAY | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING;
        let arrays = if adapter.features().contains(arrays) && std::env::var_os("SHARD_NO_PATHTRACE").is_none() {
            arrays
        } else {
            wgpu::Features::empty()
        };
        let al = adapter.limits();
        let limits = wgpu::Limits {
            max_sampled_textures_per_shader_stage: al.max_sampled_textures_per_shader_stage,
            max_storage_buffers_per_shader_stage: al.max_storage_buffers_per_shader_stage,
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            ..Default::default()
        };
        // GPU time of the viewport's passes for the statistics overlay
        let timestamps = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor { required_features: specific | arrays | timestamps, required_limits: limits, ..Default::default() },
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
        let format = format_hint.unwrap_or(wgpu::TextureFormat::Rgba8UnormSrgb);
        // 4x MSAA by default; `SHARD_MSAA` = 1 / 2 / 4 / 8 forces it, the viewport menu changes it afterwards
        let supported_samples = msaa_supported(&adapter, HDR_FORMAT, !specific.is_empty());
        let samples = pick_samples(&supported_samples, std::env::var("SHARD_MSAA").ok().and_then(|v| v.parse().ok()).unwrap_or(4));

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&Uniforms {
                view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
                inv_vp: glam::Mat4::IDENTITY.to_cols_array_2d(),
                eye: [0.0; 4],
                light: [0.4, 0.8, 0.5, 0.0],
                sh: [[0.0; 4]; 9],
                params: [1.0, 0.0, 0.0, 0.0],
                params2: [1.0, 0.0, 0.0, 0.0],
                params3: [0.0; 4],
                params4: [0.0; 4],
                post: [[0.0; 4]; 6],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let env_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("env_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        // trilinear + anisotropic: the set textures carry a mip chain (see `generate_mips`)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let mip_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let mip_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mip"),
            source: wgpu::ShaderSource::Wgsl(MIP_SHADER.into()),
        });
        let mip_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mip"),
            bind_group_layouts: &[&mip_bgl],
            push_constant_ranges: &[],
        });
        let mip_pipeline = |target: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mip"),
                layout: Some(&mip_layout),
                vertex: wgpu::VertexState {
                    module: &mip_module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &mip_module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(target.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let mip_srgb = mip_pipeline(wgpu::TextureFormat::Rgba8UnormSrgb);
        let mip_linear = mip_pipeline(wgpu::TextureFormat::Rgba8Unorm);
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[&bgl, &env_bgl],
            push_constant_ranges: &[],
        });
        let (pipeline, bg_pipeline) = scene_pipelines(&device, &layout, &shader, HDR_FORMAT, samples);
        let post = PostPipes::new(&device, format);
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vbuf"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ibuf"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let mut r = Self {
            device,
            queue,
            pipeline,
            bg_pipeline,
            show_background: false,
            env_yaw: 0.0,
            loading_mask: 0,
            started: std::time::Instant::now(),
            loading_fill: [0.03, 0.03, 0.03],
            loading_outline: [0.3, 0.3, 0.3],
            outline: None,
            face_normals: false,
            bgl,
            sampler,
            uniform_buf,
            vbuf,
            ibuf,
            ranges: mesh.set_ranges.clone(),
            sets: Vec::new(),
            depth: None,
            msaa: None,
            samples,
            supported_samples,
            shader,
            layout,
            post_pipes: post,
            targets: None,
            path_trace: false,
            pt: None,
            scene_gen: 0,
            timer: None,
            measure_gpu: false,
            ai_denoise: true,
            env_view: None,
            mip_bgl,
            mip_sampler,
            mip_srgb,
            mip_linear,
            format,
            env_bgl,
            env_sampler,
            env_bind_group: None,
            sh: [[0.0; 4]; 9],
            env_max_lod: 0.0,
            exposure: 1.0,
            ao_intensity: 1.0,
            post: PostFx::default(),
            emissive_intensity: 1.0,
            isolate: None,
            view: None,
        };
        if r.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            r.timer = Some(GpuTimer::new(&r.device, r.queue.get_timestamp_period()));
        }
        if crate::pathtrace::PathTracer::supported(r.device.features()) {
            r.pt = Some(crate::pathtrace::PathTracer::new(&r.device, mesh));
        }
        r.set_textures(texs);
        r.set_environment(env);
        Ok(r)
    }

    /// Upload an environment (mip chain + SH9) used for image-based lighting.
    pub fn set_environment(&mut self, env: &crate::env::Environment) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment"),
            size: wgpu::Extent3d { width: env.width, height: env.height, depth_or_array_layers: 1 },
            mip_level_count: env.mip_count(),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, data) in env.mips.iter().enumerate() {
            let w = (env.width >> level).max(1);
            let h = (env.height >> level).max(1);
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(data),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // importance-sampling tables of the path tracer from a level about 256 texels wide
        if let Some(pt) = &mut self.pt {
            let k = env.mips.iter().enumerate().position(|(k, _)| (env.width >> k) <= 256).unwrap_or(env.mips.len() - 1);
            let (w, h) = ((env.width >> k).max(1), (env.height >> k).max(1));
            let rgb: Vec<f32> = env.mips[k]
                .chunks_exact(4)
                .flat_map(|c| [image_f16(c[0]), image_f16(c[1]), image_f16(c[2])])
                .collect();
            pt.set_environment(&self.device, w, h, &rgb);
        }
        self.scene_gen += 1;
        self.env_view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.env_bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("env_bind"),
            layout: &self.env_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.env_sampler) },
            ],
        }));
        for k in 0..9 {
            self.sh[k] = [env.sh[k][0], env.sh[k][1], env.sh[k][2], 0.0];
        }
        self.env_max_lod = (env.mip_count().saturating_sub(1)) as f32;
    }

    fn rgba_texture(&self, w: u32, h: u32, format: wgpu::TextureFormat, label: &str) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mip_levels(w, h),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    fn bind(&self, texture: &wgpu::Texture, aux: &wgpu::Texture, nrm: &wgpu::Texture, emi: &wgpu::Texture) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let aux_view = aux.create_view(&wgpu::TextureViewDescriptor::default());
        let nrm_view = nrm.create_view(&wgpu::TextureViewDescriptor::default());
        let emi_view = emi.create_view(&wgpu::TextureViewDescriptor::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&aux_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&nrm_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&emi_view) },
            ],
        })
    }

    /// New set texture of `w x h`; `aux` reuses an existing auxiliary map (else 1x1 white).
    fn make_set(&self, w: u32, h: u32, aux: Option<wgpu::Texture>, nrm: Option<wgpu::Texture>, emi: Option<wgpu::Texture>) -> SetGpu {
        let texture = self.rgba_texture(w, h, wgpu::TextureFormat::Rgba8UnormSrgb, "paint");
        let aux = aux.unwrap_or_else(|| {
            let t = self.rgba_texture(1, 1, wgpu::TextureFormat::Rgba8Unorm, "aux");
            self.write_tex(&t, 0, 0, 1, 1, &[255, 140, 0, 255]);
            t
        });
        let nrm = nrm.unwrap_or_else(|| {
            let t = self.rgba_texture(1, 1, wgpu::TextureFormat::Rgba8Unorm, "nrm");
            self.write_tex(&t, 0, 0, 1, 1, &[128, 128, 255, 255]);
            t
        });
        let emi = emi.unwrap_or_else(|| {
            let t = self.rgba_texture(1, 1, wgpu::TextureFormat::Rgba8UnormSrgb, "emi");
            self.write_tex(&t, 0, 0, 1, 1, &[0, 0, 0, 255]);
            t
        });
        let bind_group = self.bind(&texture, &aux, &nrm, &emi);
        SetGpu { texture, aux, nrm, emi, bind_group, size: (w, h) }
    }

    /// Set the auxiliary map (R = ambient occlusion) of a texture set.
    pub fn set_aux(&mut self, set: usize, w: u32, h: u32, rgba: &[u8]) {
        if set >= self.sets.len() {
            return;
        }
        let t = self.rgba_texture(w, h, wgpu::TextureFormat::Rgba8Unorm, "aux");
        self.write_tex(&t, 0, 0, w, h, rgba);
        self.generate_mips(&t);
        let bind_group = self.bind(&self.sets[set].texture, &t, &self.sets[set].nrm, &self.sets[set].emi);
        self.sets[set].aux = t;
        self.sets[set].bind_group = bind_group;
    }

    /// Set the tangent-space normal map (OpenGL convention, RGBA8) of a texture set.
    pub fn set_normal(&mut self, set: usize, w: u32, h: u32, rgba: &[u8]) {
        if set >= self.sets.len() {
            return;
        }
        let t = self.rgba_texture(w, h, wgpu::TextureFormat::Rgba8Unorm, "nrm");
        self.write_tex(&t, 0, 0, w, h, rgba);
        self.generate_mips(&t);
        let bind_group = self.bind(&self.sets[set].texture, &self.sets[set].aux, &t, &self.sets[set].emi);
        self.sets[set].nrm = t;
        self.sets[set].bind_group = bind_group;
    }

    /// Set the emissive colour map (sRGB RGBA8) of a texture set; 1x1 black = no emission.
    pub fn set_emissive(&mut self, set: usize, w: u32, h: u32, rgba: &[u8]) {
        if set >= self.sets.len() {
            return;
        }
        let t = self.rgba_texture(w, h, wgpu::TextureFormat::Rgba8UnormSrgb, "emi");
        self.write_tex(&t, 0, 0, w, h, rgba);
        self.generate_mips(&t);
        let bind_group = self.bind(&self.sets[set].texture, &self.sets[set].aux, &self.sets[set].nrm, &t);
        self.sets[set].emi = t;
        self.sets[set].bind_group = bind_group;
    }

    /// Show per-set images unlit (`Some`, one `(w, h, rgba)` per set) or the shaded material (`None`).
    pub fn set_view(&mut self, images: Option<&[(u32, u32, Vec<u8>)]>) {
        self.view = images.map(|imgs| {
            imgs.iter()
                .map(|(w, h, data)| {
                    let s = self.make_set(*w, *h, None, None, None);
                    self.write_tex(&s.texture, 0, 0, *w, *h, data);
                    self.generate_mips(&s.texture);
                    s
                })
                .collect()
        });
    }

    /// (Re)create one GPU texture per texture set and upload their full contents.
    pub fn set_textures(&mut self, texs: &[PaintTexture]) {
        self.sets = texs
            .iter()
            .map(|t| {
                let (w, h) = t.gpu_dims();
                self.make_set(w, h, None, None, None)
            })
            .collect();
        for (i, t) in texs.iter().enumerate() {
            self.upload_full(i, t);
        }
    }

    /// Replace the vertex/index buffers and set ranges (after loading another mesh).
    pub fn set_mesh(&mut self, mesh: &Mesh) {
        self.vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vbuf"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ibuf"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.ranges = mesh.set_ranges.clone();
        if let Some(pt) = &mut self.pt {
            pt.set_mesh(&self.device, mesh);
        }
        self.scene_gen += 1;
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    fn write(&self, set: usize, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        let Some(s) = self.sets.get(set) else { return };
        self.write_tex(&s.texture, x, y, w, h, rgba);
    }

    /// Rebuild the mip chain of `texture` from level 0 (one tiny draw per level).
    fn generate_mips(&self, texture: &wgpu::Texture) {
        let levels = texture.mip_level_count();
        if levels < 2 {
            return;
        }
        let pipeline = if texture.format() == wgpu::TextureFormat::Rgba8UnormSrgb { &self.mip_srgb } else { &self.mip_linear };
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mips") });
        for level in 1..levels {
            let view = |l: u32| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: l,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            };
            let (src, dst) = (view(level - 1), view(level));
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip"),
                layout: &self.mip_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.mip_sampler) },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(enc.finish()));
    }

    fn write_tex(&self, texture: &wgpu::Texture, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        self.scene_gen_bump();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Something the path tracer reads changed (interior mutability: called from `&self` uploads too).
    fn scene_gen_bump(&self) {
        SCENE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Upload the whole resolved texture of `set`.
    pub fn upload_full(&mut self, set: usize, tex: &PaintTexture) {
        let want = tex.gpu_dims();
        if set >= self.sets.len() {
            return;
        }
        if self.sets[set].size != want {
            let (aux, nrm, emi) = (self.sets[set].aux.clone(), self.sets[set].nrm.clone(), self.sets[set].emi.clone());
            self.sets[set] = self.make_set(want.0, want.1, Some(aux), Some(nrm), Some(emi));
        }
        if want != (tex.w, tex.h) {
            // unpainted: flat colour (one texel) or the composited base image at its own size
            let data = tex.resolve_base_rgba();
            self.write(set, 0, 0, want.0, want.1, &data);
            self.generate_mips(&self.sets[set].texture);
            return;
        }
        // upload in bands so no full-size CPU/staging buffer is materialised (16 MB for 2K, 64 MB for 4K)
        const BAND: u32 = 128;
        let mut y = 0;
        while y < tex.h {
            let y1 = (y + BAND).min(tex.h);
            let data = tex.resolve_rect(Rect4 { x0: 0, y0: y, x1: tex.w, y1 });
            self.write(set, 0, y, tex.w, y1 - y, &data);
            y = y1;
        }
        self.generate_mips(&self.sets[set].texture);
    }

    /// Upload only the dirty rectangle of `set`.
    pub fn upload_rect(&mut self, set: usize, tex: &PaintTexture, r: Rect4) {
        if r.x1 <= r.x0 || r.y1 <= r.y0 {
            return;
        }
        let data = tex.resolve_rect(r);
        self.write(set, r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0, &data);
        if let Some(s) = self.sets.get(set) {
            self.generate_mips(&s.texture);
        }
    }

    /// Push pending changes of all paint targets to the GPU.
    pub fn sync_textures(&mut self, texs: &mut [PaintTexture]) {
        for (i, t) in texs.iter_mut().enumerate() {
            let want = t.gpu_dims();
            let size_ok = self.sets.get(i).map(|s| s.size) == Some(want);
            if t.full_dirty || !size_ok {
                self.upload_full(i, t); // also (re)allocates the GPU texture when the size class changed
            } else if let Some(r) = t.dirty_rect {
                self.upload_rect(i, t, r);
            }
            t.clear_dirty();
        }
    }

    /// Whether the path tracer can run on this device (and how many sets it can read).
    pub fn path_trace_supported(&self) -> bool {
        self.pt.is_some()
            && self.device.limits().max_sampled_textures_per_shader_stage as usize >= 4 * self.sets.len().max(1) + 1
    }

    /// Samples accumulated by the path tracer in the picture shown (None in the real-time mode).
    pub fn path_trace_samples(&self) -> Option<u32> {
        if self.path_trace && self.path_trace_supported() {
            self.pt.as_ref().map(|p| p.samples())
        } else {
            None
        }
    }

    /// Whether the path-traced picture still changes without input (samples to add, the neural denoiser at work).
    pub fn path_trace_busy(&self) -> bool {
        self.path_trace_samples().is_some() && self.pt.as_ref().is_some_and(|p| p.busy())
    }

    /// The neural denoiser is reading back or denoising.
    pub fn path_trace_ai_pending(&self) -> bool {
        self.path_trace_samples().is_some() && self.pt.as_ref().is_some_and(|p| p.ai_pending())
    }

    /// Samples of the neural denoiser's picture on screen (None: the fast filter's, or real time).
    pub fn path_trace_ai_samples(&self) -> Option<u32> {
        self.path_trace_samples()?;
        self.pt.as_ref()?.ai_samples()
    }

    /// GPU time of the last measured frame, ms (with `measure_gpu` on and timestamp queries available).
    pub fn gpu_ms(&self) -> Option<f32> {
        self.timer.as_ref().and_then(|t| t.last_ms)
    }

    /// Samples per pixel of the shaded view (1 = no antialiasing).
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// The sample counts this GPU can use for the shaded view.
    pub fn supported_samples(&self) -> &[u32] {
        &self.supported_samples
    }

    /// Antialiasing of the shaded view: `want` samples per pixel (the closest supported count below it). The scene
    /// pipelines and the multisampled targets are rebuilt.
    pub fn set_samples(&mut self, want: u32) {
        let n = pick_samples(&self.supported_samples, want);
        if n == self.samples {
            return;
        }
        let (scene, background) = scene_pipelines(&self.device, &self.layout, &self.shader, HDR_FORMAT, n);
        self.pipeline = scene;
        self.bg_pipeline = background;
        self.samples = n;
        self.msaa = None;
        self.depth = None;
    }

    /// Multisampled colour target of `size` (created on demand).
    fn ensure_msaa(&mut self, size: (u32, u32)) {
        if self.samples < 2 || self.msaa.as_ref().is_some_and(|m| m.1 == size) {
            return;
        }
        let t = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa"),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: self.samples,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.msaa = Some((t.create_view(&wgpu::TextureViewDescriptor::default()), size));
    }

    fn depth_view(&mut self, size: (u32, u32)) -> &wgpu::TextureView {
        if self.depth.as_ref().map(|d| d.1 != size).unwrap_or(true) {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: self.samples,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((t.create_view(&wgpu::TextureViewDescriptor::default()), size));
        }
        &self.depth.as_ref().unwrap().0
    }

    /// `viewport` = optional pixel rect `[x, y, w, h]` the 3D scene is drawn into.
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        size: (u32, u32),
        viewport: Option<[f32; 4]>,
        view_proj: glam::Mat4,
        eye: glam::Vec3,
    ) {
        self.queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniforms {
                view_proj: view_proj.to_cols_array_2d(),
                inv_vp: view_proj.inverse().to_cols_array_2d(),
                eye: [eye.x, eye.y, eye.z, 1.0],
                light: [0.45, 0.8, 0.55, self.ao_intensity],
                sh: self.sh,
                params: [self.exposure, self.env_max_lod, if self.view.is_some() { 1.0 } else { 0.0 }, self.env_yaw],
                // the baked normal map is read with +Y up (verified by the user); SHARD_NORMAL_DIRECTX flips it
                params2: [
                    if std::env::var_os("SHARD_NORMAL_DIRECTX").is_some() { -1.0 } else { 1.0 },
                    if self.face_normals && std::env::var_os("SHARD_VERTEX_NORMALS").is_none()
                        || std::env::var_os("SHARD_FACE_NORMALS").is_some()
                    {
                        1.0
                    } else {
                        0.0
                    },
                    0.0,
                    0.0,
                ],
                params3: [f32::from_bits(self.loading_mask), self.started.elapsed().as_secs_f32() % 1000.0, self.emissive_intensity, 0.0],
                params4: [self.loading_fill[0], self.loading_fill[1], self.loading_fill[2], 1.0],
                post: self.post.uniforms(
                    viewport.unwrap_or([0.0, 0.0, size.0 as f32, size.1 as f32]),
                    self.started.elapsed().as_secs_f32() % 1000.0,
                ),
            }),
        );
        let _ = self.depth_view(size);
        self.ensure_msaa(size);
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(Targets::new(&self.device, &self.post_pipes, &self.uniform_buf, size));
        }
        let targets = self.targets.as_ref().unwrap();
        let depth = &self.depth.as_ref().unwrap().0;
        // the scene goes to the HDR target (through the multisampled one when there is MSAA)
        let (color_view, resolve) = match &self.msaa {
            Some((m, _)) => (m, Some(&targets.hdr)),
            None => (&targets.hdr, None),
        };
        let clip = viewport.map(|[x, y, w, h]| {
            let x = x.clamp(0.0, size.0 as f32 - 1.0);
            let y = y.clamp(0.0, size.1 as f32 - 1.0);
            (x, y, w.clamp(1.0, size.0 as f32 - x), h.clamp(1.0, size.1 as f32 - y))
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("enc") });
        let traced = self.path_trace && self.view.is_none() && self.path_trace_supported();
        // GPU timing: a timestamp before the first pass and one after the post pass
        if let Some(t) = &mut self.timer {
            self.device.poll(wgpu::Maintain::Poll);
            t.collect();
        }
        let timing = self.measure_gpu && self.timer.as_ref().is_some_and(|t| t.idle());
        let ts_set = if timing { self.timer.as_ref().map(|t| &t.set) } else { None };
        let mut begun = false;
        if traced {
            let views: Vec<[wgpu::TextureView; 4]> = self
                .sets
                .iter()
                .map(|s| {
                    let v = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
                    [v(&s.texture), v(&s.aux), v(&s.nrm), v(&s.emi)]
                })
                .collect();
            if let (Some(pt), Some(env)) = (self.pt.as_mut(), self.env_view.as_ref()) {
                pt.set_ai_denoise(self.ai_denoise);
                let input = crate::pathtrace::FrameInput {
                    view_proj,
                    eye,
                    size,
                    env_yaw: self.env_yaw,
                    emissive_intensity: self.emissive_intensity,
                    show_background: self.show_background,
                    isolate: self.isolate,
                    sets: views.iter().map(|v| [&v[0], &v[1], &v[2], &v[3]]).collect(),
                    env,
                    out: &targets.hdr,
                    out_tex: &targets.hdr_tex,
                    scene_gen: self.scene_gen + SCENE_GEN.load(std::sync::atomic::Ordering::Relaxed),
                };
                begun = pt.frame(&self.device, &self.queue, &mut enc, &input, ts_set.map(|q| (q, 0))) && ts_set.is_some();
            }
        }
        if !traced {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        // nothing drawn = coverage 0: the post pass puts the viewport background there
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
                        store: if resolve.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts_set.map(|q| wgpu::RenderPassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: None,
                }),
                occlusion_query_set: None,
            });
            begun = ts_set.is_some();
            if let Some((x, y, w, h)) = clip {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
            }
            if self.show_background {
                if let (Some(env), Some(first)) = (&self.env_bind_group, self.sets.first()) {
                    pass.set_pipeline(&self.bg_pipeline);
                    pass.set_bind_group(0, &first.bind_group, &[]);
                    pass.set_bind_group(1, env, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            pass.set_pipeline(&self.pipeline);
            if let Some(g) = &self.env_bind_group {
                pass.set_bind_group(1, g, &[]);
            }
            pass.set_vertex_buffer(0, self.vbuf.slice(..));
            pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            let drawn = self.view.as_ref().unwrap_or(&self.sets);
            for (i, set) in drawn.iter().enumerate() {
                let Some(&(first, count)) = self.ranges.get(i) else { continue };
                if count == 0 || self.isolate.is_some_and(|only| only != i) {
                    continue;
                }
                pass.set_bind_group(0, &set.bind_group, &[]);
                // the instance index carries the set (its loading stripes)
                pass.draw_indexed(first..first + count, 0, i as u32..i as u32 + 1);
            }
        }
        // bloom: bright pass and blur chain over the HDR scene
        if self.post.enabled && self.post.bloom && self.view.is_none() {
            for (bind, level, additive) in &targets.passes {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("bloom"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &targets.levels[*level].0,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if *additive { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(if *additive { &self.post_pipes.bloom_up } else { &self.post_pipes.bloom_down });
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        // post effects onto the frame
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0176, g: 0.0176, b: 0.0176, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: ts_set.map(|q| wgpu::RenderPassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: (!begun).then_some(0),
                    end_of_pass_write_index: Some(1),
                }),
                occlusion_query_set: None,
            });
            if let Some((x, y, w, h)) = clip {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
            }
            pass.set_pipeline(&self.post_pipes.post);
            pass.set_bind_group(0, &targets.post_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // the outline of the sets still loading
        if self.loading_mask != 0 && std::env::var_os("SHARD_NO_LOADING_OUTLINE").is_none() {
            let draws: Vec<(usize, u32, u32)> = (0..self.ranges.len())
                .filter(|i| self.isolate.is_none_or(|only| only == *i))
                .filter_map(|i| self.ranges.get(i).map(|&(f, c)| (i, f, c)))
                .filter(|d| d.2 > 0)
                .collect();
            let mut ol = self.outline.take().unwrap_or_else(|| crate::outline::Outline::new(&self.device, self.format, &self.uniform_buf));
            let c = self.loading_outline;
            ol.draw(&self.device, &self.queue, &mut enc, target, size, viewport, &self.vbuf, &self.ibuf, &draws, self.loading_mask, [c[0], c[1], c[2], 1.0], 2.0);
            self.outline = Some(ol);
        }
        if let (true, Some(t)) = (timing, &self.timer) {
            enc.resolve_query_set(&t.set, 0..2, &t.resolve, 0);
            enc.copy_buffer_to_buffer(&t.resolve, 0, &t.read, 0, 16);
        }
        self.queue.submit(Some(enc.finish()));
        if let (true, Some(t)) = (timing, &self.timer) {
            t.state.store(1, std::sync::atomic::Ordering::Release);
            let state = t.state.clone();
            t.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                state.store(if r.is_ok() { 2 } else { 0 }, std::sync::atomic::Ordering::Release);
            });
        }
    }

    /// Create an offscreen colour target of the renderer's format.
    pub fn offscreen_target(&self, size: (u32, u32)) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Read back a rendered offscreen texture as tightly packed RGBA8.
    pub fn read_texture(&self, tex: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        let bpr = (4 * size.0 + 255) / 256 * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * size.1) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("copy") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(size.1) },
            },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((size.0 * size.1 * 4) as usize);
        for y in 0..size.1 {
            let s = (y * bpr) as usize;
            out.extend_from_slice(&data[s..s + (size.0 * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        out
    }
}
