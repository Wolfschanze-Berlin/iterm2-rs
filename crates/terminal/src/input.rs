//! Keyboard input translation from winit key events to VT/ANSI escape sequences.

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Translate a winit `KeyEvent` into the byte sequence that should be written to
/// the PTY. Returns `None` for events we don't handle (key releases, lone
/// modifier presses, etc.).
pub fn translate_key_event(event: &KeyEvent, modifiers: &ModifiersState) -> Option<Vec<u8>> {
    // Only handle key presses.
    if event.state != ElementState::Pressed {
        return None;
    }

    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();
    let shift = modifiers.shift_key();

    match &event.logical_key {
        // ----- Named (special) keys -----
        Key::Named(named) => translate_named(*named, ctrl, alt, shift),

        // ----- Character keys -----
        Key::Character(c) => translate_character(c, ctrl, alt),

        _ => None,
    }
}

// ── Named keys ──────────────────────────────────────────────────────────────

fn translate_named(key: NamedKey, ctrl: bool, alt: bool, shift: bool) -> Option<Vec<u8>> {
    // Simple named keys (no modifier encoding).
    match key {
        NamedKey::Enter => return Some(vec![0x0D]),
        NamedKey::Backspace => return Some(vec![0x7F]),
        NamedKey::Tab => {
            if shift {
                return Some(b"\x1b[Z".to_vec()); // back-tab
            }
            return Some(vec![0x09]);
        }
        NamedKey::Escape => return Some(vec![0x1B]),
        _ => {}
    }

    // Arrow keys — support modifier encoding.
    if let Some(suffix) = arrow_suffix(key) {
        let mod_param = modifier_param(shift, alt, ctrl);
        return if mod_param > 1 {
            Some(format!("\x1b[1;{}{}", mod_param, suffix).into_bytes())
        } else {
            Some(format!("\x1b[{}", suffix).into_bytes())
        };
    }

    // Home / End — same modifier encoding pattern as arrows.
    if let Some(suffix) = home_end_suffix(key) {
        let mod_param = modifier_param(shift, alt, ctrl);
        return if mod_param > 1 {
            Some(format!("\x1b[1;{}{}", mod_param, suffix).into_bytes())
        } else {
            Some(format!("\x1b[{}", suffix).into_bytes())
        };
    }

    // Tilde-style keys: Insert, Delete, PageUp, PageDown.
    if let Some(num) = tilde_key_number(key) {
        let mod_param = modifier_param(shift, alt, ctrl);
        return if mod_param > 1 {
            Some(format!("\x1b[{};{}~", num, mod_param).into_bytes())
        } else {
            Some(format!("\x1b[{}~", num).into_bytes())
        };
    }

    // Function keys F1-F12.
    if let Some(bytes) = function_key(key, shift, alt, ctrl) {
        return Some(bytes);
    }

    None
}

fn arrow_suffix(key: NamedKey) -> Option<char> {
    match key {
        NamedKey::ArrowUp => Some('A'),
        NamedKey::ArrowDown => Some('B'),
        NamedKey::ArrowRight => Some('C'),
        NamedKey::ArrowLeft => Some('D'),
        _ => None,
    }
}

fn home_end_suffix(key: NamedKey) -> Option<char> {
    match key {
        NamedKey::Home => Some('H'),
        NamedKey::End => Some('F'),
        _ => None,
    }
}

fn tilde_key_number(key: NamedKey) -> Option<u8> {
    match key {
        NamedKey::Insert => Some(2),
        NamedKey::Delete => Some(3),
        NamedKey::PageUp => Some(5),
        NamedKey::PageDown => Some(6),
        _ => None,
    }
}

/// xterm modifier parameter: 1 = none, 2 = Shift, 3 = Alt, 5 = Ctrl, and
/// combinations are additive (Shift+Ctrl = 6, etc.).
fn modifier_param(shift: bool, alt: bool, ctrl: bool) -> u8 {
    let mut m: u8 = 1;
    if shift {
        m += 1;
    }
    if alt {
        m += 2;
    }
    if ctrl {
        m += 4;
    }
    m
}

