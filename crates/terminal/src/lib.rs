//! Terminal emulation layer for iterm2-rs.
//! Wraps alacritty_terminal behind a TerminalBackend trait.

pub mod alacritty;
pub mod backend;
pub mod input;
pub mod pane;
pub mod pty;
pub mod search;
pub mod shell_integration;
pub mod tab;

pub use alacritty::AlacrittyBackend;
pub use backend::TerminalBackend;
pub use pane::{PaneLayout, PaneNode, PaneRect, SplitDirection};
pub use pty::PtyHandle;
pub use tab::{Tab, TabManager};
