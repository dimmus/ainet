//! What a terminal screen paints, worked out without painting: the
//! screen's cells (a `vt100::Screen`, as ainet keeps every agent's
//! terminal) turned into background rectangles merged along each row and
//! runs of text of one style, as Zed's terminal element batches them; the
//! colours mapped through the theme's terminal ANSI colours; selections;
//! scrollback laid out as a screen of its own; and the cache that keeps
//! rows laid out (and shaped) from one frame to the next. Pure, so tested
//! without GPUI.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use gpui::{Hsla, Rgba};
use theme::ThemeColors;
use unicode_width::UnicodeWidthChar;

/// What dimmed (faint) text keeps of its colour, as Zed's terminal.
const DIM_ALPHA: f32 = 0.7;

/// The colours a terminal paints with: the theme's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the
    /// bright eight.
    pub ansi: [Hsla; 16],
    pub foreground: Hsla,
    pub background: Hsla,
}

impl Palette {
    pub fn from_theme(c: &ThemeColors) -> Self {
        Palette {
            ansi: [
                c.terminal_ansi_black,
                c.terminal_ansi_red,
                c.terminal_ansi_green,
                c.terminal_ansi_yellow,
                c.terminal_ansi_blue,
                c.terminal_ansi_magenta,
                c.terminal_ansi_cyan,
                c.terminal_ansi_white,
                c.terminal_ansi_bright_black,
                c.terminal_ansi_bright_red,
                c.terminal_ansi_bright_green,
                c.terminal_ansi_bright_yellow,
                c.terminal_ansi_bright_blue,
                c.terminal_ansi_bright_magenta,
                c.terminal_ansi_bright_cyan,
                c.terminal_ansi_bright_white,
            ],
            foreground: c.terminal_foreground,
            background: c.terminal_background,
        }
    }

    /// A cell's colour; `None` for the terminal's default.
    pub fn color(&self, color: vt100::Color) -> Option<Hsla> {
        match color {
            vt100::Color::Default => None,
            vt100::Color::Idx(i) => Some(self.indexed(i)),
            vt100::Color::Rgb(r, g, b) => Some(rgb(r, g, b)),
        }
    }

    /// One of the 256 indexed colours: the theme's sixteen, then xterm's
    /// 6×6×6 cube and grey ramp (as Zed's terminal).
    pub fn indexed(&self, i: u8) -> Hsla {
        match i {
            0..=15 => self.ansi[usize::from(i)],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            232..=255 => {
                let v = (i - 232) * 10 + 8;
                rgb(v, v, v)
            }
        }
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a: 1.,
    }
    .into()
}

/// Cells of one row in one style, shaped and painted as one line.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub row: u16,
    pub col: u16,
    /// Columns covered (a wide character covers two).
    pub cells: u16,
    pub text: String,
    pub color: Hsla,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Run {
    fn same_style(&self, other: &Run) -> bool {
        self.color == other.color
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
    }
}

/// A background other than the terminal's, over cells of one row.
#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub row: u16,
    pub col: u16,
    pub cells: u16,
    pub color: Hsla,
}

/// A screen, ready to paint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub backgrounds: Vec<Background>,
    pub runs: Vec<Run>,
}

/// A cell's colours after `inverse`, `dim` and the defaults: its text
/// colour, and its background if not the terminal's.
pub fn cell_colors(cell: &vt100::Cell, palette: &Palette) -> (Hsla, Option<Hsla>) {
    let fg = palette.color(cell.fgcolor());
    let bg = palette.color(cell.bgcolor());
    let (mut fg, bg) = if cell.inverse() {
        (
            bg.unwrap_or(palette.background),
            Some(fg.unwrap_or(palette.foreground)),
        )
    } else {
        (fg.unwrap_or(palette.foreground), bg)
    };
    if cell.dim() {
        fg.a *= DIM_ALPHA;
    }
    (fg, bg)
}

/// Lays out what of `screen` fits in `rows` × `cols`: backgrounds merged
/// along each row, and text batched into runs of one style (see
/// [`layout_row`]).
#[cfg_attr(not(test), allow(dead_code))]
pub fn layout(screen: &vt100::Screen, rows: u16, cols: u16, palette: &Palette) -> Layout {
    let mut out = Layout::default();
    for row in 0..rows.min(screen.size().0) {
        let one = layout_row(screen, row, cols, palette);
        out.backgrounds.extend(one.backgrounds);
        out.runs.extend(one.runs);
    }
    out
}

