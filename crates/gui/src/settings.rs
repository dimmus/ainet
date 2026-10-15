//! The appearance settings Zed's `theme` and `ui` crates read: fonts, font
//! sizes and UI density. Zed backs them with its settings files; ainet
//! registers its own provider. Phase 1 has only defaults; phase 2 reads
//! them from `~/.config/ainet/settings.json`.

use gpui::{App, Font, Pixels, font, px};
use theme::{ThemeSettingsProvider, UiDensity};

/// Fonts, sizes and density for the whole app.
#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    pub ui_font: Font,
    /// The font for terminals and other monospace text.
    pub buffer_font: Font,
    pub ui_font_size: Pixels,
    pub buffer_font_size: Pixels,
    pub density: UiDensity,
}

impl Default for Appearance {
    /// Zed's own defaults: its Plex UI font and Lilex for code, at 16 and 15
    /// pixels. GPUI resolves the `.Zed…` names and falls back to system
    /// fonts until phase 2 bundles them.
    fn default() -> Self {
        Appearance {
            ui_font: font(".ZedSans"),
            buffer_font: font(".ZedMono"),
            ui_font_size: px(16.),
            buffer_font_size: px(15.),
            density: UiDensity::Default,
        }
    }
}

/// Registers `appearance` as what the theme and ui crates see.
pub fn init(appearance: Appearance, cx: &mut App) {
    theme::set_theme_settings_provider(Box::new(Provider(appearance)), cx);
}

/// The UI font and its size, as registered; the size is also the window's
/// rem size, as in Zed.
pub fn ui_font(cx: &App) -> (Font, Pixels) {
    let settings = theme::theme_settings(cx);
    (settings.ui_font(cx).clone(), settings.ui_font_size(cx))
}

struct Provider(Appearance);

impl ThemeSettingsProvider for Provider {
    fn ui_font<'a>(&'a self, _cx: &'a App) -> &'a Font {
        &self.0.ui_font
    }

    fn buffer_font<'a>(&'a self, _cx: &'a App) -> &'a Font {
        &self.0.buffer_font
    }

    fn ui_font_size(&self, _cx: &App) -> Pixels {
        self.0.ui_font_size
    }

    fn buffer_font_size(&self, _cx: &App) -> Pixels {
        self.0.buffer_font_size
    }

    fn ui_density(&self, _cx: &App) -> UiDensity {
        self.0.density
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_zed() {
        let a = Appearance::default();
        assert_eq!(a.ui_font.family.as_ref(), ".ZedSans");
        assert_eq!(a.buffer_font.family.as_ref(), ".ZedMono");
        assert_eq!(a.ui_font_size, px(16.));
        assert_eq!(a.buffer_font_size, px(15.));
        assert_eq!(a.density, UiDensity::Default);
    }
}
