//! Text rendering using glyphon (cosmic-text + wgpu).

use anyhow::Result;
use glyphon::{
    Buffer, Cache, Color, FontSystem, Metrics, Resolution, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer as GlyphonRenderer, Viewport,
};

use crate::config::RgbColor;

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
    fg_color: RgbColor,
}

impl TextRenderer {
    /// Create a new text renderer for the given wgpu device/queue/format.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        font_size: f32,
        fg_color: RgbColor,
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
            fg_color,
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
        let (r, g, b) = self.fg_color.to_rgb_u8();
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
            default_color: Color::rgb(r, g, b),
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

    /// Approximate character width for monospace font.
    ///
    /// Uses the font system to measure 'M' width. Falls back to 0.6 * font_size.
    pub fn char_width(&mut self) -> f32 {
        use glyphon::cosmic_text::{Attrs, Family, Shaping};

        // Measure by setting a single character and reading the layout.
        let mut measure_buf = Buffer::new(
            &mut self.font_system,
            Metrics::new(self.font_size, self.line_height),
        );
        let attrs = Attrs::new().family(Family::Monospace);
        measure_buf.set_text(
            &mut self.font_system,
            "M",
            &attrs,
            Shaping::Advanced,
            None,
        );
        measure_buf.set_size(&mut self.font_system, Some(f32::MAX), Some(self.line_height));
        measure_buf.shape_until_scroll(&mut self.font_system, false);

        for line in measure_buf.layout_runs() {
            for glyph in line.glyphs.iter() {
                if glyph.w > 0.0 {
                    return glyph.w;
                }
            }
        }
        // Fallback
        self.font_size * 0.6
    }

    /// Set text content from terminal grid lines.
    /// Each line is a string of characters for that terminal row.
    pub fn set_lines(&mut self, lines: &[String], width: f32, height: f32) {
        let text = lines.join("\n");
        self.set_text(&text, width, height);
    }

    /// Set text content from styled terminal grid lines.
    ///
    /// Each row is a `Vec<StyledCell>` where every cell carries its own
    /// foreground color. Consecutive cells with the same color are merged into
    /// a single span to reduce the number of rich-text spans passed to
    /// cosmic-text.
    pub fn set_styled_lines(
        &mut self,
        rows: &[Vec<crate::terminal_renderer::StyledCell>],
        width: f32,
        height: f32,
    ) {
        use glyphon::cosmic_text::{Attrs, Family, Shaping, Style, Weight};

        // Build owned spans: Vec<(String, color, bold, italic)>.
        // We merge runs of identical color+attributes within each row, and
        // insert '\n' between rows.
        let mut spans: Vec<(String, Color, bool, bool)> = Vec::new();
        let (fr, fg, fb) = self.fg_color.to_rgb_u8();

        for (row_idx, row) in rows.iter().enumerate() {
            if row_idx > 0 {
                // Append newline with default fg color.
                spans.push(("\n".to_string(), Color::rgb(fr, fg, fb), false, false));
            }

            if row.is_empty() {
                continue;
            }

            // Trim trailing spaces for cleaner output.
            let trimmed_len = row
                .iter()
                .rposition(|cell| cell.c != ' ')
                .map(|i| i + 1)
                .unwrap_or(0);

            let row = &row[..trimmed_len];

            if row.is_empty() {
                continue;
            }

            let mut current_color = row[0].fg;
            let mut current_bold = row[0].bold;
            let mut current_italic = row[0].italic;
            let mut current_run = String::new();
            current_run.push(row[0].c);

            for cell in &row[1..] {
                if cell.fg == current_color
                    && cell.bold == current_bold
                    && cell.italic == current_italic
                {
                    current_run.push(cell.c);
                } else {
                    spans.push((
                        std::mem::take(&mut current_run),
                        current_color,
                        current_bold,
                        current_italic,
                    ));
                    current_color = cell.fg;
                    current_bold = cell.bold;
                    current_italic = cell.italic;
                    current_run.push(cell.c);
                }
            }
            if !current_run.is_empty() {
                spans.push((current_run, current_color, current_bold, current_italic));
            }
        }

        let default_attrs = Attrs::new().family(Family::Monospace);

        let rich_spans: Vec<(&str, Attrs<'_>)> = spans
            .iter()
            .map(|(text, color, bold, italic)| {
                let mut attrs = Attrs::new().family(Family::Monospace).color(*color);
                if *bold {
                    attrs = attrs.weight(Weight::BOLD);
                }
                if *italic {
                    attrs = attrs.style(Style::Italic);
                }
                (text.as_str(), attrs)
            })
            .collect();

        self.buffer.set_rich_text(
            &mut self.font_system,
            rich_spans,
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer
            .set_size(&mut self.font_system, Some(width), Some(height));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }
}
