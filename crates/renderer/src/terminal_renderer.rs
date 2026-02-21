//! Bridge between the terminal emulation backend and the text renderer.
//!
//! Extracts text, per-cell foreground/background colors, and cursor info from
//! `AlacrittyBackend::renderable_content()`.

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use glyphon::cosmic_text::Color as CsColor;
use terminal::AlacCursorShape;
use terminal::AlacrittyBackend;
use terminal::TerminalBackend;

// ---------------------------------------------------------------------------
// Cursor types
// ---------------------------------------------------------------------------

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    HollowBlock,
    Underline,
    Beam,
    Hidden,
}

/// Information about the terminal cursor for rendering.
#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    /// Row index (0-based, from the top of the visible area).
    pub line: usize,
    /// Column index (0-based).
    pub col: usize,
    /// Cursor shape to render.
    pub shape: CursorShape,
}

// ---------------------------------------------------------------------------
// Styled cell types
// ---------------------------------------------------------------------------

/// A single cell with its character, resolved foreground color, and text attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyledCell {
    pub c: char,
    pub fg: CsColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// A batched background rectangle (consecutive same-colored cells on one line).
#[derive(Debug, Clone)]
pub struct BgRect {
    pub col: usize,
    pub line: usize,
    pub width: usize,
    pub color: (u8, u8, u8),
}

/// Complete extracted grid content: styled cells, cursor, and background rects.
pub struct GridContent {
    pub styled_lines: Vec<Vec<StyledCell>>,
    pub cursor: Option<CursorInfo>,
    pub bg_rects: Vec<BgRect>,
}

// ---------------------------------------------------------------------------
// Catppuccin Mocha palette
// ---------------------------------------------------------------------------

fn named_color_to_rgb(named: &NamedColor) -> (u8, u8, u8) {
    match named {
        NamedColor::Black => (0x45, 0x47, 0x5a),
        NamedColor::Red => (0xf3, 0x8b, 0xa8),
        NamedColor::Green => (0xa6, 0xe3, 0xa1),
        NamedColor::Yellow => (0xf9, 0xe2, 0xaf),
        NamedColor::Blue => (0x89, 0xb4, 0xfa),
        NamedColor::Magenta => (0xf5, 0xc2, 0xe7),
        NamedColor::Cyan => (0x94, 0xe2, 0xd5),
        NamedColor::White => (0xba, 0xc2, 0xde),
        NamedColor::BrightBlack => (0x58, 0x5b, 0x70),
        NamedColor::BrightRed => (0xf3, 0x8b, 0xa8),
        NamedColor::BrightGreen => (0xa6, 0xe3, 0xa1),
        NamedColor::BrightYellow => (0xf9, 0xe2, 0xaf),
        NamedColor::BrightBlue => (0x89, 0xb4, 0xfa),
        NamedColor::BrightMagenta => (0xf5, 0xc2, 0xe7),
        NamedColor::BrightCyan => (0x94, 0xe2, 0xd5),
        NamedColor::BrightWhite => (0xa6, 0xad, 0xc8),
        NamedColor::Foreground | NamedColor::BrightForeground => (0xcd, 0xd6, 0xf4),
        NamedColor::Background => (0x1e, 0x1e, 0x2e),
        NamedColor::Cursor => (0xf5, 0xe0, 0xdc),
        NamedColor::DimBlack => (0x45, 0x47, 0x5a),
        NamedColor::DimRed => (0xc0, 0x6f, 0x86),
        NamedColor::DimGreen => (0x85, 0xb6, 0x81),
        NamedColor::DimYellow => (0xc7, 0xb5, 0x8c),
        NamedColor::DimBlue => (0x6e, 0x90, 0xc8),
        NamedColor::DimMagenta => (0xc4, 0x9b, 0xb9),
        NamedColor::DimCyan => (0x76, 0xb5, 0xaa),
        NamedColor::DimWhite => (0x95, 0x9b, 0xb2),
        NamedColor::DimForeground => (0xa4, 0xab, 0xc3),
    }
}

fn indexed_color_to_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        0 => named_color_to_rgb(&NamedColor::Black),
        1 => named_color_to_rgb(&NamedColor::Red),
        2 => named_color_to_rgb(&NamedColor::Green),
        3 => named_color_to_rgb(&NamedColor::Yellow),
        4 => named_color_to_rgb(&NamedColor::Blue),
        5 => named_color_to_rgb(&NamedColor::Magenta),
        6 => named_color_to_rgb(&NamedColor::Cyan),
        7 => named_color_to_rgb(&NamedColor::White),
        8 => named_color_to_rgb(&NamedColor::BrightBlack),
        9 => named_color_to_rgb(&NamedColor::BrightRed),
        10 => named_color_to_rgb(&NamedColor::BrightGreen),
        11 => named_color_to_rgb(&NamedColor::BrightYellow),
        12 => named_color_to_rgb(&NamedColor::BrightBlue),
        13 => named_color_to_rgb(&NamedColor::BrightMagenta),
        14 => named_color_to_rgb(&NamedColor::BrightCyan),
        15 => named_color_to_rgb(&NamedColor::BrightWhite),
        16..=231 => {
            let idx = idx - 16;
            let r_idx = idx / 36;
            let g_idx = (idx % 36) / 6;
            let b_idx = idx % 6;
            let to_val = |i: u8| if i == 0 { 0 } else { 55 + 40 * i };
            (to_val(r_idx), to_val(g_idx), to_val(b_idx))
        }
        232..=255 => {
            let v = 8 + 10 * (idx - 232);
            (v, v, v)
        }
    }
}

