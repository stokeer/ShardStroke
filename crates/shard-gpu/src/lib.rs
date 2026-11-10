//! GPU compositing (B-014): the layer stack's accumulator lives in GPU memory and every layer is blended
//! onto it by a compute shader, so an edit no longer pays a CPU pass per texel per layer. The math is the
//! CPU compositor's (`shard_compose::composite_px`), checked texel by texel in the tests below.
//!
//! First step: the leaf blend (constant or map content, constant or map mask, optional coverage of an
//! isolated group). Content and masks are uploaded once and kept by key, so an edit re-sends only what changed.

use std::collections::HashMap;
use std::sync::Arc;

use shard_model::BlendMode;
use wgpu::util::DeviceExt;

/// Mode names in the order of the shader's ids (index 0..=2 compose as Normal).
const MODES: [&str; 23] = [
    "Normal", "Replace", "Passthrough", "Multiply", "Divide", "InverseDivide", "Darken", "Lighten", "LinearDodge",
    "Subtract", "InverseSubtract", "Difference", "Exclusion", "Screen", "Overlay", "HardLight", "SoftLight",
    "ColorBurn", "ColorDodge", "LinearBurn", "LinearLight", "VividLight", "PinLight",
];

/// The shader id of a mode, `None` when the GPU path does not have it (the caller composes on the CPU).
pub fn mode_id(mode: BlendMode) -> Option<u32> {
    MODES.iter().position(|m| *m == mode.name()).map(|i| i as u32)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    npx: u32,
    n: u32,
    mode: u32,
    flags: u32,
    op: f32,
    mask_c: f32,
    _pad: [f32; 2],
    const_s: [f32; 4],
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    dummy: wgpu::Buffer,
    dummy_rw: wgpu::Buffer,
}

/// What a layer writes: one value for every texel, or an uploaded map.
pub enum Src<'a> {
    Const(&'a [f32]),
    Map(&'a wgpu::Buffer),
}

/// A per-texel weight: constant or an uploaded map.
pub enum Weight<'a> {
    Const(f32),
    Map(&'a wgpu::Buffer),
}

/// A picture resident on the GPU: `n` interleaved components per texel.
pub struct GpuImage {
    pub w: u32,
    pub h: u32,
    pub n: usize,
    pub buf: wgpu::Buffer,
}

impl Gpu {
    /// A headless device, `None` when there is no usable adapter.
    pub fn new() -> Option<Arc<Gpu>> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))?;
        let limits = wgpu::Limits { max_storage_buffer_binding_size: adapter.limits().max_storage_buffer_binding_size, max_buffer_size: adapter.limits().max_buffer_size, ..Default::default() };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor { label: Some("shard-gpu"), required_limits: limits, ..Default::default() },
            None,
        ))
        .ok()?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blend"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blend.wgsl").into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let storage = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, storage(false)),
                entry(2, storage(false)),
                entry(3, storage(true)),
                entry(4, storage(true)),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("blend"),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let dummy = device.create_buffer(&wgpu::BufferDescriptor { label: Some("dummy"), size: 16, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        let dummy_rw = device.create_buffer(&wgpu::BufferDescriptor { label: Some("dummy_rw"), size: 16, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        Some(Arc::new(Gpu { device, queue, pipeline, layout, dummy, dummy_rw }))
    }

    /// A float buffer on the GPU with these values.
    pub fn upload(&self, data: &[f32]) -> wgpu::Buffer {
        let pad;
        let data = if data.is_empty() {
            pad = [0f32; 4];
            &pad[..]
        } else {
            data
        };
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        })
    }

    pub fn upload_image(&self, img: &shard_compose::Image) -> GpuImage {
        GpuImage { w: img.w, h: img.h, n: img.n, buf: self.upload(&img.data) }
    }

    /// A copy of a resident picture (a group's own accumulator starts from the one below it).
    pub fn duplicate(&self, img: &GpuImage) -> GpuImage {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: img.buf.size(),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&img.buf, 0, &buf, 0, img.buf.size());
        self.queue.submit([enc.finish()]);
        GpuImage { w: img.w, h: img.h, n: img.n, buf }
    }

    /// Composite one layer onto `acc` (and its coverage, inside an isolated group). Returns false when the mode
    /// is not on the GPU path.
    pub fn blend(&self, acc: &GpuImage, cover: Option<&wgpu::Buffer>, src: Src, mask: Weight, mode: BlendMode, op: f32) -> bool {
        let Some(mode) = mode_id(mode) else { return false };
        let npx = acc.w * acc.h;
        let mut p = Params { npx, n: acc.n as u32, mode, flags: 0, op, mask_c: 1.0, _pad: [0.0; 2], const_s: [0.0; 4] };
        let content = match src {
            Src::Const(v) => {
                p.flags |= 1;
                for c in 0..4 {
                    p.const_s[c] = v[c.min(v.len().saturating_sub(1))];
                }
                &self.dummy
            }
            Src::Map(b) => b,
        };
        let mask = match mask {
            Weight::Const(m) => {
                p.flags |= 2;
                p.mask_c = m;
                &self.dummy
            }
            Weight::Map(b) => b,
        };
        if cover.is_some() {
            p.flags |= 4;
        }
        let ubuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&p), usage: wgpu::BufferUsages::UNIFORM });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: acc.buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: cover.unwrap_or(&self.dummy_rw).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: content.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: mask.as_entire_binding() },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let groups = npx.div_ceil(256);
            pass.dispatch_workgroups(groups.min(65535), groups.div_ceil(65535), 1);
        }
        self.queue.submit([enc.finish()]);
        true
    }

    /// Read a buffer back (the finished channel goes to the viewport textures and the export).
    pub fn download(&self, buf: &wgpu::Buffer) -> Vec<f32> {
        let size = buf.size();
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        self.queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::Maintain::Wait);
        let out = bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range()).to_vec();
        staging.unmap();
        out
    }

    pub fn download_image(&self, img: &GpuImage) -> shard_compose::Image {
        let mut data = self.download(&img.buf);
        data.truncate((img.w * img.h) as usize * img.n);
        shard_compose::Image { w: img.w, h: img.h, n: img.n, data }
    }
}

