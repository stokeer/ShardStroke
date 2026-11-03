//! egui <-> wgpu glue.

use egui_wgpu::ScreenDescriptor;

pub struct Gui {
    pub ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
}

impl Gui {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
        Self { ctx, renderer }
    }

    /// Apply texture uploads of an egui frame that will not be painted (e.g. a layout warm-up pass).
    pub fn apply_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, out: &egui::FullOutput) {
        for (id, delta) in &out.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
    }

    /// Draw the tessellated output of one egui frame over `target` (the scene is already in it), or into a
    /// target cleared to `clear` (linear colour) first.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size: (u32, u32),
        clear: Option<wgpu::Color>,
        out: egui::FullOutput,
    ) {
        let ppp = out.pixels_per_point;
        let prims = self.ctx.tessellate(out.shapes, ppp);
        for (id, delta) in &out.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let screen = ScreenDescriptor { size_in_pixels: [size.0, size.1], pixels_per_point: ppp };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });
        self.renderer.update_buffers(device, queue, &mut enc, &prims, &screen);
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &prims, &screen);
        }
        queue.submit(Some(enc.finish()));
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
