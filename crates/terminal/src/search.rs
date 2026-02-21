//! Scrollback search for the terminal grid.
//!
//! Provides substring search (case-insensitive) across all lines in the
//! terminal, including scrollback history above the visible viewport.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::Term;

/// Direction in which to advance through matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// A single match within the terminal grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Line index (same coordinate system as `alacritty_terminal::index::Line`).
    /// Negative values represent scrollback history; 0 is the topmost visible
    /// line.
    pub line: i32,
    /// Starting column of the match (inclusive).
    pub start_col: usize,
    /// Ending column of the match (exclusive).
    pub end_col: usize,
}

/// Persistent search state that tracks the current query, collected matches,
/// and the highlighted (current) match index.
#[derive(Debug, Clone)]
pub struct SearchState {
    /// The active search query.
    pub query: String,
    /// All matches found by the last search invocation.
    pub matches: Vec<SearchMatch>,
    /// Index into `matches` for the currently highlighted match.
    pub current_match: Option<usize>,
    /// Direction used when advancing with `next_match` / `prev_match`.
    pub direction: SearchDirection,
}

impl SearchState {
    /// Create a new, empty search state.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current_match: None,
            direction: SearchDirection::Forward,
        }
    }

    /// Search through every line of the terminal grid (including scrollback)
    /// for case-insensitive substring matches of `query`.
    ///
    /// The results are stored in `self.matches` and `self.current_match` is
    /// reset to the first match (if any). The query is also saved in
    /// `self.query`.
    pub fn search<E: EventListener>(&mut self, term: &Term<E>, query: &str) -> Vec<SearchMatch> {
        self.query = query.to_string();
        self.matches.clear();
        self.current_match = None;

        if query.is_empty() {
            return Vec::new();
        }

        let grid = term.grid();
        let topmost = grid.topmost_line();
        let bottommost = grid.bottommost_line();
        let columns = grid.columns();

        let query_lower = query.to_lowercase();

        let mut line_idx = topmost.0;
        while line_idx <= bottommost.0 {
            // Collect all characters on this line into a String.
            let mut line_text = String::with_capacity(columns);
            for col in 0..columns {
                let cell = &grid[Line(line_idx)][Column(col)];
                line_text.push(cell.c);
            }

            let line_lower = line_text.to_lowercase();

            // Find all non-overlapping occurrences.
            let mut search_start = 0;
            while let Some(pos) = line_lower[search_start..].find(&query_lower) {
                let abs_pos = search_start + pos;
                self.matches.push(SearchMatch {
                    line: line_idx,
                    start_col: abs_pos,
                    end_col: abs_pos + query.len(),
                });
                search_start = abs_pos + query.len();
            }

            line_idx += 1;
        }

        if !self.matches.is_empty() {
            self.current_match = Some(0);
        }

        self.matches.clone()
    }

    /// Advance to the next match (wrapping around at the end).
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current_match = Some(match self.current_match {
            Some(idx) => (idx + 1) % self.matches.len(),
            None => 0,
        });
    }

    /// Move to the previous match (wrapping around at the beginning).
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current_match = Some(match self.current_match {
            Some(0) => self.matches.len() - 1,
            Some(idx) => idx - 1,
            None => self.matches.len() - 1,
        });
    }

    /// Return a reference to the currently highlighted match, if any.
    pub fn current(&self) -> Option<&SearchMatch> {
        self.current_match.and_then(|idx| self.matches.get(idx))
    }

    /// Reset the search state entirely.
    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current_match = None;
    }
}

impl Default for SearchState {
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
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    /// Minimal event listener used for testing.
    #[derive(Clone, Debug)]
    struct TestEventProxy;

    impl EventListener for TestEventProxy {
        fn send_event(&self, _event: Event) {}
    }

    struct TestSize {
        cols: usize,
        rows: usize,
    }

    impl Dimensions for TestSize {
        fn total_lines(&self) -> usize {
            self.rows
        }
        fn screen_lines(&self) -> usize {
            self.rows
        }
        fn columns(&self) -> usize {
            self.cols
        }
    }

    /// Helper: create a term and feed it some bytes.
    fn make_term(cols: u16, rows: u16, input: &[u8]) -> Term<TestEventProxy> {
        let size = TestSize { cols: cols as usize, rows: rows as usize };
        let config = Config::default();
        let mut term = Term::new(config, &size, TestEventProxy);
        let mut proc: Processor = Processor::new();
        proc.advance(&mut term, input);
        term
    }

    #[test]
    fn empty_query_returns_no_matches() {
        let term = make_term(80, 24, b"hello world");
        let mut state = SearchState::new();
        let matches = state.search(&term, "");
        assert!(matches.is_empty());
        assert!(state.current_match.is_none());
    }

    #[test]
    fn query_not_found_returns_no_matches() {
        let term = make_term(80, 24, b"hello world");
        let mut state = SearchState::new();
        let matches = state.search(&term, "foobar");
        assert!(matches.is_empty());
        assert!(state.current_match.is_none());
    }

    #[test]
    fn basic_string_matching() {
        let term = make_term(80, 24, b"hello world");
        let mut state = SearchState::new();
        let matches = state.search(&term, "hello");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 0);
        assert_eq!(matches[0].end_col, 5);
    }

    #[test]
    fn case_insensitive_search() {
        let term = make_term(80, 24, b"Hello World");
        let mut state = SearchState::new();

        let matches = state.search(&term, "hello");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 0);

        let matches = state.search(&term, "WORLD");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 6);
    }

    #[test]
    fn multiple_matches_on_same_line() {
        let term = make_term(80, 24, b"abcabc");
        let mut state = SearchState::new();
        let matches = state.search(&term, "abc");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start_col, 0);
        assert_eq!(matches[0].end_col, 3);
        assert_eq!(matches[1].start_col, 3);
        assert_eq!(matches[1].end_col, 6);
    }

    #[test]
    fn next_match_cycles_forward() {
        let term = make_term(80, 24, b"aaa");
        let mut state = SearchState::new();
        state.search(&term, "a");
        assert_eq!(state.matches.len(), 3);

        assert_eq!(state.current_match, Some(0));
        state.next_match();
        assert_eq!(state.current_match, Some(1));
        state.next_match();
        assert_eq!(state.current_match, Some(2));
        // Wraps around.
        state.next_match();
        assert_eq!(state.current_match, Some(0));
    }

    #[test]
    fn prev_match_cycles_backward() {
        let term = make_term(80, 24, b"aaa");
        let mut state = SearchState::new();
        state.search(&term, "a");
        assert_eq!(state.matches.len(), 3);

        assert_eq!(state.current_match, Some(0));
        // Wraps to end.
        state.prev_match();
        assert_eq!(state.current_match, Some(2));
        state.prev_match();
        assert_eq!(state.current_match, Some(1));
        state.prev_match();
        assert_eq!(state.current_match, Some(0));
    }

    #[test]
    fn current_returns_highlighted_match() {
        let term = make_term(80, 24, b"hello world");
        let mut state = SearchState::new();
        state.search(&term, "world");
        let current = state.current().expect("should have a current match");
        assert_eq!(current.start_col, 6);
        assert_eq!(current.end_col, 11);
    }

    #[test]
    fn clear_resets_everything() {
        let term = make_term(80, 24, b"hello");
        let mut state = SearchState::new();
        state.search(&term, "hello");
        assert!(!state.matches.is_empty());

        state.clear();
        assert!(state.query.is_empty());
        assert!(state.matches.is_empty());
        assert!(state.current_match.is_none());
    }
}
