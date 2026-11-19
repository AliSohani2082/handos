//! The wgpu backend: every primitive is one instanced quad shaded by its distance to a segment,
//! so a frame is one draw call (ported from `spikes/overlay-gpu`).
#![allow(unsafe_code)]

use std::{ffi::c_void, ptr::NonNull};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};

use crate::{DisplayList, Prim};

/// Room for four hands of 21 bones and 21 joints, plus a few markers. Longer lists are cut.
pub const MAX_INST: usize = 4 * 42 + 16;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Inst {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub radius: f32,
    pub color: [f32; 4],
}

/// The GPU instances for `list`, in surface pixels (`scale` × logical), in list order.
pub fn instances(list: &DisplayList, scale: f32, out: &mut Vec<Inst>) {
    out.clear();
    let px = |p: [f32; 2]| [p[0] * scale, p[1] * scale];
    out.extend(list.0.iter().take(MAX_INST).map(|prim| match *prim {
        Prim::Line { a, b, width, rgba } => Inst {
            a: px(a),
            b: px(b),
            radius: width * scale / 2.0,
            color: rgba,
        },
        Prim::Dot { at, radius, rgba } => Inst {
            a: px(at),
            b: px(at),
            radius: radius * scale,
            color: rgba,
        },
    }));
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("cannot create a GPU surface: {0}")]
    Surface(String),
    #[error("no Vulkan adapter can present to this surface: {0}")]
    Adapter(String),
    #[error("cannot open the GPU device: {0}")]
    Device(String),
}

pub struct Gpu {
    // Field order is drop order: the surface goes before the device that owns it.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind: wgpu::BindGroup,
    insts: wgpu::Buffer,
    scale: f32,
    scratch: Vec<Inst>,
    adapter: String,
}

impl Gpu {
    /// # Safety
    ///
    /// `display` must be a live `wl_display*` and `surface` a live `wl_surface*` created on it,
    /// and both must outlive the returned `Gpu` (drop the `Gpu` first).
    pub unsafe fn new(
        display: NonNull<c_void>,
        surface: NonNull<c_void>,
    ) -> Result<Self, RenderError> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);
        // SAFETY: the caller guarantees both handles are live and outlive the surface.
        let wsurface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                    display,
                ))),
                raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
            })
        }
        .map_err(|e| RenderError::Surface(e.to_string()))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&wsurface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| RenderError::Adapter(e.to_string()))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("handos-overlay"),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| RenderError::Device(e.to_string()))?;

        let caps = wsurface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| RenderError::Surface("the surface offers no formats".into()))?;
        let alpha_mode = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m))
        .or_else(|| caps.alpha_modes.first().copied())
        .ok_or_else(|| RenderError::Surface("the surface offers no alpha modes".into()))?;
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: Default::default(),
            width: 1,
            height: 1,
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };

        let shader = device.create_shader_module(wgpu::include_wgsl!("skeleton.wgsl"));
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skeleton"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Inst>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let insts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (MAX_INST * size_of::<Inst>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            surface: wsurface,
            device,
            queue,
            config,
            pipeline,
            globals,
            bind,
            insts,
            scale: 1.0,
            scratch: Vec::with_capacity(MAX_INST),
            adapter: format!("{} ({:?})", info.name, info.backend),
        })
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Sets the surface size in buffer pixels and the logical → pixel scale for primitives.
    pub fn resize(&mut self, width_px: u32, height_px: u32, scale: f32) {
        self.config.width = width_px.max(1);
        self.config.height = height_px.max(1);
        self.scale = scale.max(1.0);
        self.surface.configure(&self.device, &self.config);
        let size = [
            self.config.width as f32,
            self.config.height as f32,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::cast_slice(&size));
    }

    /// Records, submits and presents one frame (an empty list clears the surface). Returns
    /// false when no texture was available, in which case nothing was committed.
    pub fn render(&mut self, list: &DisplayList) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        instances(list, self.scale, &mut self.scratch);
        if !self.scratch.is_empty() {
            self.queue
                .write_buffer(&self.insts, 0, bytemuck::cast_slice(&self.scratch));
        }
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if !self.scratch.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind, &[]);
                pass.set_vertex_buffer(0, self.insts.slice(..));
                pass.draw(0..4, 0..self.scratch.len() as u32);
            }
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Style, skeleton, skeleton::CONNECTIONS};

    fn hands(n: usize, at: impl Fn(usize) -> [f32; 2]) -> DisplayList {
        let points: [[f32; 2]; 21] = std::array::from_fn(at);
        (0..n)
            .flat_map(|_| skeleton(&points, Style::default()))
            .collect()
    }

    #[test]
    fn instances_cover_every_bone_then_every_joint() {
        let list = hands(2, |i| [i as f32 * 25.0, 125.0]);
        let mut out = Vec::new();
        instances(&list, 2.0, &mut out);
        assert_eq!(out.len(), 2 * (CONNECTIONS.len() + 21));
        let style = Style::default();
        for hand in out.chunks(CONNECTIONS.len() + 21) {
            let (lines, joints) = hand.split_at(CONNECTIONS.len());
            assert!(
                lines
                    .iter()
                    .all(|i| i.radius == style.line_width && i.color == style.line_rgba)
            );
            assert!(joints.iter().all(|i| i.a == i.b
                && i.radius == style.joint_radius * 2.0
                && i.color == style.joint_rgba));
        }
        assert_eq!(out[0].a, [0.0, 250.0]);
        assert_eq!(out[0].b, [50.0, 250.0]);
    }

    #[test]
    fn instances_are_capped_at_the_buffer_size() {
        let list = hands(9, |_| [5.0, 5.0]);
        let mut out = Vec::new();
        instances(&list, 1.0, &mut out);
        assert_eq!(out.len(), MAX_INST);
    }

    #[test]
    fn shader_has_both_entry_points() {
        let src = include_str!("skeleton.wgsl");
        assert!(src.contains("fn vs") && src.contains("fn fs"));
    }
}
