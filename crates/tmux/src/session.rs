//! Tmux session manager — maintains state about tmux windows, panes, and
//! sessions by processing [`TmuxEvent`]s from the control-mode parser.

use crate::TmuxEvent;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A single tmux pane (identified by `%<id>` in tmux protocol).
#[derive(Debug, Clone, PartialEq)]
pub struct TmuxPane {
    pub id: u32,
    pub active: bool,
    pub width: u16,
    pub height: u16,
}

/// A tmux window (identified by `@<id>` in tmux protocol).
#[derive(Debug, Clone, PartialEq)]
pub struct TmuxWindow {
    pub id: u32,
    pub name: String,
    pub layout: String,
    pub panes: Vec<TmuxPane>,
    pub active: bool,
}

/// A tmux session (identified by `$<id>` in tmux protocol).
#[derive(Debug, Clone, PartialEq)]
pub struct TmuxSession {
    pub id: u32,
    pub name: String,
    pub windows: Vec<TmuxWindow>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// High-level commands that can be sent to tmux.
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxCommand {
    ListWindows,
    ListPanes { window_id: u32 },
    ListSessions,
    NewWindow { name: Option<String> },
    KillWindow { window_id: u32 },
    SelectWindow { window_id: u32 },
    SendKeys { pane_id: u32, keys: String },
    ResizePane { pane_id: u32, width: u16, height: u16 },
    /// Split a window — `direction` is `'h'` (horizontal) or `'v'` (vertical).
    SplitWindow { direction: char, pane_id: u32 },
    RefreshClient { width: u16, height: u16 },
}

/// Discriminant for what kind of response we expect from a pending command.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandType {
    ListWindows,
    ListPanes,
    ListSessions,
    SendKeys,
    ResizePane,
    Custom(String),
}

/// Tracks a command that was sent to tmux and is awaiting a response.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCommand {
    pub guard: String,
    pub command: String,
    pub callback_type: CommandType,
}

// ---------------------------------------------------------------------------
// Session manager
// ---------------------------------------------------------------------------

/// Central state manager for a tmux control-mode connection.
///
/// Feed it [`TmuxEvent`]s (produced by [`crate::TmuxParser`]) via
/// [`process_event`](SessionManager::process_event) to keep the model in sync
/// with the actual tmux server.
pub struct SessionManager {
    sessions: Vec<TmuxSession>,
    active_session: Option<u32>,
    pending_commands: Vec<PendingCommand>,
    control_mode_active: bool,
}