/// Lays out the first `cols` cells of one row: backgrounds merged along
/// it, and text batched into runs of one style. Blank cells break runs
/// (they paint nothing), and so does a wide character, whose glyph spans
/// two cells.
pub fn layout_row(screen: &vt100::Screen, row: u16, cols: u16, palette: &Palette) -> Layout {
    let mut out = Layout::default();
    let mut run: Option<Run> = None;
    for col in 0..cols.min(screen.size().1) {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let wide = cell.is_wide();
        let cells = if wide { 2 } else { 1 };
        let (fg, bg) = cell_colors(cell, palette);
        if let Some(color) = bg {
            match out.backgrounds.last_mut() {
                Some(last) if last.color == color && last.col + last.cells == col => last.cells += cells,
                _ => out.backgrounds.push(Background {
                    row,
                    col,
                    cells,
                    color,
                }),
            }
        }
        let text = cell.contents();
        if text.is_empty() || (text == " " && !cell.underline()) {
            out.runs.extend(run.take());
            continue;
        }
        let next = Run {
            row,
            col,
            cells,
            text: text.to_string(),
            color: fg,
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
        };
        match &mut run {
            Some(current) if current.same_style(&next) && current.col + current.cells == col => {
                current.text.push_str(text);
                current.cells += cells;
            }
            _ => {
                out.runs.extend(run.take());
                run = Some(next);
            }
        }
        if wide {
            out.runs.extend(run.take());
        }
    }
    out.runs.extend(run);
    out
}

/// What decides how each visible row's first `cols` cells look: a hash
/// of their contents and attributes (colours as the program gave them, so
/// it holds across themes), and of `cols`. Rows with the same key paint
/// the same, wherever they are on the screen. Read from the screen's
/// formatted rows in one pass: asking for each cell would cost a lookup
/// of its row every time.
pub fn row_keys(screen: &vt100::Screen, cols: u16) -> Vec<u64> {
    let cols = cols.min(screen.size().1);
    screen
        .rows_formatted(0, cols)
        .map(|bytes| {
            let mut h = DefaultHasher::new();
            cols.hash(&mut h);
            h.write(&bytes);
            h.finish()
        })
        .collect()
}

/// Rows laid out (and shaped) on earlier frames, by [`row_keys`], for one
/// look (palette, font and cell size, hashed by the caller): a row that
/// shows what it showed on the last frame, wherever it is now, is not laid
/// out or shaped again. Rows not used on a frame are forgotten after the
/// next; a new look forgets them all.
pub struct RowCache<T> {
    look: Option<u64>,
    current: HashMap<u64, Rc<T>>,
    previous: HashMap<u64, Rc<T>>,
    /// Rows found and rows made, so far.
    pub hits: usize,
    pub misses: usize,
}

impl<T> Default for RowCache<T> {
    fn default() -> Self {
        RowCache {
            look: None,
            current: HashMap::new(),
            previous: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }
}

impl<T> RowCache<T> {
    /// Starts a frame painted in `look`.
    pub fn frame(&mut self, look: u64) {
        if self.look == Some(look) {
            self.previous = std::mem::take(&mut self.current);
        } else {
            self.look = Some(look);
            self.previous.clear();
            self.current.clear();
        }
    }

    /// The row keyed `key`, made with `make` unless a frame before (or
    /// this one) made it.
    pub fn row(&mut self, key: u64, make: impl FnOnce() -> T) -> Rc<T> {
        if let Some(row) = self.current.get(&key) {
            self.hits += 1;
            return Rc::clone(row);
        }
        let row = match self.previous.remove(&key) {
            Some(row) => {
                self.hits += 1;
                row
            }
            None => {
                self.misses += 1;
                Rc::new(make())
            }
        };
        self.current.insert(key, Rc::clone(&row));
        row
    }

    /// Rows kept for the next frame.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.current.len()
    }
}

/// Cells picked with the mouse, from where the drag began (`anchor`) to
/// where it is (`head`), as (row, col).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
}

impl Selection {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn at(cell: (u16, u16)) -> Self {
        Selection {
            anchor: cell,
            head: cell,
        }
    }

