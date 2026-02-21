---
name: Rust Terminal Emulation Patterns
description: This skill provides guidance on terminal emulation patterns, VT parsing, PTY management, and GPU rendering for the iterm2-rs project. Use when working on terminal emulation, adding escape sequence handling, PTY operations, or renderer improvements.
version: 1.0.0
---

# Rust Terminal Emulation Patterns

## Terminal Emulation Stack

```
User Input (winit) → Input Handler → PTY Write
PTY Read → VT Parser (alacritty_terminal) → Terminal State
Terminal State → Renderer (wgpu + glyphon) → Screen
```

## Key Libraries and Their Roles

### alacritty_terminal (VT emulation)
- Provides `Term<T>` — the terminal state machine
- Handles VT100/VT220 escape sequences
- Manages screen buffer, cursor, scrollback
- Used in `crates/terminal/src/alacritty.rs`

### vte (low-level VT parsing)
- Byte-level VT sequence parser
- Used for custom sequence handling beyond alacritty_terminal
- Parser state machine with Perform trait

### portable-pty (PTY management)
- Cross-platform PTY abstraction
- ConPTY backend on Windows
- Used in `crates/terminal/src/pty.rs`

### wgpu (GPU rendering)
- WebGPU-based rendering backend
- Device, surface, and render pipeline management
- Used in `crates/renderer/src/gpu.rs`

### glyphon + cosmic-text (text rendering)
- Font loading and shaping via cosmic-text FontSystem
- GPU text rendering via glyphon TextRenderer
- Used in `crates/renderer/src/text.rs`

## Crate Interaction Patterns

### Terminal → Renderer
The renderer crate depends on the terminal crate to read terminal state:
- Grid content (characters, attributes, colors)
- Cursor position and style
- Selection state
- Scrollback buffer

### Main App → All Crates
The iterm2-rs crate orchestrates:
- Window creation (winit) → passes to renderer
- PTY spawning → passes to terminal
- Config loading → distributes to all crates
- Event loop → dispatches input/resize/render

## Split Pane Architecture
- `crates/terminal/src/pane.rs` — Individual terminal pane (owns PTY + terminal state)
- `crates/terminal/src/tab.rs` — Tab containing pane layout tree
- Layout is a binary tree of horizontal/vertical splits

## Tmux Control Mode
- `crates/tmux/src/parser.rs` — Parses tmux control mode output
- `crates/tmux/src/events.rs` — Tmux event types
- `crates/tmux/src/session.rs` — Session and window management
- Protocol: Line-based, commands prefixed with `%`

## Anti-Patterns to Avoid

- Do NOT block the winit event loop with synchronous PTY reads
- Do NOT allocate per-frame in the render loop — reuse buffers
- Do NOT mix anyhow and thiserror in the same crate
- Do NOT handle raw VT sequences if alacritty_terminal already handles them
- Do NOT create new windows outside the winit event loop
