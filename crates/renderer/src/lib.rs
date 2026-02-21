//! GPU rendering engine for iterm2-rs.
//! Uses wgpu + glyphon for hardware-accelerated text rendering.

pub mod config;
pub mod gpu;
pub mod terminal_renderer;
pub mod text;
pub mod window;

pub use config::{RendererConfig, RgbColor};
pub use gpu::{GpuState, RenderError};
pub use terminal_renderer::{CursorInfo, CursorShape};
pub use text::TextRenderer;
pub use window::App;
