//! Shell integration via OSC 133 (FinalTerm protocol).
//!
//! Detects semantic zones in terminal output by parsing OSC 133 escape sequences:
//! - `\x1b]133;A\x07` -- Prompt start
//! - `\x1b]133;B\x07` -- Command start (user pressed Enter)
//! - `\x1b]133;C\x07` -- Command output start
//! - `\x1b]133;D;{exit_code}\x07` -- Command end with exit code
//!
//! This allows the terminal to know where prompts, commands, and their output
//! begin and end, enabling features like prompt navigation and command history.

/// Semantic zone within shell output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticZone {
    /// Between A and B -- the shell prompt.
    Prompt,
    /// Between B and C -- the typed command.
    Command,
    /// Between C and D -- command output.
    Output,
    /// No active zone.
    None,
}

/// A completed command record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    /// Line where the prompt was displayed.
    pub prompt_line: usize,
    /// Line where the command was typed.
    pub command_line: usize,
    /// Line where output began.
    pub output_start_line: usize,
    /// Line where output ended.
    pub output_end_line: usize,
    /// Exit code from the D sequence, if provided.
    pub exit_code: Option<i32>,
}

/// Events emitted by the OSC parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    /// OSC 133;A -- prompt start.
    PromptStart,
    /// OSC 133;B -- command start.
    CommandStart,
    /// OSC 133;C -- output start.
    OutputStart,
    /// OSC 133;D -- command end, optionally with exit code.
    CommandEnd { exit_code: Option<i32> },
    /// Non-OSC passthrough data.
    Data(Vec<u8>),
}

/// Internal states for the OSC parser state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OscParseState {
    /// Normal data passthrough.
    Normal,
    /// Saw ESC (0x1B), waiting for next byte.
    Escape,
    /// Inside an OSC sequence (saw ESC ]), accumulating body.
    OscBody,
    /// Inside an OSC body, saw ESC -- waiting for backslash (ST terminator).
    OscBodyEscape,
}

/// Parser that detects OSC 133 sequences in a byte stream.
///
/// Handles byte-at-a-time feeding so partial sequences that span multiple
/// `feed()` calls are correctly reassembled.
pub struct OscParser {
    state: OscParseState,
    buffer: Vec<u8>,
    /// Accumulates non-OSC data bytes to be emitted as `Data` events.
    data_buffer: Vec<u8>,
}

impl OscParser {
    /// Create a new parser in the initial state.
    pub fn new() -> Self {
        Self {
            state: OscParseState::Normal,
            buffer: Vec::new(),
            data_buffer: Vec::new(),
        }
    }

    /// Feed bytes into the parser, returning any events detected.
    ///
    /// OSC 133 sequences produce `ShellEvent` variants; all other bytes
    /// pass through as `ShellEvent::Data`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ShellEvent> {
        let mut events = Vec::new();

        for &byte in bytes {
            match self.state {
                OscParseState::Normal => {
                    if byte == 0x1B {
                        // ESC -- might be start of an OSC sequence.
                        self.state = OscParseState::Escape;
                    } else {
                        self.data_buffer.push(byte);
                    }
                }
                OscParseState::Escape => {
                    if byte == b']' {
                        // ESC ] -- this is an OSC introducer.
                        // Flush any pending data before starting OSC.
                        self.flush_data(&mut events);
                        self.state = OscParseState::OscBody;
                        self.buffer.clear();
                    } else if byte == 0x1B {
                        // Another ESC -- the previous ESC was not part of an OSC.
                        // Emit the previous ESC as data, stay in Escape state.
                        self.data_buffer.push(0x1B);
                    } else {
                        // Not an OSC sequence. Emit the ESC and this byte as data.
                        self.data_buffer.push(0x1B);
                        self.data_buffer.push(byte);
                        self.state = OscParseState::Normal;
                    }
                }
                OscParseState::OscBody => {
                    if byte == 0x07 {
                        // BEL terminates the OSC sequence.
                        if let Some(event) = self.parse_osc_body() {
                            events.push(event);
                        }
                        self.buffer.clear();
                        self.state = OscParseState::Normal;
                    } else if byte == 0x1B {
                        // Might be start of ST (ESC \).
                        self.state = OscParseState::OscBodyEscape;
                    } else {
                        self.buffer.push(byte);
                    }
                }
                OscParseState::OscBodyEscape => {
                    if byte == b'\\' {
                        // ST (ESC \) terminates the OSC sequence.
                        if let Some(event) = self.parse_osc_body() {
                            events.push(event);
                        }
                        self.buffer.clear();
                        self.state = OscParseState::Normal;
                    } else {
                        // Not ST. The ESC was part of the body (unusual but handle it).
                        self.buffer.push(0x1B);
                        self.buffer.push(byte);
                        self.state = OscParseState::OscBody;
                    }
                }
            }
        }