/// F1-F12 escape sequences (SS3 for F1-F4, CSI for F5-F12).
fn function_key(key: NamedKey, shift: bool, alt: bool, ctrl: bool) -> Option<Vec<u8>> {
    // F1-F4 use SS3 encoding when unmodified, CSI with modifier otherwise.
    let mod_param = modifier_param(shift, alt, ctrl);
    let has_mod = mod_param > 1;

    match key {
        NamedKey::F1 if !has_mod => Some(b"\x1bOP".to_vec()),
        NamedKey::F2 if !has_mod => Some(b"\x1bOQ".to_vec()),
        NamedKey::F3 if !has_mod => Some(b"\x1bOR".to_vec()),
        NamedKey::F4 if !has_mod => Some(b"\x1bOS".to_vec()),

        NamedKey::F1 => Some(format!("\x1b[1;{}P", mod_param).into_bytes()),
        NamedKey::F2 => Some(format!("\x1b[1;{}Q", mod_param).into_bytes()),
        NamedKey::F3 => Some(format!("\x1b[1;{}R", mod_param).into_bytes()),
        NamedKey::F4 => Some(format!("\x1b[1;{}S", mod_param).into_bytes()),

        NamedKey::F5 if !has_mod => Some(b"\x1b[15~".to_vec()),
        NamedKey::F6 if !has_mod => Some(b"\x1b[17~".to_vec()),
        NamedKey::F7 if !has_mod => Some(b"\x1b[18~".to_vec()),
        NamedKey::F8 if !has_mod => Some(b"\x1b[19~".to_vec()),
        NamedKey::F9 if !has_mod => Some(b"\x1b[20~".to_vec()),
        NamedKey::F10 if !has_mod => Some(b"\x1b[21~".to_vec()),
        NamedKey::F11 if !has_mod => Some(b"\x1b[23~".to_vec()),
        NamedKey::F12 if !has_mod => Some(b"\x1b[24~".to_vec()),

        NamedKey::F5 => Some(format!("\x1b[15;{}~", mod_param).into_bytes()),
        NamedKey::F6 => Some(format!("\x1b[17;{}~", mod_param).into_bytes()),
        NamedKey::F7 => Some(format!("\x1b[18;{}~", mod_param).into_bytes()),
        NamedKey::F8 => Some(format!("\x1b[19;{}~", mod_param).into_bytes()),
        NamedKey::F9 => Some(format!("\x1b[20;{}~", mod_param).into_bytes()),
        NamedKey::F10 => Some(format!("\x1b[21;{}~", mod_param).into_bytes()),
        NamedKey::F11 => Some(format!("\x1b[23;{}~", mod_param).into_bytes()),
        NamedKey::F12 => Some(format!("\x1b[24;{}~", mod_param).into_bytes()),

        _ => None,
    }
}

// ── Character keys ──────────────────────────────────────────────────────────

fn translate_character(c: &str, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    if ctrl {
        return translate_ctrl_character(c, alt);
    }

    let bytes = c.as_bytes();

    if alt {
        // Alt+key => ESC prefix followed by the character bytes.
        let mut out = vec![0x1B];
        out.extend_from_slice(bytes);
        return Some(out);
    }

    // Plain character — return its UTF-8 bytes.
    Some(bytes.to_vec())
}

