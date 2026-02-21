//! Renderer-specific configuration.
//!
//! The binary crate owns the user-facing `Config` (with TOML deserialization).
//! This module defines a lightweight struct that the renderer needs at runtime,
//! keeping the renderer crate independent of the binary crate.

/// RGB color with components in 0.0..=1.0 range (suitable for wgpu).
#[derive(Debug, Clone, Copy)]
pub struct RgbColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl RgbColor {
    /// Create an RGB color from individual components (0.0..=1.0).
    pub fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }

    /// Parse a hex color string like "#1e1e2e" or "1e1e2e" into an `RgbColor`.
    ///
    /// Returns `None` if the string is not a valid 6-digit hex color.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self {
            r: r as f64 / 255.0,
            g: g as f64 / 255.0,
            b: b as f64 / 255.0,
        })
    }

    /// Convert to a `wgpu::Color` with alpha 1.0.
    pub fn to_wgpu_color(self) -> wgpu::Color {
        wgpu::Color {
            r: self.r,
            g: self.g,
            b: self.b,
            a: 1.0,
        }
    }

    /// Convert to RGB u8 tuple (0..=255), useful for glyphon `Color::rgb`.
    pub fn to_rgb_u8(self) -> (u8, u8, u8) {
        (
            (self.r * 255.0).round() as u8,
            (self.g * 255.0).round() as u8,
            (self.b * 255.0).round() as u8,
        )
    }
}

/// Configuration values the renderer needs from the application config.
#[derive(Debug, Clone)]
pub struct RendererConfig {
    /// Font family name (e.g. "Cascadia Code").
    pub font_family: String,
    /// Font size in points.
    pub font_size: f32,
    /// Background clear color.
    pub bg_color: RgbColor,
    /// Default foreground (text) color.
    pub fg_color: RgbColor,
    /// Window title.
    pub window_title: String,
    /// Initial window width in logical pixels.
    pub window_width: u32,
    /// Initial window height in logical pixels.
    pub window_height: u32,
    /// Window opacity (0.0 = fully transparent, 1.0 = opaque).
    pub opacity: f32,
    /// Default terminal columns.
    pub default_cols: u16,
    /// Default terminal rows.
    pub default_rows: u16,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            font_family: "Cascadia Code".to_string(),
            font_size: 14.0,
            // Catppuccin Mocha base: #1e1e2e
            bg_color: RgbColor::from_hex("#1e1e2e").unwrap(),
            // Catppuccin Mocha text: #cdd6f4
            fg_color: RgbColor::from_hex("#cdd6f4").unwrap(),
            window_title: "iterm2-rs".to_string(),
            window_width: 800,
            window_height: 600,
            opacity: 1.0,
            default_cols: 80,
            default_rows: 24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_with_hash() {
        let c = RgbColor::from_hex("#1e1e2e").unwrap();
        assert!((c.r - 30.0 / 255.0).abs() < 1e-6);
        assert!((c.g - 30.0 / 255.0).abs() < 1e-6);
        assert!((c.b - 46.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn parse_hex_without_hash() {
        let c = RgbColor::from_hex("cdd6f4").unwrap();
        assert!((c.r - 205.0 / 255.0).abs() < 1e-6);
        assert!((c.g - 214.0 / 255.0).abs() < 1e-6);
        assert!((c.b - 244.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn parse_hex_invalid() {
        assert!(RgbColor::from_hex("zzzzzz").is_none());
        assert!(RgbColor::from_hex("#12345").is_none());
        assert!(RgbColor::from_hex("").is_none());
    }

    #[test]
    fn to_rgb_u8_roundtrip() {
        let c = RgbColor::from_hex("#cdd6f4").unwrap();
        let (r, g, b) = c.to_rgb_u8();
        assert_eq!(r, 205);
        assert_eq!(g, 214);
        assert_eq!(b, 244);
    }

    #[test]
    fn default_config_has_catppuccin_colors() {
        let cfg = RendererConfig::default();
        assert_eq!(cfg.window_title, "iterm2-rs");
        assert_eq!(cfg.window_width, 800);
        assert_eq!(cfg.window_height, 600);
        assert_eq!(cfg.font_size, 14.0);
    }
}
