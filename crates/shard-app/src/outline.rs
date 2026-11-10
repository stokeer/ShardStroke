//! Outline of the texture sets still loading: every set's index is drawn into an id buffer, then a screen pass
//! draws the border of the sets marked as loading over the frame.

use crate::mesh::Vertex;

const ID_SHADER: &str = r#"
struct U { vp: mat4x4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct O { @builtin(position) p: vec4<f32>, @location(0) @interpolate(flat) id: u32 };
@vertex fn vs(@location(0) pos: vec3<f32>, @builtin(instance_index) inst: u32) -> O {
  var o: O;
  o.p = u.vp * vec4<f32>(pos, 1.0);
  o.id = inst + 1u;
  return o;
}
@fragment fn fs(i: O) -> @location(0) u32 { return i.id; }
"#;

const EDGE_SHADER: &str = r#"
struct E { color: vec4<f32>, params: vec4<f32> };
@group(0) @binding(0) var ids: texture_2d<u32>;
@group(0) @binding(1) var<uniform> e: E;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
  let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
  let y = f32(i & 2u) * 2.0 - 1.0;
  return vec4<f32>(x, y, 0.0, 1.0);
}
fn loading(id: u32) -> bool {
  if (id == 0u || id > 32u) { return false; }
  return ((bitcast<u32>(e.params.x) >> (id - 1u)) & 1u) == 1u;
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  let dim = vec2<i32>(textureDimensions(ids));
  let c = vec2<i32>(p.xy);
  let id0 = textureLoad(ids, c, 0).r;
  let r = i32(e.params.y);
  for (var dy = -r; dy <= r; dy++) {
    for (var dx = -r; dx <= r; dx++) {
      if (dx * dx + dy * dy > r * r) { continue; }
      let q = clamp(c + vec2<i32>(dx, dy), vec2<i32>(0), dim - vec2<i32>(1));
      let id = textureLoad(ids, q, 0).r;
      if (id != id0 && (loading(id) || loading(id0))) {
        return e.color;
      }
    }
  }
  discard;
}
"#;

pub struct Outline {
    id_pipeline: wgpu::RenderPipeline,
    id_bind: wgpu::BindGroup,
    edge_pipeline: wgpu::RenderPipeline,
    edge_bgl: wgpu::BindGroupLayout,
    edge_buf: wgpu::Buffer,
    targets: Option<((u32, u32), wgpu::TextureView, wgpu::TextureView, wgpu::BindGroup)>,
}

impl Outline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, uniform_buf: &wgpu::Buffer) -> Self {
        let id_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline ids"),
            source: wgpu::ShaderSource::Wgsl(ID_SHADER.into()),
        });
        let id_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline ids"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let id_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline ids"),
            layout: &id_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
        });
        let id_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("outline ids"),
            bind_group_layouts: &[&id_bgl],
            push_constant_ranges: &[],
        });
        let id_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline ids"),
            layout: Some(&id_layout),
            vertex: wgpu::VertexState {
                module: &id_mod,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &id_mod,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::R32Uint.into())],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let edge_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline edge"),
            source: wgpu::ShaderSource::Wgsl(EDGE_SHADER.into()),
        });
        let edge_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline edge"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let edge_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("outline edge"),
            bind_group_layouts: &[&edge_bgl],
            push_constant_ranges: &[],
        });
        let edge_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline edge"),
            layout: Some(&edge_layout),
            vertex: wgpu::VertexState {
                module: &edge_mod,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &edge_mod,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let edge_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("outline edge"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { id_pipeline, id_bind, edge_pipeline, edge_bgl, edge_buf, targets: None }
    }

    fn ensure(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.0 == size) {
            return;
        }
        let tex = |format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("outline target"),
                    size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let ids = tex(wgpu::TextureFormat::R32Uint, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let depth = tex(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline edge"),
            layout: &self.edge_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&ids) },
                wgpu::BindGroupEntry { binding: 1, resource: self.edge_buf.as_entire_binding() },
            ],
        });
        self.targets = Some((size, ids, depth, bind));
    }

    /// Draw the outline of the sets in `mask` over `target`. `draws` = (first index, count) per set; `color` is
    /// linear RGBA; `width` in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        viewport: Option<[f32; 4]>,
        vbuf: &wgpu::Buffer,
        ibuf: &wgpu::Buffer,
        draws: &[(usize, u32, u32)],
        mask: u32,
        color: [f32; 4],
        width: f32,
    ) {
        self.ensure(device, size);
        let (_, ids, depth, bind) = self.targets.as_ref().unwrap();
        let data: [f32; 8] = [color[0], color[1], color[2], color[3], f32::from_bits(mask), width, 0.0, 0.0];
        queue.write_buffer(&self.edge_buf, 0, bytemuck::cast_slice(&data));
        let set_view = |pass: &mut wgpu::RenderPass| {
            if let Some([x, y, w, h]) = viewport {
                let x = x.clamp(0.0, size.0 as f32 - 1.0);
                let y = y.clamp(0.0, size.1 as f32 - 1.0);
                let w = w.clamp(1.0, size.0 as f32 - x);
                let h = h.clamp(1.0, size.1 as f32 - y);
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
            }
        };
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ids,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            set_view(&mut pass);
            pass.set_pipeline(&self.id_pipeline);
            pass.set_bind_group(0, &self.id_bind, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for &(i, first, count) in draws {
                pass.draw_indexed(first..first + count, 0, i as u32..i as u32 + 1);
            }
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("outline edge"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        set_view(&mut pass);
        pass.set_pipeline(&self.edge_pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