/// Uploaded pictures kept by a caller key (content and masks of unchanged layers stay on the GPU between
/// edits), within a byte budget, least recently used out first.
pub struct Resident {
    map: HashMap<u64, (Arc<wgpu::Buffer>, u64)>,
    bytes: u64,
    budget: u64,
    tick: u64,
}

impl Resident {
    pub fn new(budget_mb: u64) -> Self {
        Resident { map: HashMap::new(), bytes: 0, budget: budget_mb << 20, tick: 0 }
    }

    pub fn get_or_upload(&mut self, gpu: &Gpu, key: u64, data: impl FnOnce() -> Vec<f32>) -> Arc<wgpu::Buffer> {
        self.tick += 1;
        if let Some((b, t)) = self.map.get_mut(&key) {
            *t = self.tick;
            return b.clone();
        }
        let b = Arc::new(gpu.upload(&data()));
        self.bytes += b.size();
        while self.bytes > self.budget && self.map.len() > 1 {
            let Some((&k, _)) = self.map.iter().min_by_key(|(_, (_, t))| *t) else { break };
            if let Some((old, _)) = self.map.remove(&k) {
                self.bytes -= old.size();
            }
        }
        self.map.insert(key, (b.clone(), self.tick));
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Every GPU mode against the CPU compositor, opaque and partly covered, map and constant inputs.
    #[test]
    fn blend_matches_cpu() {
        let Some(gpu) = Gpu::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let (w, h, n) = (64u32, 32u32, 3usize);
        let npx = (w * h) as usize;
        let mut seed = 0x9E37_79B9;
        for name in MODES {
            let mode = (0..64u8).map(BlendMode).find(|m| m.name() == name).expect(name);
            for covered in [false, true] {
                for const_src in [false, true] {
                    let acc: Vec<f32> = (0..npx * n).map(|_| rng(&mut seed)).collect();
                    let src: Vec<f32> = (0..npx * n).map(|_| rng(&mut seed)).collect();
                    let mask: Vec<f32> = (0..npx).map(|_| rng(&mut seed) * 1.2 - 0.1).collect();
                    let cover: Vec<f32> = (0..npx).map(|_| rng(&mut seed)).collect();
                    let k = [0.3f32, 0.6, 0.9];
                    let op = 0.8;
                    let g_acc = gpu.upload_image(&shard_compose::Image { w, h, n, data: acc.clone() });
                    let g_src = gpu.upload(&src);
                    let g_mask = gpu.upload(&mask);
                    let g_cov = gpu.upload(&cover);
                    let s = if const_src { Src::Const(&k) } else { Src::Map(&g_src) };
                    assert!(gpu.blend(&g_acc, covered.then_some(&g_cov), s, Weight::Map(&g_mask), mode, op));
                    let got = gpu.download_image(&g_acc).data;
                    let got_cov = gpu.download(&g_cov);
                    for i in 0..npx {
                        let a = (op * mask[i]).clamp(0.0, 1.0);
                        let pa = if covered { cover[i] } else { 1.0 };
                        let mut ar = pa;
                        for c in 0..n {
                            let sv = if const_src { k[c] } else { src[i * n + c] };
                            let mut want = acc[i * n + c];
                            if a > 0.0 {
                                let (d, na) = shard_compose::composite_px(mode, want, sv, a, pa);
                                want = d;
                                ar = na;
                            }
                            let g = got[i * n + c];
                            assert!((g - want).abs() <= 2e-6 + want.abs() * 2e-6, "{name} covered={covered} const={const_src} texel {i}.{c}: gpu {g} cpu {want}");
                        }
                        if covered {
                            let want = if a > 0.0 { ar } else { pa };
                            assert!((got_cov[i] - want).abs() <= 2e-6, "{name} coverage texel {i}: gpu {} cpu {want}", got_cov[i]);
                        }
                    }
                }
            }
        }
    }

    /// `cargo test --release -p shard-gpu bench -- --ignored --nocapture`: 30 resident layers at 1024, RGB.
    #[test]
    #[ignore]
    fn bench_stack() {
        let Some(gpu) = Gpu::new() else { return };
        let (w, h, n) = (1024u32, 1024u32, 3usize);
        let npx = (w * h) as usize;
        let mut seed = 7;
        let layers: Vec<(Vec<f32>, Vec<f32>)> =
            (0..30).map(|_| ((0..npx * n).map(|_| rng(&mut seed)).collect(), (0..npx).map(|_| rng(&mut seed)).collect())).collect();
        let g: Vec<(wgpu::Buffer, wgpu::Buffer)> = layers.iter().map(|(c, m)| (gpu.upload(c), gpu.upload(m))).collect();
        let acc0 = shard_compose::Image { w, h, n, data: vec![0.5; npx * n] };
        let acc = gpu.upload_image(&acc0);
        let modes = [BlendMode::NORMAL, BlendMode::MULTIPLY];
        for round in 0..3 {
            let t = std::time::Instant::now();
            for (k, (c, m)) in g.iter().enumerate() {
                gpu.blend(&acc, None, Src::Map(c), Weight::Map(m), modes[k % 2], 0.9);
            }
            let out = gpu.download_image(&acc);
            eprintln!("gpu round {round}: 30 layers + readback {:?} ({})", t.elapsed(), out.data[0]);
        }
        let t = std::time::Instant::now();
        let mut a = acc0.data.clone();
        use rayon_free::*;
        cpu_stack(&mut a, &layers, n, &modes);
        eprintln!("cpu single-thread 30 layers {:?}", t.elapsed());
    }

    mod rayon_free {
        use shard_model::BlendMode;
        pub fn cpu_stack(a: &mut [f32], layers: &[(Vec<f32>, Vec<f32>)], n: usize, modes: &[BlendMode]) {
            for (k, (c, m)) in layers.iter().enumerate() {
                for i in 0..m.len() {
                    let al = (0.9 * m[i]).clamp(0.0, 1.0);
                    for ch in 0..n {
                        a[i * n + ch] = shard_compose::composite_px(modes[k % 2], a[i * n + ch], c[i * n + ch], al, 1.0).0;
                    }
                }
            }
        }
    }
}

/// The compositor `shard-compose` drives: one resident store of content and masks shared by every pass.
pub struct Compositor {
    gpu: Arc<Gpu>,
    resident: std::sync::Mutex<Resident>,
}

impl Compositor {
    /// `SHARD_GPU_MB` (default 1024): budget of content and masks kept on the GPU between edits.
    pub fn new() -> Option<Compositor> {
        let mb = std::env::var("SHARD_GPU_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
        Some(Compositor { gpu: Gpu::new()?, resident: std::sync::Mutex::new(Resident::new(mb)) })
    }
}

struct Session<'a> {
    c: &'a Compositor,
    acc: Option<GpuImage>,
    cover: Option<wgpu::Buffer>,
}

impl shard_compose::GpuCompositor for Compositor {
    fn session(&self) -> Option<Box<dyn shard_compose::GpuSession + '_>> {
        Some(Box::new(Session { c: self, acc: None, cover: None }))
    }
}

