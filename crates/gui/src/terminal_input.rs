//! What a GPUI keystroke types into an agent's terminal: the key named,
//! then `ainet-model`'s xterm table (the TUI's), so both front ends send
//! the same bytes for the same key.

use ainet_model::terminal::{Key, Mode, Mods, encode_in};
use gpui::Keystroke;

/// The bytes `keystroke` sends to a terminal, or `None` for keys a
/// terminal has no code for (and anything with the platform key held,
/// which is the app's).
pub fn keystroke_bytes(keystroke: &Keystroke, mode: Mode) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    let mods = Mods {
        ctrl: m.control,
        alt: m.alt,
        shift: m.shift,
    };
    let named = match keystroke.key.as_str() {
        "enter" => Some(Key::Enter),
        "tab" if m.shift => Some(Key::BackTab),
        "tab" => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "escape" => Some(Key::Esc),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "delete" => Some(Key::Delete),
        "insert" => Some(Key::Insert),
        "space" => Some(Key::Char(' ')),
        key => key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| *n >= 1)
            .map(Key::F),
    };
    if let Some(key) = named {
        return encode_in(key, mods, mode);
    }
    let mut chars = keystroke.key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        // A key with a longer name, typed as text if it typed any.
        return keystroke.key_char.as_ref().map(|t| t.clone().into_bytes());
    };
    if m.control || m.alt {
        // The key itself: what it types may be a dead key or an Option
        // character, which xterm does not send.
        let c = if m.shift { c.to_ascii_uppercase() } else { c };
        return encode_in(Key::Char(c), mods, mode);
    }
    // Plain typing: the text the layout produced (shifted, accented).
    match &keystroke.key_char {
        Some(text) => Some(text.clone().into_bytes()),
        None => encode_in(Key::Char(c), mods, mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(keys: &str) -> Option<Vec<u8>> {
        let keystroke = Keystroke::parse(keys).unwrap().with_simulated_ime();
        keystroke_bytes(&keystroke, Mode::default())
    }

    /// The TUI's table (`crates/tui/src/term.rs`), as GPUI names the keys.
    #[test]
    fn keystrokes_encode_like_the_tuis_keys() {
        let table: &[(&str, &[u8])] = &[
            ("ctrl-c", b"\x03"),
            ("ctrl-a", b"\x01"),
            ("ctrl-z", b"\x1a"),
            ("ctrl-space", b"\x00"),
            ("ctrl-[", b"\x1b"),
            ("ctrl-\\", b"\x1c"),
            ("ctrl-]", b"\x1d"),
            ("ctrl-/", b"\x1f"),
            ("a", b"a"),
            ("shift-a", b"A"),
            ("space", b" "),
            ("alt-x", b"\x1bx"),
            ("alt-shift-x", b"\x1bX"),
            ("enter", b"\r"),
            ("alt-enter", b"\x1b\r"),
            ("shift-enter", b"\x1b\r"),
            ("tab", b"\t"),
            ("shift-tab", b"\x1b[Z"),
            ("backspace", b"\x7f"),
            ("alt-backspace", b"\x1b\x7f"),
            ("escape", b"\x1b"),
            ("up", b"\x1b[A"),
            ("down", b"\x1b[B"),
            ("right", b"\x1b[C"),
            ("left", b"\x1b[D"),
            ("home", b"\x1b[H"),
            ("end", b"\x1b[F"),
            ("pageup", b"\x1b[5~"),
            ("pagedown", b"\x1b[6~"),
            ("delete", b"\x1b[3~"),
            ("insert", b"\x1b[2~"),
            ("f1", b"\x1bOP"),
            ("f5", b"\x1b[15~"),
            ("f11", b"\x1b[23~"),
        ];
        for (keys, want) in table {
            assert_eq!(bytes(keys).as_deref(), Some(*want), "{keys}");
        }
        assert_eq!(bytes("f12"), None);
        assert_eq!(bytes("ctrl-shift-c").as_deref(), Some(&b"\x03"[..]));
        // The platform key belongs to the app.
        assert_eq!(bytes("super-c"), None);
    }

    #[test]
    fn typed_text_goes_as_typed() {
        let mut keystroke = Keystroke::parse("e").unwrap();
        keystroke.key_char = Some("é".into());
        assert_eq!(
            keystroke_bytes(&keystroke, Mode::default()).unwrap(),
            "é".as_bytes()
        );
    }

    #[test]
    fn application_cursor_mode_changes_the_arrows() {
        let app = Mode { app_cursor: true };
        let up = Keystroke::parse("up").unwrap();
        assert_eq!(keystroke_bytes(&up, app).unwrap(), b"\x1bOA");
    }
}