fn translate_ctrl_character(c: &str, alt: bool) -> Option<Vec<u8>> {
    // Ctrl+[ => ESC
    if c == "[" {
        return Some(vec![0x1B]);
    }

    // Ctrl+letter => byte 1-26.
    let first = c.chars().next()?;
    if first.is_ascii_alphabetic() {
        let byte = (first.to_ascii_lowercase() as u8) - b'a' + 1;
        return if alt {
            Some(vec![0x1B, byte])
        } else {
            Some(vec![byte])
        };
    }

    None
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::{ElementState, KeyEvent};
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    /// Helper to build a pressed KeyEvent with a named key.
    fn named_event(key: NamedKey) -> KeyEvent {
        // SAFETY: KeyEventExtra is a simple struct with Option<SmolStr> and Key fields.
        // We cannot construct it directly because `platform_specific` is pub(crate).
        // Using zeroed memory is safe for these types (Option is repr-compatible with
        // nullable pointer, Key enum starts at 0).
        unsafe {
            let mut ev: KeyEvent = std::mem::zeroed();
            ev.physical_key = winit::keyboard::PhysicalKey::Unidentified(
                winit::keyboard::NativeKeyCode::Unidentified,
            );
            ev.logical_key = Key::Named(key);
            ev.text = None;
            ev.location = winit::keyboard::KeyLocation::Standard;
            ev.state = ElementState::Pressed;
            ev.repeat = false;
            ev
        }
    }

    /// Helper to build a pressed KeyEvent with a character key.
    fn char_event(c: &str) -> KeyEvent {
        unsafe {
            let mut ev: KeyEvent = std::mem::zeroed();
            ev.physical_key = winit::keyboard::PhysicalKey::Unidentified(
                winit::keyboard::NativeKeyCode::Unidentified,
            );
            ev.logical_key = Key::Character(c.into());
            ev.text = Some(c.into());
            ev.location = winit::keyboard::KeyLocation::Standard;
            ev.state = ElementState::Pressed;
            ev.repeat = false;
            ev
        }
    }

    fn no_mods() -> ModifiersState {
        ModifiersState::empty()
    }

    fn ctrl() -> ModifiersState {
        ModifiersState::CONTROL
    }

    fn shift() -> ModifiersState {
        ModifiersState::SHIFT
    }

    fn alt() -> ModifiersState {
        ModifiersState::ALT
    }

    // ── Basic keys ──

    #[test]
    fn enter_returns_cr() {
        let ev = named_event(NamedKey::Enter);
        assert_eq!(translate_key_event(&ev, &no_mods()), Some(vec![0x0D]));
    }

    #[test]
    fn regular_character_a() {
        let ev = char_event("a");
        assert_eq!(translate_key_event(&ev, &no_mods()), Some(b"a".to_vec()));
    }

    #[test]
    fn backspace_returns_del() {
        let ev = named_event(NamedKey::Backspace);
        assert_eq!(translate_key_event(&ev, &no_mods()), Some(vec![0x7F]));
    }

    #[test]
    fn tab_returns_ht() {
        let ev = named_event(NamedKey::Tab);
        assert_eq!(translate_key_event(&ev, &no_mods()), Some(vec![0x09]));
    }

    #[test]
    fn escape_key() {
        let ev = named_event(NamedKey::Escape);
        assert_eq!(translate_key_event(&ev, &no_mods()), Some(vec![0x1B]));
    }

    // ── Ctrl combos ──

    #[test]
    fn ctrl_c_returns_etx() {
        let ev = char_event("c");
        assert_eq!(translate_key_event(&ev, &ctrl()), Some(vec![0x03]));
    }

    #[test]
    fn ctrl_d_returns_eot() {
        let ev = char_event("d");
        assert_eq!(translate_key_event(&ev, &ctrl()), Some(vec![0x04]));
    }

    #[test]
    fn ctrl_bracket_returns_esc() {
        let ev = char_event("[");
        assert_eq!(translate_key_event(&ev, &ctrl()), Some(vec![0x1B]));
    }

    // ── Alt combos ──

    #[test]
    fn alt_a() {
        let ev = char_event("a");
        assert_eq!(
            translate_key_event(&ev, &alt()),
            Some(vec![0x1B, b'a'])
        );
    }

    // ── Arrow keys ──

    #[test]
    fn arrow_up() {
        let ev = named_event(NamedKey::ArrowUp);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn arrow_down() {
        let ev = named_event(NamedKey::ArrowDown);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[B".to_vec())
        );
    }

    #[test]
    fn arrow_right() {
        let ev = named_event(NamedKey::ArrowRight);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[C".to_vec())
        );
    }

    #[test]
    fn arrow_left() {
        let ev = named_event(NamedKey::ArrowLeft);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[D".to_vec())
        );
    }

    // ── Modifier + Arrow ──

    #[test]
    fn shift_arrow_up() {
        let ev = named_event(NamedKey::ArrowUp);
        assert_eq!(
            translate_key_event(&ev, &shift()),
            Some(b"\x1b[1;2A".to_vec())
        );
    }

    #[test]
    fn shift_arrow_down() {
        let ev = named_event(NamedKey::ArrowDown);
        assert_eq!(
            translate_key_event(&ev, &shift()),
            Some(b"\x1b[1;2B".to_vec())
        );
    }

    #[test]
    fn ctrl_arrow_up() {
        let ev = named_event(NamedKey::ArrowUp);
        assert_eq!(
            translate_key_event(&ev, &ctrl()),
            Some(b"\x1b[1;5A".to_vec())
        );
    }

    #[test]
    fn alt_arrow_up() {
        let ev = named_event(NamedKey::ArrowUp);
        assert_eq!(
            translate_key_event(&ev, &alt()),
            Some(b"\x1b[1;3A".to_vec())
        );
    }

    // ── Navigation keys ──

    #[test]
    fn home_key() {
        let ev = named_event(NamedKey::Home);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[H".to_vec())
        );
    }

    #[test]
    fn end_key() {
        let ev = named_event(NamedKey::End);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[F".to_vec())
        );
    }

    #[test]
    fn page_up() {
        let ev = named_event(NamedKey::PageUp);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[5~".to_vec())
        );
    }

    #[test]
    fn page_down() {
        let ev = named_event(NamedKey::PageDown);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[6~".to_vec())
        );
    }

    #[test]
    fn insert_key() {
        let ev = named_event(NamedKey::Insert);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[2~".to_vec())
        );
    }

    #[test]
    fn delete_key() {
        let ev = named_event(NamedKey::Delete);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[3~".to_vec())
        );
    }

    // ── Function keys ──

    #[test]
    fn f1_key() {
        let ev = named_event(NamedKey::F1);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1bOP".to_vec())
        );
    }

    #[test]
    fn f5_key() {
        let ev = named_event(NamedKey::F5);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn f12_key() {
        let ev = named_event(NamedKey::F12);
        assert_eq!(
            translate_key_event(&ev, &no_mods()),
            Some(b"\x1b[24~".to_vec())
        );
    }

    // ── Key release ignored ──

    #[test]
    fn key_release_returns_none() {
        let mut ev = named_event(NamedKey::Enter);
        ev.state = ElementState::Released;
        assert_eq!(translate_key_event(&ev, &no_mods()), None);
    }
}
