//! wgpu initialization: Instance, Adapter, Device, Queue, Surface.
//!
//! Provides the GPU rendering pipeline including:
//! - Surface management and frame presentation
//! - Instanced quad pipeline for background rectangles and cursor
//! - Text rendering via glyphon

use std::sync::Arc;

use anyhow::{Context, Result};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::config::{RendererConfig, RgbColor};
use crate::terminal_renderer::{BgRect, CursorInfo, CursorShape};
use crate::text::TextRenderer;

/// Errors that can occur during a render pass.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The wgpu surface failed (lost, outdated, timeout, or out-of-memory).
    #[error("surface error: {0}")]
    Surface(#[from] wgpu::SurfaceError),
    /// Any other rendering failure (e.g. text preparation).
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// WGSL shader for rendering colored quads via instanced rendering.
///
/// Used for both background rectangles and the cursor. Each instance provides
/// position, size, and color. Vertex positions are generated from vertex_index.
const QUAD_SHADER: &str = r#"
struct QuadInstance {
    @location(0) pos_size: vec4<f32>,  // xy = top-left position, zw = width/height
    @location(1) color: vec4<f32>,     // rgba
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen_size: vec2<f32>;

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: QuadInstance,
) -> VertexOutput {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );

    let unit_pos = positions[vertex_index];
    let pixel_pos = instance.pos_size.xy + unit_pos * instance.pos_size.zw;

    let ndc = vec2<f32>(
        pixel_pos.x / screen_size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / screen_size.y * 2.0,
    );

    var output: VertexOutput;
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.color = instance.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

/// Per-instance data for a colored quad, matching the shader layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadInstanceRaw {
    /// xy = top-left pixel position, zw = width and height in pixels.
    pos_size: [f32; 4],
    /// RGBA color (0.0..1.0).
    color: [f32; 4],
}

/// Holds all wgpu state needed for rendering.
pub struct GpuState {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: PhysicalSize<u32>,
    pub text: TextRenderer,
    pub bg_color: RgbColor,
    // Quad pipeline state (shared by bg rects and cursor).
    quad_pipeline: wgpu::RenderPipeline,
    quad_bind_group_layout: wgpu::BindGroupLayout,
    screen_size_buffer: wgpu::Buffer,
    screen_size_bind_group: wgpu::BindGroup,
    /// Persistent instance buffer for quads. Reused across frames; only
    /// re-allocated when the required capacity grows.
    quad_instance_buffer: Option<wgpu::Buffer>,
    /// Allocated capacity of `quad_instance_buffer` in bytes.
    quad_instance_buffer_capacity: u64,
    quad_instance_count: u32,
    /// Current cursor info for rendering.
    cursor_info: Option<CursorInfo>,
}

/// Text padding used by the text renderer (must match `TextArea::left`/`top`).
const TEXT_PADDING: f32 = 4.0;

impl GpuState {
    /// Create a new `GpuState` by initializing wgpu against the given window.
    pub async fn new(window: Arc<Window>, renderer_config: &RendererConfig) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to find a suitable GPU adapter")?;

