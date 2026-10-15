//! A believable busy agent terminal, as the bytes its program writes: what
//! the measurements of screen fetching and streaming (and the GUI's
//! drawing) run on, so their numbers are about screens like Claude Code's
//! rather than empty ones. Not used by the front ends themselves.

/// Claude Code's accent, its dim grey, and the backgrounds of a diff.
const ACCENT: &str = "38;2;215;119;87";
const DIM: &str = "38;2;153;153;153";
const REMOVED: &str = "48;2;61;1;0";
const ADDED: &str = "48;2;2;40;0";

/// The lines of a session so far, `step` turns in: prose, tool calls with
/// their results, diffs on coloured backgrounds. Each is one row's bytes,
/// at most `cols` wide.
fn lines(cols: usize, step: usize) -> Vec<String> {
    let fit = |s: String| -> String { s.chars().take(cols).collect() };
    let mut out = Vec::new();
    for turn in 0..=step {
        let n = turn % 7 + 1;
        out.push(fit(format!(
            "\x1b[1;37m⏺\x1b[0m I'll look at part {n} next: the parser keeps its state between calls, so the \
             change has to go where the buffer is filled ({turn})."
        )));
        out.push(String::new());
        out.push(fit(format!("\x1b[1;37m⏺ Update\x1b[0m(src/part{n}.rs)")));
        out.push(fit(format!(
            "  \x1b[{DIM}m⎿  Updated src/part{n}.rs with 2 additions and 1 removal\x1b[0m"
        )));
        for (sign, bg, text) in [
            ('-', REMOVED, "    let state = State::new(buf);"),
            ('+', ADDED, "    let mut state = State::new(buf);"),
            ('+', ADDED, "    state.reset_after(turn);"),
        ] {
            let body = format!("{:>6} {sign} {text}", 40 + n);
            let pad = cols.saturating_sub(body.chars().count() + 4);
            out.push(fit(format!(
                "    \x1b[{bg}m\x1b[{DIM}m{:>6}\x1b[39m {sign} {text}{}\x1b[0m",
                40 + n,
                " ".repeat(pad)
            )));
        }
        out.push(String::new());
        out.push(fit(format!("\x1b[1;37m⏺ Bash\x1b[0m(cargo test part{n})")));
        out.push(fit(format!(
            "  \x1b[{DIM}m⎿  test result: \x1b[32mok\x1b[{DIM}m. {} passed; 0 failed; finished in 0.{n}s\x1b[0m",
            3 + n
        )));
        out.push(String::new());
    }
    out
}

/// The bottom of the screen Claude Code redraws on every frame: the
/// spinner, the input box and the hint line, from row `top` (1-based).
fn footer(top: usize, cols: usize, step: usize) -> String {
    let inner = cols.saturating_sub(2);
    let spinner = format!(
        "✻ Thinking{} ({}s · ↑ {}.{}k tokens · esc to interrupt)",
        ".".repeat(step % 4),
        step / 8,
        step % 9 + 1,
        step % 10
    );
    let spinner: String = spinner.chars().take(cols).collect();
    let mut out = String::new();
    let mut row = |r: usize, text: String| {
        out.push_str(&format!("\x1b[{};1H\x1b[2K{text}", top + r));
    };
    row(0, format!("\x1b[{ACCENT}m{spinner}\x1b[0m"));
    row(1, String::new());
    row(2, format!("\x1b[{DIM}m╭{}╮\x1b[0m", "─".repeat(inner)));
    row(
        3,
        format!(
            "\x1b[{DIM}m│\x1b[0m > {}\x1b[{DIM}m│\x1b[0m",
            " ".repeat(inner.saturating_sub(3))
        ),
    );
    row(4, format!("\x1b[{DIM}m╰{}╯\x1b[0m", "─".repeat(inner)));
    row(5, format!("\x1b[{DIM}m  ? for shortcuts\x1b[0m"));
    out.push_str(&format!("\x1b[{};5H", top + 3));
    out
}

/// Rows the footer takes at the bottom.
const FOOTER: usize = 6;

/// A whole screen `rows` × `cols` of a busy session at `step`: what a
/// program has written to get there (clearing first).
pub fn screen(rows: u16, cols: u16, step: usize) -> Vec<u8> {
    let (rows, cols) = (usize::from(rows.max(8)), usize::from(cols.max(20)));
    let body = lines(cols, step);
    let room = rows - FOOTER;
    let skip = body.len().saturating_sub(room);
    let mut out = String::from("\x1b[H\x1b[2J");
    for (i, line) in body[skip..].iter().enumerate() {
        out.push_str(&format!("\x1b[{};1H{line}", i + 1));
    }
    out.push_str(&footer(rows - FOOTER + 1, cols, step));
    out.into_bytes()
}

/// What the program writes for one frame after `step`: the footer
/// redrawn (the spinner moved), as Claude Code's renderer does many times
/// a second while it works.
pub fn frame(rows: u16, cols: u16, step: usize) -> Vec<u8> {
    let (rows, cols) = (usize::from(rows.max(8)), usize::from(cols.max(20)));
    footer(rows - FOOTER + 1, cols, step + 1).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_screen_is_full_and_moves() {
        let mut parser = vt100::Parser::new(24, 90, 0);
        parser.process(&screen(24, 90, 5));
        let text = parser.screen().contents();
        assert!(text.contains("⏺ Update(src/part"), "{text}");
        assert!(text.contains("✻ Thinking"), "{text}");
        assert!(text.lines().filter(|l| !l.trim().is_empty()).count() > 15);
        let before = parser.screen().clone();
        parser.process(&frame(24, 90, 5));
        assert_ne!(parser.screen().contents(), before.contents(), "the spinner moved");
        // Only the footer changed.
        let rows = |s: &vt100::Screen| s.rows(0, 90).collect::<Vec<_>>();
        assert_eq!(rows(parser.screen())[..18], rows(&before)[..18]);
    }
}
