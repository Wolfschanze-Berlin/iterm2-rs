---
name: iterm2-rs Project Patterns
description: This skill provides guidance on workspace conventions, crate architecture, dependency management, and coding patterns for the iterm2-rs terminal emulator. Use when working on any crate in the workspace, adding dependencies, or creating new modules.
version: 1.0.0
---

# iterm2-rs Project Patterns

## Workspace Architecture

```
iterm2-rs/
├── Cargo.toml              # Workspace root (defines members + shared deps)
├── .cargo/config.toml      # Build config (rust-lld, sccache, 16 jobs)
├── crates/
│   ├── iterm2-rs/          # Main binary — app orchestration
│   │   └── src/            # main.rs, config.rs, clipboard.rs, profiles.rs
│   ├── terminal/           # Library — VT emulation, PTY, input, panes
│   │   └── src/            # lib.rs, alacritty.rs, backend.rs, input.rs, ...
│   ├── renderer/           # Library — GPU rendering, text, windows
│   │   └── src/            # lib.rs, gpu.rs, text.rs, terminal_renderer.rs, window.rs
│   └── tmux/               # Library — Tmux control mode protocol
│       └── src/            # lib.rs, events.rs, parser.rs, session.rs
```

## Dependency Management

All shared dependencies are declared in the workspace `Cargo.toml` under `[workspace.dependencies]`. Crates reference them with `workspace = true`:

```toml
# In workspace Cargo.toml
[workspace.dependencies]
log = "0.4"

# In crate Cargo.toml
[dependencies]
log = { workspace = true }
```

## Error Handling Pattern

- **Main crate (iterm2-rs)**: Use `anyhow::Result` for application-level errors
- **Library crates (terminal, renderer, tmux)**: Define custom errors with `thiserror`

```rust
// Library crate pattern
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("PTY error: {0}")]
    Pty(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}
```

## Commit Convention

Format: `{emoji} {type}({scope}): {description} (#{issue})`

| Emoji | Type | Usage |
|-------|------|-------|
| ✨ | feat | New features |
| 🐛 | fix | Bug fixes |
| 🔧 | chore | Config, tooling |
| 🏗️ | chore | Architecture changes |
| ♻️ | refactor | Code restructuring |
| 📝 | docs | Documentation |
| ✅ | test | Tests |

## Testing Pattern

Inline tests within source files:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_name() {
        // test body
    }
}
```

Run: `cargo test --workspace`

## Anti-Patterns to Avoid

- Do NOT add `build.rs` files — the project intentionally avoids them
- Do NOT use external test frameworks — use built-in `#[test]`
- Do NOT add features/feature flags without discussion
- Do NOT declare dependencies at crate level if they could be workspace-level
- Do NOT use `unwrap()` in library crates — return proper errors