        // Flush any remaining data.
        self.flush_data(&mut events);

        events
    }

    /// Flush accumulated non-OSC data bytes as a `Data` event.
    fn flush_data(&mut self, events: &mut Vec<ShellEvent>) {
        if !self.data_buffer.is_empty() {
            events.push(ShellEvent::Data(std::mem::take(&mut self.data_buffer)));
        }
    }

    /// Try to parse the accumulated OSC body as an OSC 133 sequence.
    /// Returns `None` if this is not an OSC 133 sequence we recognize.
    fn parse_osc_body(&self) -> Option<ShellEvent> {
        let body = std::str::from_utf8(&self.buffer).ok()?;

        if !body.starts_with("133;") {
            return None;
        }

        let payload = &body[4..];

        if payload == "A" {
            Some(ShellEvent::PromptStart)
        } else if payload == "B" {
            Some(ShellEvent::CommandStart)
        } else if payload == "C" {
            Some(ShellEvent::OutputStart)
        } else if payload == "D" {
            Some(ShellEvent::CommandEnd { exit_code: None })
        } else if let Some(code_str) = payload.strip_prefix("D;") {
            let exit_code = code_str.trim().parse::<i32>().ok();
            Some(ShellEvent::CommandEnd { exit_code })
        } else {
            // Unrecognized OSC 133 sub-command; ignore.
            None
        }
    }
}

impl Default for OscParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks shell integration state by processing `ShellEvent`s.
pub struct ShellIntegration {
    current_zone: SemanticZone,
    prompt_line: Option<usize>,
    command_start_line: Option<usize>,
    output_start_line: Option<usize>,
    history: Vec<CommandRecord>,
}

impl ShellIntegration {
    /// Create a new instance with no active zone.
    pub fn new() -> Self {
        Self {
            current_zone: SemanticZone::None,
            prompt_line: None,
            command_start_line: None,
            output_start_line: None,
            history: Vec::new(),
        }
    }

    /// Update internal state based on a shell event.
    pub fn handle_event(&mut self, event: &ShellEvent, current_line: usize) {
        match event {
            ShellEvent::PromptStart => {
                self.current_zone = SemanticZone::Prompt;
                self.prompt_line = Some(current_line);
                self.command_start_line = None;
                self.output_start_line = None;
            }
            ShellEvent::CommandStart => {
                self.current_zone = SemanticZone::Command;
                self.command_start_line = Some(current_line);
            }
            ShellEvent::OutputStart => {
                self.current_zone = SemanticZone::Output;
                self.output_start_line = Some(current_line);
            }
            ShellEvent::CommandEnd { exit_code } => {
                if let (Some(prompt_line), Some(command_line), Some(output_start)) =
                    (self.prompt_line, self.command_start_line, self.output_start_line)
                {
                    self.history.push(CommandRecord {
                        prompt_line,
                        command_line,
                        output_start_line: output_start,
                        output_end_line: current_line,
                        exit_code: *exit_code,
                    });
                }
                self.current_zone = SemanticZone::None;
                self.prompt_line = None;
                self.command_start_line = None;
                self.output_start_line = None;
            }
            ShellEvent::Data(_) => {
                // Data events don't change zone state.
            }
        }
    }

    /// The current semantic zone.
    pub fn current_zone(&self) -> &SemanticZone {
        &self.current_zone
    }

    /// All completed command records.
    pub fn history(&self) -> &[CommandRecord] {
        &self.history
    }

    /// The most recently completed command, if any.
    pub fn last_command(&self) -> Option<&CommandRecord> {
        self.history.last()
    }

