//! GPU rendering engine for iterm2-rs.
//! Uses wgpu + glyphon for hardware-accelerated text rendering.

pub mod gpu;
pub mod terminal_renderer;
pub mod text;
pub mod window;

pub use gpu::GpuState;
pub use text::TextRenderer;
pub use window::App;