    /// The first and last cell, in reading order.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The stretch of each row it covers in a screen `cols` wide: (row,
    /// first column, columns).
    pub fn rows(&self, cols: u16) -> Vec<(u16, u16, u16)> {
        let ((r0, c0), (r1, c1)) = self.ordered();
        (r0..=r1)
            .map(|r| {
                let start = if r == r0 { c0 } else { 0 };
                let end = if r == r1 { c1 + 1 } else { cols };
                (r, start, end.min(cols).saturating_sub(start))
            })
            .filter(|(_, _, n)| *n > 0)
            .collect()
    }

    /// Its text in `screen`: lines end where rows do, unless the row
    /// wrapped; trailing blanks are dropped, as terminals copy. The grid
    /// drawn can be bigger than the screen (before a resize takes, or a
    /// pane that keeps its size): what lies outside it is nothing.
    pub fn text(&self, screen: &vt100::Screen) -> String {
        let (rows, cols) = screen.size();
        let ((r0, c0), (r1, c1)) = self.ordered();
        if rows == 0 || r0 >= rows {
            return String::new();
        }
        let (r1, end) = if r1 >= rows {
            (rows - 1, cols)
        } else {
            (r1, (c1 + 1).min(cols))
        };
        screen.contents_between(r0, c0.min(cols), r1, end)
    }
}

/// Scrollback shown as a screen of its own: `rows` × `cols` of lines, the
/// last of them `scroll` lines above the end of `lines`. With `styled`
/// (the same lines with their colours, as ainetd sends them), those are
/// drawn instead of the plain text. Long lines are cut at the right edge.
pub fn history_screen(
    lines: &[String],
    styled: Option<&[String]>,
    scroll: usize,
    rows: u16,
    cols: u16,
) -> vt100::Parser {
    let end = lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(usize::from(rows));
    let mut bytes = String::new();
    let parser = match styled.filter(|s| s.len() == lines.len()) {
        Some(styled) => {
            // vt100 always wraps: the screen is made wide enough that no
            // row does, and only its first `cols` columns are painted.
            let shown = &styled[start..end];
            let widest = shown.iter().map(|l| visible_width(l)).max().unwrap_or(0);
            let width = widest.clamp(usize::from(cols), usize::from(cols) + 1000);
            for (i, line) in shown.iter().enumerate() {
                bytes.push_str(&format!("\x1b[{};1H\x1b[m{line}", i + 1));
            }
            vt100::Parser::new(rows.max(1), width.max(1) as u16, 0)
        }
        None => {
            for (i, line) in lines[start..end].iter().enumerate() {
                bytes.push_str(&format!("\x1b[{};1H", i + 1));
                let mut width = 0;
                for c in line.chars() {
                    width += c.width().unwrap_or(0);
                    if width > usize::from(cols) {
                        break;
                    }
                    bytes.push(c);
                }
            }
            vt100::Parser::new(rows.max(1), cols.max(1), 0)
        }
    };
    let mut parser = parser;
    // The cursor is no part of history.
    bytes.push_str("\x1b[?25l");
    parser.process(bytes.as_bytes());
    parser
}

/// The cells a styled line takes: its characters' widths, its escape
/// sequences none.
fn visible_width(line: &str) -> usize {
    let mut width = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // `ESC [`, parameters, then a final byte from `@` to `~`.
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            width += c.width().unwrap_or(0);
        }
    }
    width
}

/// How far back `lines` of scrollback can be scrolled on a screen `rows`
/// high.
pub fn max_scroll(lines: usize, rows: u16) -> usize {
    lines.saturating_sub(usize::from(rows))
}

#[cfg(test)]
mod tests {
    use gpui::hsla;

    use super::*;

    fn palette() -> Palette {
        let ansi = std::array::from_fn(|i| hsla(i as f32 / 16., 0.5, 0.5, 1.));
        Palette {
            ansi,
            foreground: hsla(0., 0., 0.9, 1.),
            background: hsla(0., 0., 0.1, 1.),
        }
    }

    fn screen(rows: u16, cols: u16, bytes: &str) -> vt100::Parser {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(bytes.as_bytes());
        parser
    }