    /// Return the prompt line of the command record before the most recent one
    /// that has a prompt_line less than the current prompt_line (or output_end_line
    /// if no current prompt). Useful for "jump to previous prompt" navigation.
    pub fn navigate_to_prev_prompt(&self) -> Option<usize> {
        if self.history.len() < 2 {
            return None;
        }
        // The last entry in history is the most recent completed command.
        // The second-to-last is the "previous" one.
        let idx = self.history.len() - 2;
        Some(self.history[idx].prompt_line)
    }

    /// Return the prompt line of the next prompt after the oldest completed
    /// command. If we only have one command in history and a current prompt
    /// is active, return that prompt line.
    pub fn navigate_to_next_prompt(&self) -> Option<usize> {
        if self.history.len() >= 2 {
            // Return the prompt line of the last completed command.
            return Some(self.history.last().unwrap().prompt_line);
        }
        // If there is a current prompt active, navigate to it.
        if self.current_zone == SemanticZone::Prompt
            || self.current_zone == SemanticZone::Command
        {
            return self.prompt_line;
        }
        None
    }
}

impl Default for ShellIntegration {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // OscParser tests
    // ---------------------------------------------------------------

    #[test]
    fn parse_prompt_start() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;A\x07");
        assert_eq!(events, vec![ShellEvent::PromptStart]);
    }

    #[test]
    fn parse_command_start() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;B\x07");
        assert_eq!(events, vec![ShellEvent::CommandStart]);
    }

    #[test]
    fn parse_output_start() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;C\x07");
        assert_eq!(events, vec![ShellEvent::OutputStart]);
    }

    #[test]
    fn parse_command_end_with_exit_code() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;D;0\x07");
        assert_eq!(
            events,
            vec![ShellEvent::CommandEnd {
                exit_code: Some(0)
            }]
        );
    }

    #[test]
    fn parse_command_end_without_exit_code() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;D\x07");
        assert_eq!(
            events,
            vec![ShellEvent::CommandEnd { exit_code: None }]
        );
    }

    #[test]
    fn parse_command_end_nonzero_exit_code() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;D;127\x07");
        assert_eq!(
            events,
            vec![ShellEvent::CommandEnd {
                exit_code: Some(127)
            }]
        );
    }

    #[test]
    fn parse_st_terminator() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b]133;A\x1b\\");
        assert_eq!(events, vec![ShellEvent::PromptStart]);
    }

    #[test]
    fn non_osc_data_passthrough() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"hello world");
        assert_eq!(events, vec![ShellEvent::Data(b"hello world".to_vec())]);
    }

    #[test]
    fn data_before_and_after_osc() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"before\x1b]133;A\x07after");
        assert_eq!(
            events,
            vec![
                ShellEvent::Data(b"before".to_vec()),
                ShellEvent::PromptStart,
                ShellEvent::Data(b"after".to_vec()),
            ]
        );
    }

    #[test]
    fn split_across_feeds() {
        let mut parser = OscParser::new();

        // Split the sequence "\x1b]133;A\x07" across three feed calls.
        let e1 = parser.feed(b"\x1b");
        assert_eq!(e1, vec![]);

        let e2 = parser.feed(b"]133;");
        assert_eq!(e2, vec![]);

        let e3 = parser.feed(b"A\x07");
        assert_eq!(e3, vec![ShellEvent::PromptStart]);
    }

    #[test]
    fn split_across_feeds_mid_body() {
        let mut parser = OscParser::new();

        let e1 = parser.feed(b"\x1b]133");
        assert_eq!(e1, vec![]);

        let e2 = parser.feed(b";D;42\x07");
        assert_eq!(
            e2,
            vec![ShellEvent::CommandEnd {
                exit_code: Some(42)
            }]
        );
    }

    #[test]
    fn split_st_terminator_across_feeds() {
        let mut parser = OscParser::new();

        let e1 = parser.feed(b"\x1b]133;B\x1b");
        assert_eq!(e1, vec![]);

        let e2 = parser.feed(b"\\");
        assert_eq!(e2, vec![ShellEvent::CommandStart]);
    }

    #[test]
    fn non_133_osc_ignored() {
        let mut parser = OscParser::new();
        // OSC 0 (set window title) should not produce a ShellEvent variant.
        let events = parser.feed(b"\x1b]0;My Title\x07");
        assert_eq!(events, vec![]);
    }

    #[test]
    fn full_sequence_a_b_c_d() {
        let mut parser = OscParser::new();
        let input = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\n\x1b]133;C\x07file1 file2\n\x1b]133;D;0\x07";
        let events = parser.feed(input);
        assert_eq!(
            events,
            vec![
                ShellEvent::PromptStart,
                ShellEvent::Data(b"$ ".to_vec()),
                ShellEvent::CommandStart,
                ShellEvent::Data(b"ls\n".to_vec()),
                ShellEvent::OutputStart,
                ShellEvent::Data(b"file1 file2\n".to_vec()),
                ShellEvent::CommandEnd {
                    exit_code: Some(0)
                },
            ]
        );
    }

    #[test]
    fn incomplete_sequence_no_spurious_events() {
        let mut parser = OscParser::new();
        // Feed an incomplete OSC -- no terminator.
        let events = parser.feed(b"\x1b]133;A");
        // Should produce no events (data is buffered in the OSC body).
        assert_eq!(events, vec![]);
    }

    #[test]
    fn bare_esc_not_followed_by_bracket() {
        let mut parser = OscParser::new();
        // ESC followed by a normal character (e.g., ESC [ for CSI).
        let events = parser.feed(b"\x1b[31mred\x1b[0m");
        // All bytes should pass through as data.
        assert_eq!(
            events,
            vec![ShellEvent::Data(b"\x1b[31mred\x1b[0m".to_vec())]
        );
    }

    #[test]
    fn consecutive_esc_bytes() {
        let mut parser = OscParser::new();
        let events = parser.feed(b"\x1b\x1b]133;A\x07");
        // First ESC is not followed by ], so emitted as data.
        // Second ESC ] starts the OSC.
        assert_eq!(
            events,
            vec![
                ShellEvent::Data(vec![0x1B]),
                ShellEvent::PromptStart,
            ]
        );
    }

    // ---------------------------------------------------------------
    // ShellIntegration tests
    // ---------------------------------------------------------------

    #[test]
    fn initial_zone_is_none() {
        let si = ShellIntegration::new();
        assert_eq!(*si.current_zone(), SemanticZone::None);
        assert!(si.history().is_empty());
        assert!(si.last_command().is_none());
    }

    #[test]
    fn zone_transitions() {
        let mut si = ShellIntegration::new();

        si.handle_event(&ShellEvent::PromptStart, 0);
        assert_eq!(*si.current_zone(), SemanticZone::Prompt);

        si.handle_event(&ShellEvent::CommandStart, 0);
        assert_eq!(*si.current_zone(), SemanticZone::Command);

        si.handle_event(&ShellEvent::OutputStart, 1);
        assert_eq!(*si.current_zone(), SemanticZone::Output);

        si.handle_event(
            &ShellEvent::CommandEnd {
                exit_code: Some(0),
            },
            5,
        );
        assert_eq!(*si.current_zone(), SemanticZone::None);
    }

    #[test]
    fn history_records_command() {
        let mut si = ShellIntegration::new();

        si.handle_event(&ShellEvent::PromptStart, 10);
        si.handle_event(&ShellEvent::CommandStart, 10);
        si.handle_event(&ShellEvent::OutputStart, 11);
        si.handle_event(
            &ShellEvent::CommandEnd {
                exit_code: Some(0),
            },
            20,
        );

        assert_eq!(si.history().len(), 1);
        let rec = &si.history()[0];
        assert_eq!(rec.prompt_line, 10);
        assert_eq!(rec.command_line, 10);
        assert_eq!(rec.output_start_line, 11);
        assert_eq!(rec.output_end_line, 20);
        assert_eq!(rec.exit_code, Some(0));
    }

    #[test]
    fn last_command_returns_most_recent() {
        let mut si = ShellIntegration::new();

        // First command.
        si.handle_event(&ShellEvent::PromptStart, 0);
        si.handle_event(&ShellEvent::CommandStart, 0);
        si.handle_event(&ShellEvent::OutputStart, 1);
        si.handle_event(
            &ShellEvent::CommandEnd {
                exit_code: Some(0),
            },
            5,
        );

        // Second command.
        si.handle_event(&ShellEvent::PromptStart, 6);
        si.handle_event(&ShellEvent::CommandStart, 6);
        si.handle_event(&ShellEvent::OutputStart, 7);
        si.handle_event(
            &ShellEvent::CommandEnd {
                exit_code: Some(1),
            },
            10,
        );

        let last = si.last_command().unwrap();
        assert_eq!(last.exit_code, Some(1));
        assert_eq!(last.prompt_line, 6);
    }

    #[test]
    fn navigate_prev_prompt() {
        let mut si = ShellIntegration::new();

        // Command 1 at line 0.
        si.handle_event(&ShellEvent::PromptStart, 0);
        si.handle_event(&ShellEvent::CommandStart, 0);
        si.handle_event(&ShellEvent::OutputStart, 1);
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 5);

        // Command 2 at line 6.
        si.handle_event(&ShellEvent::PromptStart, 6);
        si.handle_event(&ShellEvent::CommandStart, 6);
        si.handle_event(&ShellEvent::OutputStart, 7);
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 10);

        // Previous prompt should be command 1's prompt line.
        assert_eq!(si.navigate_to_prev_prompt(), Some(0));
    }

    #[test]
    fn navigate_prev_prompt_not_enough_history() {
        let mut si = ShellIntegration::new();

        // Only one command.
        si.handle_event(&ShellEvent::PromptStart, 0);
        si.handle_event(&ShellEvent::CommandStart, 0);
        si.handle_event(&ShellEvent::OutputStart, 1);
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 5);

        assert_eq!(si.navigate_to_prev_prompt(), None);
    }

    #[test]
    fn navigate_next_prompt() {
        let mut si = ShellIntegration::new();

        // Command 1.
        si.handle_event(&ShellEvent::PromptStart, 0);
        si.handle_event(&ShellEvent::CommandStart, 0);
        si.handle_event(&ShellEvent::OutputStart, 1);
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 5);

        // Command 2.
        si.handle_event(&ShellEvent::PromptStart, 6);
        si.handle_event(&ShellEvent::CommandStart, 6);
        si.handle_event(&ShellEvent::OutputStart, 7);
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 10);

        assert_eq!(si.navigate_to_next_prompt(), Some(6));
    }

    #[test]
    fn navigate_next_prompt_to_active() {
        let mut si = ShellIntegration::new();

        // One completed command.
        si.handle_event(&ShellEvent::PromptStart, 0);
        si.handle_event(&ShellEvent::CommandStart, 0);
        si.handle_event(&ShellEvent::OutputStart, 1);
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 5);

        // New prompt is active but not yet completed.
        si.handle_event(&ShellEvent::PromptStart, 6);

        assert_eq!(si.navigate_to_next_prompt(), Some(6));
    }

    #[test]
    fn data_events_do_not_change_zone() {
        let mut si = ShellIntegration::new();

        si.handle_event(&ShellEvent::PromptStart, 0);
        assert_eq!(*si.current_zone(), SemanticZone::Prompt);

        si.handle_event(&ShellEvent::Data(b"$ ".to_vec()), 0);
        assert_eq!(*si.current_zone(), SemanticZone::Prompt);
    }

    #[test]
    fn command_end_without_full_sequence_does_not_record() {
        let mut si = ShellIntegration::new();

        // D without prior A/B/C should not create a record.
        si.handle_event(&ShellEvent::CommandEnd { exit_code: Some(0) }, 5);
        assert!(si.history().is_empty());
    }

    #[test]
    fn integration_parser_and_state() {
        let mut parser = OscParser::new();
        let mut si = ShellIntegration::new();
        let mut line = 0usize;

        let input = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\n\x1b]133;C\x07file1\nfile2\n\x1b]133;D;0\x07";
        let events = parser.feed(input);

        for event in &events {
            si.handle_event(event, line);
            // Advance line on newlines in data.
            if let ShellEvent::Data(data) = event {
                line += data.iter().filter(|&&b| b == b'\n').count();
            }
        }

        assert_eq!(si.history().len(), 1);
        let rec = si.last_command().unwrap();
        assert_eq!(rec.exit_code, Some(0));
        assert_eq!(*si.current_zone(), SemanticZone::None);
    }
}
