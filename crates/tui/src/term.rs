//! Typing into an agent's terminal from the TUI, and laying terminals out
//! in a grid.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// The bytes a terminal would send for a key, as xterm encodes them.
pub fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let mut out: Vec<u8> = match key.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '-' | '/' => 0x1f,
                '8' | '?' => 0x7f,
                _ => return None,
            };
            vec![b]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        // Claude Code takes Alt+Enter (and Shift+Enter) as a newline.
        KeyCode::Enter if alt || shift => b"\x1b\r".to_vec(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            _ => return None,
        },
        _ => return None,
    };
    if alt && !matches!(key.code, KeyCode::Enter) {
        out.insert(0, 0x1b);
    }
    Some(out)
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
    if n == 0 {
        return 1;
    }
    if let Some(c) = fixed {
        return (c as usize).clamp(1, n);
    }
    (1..=n)
        .max_by(|&a, &b| {
            let score = |c: usize| {
                let rows = n.div_ceil(c);
                let w = f64::from(area.width) / c as f64;
                let h = f64::from(area.height) / rows as f64;
                // The smaller of width and height, in comparable units.
                (w / 2.4).min(h)
            };
            score(a).total_cmp(&score(b)).then(b.cmp(&a))
        })
        .unwrap_or(1)
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
