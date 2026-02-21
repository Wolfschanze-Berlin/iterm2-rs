/// Events parsed from tmux control mode protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxEvent {
    /// DCS sequence detected — tmux control mode has started.
    ControlModeEntered,
    /// Pane output: %output %<pane_id> <data>
    Output { pane_id: u32, data: Vec<u8> },
    /// Window added: %window-add @<id>
    WindowAdd { window_id: u32 },
    /// Window closed: %window-close @<id>
    WindowClose { window_id: u32 },
    /// Window renamed: %window-renamed @<id> <name>
    WindowRenamed { window_id: u32, name: String },
    /// Layout changed: %layout-change @<id> <layout>
    LayoutChange { window_id: u32, layout: String },
    /// Session changed: %session-changed $<id> <name>
    SessionChanged { session_id: u32, name: String },
    /// Sessions changed notification (informational only).
    SessionsChanged,
    /// Response to a command: between %begin and %end
    Response { guard: String, lines: Vec<String> },
    /// tmux exiting control mode
    Exit,
}