    #[test]
    fn colours_come_from_the_theme_and_xterms_tables() {
        let p = palette();
        assert_eq!(p.color(vt100::Color::Default), None);
        assert_eq!(p.color(vt100::Color::Idx(1)), Some(p.ansi[1]));
        assert_eq!(p.color(vt100::Color::Idx(9)), Some(p.ansi[9]));
        assert_eq!(p.indexed(16), rgb(0, 0, 0));
        assert_eq!(p.indexed(196), rgb(255, 0, 0));
        assert_eq!(p.indexed(231), rgb(255, 255, 255));
        assert_eq!(p.indexed(232), rgb(8, 8, 8));
        assert_eq!(p.indexed(255), rgb(238, 238, 238));
        assert_eq!(p.color(vt100::Color::Rgb(215, 119, 87)), Some(rgb(215, 119, 87)));
    }

    #[test]
    fn cells_of_one_style_merge_into_runs() {
        let p = palette();
        let s = screen(3, 20, "ab \x1b[31mcd\x1b[1mef\x1b[0m\r\n\x1b[4mu v\x1b[0m");
        let l = layout(s.screen(), 3, 20, &p);
        let runs: Vec<_> = l
            .runs
            .iter()
            .map(|r| (r.row, r.col, r.cells, r.text.as_str(), r.bold, r.underline))
            .collect();
        assert_eq!(
            runs,
            [
                (0, 0, 2, "ab", false, false),
                // A blank breaks the run; a colour starts one; bold another.
                (0, 3, 2, "cd", false, false),
                (0, 5, 2, "ef", true, false),
                // Underlined blanks are drawn, for their underline.
                (1, 0, 3, "u v", false, true),
            ]
        );
        assert_eq!(l.runs[0].color, p.foreground);
        assert_eq!(l.runs[1].color, p.ansi[1]);
        assert!(l.backgrounds.is_empty(), "default backgrounds are not painted");
    }

    #[test]
    fn backgrounds_merge_along_a_row_and_inverse_swaps() {
        let p = palette();
        let s = screen(
            2,
            20,
            "\x1b[44mab  \x1b[42mc\x1b[0m d\r\n\x1b[7mx\x1b[0m\x1b[48;5;196m \x1b[0m",
        );
        let l = layout(s.screen(), 2, 20, &p);
        let bgs: Vec<_> = l.backgrounds.iter().map(|b| (b.row, b.col, b.cells)).collect();
        assert_eq!(bgs, [(0, 0, 4), (0, 4, 1), (1, 0, 1), (1, 1, 1)]);
        assert_eq!(l.backgrounds[0].color, p.ansi[4]);
        assert_eq!(l.backgrounds[1].color, p.ansi[2]);
        // Inverse: the default colours swap.
        assert_eq!(l.backgrounds[2].color, p.foreground);
        let x = l.runs.iter().find(|r| r.text == "x").unwrap();
        assert_eq!(x.color, p.background);
        assert_eq!(l.backgrounds[3].color, rgb(255, 0, 0));
    }

    #[test]
    fn truecolour_dim_italic_and_clipping() {
        let p = palette();
        let s = screen(2, 10, "\x1b[38;2;10;20;30;2;3mhi\x1b[0m 0123456\r\nnext");
        let l = layout(s.screen(), 1, 6, &p);
        let hi = &l.runs[0];
        assert!(hi.italic);
        let mut dim = rgb(10, 20, 30);
        dim.a *= DIM_ALPHA;
        assert_eq!(hi.color, dim);
        // Only what fits in 1 × 6 is laid out.
        assert_eq!(l.runs.len(), 2);
        assert_eq!(l.runs[1].text, "012");
    }

    #[test]
    fn wide_characters_take_two_cells_and_end_their_run() {
        let p = palette();
        let s = screen(1, 10, "a中b");
        let l = layout(s.screen(), 1, 10, &p);
        let runs: Vec<_> = l.runs.iter().map(|r| (r.col, r.cells, r.text.as_str())).collect();
        assert_eq!(runs, [(0, 3, "a中"), (3, 1, "b")]);
    }

    #[test]
    fn selections_cover_rows_and_copy_their_text() {
        let s = screen(3, 10, "hello\r\nworld wide\r\nend");
        let sel = Selection {
            anchor: (1, 3),
            head: (0, 1),
        };
        assert_eq!(sel.ordered(), ((0, 1), (1, 3)));
        assert_eq!(sel.rows(10), [(0, 1, 9), (1, 0, 4)]);
        assert_eq!(sel.text(s.screen()), "ello\nworl");
        assert_eq!(Selection::at((2, 0)).text(s.screen()), "e");
        assert_eq!(Selection::at((2, 9)).rows(10), [(2, 9, 1)]);
    }

