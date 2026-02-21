//! Bridge between the terminal emulation backend and the text renderer.
//!
//! Extracts text from `AlacrittyBackend::renderable_content()` into lines of
//! strings that can be fed to `TextRenderer::set_lines()`.

use terminal::AlacrittyBackend;
use terminal::TerminalBackend;

/// Extract the visible terminal grid as a `Vec<String>`, one entry per row.
///
/// Iterates the renderable content from the alacritty backend, collecting
/// characters into per-line strings. Empty trailing cells are represented as
/// spaces so that cursor positioning works correctly.
pub fn extract_grid_text(backend: &AlacrittyBackend) -> Vec<String> {
    let content = backend.renderable_content();

    // Determine grid dimensions from the underlying term.
    let (cols, rows) = backend.size();
    let cols = cols as usize;
    let rows = rows as usize;

    // Pre-fill a grid of spaces.
    let mut grid: Vec<Vec<char>> = vec![vec![' '; cols]; rows];

    for indexed in content.display_iter {
        let line = indexed.point.line.0;
        let col = indexed.point.column.0;

        // display_iter uses Line(0) for the first visible line.
        if line >= 0 && (line as usize) < rows && col < cols {
            let c = indexed.cell.c;
            // Replace control/null characters with a space.
            if !c.is_control() && c != '\0' {
                grid[line as usize][col] = c;
            }
        }
    }

    // Convert each row into a String, trimming trailing spaces for cleaner output.
    grid.into_iter()
        .map(|row| {
            let s: String = row.into_iter().collect();
            s.trim_end().to_string()
        })
        .collect()
}