impl SessionManager {
    /// Create a new, empty session manager.
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active_session: None,
            pending_commands: Vec::new(),
            control_mode_active: false,
        }
    }

    // -- queries ------------------------------------------------------------

    /// Whether the tmux connection is in control mode.
    pub fn is_active(&self) -> bool {
        self.control_mode_active
    }

    /// Return a reference to the currently active session, if any.
    pub fn active_session(&self) -> Option<&TmuxSession> {
        let id = self.active_session?;
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Find a window by its id across all sessions.
    pub fn find_window(&self, window_id: u32) -> Option<&TmuxWindow> {
        self.sessions
            .iter()
            .flat_map(|s| &s.windows)
            .find(|w| w.id == window_id)
    }

    /// Find a pane by its id across all sessions and windows.
    pub fn find_pane(&self, pane_id: u32) -> Option<&TmuxPane> {
        self.sessions
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .find(|p| p.id == pane_id)
    }

    /// Register a pending command so that the corresponding `Response` event
    /// can be matched back to it later.
    pub fn register_pending(&mut self, cmd: PendingCommand) {
        self.pending_commands.push(cmd);
    }

    // -- event processing ---------------------------------------------------

    /// Update internal state in response to a [`TmuxEvent`].
    pub fn process_event(&mut self, event: &TmuxEvent) {
        match event {
            TmuxEvent::ControlModeEntered => {
                self.control_mode_active = true;
                log::debug!("tmux control mode entered");
            }

            TmuxEvent::WindowAdd { window_id } => {
                if let Some(session) = self.active_session_mut() {
                    // Only add if not already present.
                    if !session.windows.iter().any(|w| w.id == *window_id) {
                        session.windows.push(TmuxWindow {
                            id: *window_id,
                            name: String::new(),
                            layout: String::new(),
                            panes: Vec::new(),
                            active: false,
                        });
                        log::debug!("window @{} added", window_id);
                    }
                }
            }

            TmuxEvent::WindowClose { window_id } => {
                if let Some(session) = self.active_session_mut() {
                    session.windows.retain(|w| w.id != *window_id);
                    log::debug!("window @{} closed", window_id);
                }
            }

            TmuxEvent::WindowRenamed { window_id, name } => {
                if let Some(win) = self.find_window_mut(*window_id) {
                    win.name = name.clone();
                    log::debug!("window @{} renamed to '{}'", window_id, name);
                }
            }

            TmuxEvent::LayoutChange { window_id, layout } => {
                if let Some(win) = self.find_window_mut(*window_id) {
                    win.layout = layout.clone();
                    log::debug!("window @{} layout changed", window_id);
                }
            }

            TmuxEvent::SessionChanged { session_id, name } => {
                // Ensure the session exists in our list.
                if !self.sessions.iter().any(|s| s.id == *session_id) {
                    self.sessions.push(TmuxSession {
                        id: *session_id,
                        name: name.clone(),
                        windows: Vec::new(),
                    });
                } else if let Some(s) = self.sessions.iter_mut().find(|s| s.id == *session_id) {
                    s.name = name.clone();
                }
                self.active_session = Some(*session_id);
                log::debug!("session changed to ${}:{}", session_id, name);
            }

            TmuxEvent::SessionsChanged => {
                log::debug!("sessions changed notification");
            }

            TmuxEvent::Output { pane_id, .. } => {
                log::trace!("output on pane %{}", pane_id);
            }

            TmuxEvent::Response { guard, lines } => {
                if let Some(pos) = self
                    .pending_commands
                    .iter()
                    .position(|p| p.guard == *guard)
                {
                    let pending = self.pending_commands.remove(pos);
                    self.handle_response(&pending, lines);
                } else {
                    log::debug!("response with unknown guard '{}': {:?}", guard, lines);
                }
            }

            TmuxEvent::Exit => {
                self.control_mode_active = false;
                log::debug!("tmux control mode exited");
            }
        }
    }

    // -- command formatting --------------------------------------------------

    /// Format a [`TmuxCommand`] into the string that should be written to
    /// tmux's stdin (without a trailing newline).
    pub fn format_command(cmd: &TmuxCommand) -> String {
        match cmd {
            TmuxCommand::ListWindows => {
                "list-windows -F '#{window_id} #{window_name} #{window_active} #{window_layout}'"
                    .to_string()
            }
            TmuxCommand::ListPanes { window_id } => {
                format!(
                    "list-panes -t @{} -F '#{{pane_id}} #{{pane_width}} #{{pane_height}} #{{pane_active}}'",
                    window_id
                )
            }
            TmuxCommand::ListSessions => {
                "list-sessions -F '#{session_id} #{session_name}'".to_string()
            }
            TmuxCommand::NewWindow { name } => match name {
                Some(n) => format!("new-window -n '{}'", n),
                None => "new-window".to_string(),
            },
            TmuxCommand::KillWindow { window_id } => {
                format!("kill-window -t @{}", window_id)
            }
            TmuxCommand::SelectWindow { window_id } => {
                format!("select-window -t @{}", window_id)
            }
            TmuxCommand::SendKeys { pane_id, keys } => {
                format!("send-keys -t %{} {}", pane_id, keys)
            }
            TmuxCommand::ResizePane {
                pane_id,
                width,
                height,
            } => {
                format!("resize-pane -t %{} -x {} -y {}", pane_id, width, height)
            }
            TmuxCommand::SplitWindow {
                direction,
                pane_id,
            } => {
                format!("split-window -{} -t %{}", direction, pane_id)
            }
            TmuxCommand::RefreshClient { width, height } => {
                format!("refresh-client -C {},{}", width, height)
            }
        }
    }

    // -- internal helpers ---------------------------------------------------

    fn active_session_mut(&mut self) -> Option<&mut TmuxSession> {
        let id = self.active_session?;
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    fn find_window_mut(&mut self, window_id: u32) -> Option<&mut TmuxWindow> {
        self.sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .find(|w| w.id == window_id)
    }

    /// Handle a matched response by parsing the output lines according to the
    /// command type and updating internal state.
    fn handle_response(&mut self, pending: &PendingCommand, lines: &[String]) {
        match pending.callback_type {
            CommandType::ListWindows => self.parse_list_windows(lines),
            CommandType::ListPanes => self.parse_list_panes(lines),
            CommandType::ListSessions => self.parse_list_sessions(lines),
            CommandType::SendKeys | CommandType::ResizePane => {
                // No response body to parse for these.
            }
            CommandType::Custom(ref label) => {
                log::debug!("custom response '{}': {:?}", label, lines);
            }
        }
    }

    /// Parse `list-windows` response lines.
    ///
    /// Expected format per line: `@<id> <name> <active:0|1> <layout>`
    fn parse_list_windows(&mut self, lines: &[String]) {
        let session = match self.active_session_mut() {
            Some(s) => s,
            None => return,
        };

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Expect: @<id> <name> <0|1> <layout...>
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            if parts.len() < 4 {
                log::warn!("unexpected list-windows line: {}", line);
                continue;
            }
            let id = match parts[0].strip_prefix('@').and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => continue,
            };
            let name = parts[1].to_string();
            let active = parts[2] == "1";
            let layout = parts[3].to_string();

            if let Some(win) = session.windows.iter_mut().find(|w| w.id == id) {
                win.name = name;
                win.active = active;
                win.layout = layout;
            } else {
                session.windows.push(TmuxWindow {
                    id,
                    name,
                    layout,
                    panes: Vec::new(),
                    active,
                });
            }
        }
    }

    /// Parse `list-panes` response lines.
    ///
    /// Expected format per line: `%<id> <width> <height> <active:0|1>`
    fn parse_list_panes(&mut self, lines: &[String]) {
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            if parts.len() < 4 {
                log::warn!("unexpected list-panes line: {}", line);
                continue;
            }
            let id = match parts[0].strip_prefix('%').and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => continue,
            };
            let width: u16 = parts[1].parse().unwrap_or(0);
            let height: u16 = parts[2].parse().unwrap_or(0);
            let active = parts[3] == "1";

            let pane = TmuxPane {
                id,
                active,
                width,
                height,
            };

            // Try to place the pane in the right window. We iterate all
            // windows; if the pane already exists, update it, otherwise append
            // it to the first window that doesn't already contain it (callers
            // typically issue list-panes per-window, so there's usually a
            // single window scope, but we handle the general case).
            let mut found = false;
            for session in &mut self.sessions {
                for win in &mut session.windows {
                    if let Some(existing) = win.panes.iter_mut().find(|p| p.id == id) {
                        *existing = pane.clone();
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }

            if !found {
                // Append to the first window of the active session as a
                // fallback.
                if let Some(session) = self.active_session_mut() {
                    if let Some(win) = session.windows.first_mut() {
                        win.panes.push(pane);
                    }
                }
            }
        }
    }

    /// Parse `list-sessions` response lines.
    ///
    /// Expected format per line: `$<id> <name>`
    fn parse_list_sessions(&mut self, lines: &[String]) {
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if parts.len() < 2 {
                log::warn!("unexpected list-sessions line: {}", line);
                continue;
            }
            let id = match parts[0].strip_prefix('$').and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => continue,
            };
            let name = parts[1].to_string();

            if !self.sessions.iter().any(|s| s.id == id) {
                self.sessions.push(TmuxSession {
                    id,
                    name,
                    windows: Vec::new(),
                });
            } else if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                s.name = name;
            }
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TmuxEvent;

    /// Helper: create a session manager that already has control mode active
    /// and an active session.
    fn setup_manager() -> SessionManager {
        let mut mgr = SessionManager::new();
        mgr.process_event(&TmuxEvent::ControlModeEntered);
        mgr.process_event(&TmuxEvent::SessionChanged {
            session_id: 0,
            name: "main".into(),
        });
        mgr
    }

    // -- command formatting -------------------------------------------------

    #[test]
    fn format_list_windows() {
        let s = SessionManager::format_command(&TmuxCommand::ListWindows);
        assert!(s.starts_with("list-windows"));
        assert!(s.contains("#{window_id}"));
    }

    #[test]
    fn format_list_panes() {
        let s = SessionManager::format_command(&TmuxCommand::ListPanes { window_id: 3 });
        assert!(s.contains("-t @3"));
        assert!(s.contains("#{pane_id}"));
    }

    #[test]
    fn format_list_sessions() {
        let s = SessionManager::format_command(&TmuxCommand::ListSessions);
        assert!(s.starts_with("list-sessions"));
    }

    #[test]
    fn format_send_keys() {
        let s = SessionManager::format_command(&TmuxCommand::SendKeys {
            pane_id: 1,
            keys: "ls Enter".into(),
        });
        assert_eq!(s, "send-keys -t %1 ls Enter");
    }

    #[test]
    fn format_resize_pane() {
        let s = SessionManager::format_command(&TmuxCommand::ResizePane {
            pane_id: 2,
            width: 120,
            height: 40,
        });
        assert_eq!(s, "resize-pane -t %2 -x 120 -y 40");
    }

    #[test]
    fn format_refresh_client() {
        let s = SessionManager::format_command(&TmuxCommand::RefreshClient {
            width: 200,
            height: 50,
        });
        assert_eq!(s, "refresh-client -C 200,50");
    }

    #[test]
    fn format_new_window_with_name() {
        let s = SessionManager::format_command(&TmuxCommand::NewWindow {
            name: Some("build".into()),
        });
        assert_eq!(s, "new-window -n 'build'");
    }

    #[test]
    fn format_new_window_without_name() {
        let s = SessionManager::format_command(&TmuxCommand::NewWindow { name: None });
        assert_eq!(s, "new-window");
    }

    #[test]
    fn format_kill_window() {
        let s = SessionManager::format_command(&TmuxCommand::KillWindow { window_id: 5 });
        assert_eq!(s, "kill-window -t @5");
    }

    #[test]
    fn format_select_window() {
        let s = SessionManager::format_command(&TmuxCommand::SelectWindow { window_id: 2 });
        assert_eq!(s, "select-window -t @2");
    }

    #[test]
    fn format_split_window() {
        let s = SessionManager::format_command(&TmuxCommand::SplitWindow {
            direction: 'v',
            pane_id: 0,
        });
        assert_eq!(s, "split-window -v -t %0");
    }

    // -- control mode tracking ----------------------------------------------

    #[test]
    fn control_mode_tracking() {
        let mut mgr = SessionManager::new();
        assert!(!mgr.is_active());

        mgr.process_event(&TmuxEvent::ControlModeEntered);
        assert!(mgr.is_active());

        mgr.process_event(&TmuxEvent::Exit);
        assert!(!mgr.is_active());
    }

    // -- session changed ----------------------------------------------------

    #[test]
    fn session_changed_creates_and_activates() {
        let mut mgr = SessionManager::new();
        mgr.process_event(&TmuxEvent::SessionChanged {
            session_id: 1,
            name: "dev".into(),
        });

        assert_eq!(mgr.active_session, Some(1));
        let s = mgr.active_session().expect("should have active session");
        assert_eq!(s.id, 1);
        assert_eq!(s.name, "dev");
    }

    #[test]
    fn session_changed_updates_name() {
        let mut mgr = SessionManager::new();
        mgr.process_event(&TmuxEvent::SessionChanged {
            session_id: 1,
            name: "old".into(),
        });
        mgr.process_event(&TmuxEvent::SessionChanged {
            session_id: 1,
            name: "new".into(),
        });
        assert_eq!(mgr.active_session().unwrap().name, "new");
    }

    // -- window add / remove / rename ---------------------------------------

    #[test]
    fn window_add_and_find() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 1 });

        let win = mgr.find_window(1).expect("window should exist");
        assert_eq!(win.id, 1);
    }

    #[test]
    fn window_add_idempotent() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 1 });
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 1 });

        let session = mgr.active_session().unwrap();
        assert_eq!(session.windows.len(), 1);
    }

    #[test]
    fn window_close() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 1 });
        mgr.process_event(&TmuxEvent::WindowClose { window_id: 1 });

        assert!(mgr.find_window(1).is_none());
    }

    #[test]
    fn window_rename() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 1 });
        mgr.process_event(&TmuxEvent::WindowRenamed {
            window_id: 1,
            name: "editor".into(),
        });

        assert_eq!(mgr.find_window(1).unwrap().name, "editor");
    }

    // -- layout change ------------------------------------------------------

    #[test]
    fn layout_change() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 1 });
        mgr.process_event(&TmuxEvent::LayoutChange {
            window_id: 1,
            layout: "a]b0,120x40,0,0".into(),
        });

        assert_eq!(mgr.find_window(1).unwrap().layout, "a]b0,120x40,0,0");
    }

    // -- response handling --------------------------------------------------

    #[test]
    fn response_matches_pending_command() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 0 });

        mgr.register_pending(PendingCommand {
            guard: "42".into(),
            command: "list-sessions".into(),
            callback_type: CommandType::ListSessions,
        });

        mgr.process_event(&TmuxEvent::Response {
            guard: "42".into(),
            lines: vec!["$5 work".into()],
        });

        // The list-sessions response should have added the session.
        assert!(mgr.sessions.iter().any(|s| s.id == 5 && s.name == "work"));
        // Pending command should be consumed.
        assert!(mgr.pending_commands.is_empty());
    }

    #[test]
    fn find_pane_across_windows() {
        let mut mgr = setup_manager();
        mgr.process_event(&TmuxEvent::WindowAdd { window_id: 0 });

        // Manually inject a pane for the test.
        mgr.active_session_mut()
            .unwrap()
            .windows[0]
            .panes
            .push(TmuxPane {
                id: 7,
                active: true,
                width: 80,
                height: 24,
            });

        let pane = mgr.find_pane(7).expect("pane should be found");
        assert_eq!(pane.width, 80);
        assert!(mgr.find_pane(999).is_none());
    }
}