    #[test]
    fn selections_past_the_screen_copy_what_is_on_it() {
        let s = screen(4, 10, "hello\r\nworld\r\n\r\nlast");
        let past_the_edge = Selection {
            anchor: (0, 15),
            head: (1, 3),
        };
        assert_eq!(past_the_edge.text(s.screen()), "\nworl");
        let past_the_bottom = Selection {
            anchor: (3, 0),
            head: (7, 2),
        };
        assert_eq!(past_the_bottom.text(s.screen()), "last");
        assert_eq!(Selection::at((6, 30)).text(s.screen()), "");
        assert_eq!(Selection::at((0, 12)).text(s.screen()), "");
    }

    #[test]
    fn rows_are_made_once_and_kept_while_they_show() {
        let mut cache: RowCache<String> = RowCache::default();
        let mut made = Vec::new();
        let mut frame = |cache: &mut RowCache<String>, look: u64, keys: &[u64]| {
            cache.frame(look);
            for &key in keys {
                cache.row(key, || {
                    made.push(key);
                    format!("row {key}")
                });
            }
        };
        frame(&mut cache, 1, &[10, 11, 10]);
        assert_eq!((cache.hits, cache.misses), (1, 2), "a repeated row is made once");
        // The next frame: what still shows is found, moved or not.
        frame(&mut cache, 1, &[11, 12]);
        assert_eq!((cache.hits, cache.misses), (2, 3));
        assert_eq!(*cache.row(12, String::new), "row 12");
        // Row 10 went a frame ago, and is forgotten.
        frame(&mut cache, 1, &[10]);
        assert_eq!(cache.misses, 4);
        assert_eq!(cache.len(), 1);
        // Another theme or font: everything is made anew.
        frame(&mut cache, 2, &[10]);
        assert_eq!(cache.misses, 5);
        drop(frame);
        assert_eq!(made, [10, 11, 12, 10, 10]);
    }

    #[test]
    fn a_resize_or_another_palette_changes_the_keys() {
        let s = screen(3, 20, "\x1b[31mred\x1b[0m text");
        let mut cache: RowCache<Layout> = RowCache::default();
        let paint = |cache: &mut RowCache<Layout>, p: &Palette, cols: u16| {
            cache.frame(look(p));
            cache.row(row_keys(s.screen(), cols)[0], || {
                layout_row(s.screen(), 0, cols, p)
            })
        };
        let p = palette();
        let first = paint(&mut cache, &p, 20);
        assert_eq!(paint(&mut cache, &p, 20), first);
        assert_eq!(cache.misses, 1);
        // Narrower: the row is cut elsewhere, so laid out anew.
        let narrow = paint(&mut cache, &p, 3);
        assert_eq!(cache.misses, 2);
        assert_eq!(narrow.runs[0].text, "red");
        assert_eq!(narrow.runs.len(), 1);
        // Another theme's red: laid out anew in it.
        let mut other = palette();
        other.ansi[1] = gpui::hsla(0., 1., 0.5, 1.);
        let themed = paint(&mut cache, &other, 3);
        assert_eq!(cache.misses, 3);
        assert_eq!(themed.runs[0].color, other.ansi[1]);
    }

    fn look(p: &Palette) -> u64 {
        let mut h = DefaultHasher::new();
        p.hash(&mut h);
        h.finish()
    }

    #[test]
    fn rows_are_keyed_by_what_they_show() {
        let s = screen(
            8,
            10,
            "same\r\nsame\r\n\x1b[31msame\x1b[0m\r\n\x1b[1msame\x1b[0m\r\nsamf\r\n\x1b[44m  \x1b[0m\r\n\r\n世",
        );
        let key = |row: usize, cols| row_keys(s.screen(), cols)[row];
        assert_eq!(key(0, 10), key(1, 10), "where a row is does not matter");
        assert_ne!(key(0, 10), key(2, 10), "its colours do");
        assert_ne!(key(0, 10), key(3, 10), "and its attributes");
        assert_ne!(key(0, 10), key(4, 10), "and its text");
        assert_ne!(key(0, 10), key(0, 8), "and how much of it shows");
        assert_ne!(key(5, 10), key(6, 10), "and blanks on a background");
        assert_ne!(key(7, 10), key(6, 10), "and wide characters");
        assert_eq!(row_keys(s.screen(), 10).len(), 8);
        // A row's layout is the screen's layout of that row.
        let p = palette();
        let whole = layout(s.screen(), 5, 10, &p);
        let row = layout_row(s.screen(), 2, 10, &p);
        assert_eq!(
            row.runs,
            whole
                .runs
                .iter()
                .filter(|r| r.row == 2)
                .cloned()
                .collect::<Vec<_>>()
        );
    }

