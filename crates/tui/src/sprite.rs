//! Critters and the wordmark as ratatui text.

use ainet_critter::{Cell, Pose};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{Palette, c, mix};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// 16x8 half blocks.
    Big,
    /// 8x4 half blocks, half resolution.
    Small,
    /// 8x4 octants, full resolution (Unicode 16 fonts).
    Octant,
}

/// The critter for `id`, animated at `tick`.
pub fn critter(pal: &Palette, id: &str, tick: u64, working: bool, size: Size) -> Vec<Line<'static>> {
    let seed = ainet_critter::hash(id);
    let pose = Pose::at(seed, tick, working);
    let px = ainet_critter::pixels(seed, &pal.critter_colors(id), pose);
    match size {
        Size::Big => lines(&ainet_critter::half_blocks(&px)),
        Size::Small => lines(&ainet_critter::small(&px)),
        Size::Octant => lines(&ainet_critter::octants(&px)),
    }
}

/// A ghostly, greyed critter for agents that are gone.
pub fn ghost(pal: &Palette, id: &str, size: Size) -> Vec<Line<'static>> {
    let mut out = critter(pal, id, 0, false, size);
    for line in &mut out {
        for span in &mut line.spans {
            span.style = Style::new().fg(c(pal.muted)).add_modifier(Modifier::DIM);
        }
    }
    out
}

fn lines(rows: &[impl AsRef<[Cell]>]) -> Vec<Line<'static>> {
    rows.iter()
        .map(|row| {
            Line::from(
                row.as_ref()
                    .iter()
                    .map(|cell| {
                        let mut style = Style::new();
                        if let Some(fg) = cell.fg {
                            style = style.fg(c(fg));
                        }
                        if let Some(bg) = cell.bg {
                            style = style.bg(c(bg));
                        }
                        Span::styled(cell.ch.to_string(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// The three-row block-letter logo, drawn the way Den draws its own.
const WORDMARK: [&str; 3] = [
    "▄▀▀▄ ▀█▀ █▄  █ █▀▀▀ ▀▀█▀▀",
    "█▄▄█  █  █ ▀▄█ █▀▀    █  ",
    "█  █ ▄█▄ █   █ █▄▄▄   █  ",
];

/// The wordmark split into six half-cell pixel rows, shaded from white at
/// the top to grey at the foot, with a band of light that sweeps across it
/// every few seconds.
pub fn wordmark(pal: &Palette, tick: u64) -> Vec<Line<'static>> {
    let bands = [
        pal.bright,
        pal.fg,
        pal.fg,
        mix(pal.fg, pal.fg_dim, 0.5),
        pal.fg_dim,
        pal.fg_dim,
    ];
    // The shimmer: a soft highlight column travelling left to right, then a rest.
    let cycle = 60;
    let pos = (tick % cycle) as f32 * 0.9 - 6.0;
    let shine = |x: usize, base: [u8; 3]| {
        let d = (x as f32 - pos).abs();
        if d < 3.0 {
            mix(base, pal.bright, (1.0 - d / 3.0) * 0.85)
        } else {
            base
        }
    };
    WORDMARK
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let (top, bottom) = (bands[2 * i], bands[2 * i + 1]);
            Line::from(
                row.chars()
                    .enumerate()
                    .map(|(x, ch)| {
                        let (t, b) = (shine(x, top), shine(x, bottom));
                        match ch {
                            '█' => Span::styled("▀", Style::new().fg(c(t)).bg(c(b))),
                            '▀' => Span::styled("▀", Style::new().fg(c(t))),
                            '▄' => Span::styled("▄", Style::new().fg(c(b))),
                            other => Span::raw(other.to_string()),
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}