/// Resolve an `alacritty_terminal` color to an `(r, g, b)` tuple.
pub fn resolve_color(color: &Color) -> (u8, u8, u8) {
    match color {
        Color::Spec(rgb) => (rgb.r, rgb.g, rgb.b),
        Color::Named(named) => named_color_to_rgb(named),
        Color::Indexed(idx) => indexed_color_to_rgb(*idx),
    }
}

/// Check if a background color is the default (should not be rendered as a rect).
fn is_default_bg(color: &Color) -> bool {
    match color {
        Color::Named(NamedColor::Background) => true,
        Color::Spec(rgb) => rgb.r == 0x1e && rgb.g == 0x1e && rgb.b == 0x2e,
        Color::Indexed(0) => false, // Black is not "default bg"
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Grid extraction
// ---------------------------------------------------------------------------

/// Extract the visible terminal grid with styled cells, cursor, and bg rects.
pub fn extract_grid(backend: &AlacrittyBackend) -> GridContent {
    let content = backend.renderable_content();

    // Extract cursor info.
    let cursor = {
        let rc = &content.cursor;
        let shape = match rc.shape {
            AlacCursorShape::Block => CursorShape::Block,
            AlacCursorShape::HollowBlock => CursorShape::HollowBlock,
            AlacCursorShape::Underline => CursorShape::Underline,
            AlacCursorShape::Beam => CursorShape::Beam,
            AlacCursorShape::Hidden => CursorShape::Hidden,
        };
        let line = rc.point.line.0;
        if shape != CursorShape::Hidden && line >= 0 {
            Some(CursorInfo {
                line: line as usize,
                col: rc.point.column.0,
                shape,
            })
        } else {
            None
        }
    };

    let (cols, rows) = backend.size();
    let cols = cols as usize;
    let rows = rows as usize;

    let default_fg = CsColor::rgb(0xcd, 0xd6, 0xf4);

    let default_cell = StyledCell {
        c: ' ',
        fg: default_fg,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
    };
    let mut styled_lines: Vec<Vec<StyledCell>> = vec![vec![default_cell; cols]; rows];

    // Collect bg colors per cell for batching.
    // None = default bg, Some((r,g,b)) = explicit bg.
    let mut bg_grid: Vec<Vec<Option<(u8, u8, u8)>>> = vec![vec![None; cols]; rows];

    for indexed in content.display_iter {
        let line = indexed.point.line.0;
        let col = indexed.point.column.0;

        if line >= 0 && (line as usize) < rows && col < cols {
            let li = line as usize;
            let c = indexed.cell.c;
            let (r, g, b) = resolve_color(&indexed.cell.fg);
            let flags = indexed.cell.flags;

            if !c.is_control() && c != '\0' {
                styled_lines[li][col] = StyledCell {
                    c,
                    fg: CsColor::rgb(r, g, b),
                    bold: flags.contains(CellFlags::BOLD),
                    italic: flags.contains(CellFlags::ITALIC),
                    underline: flags.contains(CellFlags::UNDERLINE),
                    strikethrough: flags.contains(CellFlags::STRIKEOUT),
                };
            }

            // Collect background color if non-default.
            if !is_default_bg(&indexed.cell.bg) {
                bg_grid[li][col] = Some(resolve_color(&indexed.cell.bg));
            }
        }
    }

    // Batch bg cells into rectangles (merge consecutive same-colored cells per line).
    let mut bg_rects = Vec::new();
    for (line_idx, row) in bg_grid.iter().enumerate() {
        let mut col = 0;
        while col < cols {
            if let Some(color) = row[col] {
                let start_col = col;
                col += 1;
                while col < cols && row[col] == Some(color) {
                    col += 1;
                }
                bg_rects.push(BgRect {
                    col: start_col,
                    line: line_idx,
                    width: col - start_col,
                    color,
                });
            } else {
                col += 1;
            }
        }
    }

    GridContent {
        styled_lines,
        cursor,
        bg_rects,
    }
}

/// Extract the visible terminal grid as a `Vec<String>`, one entry per row.
///
/// Convenience wrapper that discards color/cursor info. Kept for backward compat.
pub fn extract_grid_text(backend: &AlacrittyBackend) -> Vec<String> {
    extract_grid(backend)
        .styled_lines
        .into_iter()
        .map(|row| {
            let s: String = row.into_iter().map(|cell| cell.c).collect();
            s.trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_foreground_resolves_to_catppuccin_text() {
        let (r, g, b) = named_color_to_rgb(&NamedColor::Foreground);
        assert_eq!((r, g, b), (0xcd, 0xd6, 0xf4));
    }

    #[test]
    fn indexed_0_through_15_match_named() {
        assert_eq!(indexed_color_to_rgb(0), named_color_to_rgb(&NamedColor::Black));
        assert_eq!(indexed_color_to_rgb(9), named_color_to_rgb(&NamedColor::BrightRed));
        assert_eq!(indexed_color_to_rgb(15), named_color_to_rgb(&NamedColor::BrightWhite));
    }

    #[test]
    fn indexed_color_cube_origin() {
        assert_eq!(indexed_color_to_rgb(16), (0, 0, 0));
    }

    #[test]
    fn indexed_color_cube_max() {
        assert_eq!(indexed_color_to_rgb(231), (255, 255, 255));
    }

    #[test]
    fn indexed_grayscale_first() {
        assert_eq!(indexed_color_to_rgb(232), (8, 8, 8));
    }

    #[test]
    fn indexed_grayscale_last() {
        assert_eq!(indexed_color_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn resolve_spec_color() {
        let color = Color::Spec(alacritty_terminal::vte::ansi::Rgb { r: 100, g: 200, b: 50 });
        assert_eq!(resolve_color(&color), (100, 200, 50));
    }

    #[test]
    fn resolve_named_color() {
        let color = Color::Named(NamedColor::Red);
        assert_eq!(resolve_color(&color), (0xf3, 0x8b, 0xa8));
    }

    #[test]
    fn resolve_indexed_color() {
        let color = Color::Indexed(1);
        assert_eq!(resolve_color(&color), named_color_to_rgb(&NamedColor::Red));
    }

    #[test]
    fn default_bg_is_detected() {
        assert!(is_default_bg(&Color::Named(NamedColor::Background)));
        assert!(!is_default_bg(&Color::Named(NamedColor::Red)));
    }

    #[test]
    fn cursor_shape_equality() {
        assert_eq!(CursorShape::Hidden, CursorShape::Hidden);
        assert_ne!(CursorShape::Block, CursorShape::Hidden);
    }

    // -- extract_grid / extract_grid_text tests (#69) -------------------------

    #[test]
    fn extract_grid_empty_terminal_has_space_cells() {
        let backend = AlacrittyBackend::new(10, 3);
        let grid = extract_grid(&backend);

        assert_eq!(grid.styled_lines.len(), 3);
        for row in &grid.styled_lines {
            assert_eq!(row.len(), 10);
            for cell in row {
                assert_eq!(cell.c, ' ');
            }
        }
    }

    #[test]
    fn extract_grid_text_empty_terminal_returns_empty_strings() {
        let backend = AlacrittyBackend::new(10, 3);
        let lines = extract_grid_text(&backend);

        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(line.is_empty(), "Expected empty string, got: {:?}", line);
        }
    }

    #[test]
    fn extract_grid_after_simple_text() {
        let mut backend = AlacrittyBackend::new(20, 3);
        backend.process_bytes(b"hello");

        let grid = extract_grid(&backend);
        let first_row = &grid.styled_lines[0];
        let text: String = first_row.iter().take(5).map(|c| c.c).collect();
        assert_eq!(text, "hello");
    }

    #[test]
    fn extract_grid_text_trims_trailing_spaces() {
        let mut backend = AlacrittyBackend::new(20, 3);
        backend.process_bytes(b"hi");

        let lines = extract_grid_text(&backend);
        assert_eq!(lines[0], "hi");
    }

    #[test]
    fn extract_grid_control_chars_do_not_advance_cursor() {
        let mut backend = AlacrittyBackend::new(20, 3);
        // Control chars like SOH (\x01) are consumed by the VT parser without
        // advancing the cursor. So 'b' ends up at column 1, not column 2.
        backend.process_bytes(b"a\x01b");

        let grid = extract_grid(&backend);
        let first_row = &grid.styled_lines[0];
        assert_eq!(first_row[0].c, 'a');
        assert_eq!(first_row[1].c, 'b');
    }

    #[test]
    fn extract_grid_has_cursor_info() {
        let backend = AlacrittyBackend::new(10, 3);
        let grid = extract_grid(&backend);

        // Fresh terminal should have a cursor at (0, 0).
        assert!(grid.cursor.is_some());
        let cursor = grid.cursor.unwrap();
        assert_eq!(cursor.line, 0);
        assert_eq!(cursor.col, 0);
    }

    #[test]
    fn extract_grid_bg_rects_empty_for_default_bg() {
        let backend = AlacrittyBackend::new(10, 3);
        let grid = extract_grid(&backend);

        // No colored backgrounds on a fresh terminal.
        assert!(grid.bg_rects.is_empty());
    }
}
