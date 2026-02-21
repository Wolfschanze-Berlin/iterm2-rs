//! Alacritty-based terminal backend.
//!
//! Wraps `alacritty_terminal::Term` behind the `TerminalBackend` trait.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::Term;

use crate::backend::TerminalBackend;

/// Minimal event listener that logs terminal events.
#[derive(Clone, Debug)]
pub struct EventProxy;

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        log::debug!("Terminal event: {:?}", event);
    }
}

/// A simple `Dimensions` implementation for creating and resizing the terminal.
struct TerminalSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Terminal backend powered by `alacritty_terminal`.
pub struct AlacrittyBackend {
    term: Term<EventProxy>,
    processor: Processor,
}

impl AlacrittyBackend {
    /// Create a new `AlacrittyBackend` with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TerminalSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        };
        let config = Config::default();
        let event_proxy = EventProxy;
        let term = Term::new(config, &size, event_proxy);
        let processor = Processor::new();

        Self { term, processor }
    }

    /// Returns renderable content for the terminal's current state.
    pub fn renderable_content(&self) -> alacritty_terminal::term::RenderableContent<'_> {
        self.term.renderable_content()
    }

    /// Accessor for the underlying `Term`.
    pub fn term(&self) -> &Term<EventProxy> {
        &self.term
    }
}

impl TerminalBackend for AlacrittyBackend {
    fn process_bytes(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    fn size(&self) -> (u16, u16) {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        (cols, rows)
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let size = TerminalSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        };
        self.term.resize(size);
    }
}
