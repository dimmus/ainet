//! Colours. Den's "Deepharness" rule: surfaces near black, text a white
//! opacity ladder, hue reserved for status and for each agent's body colour.
//! Follows the current Omarchy theme when there is one.

use ainet_critter::{Rgb, parse_hex};
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Debug)]
pub struct Palette {
    pub bg: Rgb,
    pub surface: Rgb,
    pub sel: Rgb,
    pub fg: Rgb,
    pub fg_dim: Rgb,
    pub muted: Rgb,
    pub bright: Rgb,
    pub ok: Rgb,
    pub attn: Rgb,
    pub err: Rgb,
    pub info: Rgb,
    pub code: Rgb,
    pub blush: Rgb,
    /// Agent body colours, in assignment order.
    pub hues: Vec<Rgb>,
}

/// Soft, distinct hues that read on a dark background.
const HUES: [&str; 12] = [
    "#ea9e9e", "#f0b07a", "#e8d27a", "#a7d98c", "#66cb8e", "#7fd3c4", "#86c6e8", "#9bb0f0", "#b9a3f0",
    "#e39ad8", "#f0a8c0", "#c9b89a",
];

fn hex(s: &str) -> Rgb {
    parse_hex(s).unwrap_or([0x80; 3])
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            bg: hex("#000000"),
            surface: hex("#0d0d0d"),
            sel: hex("#2e2e2e"),
            fg: hex("#dfdfdf"),
            fg_dim: hex("#7c7c7c"),
            muted: hex("#575757"),
            bright: hex("#f8f8f8"),
            ok: hex("#66cb8e"),
            attn: hex("#e8b84a"),
            err: hex("#ea9e9e"),
            info: hex("#86c6e8"),
            code: hex("#9bb0f0"),
            blush: hex("#f4c4c4"),
            hues: HUES.iter().map(|h| hex(h)).collect(),
        }
    }
}

impl Palette {
    /// The Omarchy palette when `colors.toml` exists, else Deepharness.
    pub fn load() -> Self {
        let mut p = Palette::default();
        let Ok(raw) = std::fs::read_to_string(ainet_core::paths::omarchy_colors()) else {
            return p;
        };
        let kv: std::collections::HashMap<&str, Rgb> = raw
            .lines()
            .filter_map(|l| l.split_once('='))
            .filter_map(|(k, v)| Some((k.trim(), parse_hex(v.trim().trim_matches('"'))?)))
            .collect();
        let set = |dst: &mut Rgb, keys: &[&str]| {
            if let Some(v) = keys.iter().find_map(|k| kv.get(k)) {
                *dst = *v;
            }
        };
        set(&mut p.bg, &["background"]);
        set(&mut p.surface, &["dark_background", "lighter_background"]);
        set(&mut p.sel, &["selection_background", "selection"]);
        set(&mut p.fg, &["foreground"]);
        set(&mut p.fg_dim, &["dark_foreground"]);
        set(&mut p.muted, &["muted"]);
        set(&mut p.bright, &["bright_foreground", "accent"]);
        set(&mut p.ok, &["green"]);
        set(&mut p.attn, &["yellow", "orange"]);
        set(&mut p.err, &["red"]);
        set(&mut p.info, &["blue", "cyan"]);
        set(&mut p.code, &["cyan", "blue", "accent"]);
        set(&mut p.blush, &["bright_red", "red"]);
        p
    }

    /// The body colour for an agent: stable for its id.
    pub fn hue(&self, id: &str) -> Rgb {
        self.hues[ainet_critter::hash(id) as usize % self.hues.len()]
    }

    pub fn critter_colors(&self, id: &str) -> ainet_critter::Colors {
        ainet_critter::Colors {
            bodies: vec![self.hue(id)],
            dark: self.bg,
            blush: self.blush,
        }
    }

    /// A raised surface for borders and gauge tracks.
    pub fn surface2(&self) -> Rgb {
        mix(self.bg, self.sel, 0.5)
    }

    pub fn style(&self, fg: Rgb) -> Style {
        Style::new().fg(c(fg))
    }

    pub fn text(&self) -> Style {
        self.style(self.fg)
    }
    pub fn dim(&self) -> Style {
        self.style(self.fg_dim)
    }
    pub fn muted(&self) -> Style {
        self.style(self.muted)
    }
    pub fn bold(&self) -> Style {
        self.style(self.bright).add_modifier(Modifier::BOLD)
    }
}

pub fn c(rgb: Rgb) -> Color {
    Color::Rgb(rgb[0], rgb[1], rgb[2])
}

/// Linear mix, `t` = 0 gives `a`.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8)
}

/// 0..1..0 over `period` ticks: for pulsing.
pub fn pulse(tick: u64, period: u64) -> f32 {
    let phase = (tick % period) as f32 / period as f32;
    0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()
}
