//! Typing into an agent's terminal from the TUI, and laying terminals out
//! in a grid.

use ainet_model::terminal::{self, Key, Mods};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// The bytes a terminal would send for a key, as xterm encodes them (the
/// table is `ainet-model`'s, shared with the desktop app).
pub fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let mods = Mods {
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    };
    let key = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Esc,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    terminal::encode(key, mods)
}

/// The key that leaves a terminal you are typing into: Ctrl+] (as telnet),
/// with F12 as a fallback for terminals that swallow it.
pub fn is_leave_key(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5')) && ctrl || key.code == KeyCode::F(12)
}

/// Columns for `n` tiles in `area`: as given, or the count that keeps tiles
/// closest to a terminal's usual shape (about 2.4 columns per row).
pub fn grid_cols(n: usize, area: Rect, fixed: Option<u16>) -> usize {
    terminal::grid_cols(
        n,
        f64::from(area.width),
        f64::from(area.height),
        fixed.map(usize::from),
    )
}

/// Splits `area` into `n` tiles in `cols` columns, rows filled left to right;
/// the last row's tiles stretch to use the width.
pub fn tiles(n: usize, cols: usize, area: Rect) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let rows = n.div_ceil(cols);
    let mut out = Vec::with_capacity(n);
    for r in 0..rows {
        let in_row = if r + 1 == rows {
            n - cols * (rows - 1)
        } else {
            cols
        };
        let y0 = area.y + (u32::from(area.height) * r as u32 / rows as u32) as u16;
        let y1 = area.y + (u32::from(area.height) * (r as u32 + 1) / rows as u32) as u16;
        for c in 0..in_row {
            let x0 = area.x + (u32::from(area.width) * c as u32 / in_row as u32) as u16;
            let x1 = area.x + (u32::from(area.width) * (c as u32 + 1) / in_row as u32) as u16;
            out.push(Rect {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_encode_like_xterm() {
        let k = |code, m| KeyEvent::new(code, m);
        assert_eq!(
            encode_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(k(KeyCode::Char('é'), KeyModifiers::NONE)),
            Some("é".as_bytes().to_vec())
        );
        assert_eq!(
            encode_key(k(KeyCode::Up, KeyModifiers::NONE)),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(k(KeyCode::Char('x'), KeyModifiers::ALT)),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            encode_key(k(KeyCode::Enter, KeyModifiers::ALT)),
            Some(b"\x1b\r".to_vec())
        );
        assert!(is_leave_key(&k(KeyCode::Char(']'), KeyModifiers::CONTROL)));
        assert!(!is_leave_key(&k(KeyCode::Char(']'), KeyModifiers::NONE)));
    }

    #[test]
    fn tiles_cover_the_area() {
        let area = Rect::new(0, 0, 200, 50);
        for n in 1..=9 {
            let cols = grid_cols(n, area, None);
            let t = tiles(n, cols, area);
            assert_eq!(t.len(), n);
            let covered: u32 = t.iter().map(|r| u32::from(r.width) * u32::from(r.height)).sum();
            assert_eq!(covered, 200 * 50, "n={n} cols={cols}");
        }
        assert_eq!(grid_cols(4, Rect::new(0, 0, 240, 50), None), 2);
        assert_eq!(grid_cols(5, area, Some(3)), 3);
    }
}