impl shard_compose::GpuSession for Session<'_> {
    fn begin(&mut self, acc: &shard_compose::Image, cover: Option<&[f32]>) {
        self.acc = Some(self.c.gpu.upload_image(acc));
        self.cover = cover.map(|c| self.c.gpu.upload(c));
    }

    fn blend(
        &mut self,
        src: shard_compose::GpuSrc,
        src_key: Option<u64>,
        mask: shard_compose::GpuWeight,
        mask_key: Option<u64>,
        mode: BlendMode,
        op: f32,
    ) -> bool {
        let Some(acc) = self.acc.as_ref() else { return false };
        if mode_id(mode).is_none() {
            return false;
        }
        let gpu = &self.c.gpu;
        let trace = std::env::var_os("SHARD_TRACE_GPU").is_some();
        let mut keep = |key: Option<u64>, salt: u64, data: &[f32]| -> Arc<wgpu::Buffer> {
            let t = std::time::Instant::now();
            let hit = key.is_some_and(|k| self.c.resident.lock().unwrap().map.contains_key(&(k ^ salt)));
            let r = match key {
                Some(k) => self.c.resident.lock().unwrap().get_or_upload(gpu, k ^ salt, || data.to_vec()),
                None => Arc::new(gpu.upload(data)),
            };
            if trace {
                eprintln!("[gpu] {} {} {:.1} MB {:?}", if salt == 0x5EC0_17E7 { "content" } else { "mask" }, if key.is_none() { "unkeyed" } else if hit { "hit" } else { "miss" }, data.len() as f64 * 4.0 / 1048576.0, t.elapsed());
            }
            r
        };
        let src_buf;
        let s = match src {
            shard_compose::GpuSrc::Const(v) => Src::Const(v),
            shard_compose::GpuSrc::Map(img) => {
                if img.n != acc.n || img.w != acc.w || img.h != acc.h {
                    return false;
                }
                src_buf = keep(src_key, 0x5EC0_17E7, &img.data);
                Src::Map(&src_buf)
            }
        };
        let mask_buf;
        let m = match mask {
            shard_compose::GpuWeight::Const(c) => Weight::Const(c),
            shard_compose::GpuWeight::Map(v) => {
                if v.len() != (acc.w * acc.h) as usize {
                    return false;
                }
                mask_buf = keep(mask_key, 0x3A5C_0000, v);
                Weight::Map(&mask_buf)
            }
        };
        gpu.blend(acc, self.cover.as_ref(), s, m, mode, op)
    }

    fn end(&mut self, acc: &mut shard_compose::Image, cover: Option<&mut Vec<f32>>) {
        if let Some(g) = self.acc.take() {
            *acc = self.c.gpu.download_image(&g);
        }
        if let (Some(b), Some(c)) = (self.cover.take(), cover) {
            let mut v = self.c.gpu.download(&b);
            v.truncate(c.len());
            *c = v;
        }
    }
}
