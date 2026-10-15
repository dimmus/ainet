//! What a terminal screen paints, worked out without painting: the
//! screen's cells (a `vt100::Screen`, as ainet keeps every agent's
//! terminal) turned into background rectangles merged along each row and
//! runs of text of one style, as Zed's terminal element batches them; the
//! colours mapped through the theme's terminal ANSI colours; selections;
//! and scrollback laid out as a screen of its own. Pure, so tested
//! without GPUI.

use gpui::{Hsla, Rgba};
use theme::ThemeColors;
use unicode_width::UnicodeWidthChar;

/// What dimmed (faint) text keeps of its colour, as Zed's terminal.
const DIM_ALPHA: f32 = 0.7;

/// The colours a terminal paints with: the theme's.
#[derive(Clone, Debug, PartialEq)]
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
/// along each row, and text batched into runs of one style. Blank cells
/// break runs (they paint nothing), and so does a wide character, whose
/// glyph spans two cells.
pub fn layout(screen: &vt100::Screen, rows: u16, cols: u16, palette: &Palette) -> Layout {
    let (screen_rows, screen_cols) = screen.size();
    let mut out = Layout::default();
    let mut run: Option<Run> = None;
    for row in 0..rows.min(screen_rows) {
        out.runs.extend(run.take());
        for col in 0..cols.min(screen_cols) {
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
                    Some(last) if last.row == row && last.color == color && last.col + last.cells == col => {
                        last.cells += cells;
                    }
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
    }
    out.runs.extend(run);
    out
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

/// Scrollback shown as a screen of its own: `rows` × `cols` of plain text
/// lines, the last of them `scroll` lines above the end of `lines`. Long
/// lines are cut at the right edge.
pub fn history_screen(lines: &[String], scroll: usize, rows: u16, cols: u16) -> vt100::Parser {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    let end = lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(usize::from(rows));
    let mut bytes = String::new();
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
    // The cursor is no part of history.
    bytes.push_str("\x1b[?25l");
    parser.process(bytes.as_bytes());
    parser
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
    fn scrollback_shows_as_a_screen() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        let parser = history_screen(&lines, 2, 3, 20);
        assert_eq!(parser.screen().contents(), "line 6\nline 7\nline 8");
        assert!(parser.screen().hide_cursor());
        // Cut at the edge, not wrapped.
        let parser = history_screen(&["abcdefghij".into(), "x".into()], 0, 2, 4);
        assert_eq!(parser.screen().contents(), "abcd\nx");
        assert_eq!(max_scroll(10, 3), 7);
        assert_eq!(max_scroll(2, 3), 0);
    }
}
