//! Renders a remote video feed into a `<canvas>` via Dioxus Native's custom
//! paint API: frames are uploaded to a GPU texture and drawn letterboxed into
//! a texture the size of the canvas, which Vello then composites.

use std::sync::Arc;

use dioxus_native::{CustomPaintCtx, CustomPaintSource, DeviceHandle, TextureHandle};
use sivryn_media::RgbaFrame;
use tokio::sync::watch;

const SHADER: &str = r#"
struct Rect { top_left: vec2<f32>, bottom_right: vec2<f32> };

@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(2) var<uniform> rect: Rect;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VertexOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[index];
    var out: VertexOut;
    out.position = vec4(mix(rect.top_left, rect.bottom_right, c), 0.0, 1.0);
    out.uv = c;
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(frame, frame_sampler, in.uv);
}
"#;

pub struct VideoSurface {
    frames: watch::Receiver<Option<Arc<RgbaFrame>>>,
    gpu: Option<Gpu>,
    logged_first_render: bool,
}

impl VideoSurface {
    pub fn new(frames: watch::Receiver<Option<Arc<RgbaFrame>>>) -> Self {
        Self { frames, gpu: None, logged_first_render: false }
    }
}

struct Output {
    texture: wgpu::Texture,
    handle: TextureHandle,
    width: u32,
    height: u32,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    rect: wgpu::Buffer,
    frame: Option<(wgpu::Texture, wgpu::BindGroup)>,
    output: Option<Output>,
    dirty: bool,
}

impl Gpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sivryn video"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sivryn video"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sivryn video"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sivryn video"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sivryn video"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let rect = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sivryn video rect"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            layout,
            sampler,
            rect,
            frame: None,
            output: None,
            dirty: true,
        }
    }

    fn upload(&mut self, frame: &RgbaFrame) {
        let size = wgpu::Extent3d { width: frame.width, height: frame.height, depth_or_array_layers: 1 };
        let needs_new = self.frame.as_ref().is_none_or(|(t, _)| t.width() != frame.width || t.height() != frame.height);
        if needs_new {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sivryn video frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sivryn video"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.rect.as_entire_binding() },
                ],
            });
            self.frame = Some((texture, bind_group));
        }
        let (texture, _) = self.frame.as_ref().expect("frame texture exists");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(frame.width * 4), rows_per_image: None },
            size,
        );
        self.dirty = true;
    }

    fn draw(&mut self) {
        let (Some((frame, bind_group)), Some(output)) = (&self.frame, &self.output) else { return };

        // Letterbox: fit the frame into the canvas, keeping its aspect ratio.
        let canvas_aspect = output.width as f32 / output.height as f32;
        let frame_aspect = frame.width() as f32 / frame.height() as f32;
        let (sx, sy) = if frame_aspect > canvas_aspect {
            (1.0, canvas_aspect / frame_aspect)
        } else {
            (frame_aspect / canvas_aspect, 1.0)
        };
        let rect: [f32; 4] = [-sx, sy, sx, -sy];
        let bytes: Vec<u8> = rect.iter().flat_map(|v| v.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.rect, 0, &bytes);

        let view = output.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sivryn video"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.dirty = false;
    }
}

impl CustomPaintSource for VideoSurface {
    fn resume(&mut self, device_handle: &DeviceHandle) {
        self.gpu = Some(Gpu::new(&device_handle.device, &device_handle.queue));
    }

    fn suspend(&mut self) {
        self.gpu = None;
    }

    fn render(&mut self, mut ctx: CustomPaintCtx<'_>, width: u32, height: u32, _scale: f64) -> Option<TextureHandle> {
        if !self.logged_first_render {
            tracing::debug!(width, height, gpu_ready = self.gpu.is_some(), "video surface first render");
            self.logged_first_render = true;
        }
        if width == 0 || height == 0 {
            return None;
        }
        let gpu = self.gpu.as_mut()?;

        if self.frames.has_changed().unwrap_or(false) {
            let latest = self.frames.borrow_and_update().clone();
            if let Some(frame) = latest {
                if gpu.frame.is_none() {
                    tracing::debug!(width = frame.width, height = frame.height, "first video frame uploaded to GPU");
                }
                gpu.upload(&frame);
            }
        }
        gpu.frame.as_ref()?;

        if gpu.output.as_ref().is_none_or(|o| o.width != width || o.height != height) {
            if let Some(old) = gpu.output.take() {
                ctx.unregister_texture(old.handle);
            }
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sivryn video output"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let handle = ctx.register_texture(texture.clone());
            gpu.output = Some(Output { texture, handle, width, height });
            gpu.dirty = true;
        }

        if gpu.dirty {
            gpu.draw();
        }
        gpu.output.as_ref().map(|o| o.handle.clone())
    }
}
