/// Abstraction over terminal emulation backends.
/// Phase 1: alacritty_terminal implementation.
/// Future: custom lightweight grid for tmux panes.
pub trait TerminalBackend {
    /// Feed raw bytes from PTY into the terminal emulator
    fn process_bytes(&mut self, bytes: &[u8]);

    /// Get the terminal grid dimensions (columns, rows)
    fn size(&self) -> (u16, u16);

    /// Resize the terminal
    fn resize(&mut self, cols: u16, rows: u16);
}
