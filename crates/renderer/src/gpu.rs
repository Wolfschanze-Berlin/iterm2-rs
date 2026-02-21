//! wgpu initialization: Instance, Adapter, Device, Queue, Surface.

use std::sync::Arc;

use anyhow::{Context, Result};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::text::TextRenderer;

/// Holds all wgpu state needed for rendering.
pub struct GpuState {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: PhysicalSize<u32>,
    pub text: TextRenderer,
}

impl GpuState {
    /// Create a new `GpuState` by initializing wgpu against the given window.
    pub async fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();

        // Create wgpu instance with default backends for the platform.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Create the rendering surface from the window.
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        // Request a high-performance adapter that can render to our surface.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to find a suitable GPU adapter")?;

        log::info!("Using GPU adapter: {:?}", adapter.get_info().name);

        // Request a device and queue from the adapter.
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

        // Pick a texture format supported by the surface.
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
            present_mode: wgpu::PresentMode::Fifo, // vsync
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut text = TextRenderer::new(&device, &queue, surface_format);
        text.set_text(
            "iterm2-rs",
            size.width as f32,
            size.height as f32,
        );

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            text,
        })
    }

    /// Reconfigure the surface after a window resize.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.text.resize(&self.queue, new_size.width, new_size.height);
            log::debug!("Surface resized to {}x{}", new_size.width, new_size.height);
        }
    }

    /// Render a single frame (solid dark background color #1e1e2e + text overlay).
    pub fn render(&mut self) -> Result<()> {
        // Prepare text for this frame.
        self.text
            .prepare(&self.device, &self.queue, self.size.width, self.size.height)?;

        let output = self
            .surface
            .get_current_texture()
            .context("failed to acquire next swap chain texture")?;

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            // Catppuccin Mocha base color: #1e1e2e
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1176, // 30/255
                            g: 0.1176, // 30/255
                            b: 0.1804, // 46/255
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.text.render(&mut render_pass)?;
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
