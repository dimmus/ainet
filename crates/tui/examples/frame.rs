//! Renders the demo without a terminal, for checking layouts and making
//! README screenshots.
//!
//!     cargo run -p ainet-tui --example frame -- 150 46            # plain text
//!     cargo run -p ainet-tui --example frame -- 150 46 svg map > map.svg
//!     magick map.svg map.png
//!
//! Views: fleet (default), terminals, zoom, comms, inbox, map, review, critters, help.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let width = args.first().and_then(|a| a.parse().ok()).unwrap_or(150);
    let height = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(46);
    let svg = args.get(2).is_some_and(|a| a == "svg");
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let keys: Vec<KeyEvent> = match args.get(3).map(String::as_str) {
        Some("terminals") => vec![ctrl('g')],
        Some("zoom") => vec![ctrl('g'), ctrl('z')],
        Some("comms") => vec![KeyEvent::from(KeyCode::Tab), KeyEvent::from(KeyCode::Tab)],
        Some("inbox") => vec![
            KeyEvent::from(KeyCode::Tab),
            KeyEvent::from(KeyCode::Tab),
            KeyEvent::from(KeyCode::Tab),
        ],
        Some("map") => vec![ctrl('t')],
        Some("review") => vec![KeyEvent::from(KeyCode::BackTab)],
        Some("critters") => vec![ctrl('e')],
        Some("octants") => vec![ctrl('e'), ctrl('e')],
        Some("help") => vec![KeyEvent::from(KeyCode::Char('?'))],
        _ => vec![],
    };
    let buf = ainet_tui::demo_buffer(width, height, 12, &keys)?;
    if svg {
        print!("{}", to_svg(&buf));
    } else {
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            println!("{row}");
        }
    }
    Ok(())
}

const CW: f32 = 9.0;
const CH: f32 = 18.0;

fn rgb(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.to_string(),
    }
}

fn to_svg(buf: &Buffer) -> String {
    let (w, h) = (buf.area.width, buf.area.height);
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}' font-family='DejaVu Sans Mono' font-size='15'>\n<rect width='100%' height='100%' fill='#000000'/>\n",
        w as f32 * CW,
        h as f32 * CH
    );
    for y in 0..h {
        for x in 0..w {
            let cell = &buf[(x, y)];
            let (px, py) = (x as f32 * CW, y as f32 * CH);
            let bg = rgb(cell.bg, "#000000");
            let fg = rgb(cell.fg, "#dfdfdf");
            if bg != "#000000" {
                s += &format!("<rect x='{px}' y='{py}' width='{CW}' height='{CH}' fill='{bg}'/>");
            }
            let sym = cell.symbol();
            let rect = |x0: f32, y0: f32, w0: f32, h0: f32| {
                format!(
                    "<rect x='{}' y='{}' width='{}' height='{}' fill='{fg}'/>",
                    px + x0,
                    py + y0,
                    w0,
                    h0
                )
            };
            match sym {
                " " | "" => {}
                "▀" => s += &rect(0.0, 0.0, CW, CH / 2.0),
                "▄" => s += &rect(0.0, CH / 2.0, CW, CH / 2.0),
                "█" => s += &rect(0.0, 0.0, CW, CH),
                "▌" => s += &rect(0.0, 0.0, CW / 2.0, CH),
                "▐" => s += &rect(CW / 2.0, 0.0, CW / 2.0, CH),
                _ => {
                    let esc = sym
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                        .replace('\'', "&apos;");
                    let bold = if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
                        " font-weight='bold'"
                    } else {
                        ""
                    };
                    let under = if cell.modifier.contains(ratatui::style::Modifier::UNDERLINED) {
                        " text-decoration='underline'"
                    } else {
                        ""
                    };
                    s += &format!(
                        "<text x='{px}' y='{}' fill='{fg}'{bold}{under}>{esc}</text>",
                        py + 14.0
                    );
                }
            }
        }
        s.push('\n');
    }
    s + "</svg>\n"
}
