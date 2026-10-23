//! Renders a remote video feed into a `<canvas>` via Dioxus Native's custom
//! paint API: frames are uploaded to a GPU texture and drawn letterboxed into
//! a texture the size of the canvas, which Vello then composites.

use std::sync::Arc;

use dioxus_native::{CustomPaintCtx, CustomPaintSource, DeviceHandle, TextureHandle};
use sivryn_media::RgbaFrame;
use tokio::sync::watch;

const SHADER: &str = r#"
// Where the picture goes in clip space, and which part of the frame it shows.
struct Rect { top_left: vec2<f32>, bottom_right: vec2<f32>, uv_min: vec2<f32>, uv_max: vec2<f32> };

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
    out.uv = mix(rect.uv_min, rect.uv_max, c);
    return out;
}

// Area scaling: every output pixel averages the source texels it covers,
// weighted by how much of each it covers. Shrinking stays sharp without
// shimmering, enlarging keeps hard texel edges with smoothed transitions.
const MAX_TAPS: i32 = 16;

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let size = textureDimensions(frame);
    let dims = vec2<f32>(size);
    let last = vec2<i32>(size) - vec2(1);
    let centre = in.uv * dims;
    // The output pixel's footprint in texels.
    let half = max(vec2(abs(dpdx(centre.x)), abs(dpdy(centre.y))), vec2(1e-3)) * 0.5;
    let lo = centre - half;
    let hi = centre + half;
    let first = vec2<i32>(floor(lo));
    let end = vec2<i32>(ceil(hi));
    // Very large reductions sample blocks instead of every texel.
    let step = max(vec2(1), (end - first + vec2(MAX_TAPS - 1)) / MAX_TAPS);

    var sum = vec4(0.0);
    var total = 0.0;
    for (var y = first.y; y < end.y; y += step.y) {
        let wy = min(hi.y, f32(y + step.y)) - max(lo.y, f32(y));
        for (var x = first.x; x < end.x; x += step.x) {
            let wx = min(hi.x, f32(x + step.x)) - max(lo.x, f32(x));
            let w = max(wx, 0.0) * max(wy, 0.0);
            sum += textureLoad(frame, clamp(vec2(x, y), vec2(0), last), 0) * w;
            total += w;
        }
    }
    if total <= 0.0 {
        return textureLoad(frame, clamp(vec2<i32>(centre), vec2(0), last), 0);
    }
    return sum / total;
}
"#;

/// The part of the frame to show: a zoom factor (1 = all of it) around a
/// centre given as fractions of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub centre: (f32, f32),
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, centre: (0.5, 0.5) }
    }
}

impl View {
    /// The shown part of the frame (fractions), kept inside the frame.
    pub fn bounds(self) -> ((f32, f32), (f32, f32)) {
        let half = 0.5 / self.zoom.max(1.0);
        let clamp = |c: f32| c.clamp(half, 1.0 - half);
        let (cx, cy) = (clamp(self.centre.0), clamp(self.centre.1));
        ((cx - half, cy - half), (cx + half, cy + half))
    }
}

/// A view the UI changes while the surface shows it (equal when shared).
#[derive(Clone, Default)]
pub struct SharedView(Arc<std::sync::Mutex<View>>);

impl SharedView {
    pub fn get(&self) -> View {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Changes the view; returns whether it changed.
    pub fn set(&self, view: View) -> bool {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *current != view;
        *current = view;
        changed
    }
}

impl PartialEq for SharedView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub struct VideoSurface {
    frames: watch::Receiver<Option<Arc<RgbaFrame>>>,
    /// Zoom and pan (screen shares); `None` shows the whole frame.
    view: Option<SharedView>,
    /// The view last drawn.
    drawn_view: View,
    gpu: Option<Gpu>,
    logged_first_render: bool,
    /// Fill the canvas and crop the overflow instead of letterboxing.
    cover: bool,
    /// Asks the window to repaint whenever a new frame arrives.
    repaint: tokio::task::JoinHandle<()>,
}

impl VideoSurface {
    pub fn new(frames: watch::Receiver<Option<Arc<RgbaFrame>>>) -> Self {
        // The window only repaints when something changed: a new frame is a change.
        let mut watched = frames.clone();
        let repaint = tokio::spawn(async move {
            blitz_shell::canvas_frames::request_frame();
            while watched.changed().await.is_ok() {
                blitz_shell::canvas_frames::request_frame();
            }
        });
        Self {
            frames,
            view: None,
            drawn_view: View::default(),
            gpu: None,
            logged_first_render: false,
            cover: false,
            repaint,
        }
    }

    /// Shows the part of the frame `view` says (zooming a screen share).
    pub fn with_view(mut self, view: SharedView) -> Self {
        self.view = Some(view);
        self
    }

    /// Fills the canvas, cropping what doesn't fit (banners: rounding in
    /// the layout must not leave thin bars at the edges).
    pub fn cover(mut self) -> Self {
        self.cover = true;
        self
    }
}

impl Drop for VideoSurface {
    fn drop(&mut self) {
        self.repaint.abort();
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
            size: 32,
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

    fn draw(&mut self, cover: bool, view: View) {
        let (Some((frame, bind_group)), Some(output)) = (&self.frame, &self.output) else { return };

        // Letterbox: fit the frame into the canvas, keeping its aspect ratio
        // (or cover it, and what lies outside the canvas is clipped).
        let canvas_aspect = output.width as f32 / output.height as f32;
        let frame_aspect = frame.width() as f32 / frame.height() as f32;
        let (sx, sy) = if cover {
            if frame_aspect > canvas_aspect {
                (frame_aspect / canvas_aspect, 1.0)
            } else {
                (1.0, canvas_aspect / frame_aspect)
            }
        } else if frame_aspect > canvas_aspect {
            (1.0, canvas_aspect / frame_aspect)
        } else {
            (frame_aspect / canvas_aspect, 1.0)
        };
        let ((u0, v0), (u1, v1)) = view.bounds();
        let rect: [f32; 8] = [-sx, sy, sx, -sy, u0, v0, u1, v1];
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
                        // Transparent, so letterboxing and round (alpha) images show the
                        // page behind them.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
        // The first render may have happened before the GPU side existed.
        blitz_shell::canvas_frames::request_frame();
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
        // Larger than the GPU allows: draw nothing rather than fail (and crash).
        let max = gpu.device.limits().max_texture_dimension_2d;
        if width > max || height > max {
            return None;
        }

        // A still image only has the channel's initial value, which
        // `has_changed` never reports; take it when nothing is shown yet.
        if gpu.frame.is_none() || self.frames.has_changed().unwrap_or(false) {
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

        let view = self.view.as_ref().map(SharedView::get).unwrap_or_default();
        if view != self.drawn_view {
            self.drawn_view = view;
            gpu.dirty = true;
        }
        if gpu.dirty {
            gpu.draw(self.cover, view);
        }
        gpu.output.as_ref().map(|o| o.handle.clone())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_is_valid() {
        let module = naga::front::wgsl::parse_str(super::SHADER).expect("shader parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("shader validates");
    }
}
