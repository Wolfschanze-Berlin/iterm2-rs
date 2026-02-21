//! Text rendering using glyphon (cosmic-text + wgpu).

use anyhow::Result;
use glyphon::{
    Buffer, Cache, Color, FontSystem, Metrics, Resolution, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer as GlyphonRenderer, Viewport,
};

/// Wraps glyphon's text rendering pipeline.
pub struct TextRenderer {
    pub font_system: FontSystem,
    swash_cache: SwashCache,
    #[allow(dead_code)]
    cache: Cache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: GlyphonRenderer,
    buffer: Buffer,
    font_size: f32,
    line_height: f32,
}

impl TextRenderer {
    /// Create a new text renderer for the given wgpu device/queue/format.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let atlas = TextAtlas::new(device, queue, &cache, surface_format);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = atlas;
        let renderer = GlyphonRenderer::new(
            &mut atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );

        let font_size = 14.0;
        let line_height = font_size * 1.4;
        let buffer = Buffer::new(&mut font_system, Metrics::new(font_size, line_height));

        Self {
            font_system,
            swash_cache,
            cache,
            atlas,
            viewport,
            renderer,
            buffer,
            font_size,
            line_height,
        }
    }

    /// Set the text content to display.
    pub fn set_text(&mut self, text: &str, width: f32, height: f32) {
        use glyphon::cosmic_text::{Attrs, Family, Shaping};

        let attrs = Attrs::new().family(Family::Monospace);
        self.buffer
            .set_text(&mut self.font_system, text, &attrs, Shaping::Advanced, None);
        self.buffer
            .set_size(&mut self.font_system, Some(width), Some(height));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Update viewport resolution (call on resize).
    pub fn resize(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        self.viewport
            .update(queue, Resolution { width, height });
    }

    /// Prepare text for rendering. Must be called each frame before render().
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let text_area = TextArea {
            buffer: &self.buffer,
            left: 4.0,
            top: 4.0,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            },
            // Catppuccin Mocha text color: #cdd6f4
            default_color: Color::rgb(205, 214, 244),
            custom_glyphs: &[],
        };

        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [text_area],
                &mut self.swash_cache,
            )
            .map_err(|e| anyhow::anyhow!("glyphon prepare error: {e:?}"))?;

        Ok(())
    }

    /// Render prepared text into the given render pass.
    pub fn render<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
    ) -> Result<()> {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .map_err(|e| anyhow::anyhow!("glyphon render error: {e:?}"))?;
        Ok(())
    }

    /// Get font size in pixels.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Get line height in pixels.
    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    /// Set text content from terminal grid lines.
    /// Each line is a string of characters for that terminal row.
    pub fn set_lines(&mut self, lines: &[String], width: f32, height: f32) {
        let text = lines.join("\n");
        self.set_text(&text, width, height);
    }
}