    /// What laying a busy screen out costs per frame, what keying its
    /// rows costs (the row cache's price on a frame where nothing
    /// changed), and a frame through the cache when the spinner moved
    /// (its rows laid out anew, the rest found). Shaping, which the cache
    /// also saves, needs a real text system and is not measured here. Run
    /// with `CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test --manifest-path
    /// crates/gui/Cargo.toml layout_cost -- --ignored --nocapture` (opt
    /// level 3 for this crate only: its dependencies are optimised in
    /// every profile).
    #[test]
    #[ignore = "a measurement; run by hand"]
    fn layout_cost() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};
        let p = palette();
        for (rows, cols) in [(24u16, 90u16), (50, 200)] {
            let mut parser = vt100::Parser::new(rows, cols, 0);
            parser.process(&ainet_model::sample::screen(rows, cols, 12));
            let s = parser.screen();
            let time = |f: &mut dyn FnMut()| {
                let start = Instant::now();
                let mut n = 0u32;
                while start.elapsed() < Duration::from_millis(500) {
                    f();
                    n += 1;
                }
                start.elapsed() / n
            };
            let full = time(&mut || {
                black_box(layout(black_box(s), rows, cols, &p));
            });
            let keys = time(&mut || {
                black_box(row_keys(black_box(s), cols));
            });
            let mut next = vt100::Parser::new(rows, cols, 0);
            next.process(&ainet_model::sample::screen(rows, cols, 12));
            next.process(&ainet_model::sample::frame(rows, cols, 12));
            let mut cache: RowCache<Layout> = RowCache::default();
            let mut flip = false;
            let cached = time(&mut || {
                flip = !flip;
                let s = if flip { next.screen() } else { s };
                cache.frame(1);
                for (row, key) in (0..rows).zip(row_keys(s, cols)) {
                    black_box(cache.row(key, || layout_row(s, row, cols, &p)));
                }
            });
            let frames = (cache.hits + cache.misses) / usize::from(rows);
            let l = layout(s, rows, cols, &p);
            println!(
                "{rows}x{cols}: layout {full:.1?} ({} runs, {} backgrounds), row keys {keys:.1?}, \
                 cached frame {cached:.1?} ({:.1} rows laid out)",
                l.runs.len(),
                l.backgrounds.len(),
                cache.misses as f64 / frames as f64
            );
        }
    }

    #[test]
    fn scrollback_shows_as_a_screen() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        let parser = history_screen(&lines, None, 2, 3, 20);
        assert_eq!(parser.screen().contents(), "line 6\nline 7\nline 8");
        assert!(parser.screen().hide_cursor());
        // Cut at the edge, not wrapped.
        let parser = history_screen(&["abcdefghij".into(), "x".into()], None, 0, 2, 4);
        assert_eq!(parser.screen().contents(), "abcd\nx");
        assert_eq!(max_scroll(10, 3), 7);

        // With colours: drawn as they were, long ones still not wrapped.
        let plain: Vec<String> = vec!["red abcdefgh".into(), "x".into()];
        let styled: Vec<String> = vec!["\x1b[0;31mred\x1b[0m abcdefgh".into(), "x".into()];
        assert_eq!(visible_width(&styled[0]), 12);
        let parser = history_screen(&plain, Some(&styled), 0, 2, 4);
        let s = parser.screen();
        assert_eq!(s.cell(0, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
        assert_eq!(s.cell(1, 0).unwrap().contents(), "x");
        let l = layout(s, 2, 4, &palette());
        let texts: Vec<_> = l.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["red", "x"], "only the first four columns are painted");
        // Colours that do not match the lines are not used.
        let parser = history_screen(&plain, Some(&styled[..1]), 0, 2, 20);
        assert_eq!(
            parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Default
        );
        assert_eq!(max_scroll(2, 3), 0);
    }
}
