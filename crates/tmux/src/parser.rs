use crate::events::TmuxEvent;
use log::trace;

/// Parser state for the tmux control mode protocol.
pub struct TmuxParser {
    state: ParserState,
    line_buffer: String,
}

enum ParserState {
    /// Waiting for the next line.
    Idle,
    /// Inside a %begin/%end response block.
    InResponse { guard: String, lines: Vec<String> },
}

/// Unescape tmux %output data.
///
/// tmux octal-escapes characters with value < 32 and backslash itself
/// as `\NNN` (three-digit octal).  Everything else is passed through
/// as-is (valid UTF-8 bytes).
fn unescape_output(data: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let bytes = data.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            // Try to read three octal digits.
            let d0 = bytes[i + 1];
            let d1 = bytes[i + 2];
            let d2 = bytes[i + 3];
            if d0.is_ascii_digit() && d1.is_ascii_digit() && d2.is_ascii_digit() {
                let val =
                    (d0 - b'0') as u16 * 64 + (d1 - b'0') as u16 * 8 + (d2 - b'0') as u16;
                if val <= 255 {
                    out.push(val as u8);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

impl TmuxParser {
    pub fn new() -> Self {
        Self {
            state: ParserState::Idle,
            line_buffer: String::new(),
        }
    }

    /// Feed raw bytes into the parser, returning any complete events.
    ///
    /// Data may arrive in arbitrary chunks; the parser buffers partial lines
    /// internally and only processes complete newline-terminated lines.
    pub fn feed(&mut self, data: &[u8]) -> Vec<TmuxEvent> {
        let mut events = Vec::new();

        // We treat input as UTF-8 (tmux control mode is text).
        // Invalid UTF-8 is replaced with U+FFFD, which is acceptable.
        let text = String::from_utf8_lossy(data);
        self.line_buffer.push_str(&text);

        // Process all complete lines (terminated by '\n').
        while let Some(newline_pos) = self.line_buffer.find('\n') {
            let line = self.line_buffer[..newline_pos].to_string();
            self.line_buffer = self.line_buffer[newline_pos + 1..].to_string();

            // Strip trailing \r if present.
            let line = line.strip_suffix('\r').unwrap_or(&line).to_string();

            self.process_line(&line, &mut events);
        }

        events
    }

    fn process_line(&mut self, line: &str, events: &mut Vec<TmuxEvent>) {
        trace!("tmux line: {:?}", line);

        // Check for DCS entry sequence: \x1bP1000p (possibly with content after).
        if line.starts_with("\x1bP1000p") {
            events.push(TmuxEvent::ControlModeEntered);
            // There may be content after the DCS prefix on the same line.
            let rest = &line["\x1bP1000p".len()..];
            if !rest.is_empty() {
                self.process_line(rest, events);
            }
            return;
        }

        // If we are inside a response block, handle %end or collect the line.
        match &mut self.state {
            ParserState::InResponse { guard, lines } => {
                if line.starts_with("%end ") {
                    let end_guard = &line["%end ".len()..];
                    if end_guard == guard.as_str() {
                        let guard_owned = guard.clone();
                        let lines_owned = lines.clone();
                        self.state = ParserState::Idle;
                        events.push(TmuxEvent::Response {
                            guard: guard_owned,
                            lines: lines_owned,
                        });
                    } else {
                        // Guard mismatch — treat as content.
                        lines.push(line.to_string());
                    }
                } else if line.starts_with("%error ") {
                    // %error also closes the block; for now treat same as %end.
                    let error_guard = &line["%error ".len()..];
                    if error_guard == guard.as_str() {
                        let guard_owned = guard.clone();
                        let lines_owned = lines.clone();
                        self.state = ParserState::Idle;
                        events.push(TmuxEvent::Response {
                            guard: guard_owned,
                            lines: lines_owned,
                        });
                    } else {
                        lines.push(line.to_string());
                    }
                } else {
                    lines.push(line.to_string());
                }
                return;
            }
            ParserState::Idle => {}
        }

        // Idle state — parse notification lines.
        if line.starts_with("%begin ") {
            let guard = line["%begin ".len()..].to_string();
            self.state = ParserState::InResponse {
                guard,
                lines: Vec::new(),
            };
        } else if line.starts_with("%output ") {
            if let Some(ev) = Self::parse_output(line) {
                events.push(ev);
            }
        } else if line.starts_with("%window-add ") {
            if let Some(id) = Self::parse_window_id(&line["%window-add ".len()..]) {
                events.push(TmuxEvent::WindowAdd { window_id: id });
            }
        } else if line.starts_with("%window-close ") {
            if let Some(id) = Self::parse_window_id(&line["%window-close ".len()..]) {
                events.push(TmuxEvent::WindowClose { window_id: id });
            }
        } else if line.starts_with("%window-renamed ") {
            let rest = &line["%window-renamed ".len()..];
            if let Some((id, name)) = Self::parse_window_id_and_rest(rest) {
                events.push(TmuxEvent::WindowRenamed {
                    window_id: id,
                    name,
                });
            }
        } else if line.starts_with("%layout-change ") {
            let rest = &line["%layout-change ".len()..];
            if let Some((id, layout)) = Self::parse_window_id_and_rest(rest) {
                events.push(TmuxEvent::LayoutChange {
                    window_id: id,
                    layout,
                });
            }
        } else if line.starts_with("%session-changed ") {
            if let Some(ev) = Self::parse_session_changed(line) {
                events.push(ev);
            }
        } else if line == "%sessions-changed" {
            events.push(TmuxEvent::SessionsChanged);
        } else if line.starts_with("%exit") {
            events.push(TmuxEvent::Exit);
        }
    }

    /// Parse `%output %<pane_id> <data>`.
    fn parse_output(line: &str) -> Option<TmuxEvent> {
        let rest = &line["%output ".len()..];
        // Expect `%<digits> <data>`
        if !rest.starts_with('%') {
            return None;
        }
        let rest = &rest[1..]; // skip '%'
        let space_pos = rest.find(' ')?;
        let pane_id: u32 = rest[..space_pos].parse().ok()?;
        let data_str = &rest[space_pos + 1..];
        let data = unescape_output(data_str);
        Some(TmuxEvent::Output { pane_id, data })
    }

    /// Parse `@<digits>`, return the numeric id.
    fn parse_window_id(s: &str) -> Option<u32> {
        let s = s.trim();
        if s.starts_with('@') {
            s[1..].parse().ok()
        } else {
            None
        }
    }

    /// Parse `@<id> <rest>`, return (id, rest).
    fn parse_window_id_and_rest(s: &str) -> Option<(u32, String)> {
        if !s.starts_with('@') {
            return None;
        }
        let s = &s[1..];
        let space_pos = s.find(' ')?;
        let id: u32 = s[..space_pos].parse().ok()?;
        let rest = s[space_pos + 1..].to_string();
        Some((id, rest))
    }

    /// Parse `%session-changed $<id> <name>`.
    fn parse_session_changed(line: &str) -> Option<TmuxEvent> {
        let rest = &line["%session-changed ".len()..];
        if !rest.starts_with('$') {
            return None;
        }
        let rest = &rest[1..];
        let space_pos = rest.find(' ')?;
        let session_id: u32 = rest[..space_pos].parse().ok()?;
        let name = rest[space_pos + 1..].to_string();
        Some(TmuxEvent::SessionChanged { session_id, name })
    }
}

impl Default for TmuxParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_window_add() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%window-add @0\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::WindowAdd { window_id: 0 });

        let events = parser.feed(b"%window-add @42\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::WindowAdd { window_id: 42 });
    }

    #[test]
    fn test_parse_window_close() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%window-close @3\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::WindowClose { window_id: 3 });
    }

    #[test]
    fn test_parse_window_renamed() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%window-renamed @1 my-window\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TmuxEvent::WindowRenamed {
                window_id: 1,
                name: "my-window".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_session_changed() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%session-changed $0 0\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TmuxEvent::SessionChanged {
                session_id: 0,
                name: "0".to_string(),
            }
        );

        let events = parser.feed(b"%session-changed $5 my-session\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TmuxEvent::SessionChanged {
                session_id: 5,
                name: "my-session".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_output() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%output %0 hello world\n");
        assert_eq!(events.len(), 1);
        match &events[0] {
            TmuxEvent::Output { pane_id, data } => {
                assert_eq!(*pane_id, 0);
                assert_eq!(data, b"hello world");
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_output_with_escapes() {
        let mut parser = TmuxParser::new();
        // Simulate %output with octal-escaped ESC (\033) and newline (\012).
        let events = parser.feed(b"%output %0 \\033[?2004h\\012\n");
        assert_eq!(events.len(), 1);
        match &events[0] {
            TmuxEvent::Output { pane_id, data } => {
                assert_eq!(*pane_id, 0);
                assert_eq!(data[0], 0x1b); // ESC
                assert_eq!(&data[1..7], b"[?2004");
                assert_eq!(data[7], b'h');
                assert_eq!(data[8], 0x0a); // newline
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_response_block() {
        let mut parser = TmuxParser::new();
        let input = b"%begin 1771657050 270 1\n\
            0: bash* (1 panes) [80x24] [layout b25d,80x24,0,0,0] @0 (active)\n\
            %end 1771657050 270 1\n";
        let events = parser.feed(input);
        assert_eq!(events.len(), 1);
        match &events[0] {
            TmuxEvent::Response { guard, lines } => {
                assert_eq!(guard, "1771657050 270 1");
                assert_eq!(lines.len(), 1);
                assert!(lines[0].contains("bash*"));
            }
            other => panic!("expected Response, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_empty_response_block() {
        let mut parser = TmuxParser::new();
        let input = b"%begin 1771657050 264 0\n%end 1771657050 264 0\n";
        let events = parser.feed(input);
        assert_eq!(events.len(), 1);
        match &events[0] {
            TmuxEvent::Response { guard, lines } => {
                assert_eq!(guard, "1771657050 264 0");
                assert!(lines.is_empty());
            }
            other => panic!("expected Response, got {:?}", other),
        }
    }

    #[test]
    fn test_partial_line_buffering() {
        let mut parser = TmuxParser::new();

        // Send partial line — no events yet.
        let events = parser.feed(b"%window-");
        assert!(events.is_empty());

        // Complete the line.
        let events = parser.feed(b"add @7\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::WindowAdd { window_id: 7 });
    }

    #[test]
    fn test_partial_line_buffering_response() {
        let mut parser = TmuxParser::new();

        // Feed %begin in two chunks.
        let events = parser.feed(b"%begin 123");
        assert!(events.is_empty());
        let events = parser.feed(b" 456 0\nline one\n");
        assert!(events.is_empty()); // still in response

        // Feed %end in one chunk.
        let events = parser.feed(b"%end 123 456 0\n");
        assert_eq!(events.len(), 1);
        match &events[0] {
            TmuxEvent::Response { guard, lines } => {
                assert_eq!(guard, "123 456 0");
                assert_eq!(lines, &["line one"]);
            }
            other => panic!("expected Response, got {:?}", other),
        }
    }

    #[test]
    fn test_unescape_output() {
        // Plain ASCII passes through.
        assert_eq!(unescape_output("hello"), b"hello");

        // Octal-escaped ESC.
        assert_eq!(unescape_output("\\033"), vec![0x1b]);

        // Octal-escaped backslash: \134 = 92 = '\\'.
        assert_eq!(unescape_output("\\134"), vec![b'\\']);

        // Mixed content.
        let result = unescape_output("A\\033[0mB");
        assert_eq!(result, vec![b'A', 0x1b, b'[', b'0', b'm', b'B']);

        // Backslash not followed by three digits passes through.
        assert_eq!(unescape_output("\\xy"), b"\\xy");
    }

    #[test]
    fn test_control_mode_entry() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"\x1bP1000p\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::ControlModeEntered);
    }

    #[test]
    fn test_control_mode_entry_with_following_content() {
        let mut parser = TmuxParser::new();
        // DCS line followed by notifications.
        let events = parser.feed(
            b"\x1bP1000p\n%begin 1771657050 264 0\n%end 1771657050 264 0\n%window-add @0\n",
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], TmuxEvent::ControlModeEntered);
        match &events[1] {
            TmuxEvent::Response { guard, lines } => {
                assert_eq!(guard, "1771657050 264 0");
                assert!(lines.is_empty());
            }
            other => panic!("expected Response, got {:?}", other),
        }
        assert_eq!(events[2], TmuxEvent::WindowAdd { window_id: 0 });
    }

    #[test]
    fn test_sessions_changed() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%sessions-changed\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::SessionsChanged);
    }

    #[test]
    fn test_exit() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%exit\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], TmuxEvent::Exit);
    }

    #[test]
    fn test_layout_change() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%layout-change @0 b25d,80x24,0,0,0\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TmuxEvent::LayoutChange {
                window_id: 0,
                layout: "b25d,80x24,0,0,0".to_string(),
            }
        );
    }

    #[test]
    fn test_multiple_events_in_one_feed() {
        let mut parser = TmuxParser::new();
        let events = parser.feed(b"%window-add @0\n%sessions-changed\n%session-changed $0 0\n");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], TmuxEvent::WindowAdd { window_id: 0 });
        assert_eq!(events[1], TmuxEvent::SessionsChanged);
        assert_eq!(
            events[2],
            TmuxEvent::SessionChanged {
                session_id: 0,
                name: "0".to_string(),
            }
        );
    }

    /// Test with realistic protocol output from the spike test.
    #[test]
    fn test_full_spike_output() {
        let mut parser = TmuxParser::new();

        let input = b"\x1bP1000p\n\
            %begin 1771657050 264 0\n\
            %end 1771657050 264 0\n\
            %window-add @0\n\
            %sessions-changed\n\
            %session-changed $0 0\n\
            %begin 1771657050 270 1\n\
            0: bash* (1 panes) [80x24] [layout b25d,80x24,0,0,0] @0 (active)\n\
            %end 1771657050 270 1\n\
            %output %0 \\033[?2004h\\033[01;32mfranc@Bahnhof\\033[00m:\\033[01;34m/mnt/c/Users/franc\\033[00m$\n";

        let events = parser.feed(input);
        assert_eq!(events.len(), 7);

        assert_eq!(events[0], TmuxEvent::ControlModeEntered);

        // Empty response block.
        match &events[1] {
            TmuxEvent::Response { guard, lines } => {
                assert_eq!(guard, "1771657050 264 0");
                assert!(lines.is_empty());
            }
            other => panic!("expected Response, got {:?}", other),
        }

        assert_eq!(events[2], TmuxEvent::WindowAdd { window_id: 0 });
        assert_eq!(events[3], TmuxEvent::SessionsChanged);
        assert_eq!(
            events[4],
            TmuxEvent::SessionChanged {
                session_id: 0,
                name: "0".to_string(),
            }
        );

        // Response with one line of session listing.
        match &events[5] {
            TmuxEvent::Response { guard, lines } => {
                assert_eq!(guard, "1771657050 270 1");
                assert_eq!(lines.len(), 1);
                assert!(lines[0].starts_with("0: bash*"));
            }
            other => panic!("expected Response, got {:?}", other),
        }

        // Output event with unescaped data.
        match &events[6] {
            TmuxEvent::Output { pane_id, data } => {
                assert_eq!(*pane_id, 0);
                // Should start with ESC[?2004h
                assert_eq!(data[0], 0x1b);
                assert_eq!(&data[1..8], b"[?2004h");
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }
}
