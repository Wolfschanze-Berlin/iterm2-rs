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
    @location(1) color: vec4<f32>,     // rgba (sRGB values)
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen_size: vec2<f32>;

// Convert a single sRGB component to linear space.
// When rendering to an sRGB surface, the GPU converts linear -> sRGB on output,
// so we must provide linear values to get correct final colors.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

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
    // Convert sRGB input to linear for correct rendering on sRGB surfaces.
    output.color = vec4<f32>(
        srgb_to_linear(instance.color.r),
        srgb_to_linear(instance.color.g),
        srgb_to_linear(instance.color.b),
        instance.color.a,
    );
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
    pub opacity: f32,
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
    /// Second text renderer for the tab bar overlay.
    tab_bar_text: TextRenderer,
    /// Height of the tab bar in pixels (0 when hidden).
    tab_bar_height: f32,
    /// Pre-computed quad instances for styled tab bar backgrounds.
    tab_bar_quads: Vec<QuadInstanceRaw>,
    /// Cached tab layout info for mouse hit-testing: (x, width, tab_index).
    tab_layouts: Vec<(f32, f32, usize)>,
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
        log::debug!("Surface format: {:?} (sRGB={})", surface_format, surface_format.is_srgb());

        // Prefer pre-multiplied alpha for window transparency support.
        let alpha_mode = if renderer_config.opacity < 1.0 {
            surface_caps
                .alpha_modes
                .iter()
                .copied()
                .find(|m| *m == wgpu::CompositeAlphaMode::PreMultiplied)
                .unwrap_or(surface_caps.alpha_modes[0])
        } else {
            surface_caps.alpha_modes[0]
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
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
            renderer_config.font_family.clone(),
        );
        text.set_text("iterm2-rs", size.width as f32, size.height as f32);

        let tab_bar_text = TextRenderer::new(
            &device,
            &queue,
            surface_format,
            renderer_config.font_size * 0.85,
            renderer_config.fg_color,
            renderer_config.font_family.clone(),
        );

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
            opacity: renderer_config.opacity,
            quad_pipeline,
            quad_bind_group_layout,
            screen_size_buffer,
            screen_size_bind_group,
            quad_instance_buffer: None,
            quad_instance_buffer_capacity: 0,
            quad_instance_count: 0,
            cursor_info: None,
            tab_bar_text,
            tab_bar_height: 0.0,
            tab_bar_quads: Vec::new(),
            tab_layouts: Vec::new(),
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
    pub fn set_backgrounds(
        &mut self,
        rects: &[BgRect],
        char_width: f32,
        line_height: f32,
        y_offset: f32,
    ) {
        let mut instances: Vec<QuadInstanceRaw> = Vec::new();

        // Styled tab bar quads (bar background + individual tab shapes + separators).
        instances.extend_from_slice(&self.tab_bar_quads);

        // Terminal background rects offset by y_offset (tab bar height).
        instances.extend(rects.iter().map(|rect| {
            let x = TEXT_PADDING + rect.col as f32 * char_width;
            let y = y_offset + TEXT_PADDING + rect.line as f32 * line_height;
            let w = char_width * rect.width as f32;
            let h = line_height;
            let (r, g, b) = rect.color;
            QuadInstanceRaw {
                pos_size: [x, y, w, h],
                color: [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
            }
        }));

        // Append cursor as an additional quad instance.
        if let Some(cursor) = &self.cursor_info {
            if cursor.shape != CursorShape::Hidden {
                let x = TEXT_PADDING + cursor.col as f32 * char_width;
                let y = y_offset + TEXT_PADDING + cursor.line as f32 * line_height;

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

    /// Get the current tab bar height (0 when hidden).
    pub fn tab_bar_height(&self) -> f32 {
        self.tab_bar_height
    }

    /// Update the tab bar content with Chrome-style styled tabs.
    /// Each tab gets its own background quad and centered text label.
    /// Hidden when only one tab is open.
    /// Hit-test a pixel coordinate against the tab bar. Returns the tab index
    /// (into the `tab_titles()` slice) if the click lands on a tab.
    pub fn tab_bar_hit_test(&self, x: f32, y: f32) -> Option<usize> {
        if y > self.tab_bar_height || self.tab_layouts.is_empty() {
            return None;
        }
        for &(tab_x, tab_w, tab_index) in &self.tab_layouts {
            if x >= tab_x && x < tab_x + tab_w {
                return Some(tab_index);
            }
        }
        None
    }

    pub fn set_tab_bar(&mut self, tabs: &[(usize, &str, bool)]) {
        if tabs.len() <= 1 {
            self.tab_bar_height = 0.0;
            self.tab_bar_quads.clear();
            self.tab_layouts.clear();
            return;
        }

        // Catppuccin Mocha colors for the tab bar.
        // Tab bar strip background: Crust (#11111b)
        const BAR_BG: [f32; 4] = [0.067, 0.067, 0.106, 1.0];
        // Active tab: Surface0 (#313244)
        const TAB_ACTIVE: [f32; 4] = [0.192, 0.196, 0.267, 1.0];
        // Inactive tab: transparent (no quad, just bar background shows through)
        // Inactive tab hover would be Surface0 at 50% alpha, but we skip hover for now.
        // Separator between inactive tabs: Overlay0 (#6c7086)
        const SEPARATOR: [f32; 4] = [0.424, 0.439, 0.525, 0.5];

        self.tab_bar_height = 36.0;

        let tab_h = 28.0_f32;
        let tab_top = (self.tab_bar_height - tab_h) / 2.0; // vertically center tabs
        let tab_padding = 12.0_f32; // horizontal padding inside each tab
        let tab_gap = 1.0_f32; // gap between tabs
        let bar_left_pad = 8.0_f32; // padding before first tab

        // Measure approximate tab widths using char_width from tab_bar_text.
        // Each tab gets: padding + title_width + padding
        let approx_char_w = self.tab_bar_text.font_size() * 0.55;

        let mut quads: Vec<QuadInstanceRaw> = Vec::new();

        // Full-width tab bar background strip.
        quads.push(QuadInstanceRaw {
            pos_size: [0.0, 0.0, self.size.width as f32, self.tab_bar_height],
            color: BAR_BG,
        });

        // Compute tab positions.
        struct TabLayout {
            x: f32,
            width: f32,
            active: bool,
        }
        let mut layouts: Vec<TabLayout> = Vec::new();
        let mut x = bar_left_pad;

        for (_id, title, active) in tabs {
            let title_w = title.len() as f32 * approx_char_w;
            let tab_w = (tab_padding * 2.0 + title_w).max(60.0); // min tab width
            layouts.push(TabLayout {
                x,
                width: tab_w,
                active: *active,
            });
            x += tab_w + tab_gap;
        }

        // Draw tab backgrounds (only active tab gets a distinct background).
        for layout in &layouts {
            if layout.active {
                quads.push(QuadInstanceRaw {
                    pos_size: [layout.x, tab_top, layout.width, tab_h],
                    color: TAB_ACTIVE,
                });
            }
        }

        // Draw separators between inactive tabs.
        for i in 0..layouts.len().saturating_sub(1) {
            // Skip separator if either adjacent tab is active.
            if layouts[i].active || layouts[i + 1].active {
                continue;
            }
            let sep_x = layouts[i].x + layouts[i].width + (tab_gap / 2.0) - 0.5;
            quads.push(QuadInstanceRaw {
                pos_size: [sep_x, tab_top + 6.0, 1.0, tab_h - 12.0],
                color: SEPARATOR,
            });
        }

        self.tab_bar_quads = quads;

        // Store tab layouts for mouse hit-testing.
        self.tab_layouts = layouts
            .iter()
            .enumerate()
            .map(|(i, l)| (l.x, l.width, i))
            .collect();

        // Build styled text with per-tab coloring.
        // Active tab text: Text (#cdd6f4), Inactive: Subtext0 (#a6adc8)
        use glyphon::Color;
        let active_color = Color::rgb(205, 214, 244); // Text
        let inactive_color = Color::rgb(166, 173, 200); // Subtext0

        // Build spans positioned to align with tab backgrounds.
        // We use spaces to position each tab label at the right x offset.
        // This is a workaround since glyphon doesn't support absolute x positioning per span.
        // Instead, we'll build a single line with spaces padding each tab.

        // Calculate how many space characters correspond to each position.
        let mut spans: Vec<(String, Color)> = Vec::new();
        let mut text_x = 0.0_f32;

        for (i, (_id, title, active)) in tabs.iter().enumerate() {
            let layout = &layouts[i];
            // How many space chars to get from current text_x to the center-start of this tab label
            let label_w = title.len() as f32 * approx_char_w;
            let label_start = layout.x + (layout.width - label_w) / 2.0;
            let spaces_needed = ((label_start - text_x) / approx_char_w).max(0.0) as usize;

            if spaces_needed > 0 {
                spans.push((" ".repeat(spaces_needed), inactive_color));
                text_x += spaces_needed as f32 * approx_char_w;
            }

            let color = if *active { active_color } else { inactive_color };
            spans.push(((*title).to_string(), color));
            text_x += title.len() as f32 * approx_char_w;
        }

        self.tab_bar_text.set_colored_spans(
            &spans,
            self.size.width as f32,
            self.tab_bar_height,
        );
    }

    /// Reconfigure the surface after a window resize.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.text.resize(&self.queue, new_size.width, new_size.height);
            self.tab_bar_text.resize(&self.queue, new_size.width, new_size.height);

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

    /// Render a single frame: clear → background quads + cursor → text → tab bar.
    pub fn render(&mut self) -> std::result::Result<(), RenderError> {
        self.text
            .prepare(&self.device, &self.queue, self.size.width, self.size.height, self.tab_bar_height)
            .map_err(RenderError::Other)?;

        if self.tab_bar_height > 0.0 {
            // Vertically center tab labels within the tab bar.
            // Tab shapes are 28px tall starting at y=4. Text has 4px internal padding.
            // Nudge text down to visually center within the tab shape.
            let tab_text_y_offset = (self.tab_bar_height - self.tab_bar_text.line_height()) / 2.0
                - TEXT_PADDING;
            self.tab_bar_text
                .prepare(
                    &self.device,
                    &self.queue,
                    self.size.width,
                    self.size.height,
                    tab_text_y_offset.max(0.0),
                )
                .map_err(RenderError::Other)?;
        }

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
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            a: self.opacity as f64,
                            ..self.bg_color.to_wgpu_color()
                        }),
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

            // Draw tab bar text overlay.
            if self.tab_bar_height > 0.0 {
                self.tab_bar_text
                    .render(&mut render_pass)
                    .map_err(RenderError::Other)?;
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
