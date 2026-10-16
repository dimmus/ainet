//! Colours. The palette itself (Deepharness, or the current Omarchy theme)
//! lives in `ainet-model`, shared with the GUI; this adds ratatui styles.

use ainet_critter::Rgb;
pub use ainet_model::palette::{Palette, mix, pulse};
use ratatui::style::{Color, Modifier, Style};

/// The palette's text styles.
pub trait Styles {
    fn style(&self, fg: Rgb) -> Style;
    fn text(&self) -> Style;
    fn dim(&self) -> Style;
    fn muted(&self) -> Style;
    fn bold(&self) -> Style;
}

impl Styles for Palette {
    fn style(&self, fg: Rgb) -> Style {
        Style::new().fg(c(fg))
    }

    fn text(&self) -> Style {
        self.style(self.fg)
    }
    fn dim(&self) -> Style {
        self.style(self.fg_dim)
    }
    fn muted(&self) -> Style {
        self.style(self.muted)
    }
    fn bold(&self) -> Style {
        self.style(self.bright).add_modifier(Modifier::BOLD)
    }
}

pub fn c(rgb: Rgb) -> Color {
    Color::Rgb(rgb[0], rgb[1], rgb[2])
}