        log::info!("Using GPU adapter: {:?}", adapter.get_info().name);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("iterm2-rs device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                ..Default::default()
            })
            .await
            .context("failed to create wgpu device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut text = TextRenderer::new(
            &device,
            &queue,
            surface_format,
            renderer_config.font_size,
            renderer_config.fg_color,
        );
        text.set_text("iterm2-rs", size.width as f32, size.height as f32);

        // Create the instanced quad pipeline (shared by bg rects and cursor).
        let (quad_pipeline, quad_bind_group_layout) =
            Self::create_quad_pipeline(&device, surface_format);

        let screen_size_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Screen Size Uniform"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        queue.write_buffer(
            &screen_size_buffer,
            0,
            bytemuck::cast_slice(&[size.width.max(1) as f32, size.height.max(1) as f32]),
        );

        let screen_size_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Screen Size Bind Group"),
            layout: &quad_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_size_buffer.as_entire_binding(),
            }],
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            text,
            bg_color: renderer_config.bg_color,
            quad_pipeline,
            quad_bind_group_layout,
            screen_size_buffer,
            screen_size_bind_group,
            quad_instance_buffer: None,
            quad_instance_buffer_capacity: 0,
            quad_instance_count: 0,
            cursor_info: None,
        })
    }

    /// Create the render pipeline for instanced colored quads.
    fn create_quad_pipeline(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Quad Shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_SHADER.into()),
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Quad Bind Group Layout"),
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

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Quad Pipeline Layout"),
                bind_group_layouts: &[&bind_group_layout],
                immediate_size: 0,
            });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Quad Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        (pipeline, bind_group_layout)
    }

    /// Update the cursor info to be rendered on the next frame.
    pub fn set_cursor(&mut self, cursor: Option<CursorInfo>) {
        self.cursor_info = cursor;
    }

    /// Upload background rectangles and cursor for the next frame.
    ///
    /// Both bg rects and the cursor are rendered as instanced quads in a single
    /// draw call for efficiency.
    pub fn set_backgrounds(&mut self, rects: &[BgRect], char_width: f32, line_height: f32) {
        let mut instances: Vec<QuadInstanceRaw> = rects
            .iter()
            .map(|rect| {
                let x = TEXT_PADDING + rect.col as f32 * char_width;
                let y = TEXT_PADDING + rect.line as f32 * line_height;
                let w = char_width * rect.width as f32;
                let h = line_height;
                let (r, g, b) = rect.color;
                QuadInstanceRaw {
                    pos_size: [x, y, w, h],
                    color: [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
                }
            })
            .collect();

        // Append cursor as an additional quad instance.
        if let Some(cursor) = &self.cursor_info {
            if cursor.shape != CursorShape::Hidden {
                let x = TEXT_PADDING + cursor.col as f32 * char_width;
                let y = TEXT_PADDING + cursor.line as f32 * line_height;

                let (w, h, final_y) = match cursor.shape {
                    CursorShape::Block | CursorShape::HollowBlock => {
                        (char_width, line_height, y)
                    }
                    CursorShape::Underline => {
                        (char_width, 2.0, y + line_height - 2.0)
                    }
                    CursorShape::Beam => (2.0, line_height, y),
                    CursorShape::Hidden => unreachable!(),
                };

                let alpha = match cursor.shape {
                    CursorShape::Block => 0.7,
                    CursorShape::HollowBlock => 0.3,
                    _ => 1.0,
                };

                // Catppuccin Mocha text color for cursor: #cdd6f4
                instances.push(QuadInstanceRaw {
                    pos_size: [x, final_y, w, h],
                    color: [205.0 / 255.0, 214.0 / 255.0, 244.0 / 255.0, alpha],
                });
            }
        }

        if instances.is_empty() {
            self.quad_instance_count = 0;
            return;
        }

        let data = bytemuck::cast_slice(&instances);
        let required = data.len() as u64;

        // Reuse the existing buffer if it has enough capacity; otherwise allocate
        // a new one with some headroom to avoid frequent re-allocations.
        if required > self.quad_instance_buffer_capacity {
            let alloc_size = required.next_power_of_two().max(1024);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Quad Instance Buffer"),
                size: alloc_size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.quad_instance_buffer = Some(buffer);
            self.quad_instance_buffer_capacity = alloc_size;
        }

        if let Some(ref buffer) = self.quad_instance_buffer {
            self.queue.write_buffer(buffer, 0, data);
        }

        self.quad_instance_count = instances.len() as u32;
    }

    /// Reconfigure the surface after a window resize.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.text.resize(&self.queue, new_size.width, new_size.height);

            self.queue.write_buffer(
                &self.screen_size_buffer,
                0,
                bytemuck::cast_slice(&[new_size.width as f32, new_size.height as f32]),
            );

            self.screen_size_bind_group =
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Screen Size Bind Group"),
                    layout: &self.quad_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.screen_size_buffer.as_entire_binding(),
                    }],
                });

            log::debug!("Surface resized to {}x{}", new_size.width, new_size.height);
        }
    }

    /// Render a single frame: clear → background quads + cursor → text.
    pub fn render(&mut self) -> std::result::Result<(), RenderError> {
        self.text
            .prepare(&self.device, &self.queue, self.size.width, self.size.height)
            .map_err(RenderError::Other)?;

        let output = self.surface.get_current_texture()?;

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.bg_color.to_wgpu_color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Draw background quads and cursor (all in one instanced draw call).
            if self.quad_instance_count > 0 {
                if let Some(ref instance_buffer) = self.quad_instance_buffer {
                    render_pass.set_pipeline(&self.quad_pipeline);
                    render_pass.set_bind_group(0, &self.screen_size_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, instance_buffer.slice(..));
                    render_pass.draw(0..6, 0..self.quad_instance_count);
                }
            }

            // Draw text on top.
            self.text
                .render(&mut render_pass)
                .map_err(RenderError::Other)?;
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
