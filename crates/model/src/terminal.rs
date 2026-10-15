//! An agent's terminal, without drawing it: what a key or a paste sends
//! (as xterm encodes it), which screens to fetch and when, and how many
//! columns a grid of terminals takes. Both front ends type into and show
//! agents' terminals through these.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A key, as far as a terminal cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    /// Shift+Tab.
    BackTab,
    Backspace,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    /// A function key, F1 and on.
    F(u8),
}

/// The modifiers held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// What the screen asked of the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    /// Application cursor keys (DECCKM): unmodified arrows, Home and End
    /// send `ESC O` instead of `ESC [`, as full-screen programs ask.
    pub app_cursor: bool,
}

/// The bytes a terminal would send for a key, as xterm encodes them.
pub fn encode(key: Key, mods: Mods) -> Option<Vec<u8>> {
    let Mods { ctrl, alt, shift } = mods;
    let mut out: Vec<u8> = match key {
        Key::Char(c) if ctrl => {
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
        Key::Char(c) => c.to_string().into_bytes(),
        // Claude Code takes Alt+Enter (and Shift+Enter) as a newline.
        Key::Enter if alt || shift => b"\x1b\r".to_vec(),
        Key::Enter => b"\r".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::BackTab => b"\x1b[Z".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Esc => b"\x1b".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        Key::F(n) => match n {
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
    };
    if alt && key != Key::Enter {
        out.insert(0, 0x1b);
    }
    Some(out)
}

/// [`encode`], minding what the screen asked for.
pub fn encode_in(key: Key, mods: Mods, mode: Mode) -> Option<Vec<u8>> {
    let cursor_key = matches!(
        key,
        Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End
    );
    let mut out = encode(key, mods)?;
    if mode.app_cursor && cursor_key && mods == Mods::default() {
        // `ESC [ A` becomes `ESC O A`.
        out[1] = b'O';
    }
    Some(out)
}

/// What pasting `text` sends: wrapped in bracketed-paste marks when the
/// screen asked for them, so the program knows it was not typed; else
/// with line ends as the Enter key sends them.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// How often one terminal's screen is fetched at most: about 15 times a
/// second.
pub const FETCH_EVERY: Duration = Duration::from_millis(66);

/// The size asked for until a front end has laid a terminal out.
pub const DEFAULT_SIZE: (u16, u16) = (24, 90);

/// Which terminals' screens are stale and when to fetch them: a terminal
/// on show is marked when its agent prints something (`Event::Output`),
/// and fetched at most every [`FETCH_EVERY`]. It also remembers the size
/// each terminal was asked to take, so a front end asks once per change.
#[derive(Debug, Default)]
pub struct Pump {
    dirty: HashSet<String>,
    last_fetch: HashMap<String, Instant>,
    /// Sizes (rows, cols) we asked panes to take.
    sizes: HashMap<String, (u16, u16)>,
}

impl Pump {
    pub fn new() -> Self {
        Pump::default()
    }

    /// `id`'s screen is stale.
    pub fn mark(&mut self, id: &str) {
        if !self.dirty.contains(id) {
            self.dirty.insert(id.to_string());
        }
    }

    pub fn mark_all(&mut self, ids: impl IntoIterator<Item = String>) {
        self.dirty.extend(ids);
    }

    pub fn is_dirty(&self, id: &str) -> bool {
        self.dirty.contains(id)
    }

    /// Takes the stale screens not fetched in the last [`FETCH_EVERY`]
    /// (as of `now`), each with the size to ask for, and records them as
    /// fetched.
    pub fn due(&mut self, now: Instant) -> Vec<(String, (u16, u16))> {
        let due: Vec<String> = self
            .dirty
            .iter()
            .filter(|id| {
                self.last_fetch
                    .get(*id)
                    .is_none_or(|t| now.saturating_duration_since(*t) > FETCH_EVERY)
            })
            .cloned()
            .collect();
        due.into_iter()
            .map(|id| {
                self.dirty.remove(&id);
                self.last_fetch.insert(id.clone(), now);
                let size = self.size(&id);
                (id, size)
            })
            .collect()
    }

    /// How long until the next stale screen may be fetched, if any is
    /// stale.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.dirty
            .iter()
            .map(|id| match self.last_fetch.get(id) {
                Some(t) => {
                    (FETCH_EVERY + Duration::from_millis(1)).saturating_sub(now.saturating_duration_since(*t))
                }
                None => Duration::ZERO,
            })
            .min()
    }

    /// `id`'s terminal was laid out at `size` (rows, cols). True when the
    /// pane should be asked to take it: the size changed and is big enough
    /// to be a terminal. Its screen is then stale.
    pub fn resize(&mut self, id: &str, size: (u16, u16)) -> bool {
        if size.0 < 2 || size.1 < 10 || self.sizes.get(id) == Some(&size) {
            return false;
        }
        self.sizes.insert(id.to_string(), size);
        self.mark(id);
        true
    }

    /// The size `id`'s terminal was asked to take, or [`DEFAULT_SIZE`].
    pub fn size(&self, id: &str) -> (u16, u16) {
        self.sizes.get(id).copied().unwrap_or(DEFAULT_SIZE)
    }
}

/// Columns for `n` tiles in an area `width` × `height` cells: as given, or
/// the count that keeps tiles closest to a terminal's usual shape (about
/// 2.4 columns per row).
pub fn grid_cols(n: usize, width: f64, height: f64, fixed: Option<usize>) -> usize {
    if n == 0 {
        return 1;
    }
    if let Some(c) = fixed {
        return c.clamp(1, n);
    }
    (1..=n)
        .max_by(|&a, &b| {
            let score = |c: usize| {
                let rows = n.div_ceil(c);
                let w = width / c as f64;
                let h = height / rows as f64;
                // The smaller of width and height, in comparable units.
                (w / 2.4).min(h)
            };
            score(a).total_cmp(&score(b)).then(b.cmp(&a))
        })
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Mods {
        Mods { ctrl, alt, shift }
    }

    #[test]
    fn keys_encode_like_xterm() {
        let none = Mods::default();
        let ctrl = mods(true, false, false);
        let alt = mods(false, true, false);
        let table: &[(Key, Mods, &[u8])] = &[
            (Key::Char('c'), ctrl, b"\x03"),
            (Key::Char('A'), ctrl, b"\x01"),
            (Key::Char(' '), ctrl, b"\x00"),
            (Key::Char('['), ctrl, b"\x1b"),
            (Key::Char('\\'), ctrl, b"\x1c"),
            (Key::Char(']'), ctrl, b"\x1d"),
            (Key::Char('/'), ctrl, b"\x1f"),
            (Key::Char('?'), ctrl, b"\x7f"),
            (Key::Char('é'), none, "é".as_bytes()),
            (Key::Char('x'), alt, b"\x1bx"),
            (Key::Enter, none, b"\r"),
            (Key::Enter, alt, b"\x1b\r"),
            (Key::Enter, mods(false, false, true), b"\x1b\r"),
            (Key::Tab, none, b"\t"),
            (Key::BackTab, none, b"\x1b[Z"),
            (Key::Backspace, none, b"\x7f"),
            (Key::Backspace, alt, b"\x1b\x7f"),
            (Key::Esc, none, b"\x1b"),
            (Key::Up, none, b"\x1b[A"),
            (Key::Down, none, b"\x1b[B"),
            (Key::Right, none, b"\x1b[C"),
            (Key::Left, none, b"\x1b[D"),
            (Key::Home, none, b"\x1b[H"),
            (Key::End, none, b"\x1b[F"),
            (Key::PageUp, none, b"\x1b[5~"),
            (Key::PageDown, none, b"\x1b[6~"),
            (Key::Delete, none, b"\x1b[3~"),
            (Key::Insert, none, b"\x1b[2~"),
            (Key::F(1), none, b"\x1bOP"),
            (Key::F(4), none, b"\x1bOS"),
            (Key::F(5), none, b"\x1b[15~"),
            (Key::F(11), none, b"\x1b[23~"),
        ];
        for (key, m, bytes) in table {
            assert_eq!(encode(*key, *m).as_deref(), Some(*bytes), "{key:?} {m:?}");
        }
        assert_eq!(encode(Key::F(12), none), None);
        assert_eq!(encode(Key::Char('é'), ctrl), None);
    }

    #[test]
    fn application_cursor_keys_send_ss3() {
        let app = Mode { app_cursor: true };
        let none = Mods::default();
        assert_eq!(encode_in(Key::Up, none, app).unwrap(), b"\x1bOA");
        assert_eq!(encode_in(Key::Home, none, app).unwrap(), b"\x1bOH");
        assert_eq!(encode_in(Key::Up, none, Mode::default()).unwrap(), b"\x1b[A");
        // Modified keys and others are as usual.
        assert_eq!(
            encode_in(Key::Up, mods(false, true, false), app).unwrap(),
            b"\x1b\x1b[A"
        );
        assert_eq!(encode_in(Key::PageUp, none, app).unwrap(), b"\x1b[5~");
    }

    #[test]
    fn pastes_are_bracketed_when_asked() {
        assert_eq!(paste("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        assert_eq!(paste("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn stale_screens_are_fetched_at_most_every_interval() {
        let t0 = Instant::now();
        let mut pump = Pump::new();
        assert_eq!(pump.wait(t0), None);
        pump.mark("a");
        pump.mark("a");
        assert_eq!(pump.wait(t0), Some(Duration::ZERO));
        assert_eq!(pump.due(t0), [("a".to_string(), DEFAULT_SIZE)]);
        assert!(pump.due(t0).is_empty(), "nothing stale");

        // Stale again at once: it waits out the interval.
        pump.mark("a");
        let soon = t0 + Duration::from_millis(10);
        assert!(pump.due(soon).is_empty());
        let wait = pump.wait(soon).unwrap();
        assert!(wait > Duration::ZERO && wait <= FETCH_EVERY, "{wait:?}");
        assert_eq!(pump.due(soon + wait).len(), 1);
        assert!(!pump.is_dirty("a"));
    }

    #[test]
    fn resizes_are_asked_once_per_change() {
        let mut pump = Pump::new();
        assert!(pump.resize("a", (30, 100)));
        assert!(pump.is_dirty("a"), "a new size means a new screen");
        assert!(!pump.resize("a", (30, 100)));
        assert_eq!(pump.size("a"), (30, 100));
        assert!(!pump.resize("a", (1, 100)), "too small to be a terminal");
        assert!(!pump.resize("a", (30, 9)));
        assert_eq!(pump.size("b"), DEFAULT_SIZE);
    }

    #[test]
    fn grids_keep_tiles_terminal_shaped() {
        assert_eq!(grid_cols(0, 200., 50., None), 1);
        assert_eq!(grid_cols(4, 240., 50., None), 2);
        assert_eq!(grid_cols(5, 200., 50., Some(3)), 3);
        assert_eq!(grid_cols(2, 200., 50., Some(9)), 2);
        assert_eq!(grid_cols(1, 200., 50., None), 1);
    }
}
