//! Files the app carries inside its binary, served to GPUI the way Zed's
//! `assets` crate serves Zed's: `ui::Icon` asks for `icons/<name>.svg`, the
//! theme registry lists `themes/`, and the default keymaps and fonts are read
//! from here. All of it comes from Zed's `assets/` at the pinned tag, with
//! the licences beside each set, except ainet's own Deepharness theme.

use std::borrow::Cow;

use anyhow::Context as _;
use gpui::{App, AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

/// The bundled files. Debug builds embed them too, so the binary never
/// depends on the source tree.
#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "fonts/**/*.ttf"]
#[include = "icons/*.svg"]
#[include = "keymaps/*.json"]
#[include = "themes/**/*.json"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // Unknown paths are not errors: GPUI then draws nothing, as Zed does
        // for a missing icon.
        Ok(Self::get(path).map(|file| file.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

impl Assets {
    /// Registers the bundled fonts (IBM Plex Sans and Lilex, which GPUI
    /// knows as `.ZedSans` and `.ZedMono`) with the text system.
    pub fn load_fonts(cx: &App) -> anyhow::Result<()> {
        let fonts = Self::iter()
            .filter(|p| p.starts_with("fonts/") && p.ends_with(".ttf"))
            .filter_map(|p| Self::get(&p).map(|file| file.data))
            .collect();
        cx.text_system()
            .add_fonts(fonts)
            .context("cannot load the bundled fonts")
    }

    /// A bundled text file, such as a default keymap.
    pub fn text(path: &str) -> Option<Cow<'static, str>> {
        match Self::get(path)?.data {
            Cow::Borrowed(bytes) => std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
            Cow::Owned(bytes) => String::from_utf8(bytes).ok().map(Cow::Owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator as _;
    use ui::IconName;

    use super::*;

    #[test]
    fn serves_every_icon_ui_can_name() {
        for icon in IconName::iter() {
            let path = icon.path();
            let svg = Assets.load(&path).unwrap().unwrap_or_else(|| panic!("{path}"));
            assert!(svg.starts_with(b"<svg"), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }

    #[test]
    fn bundles_themes_fonts_and_keymaps() {
        let themes = Assets.list("themes/").unwrap();
        for family in [
            "one/one.json",
            "ayu/ayu.json",
            "gruvbox/gruvbox.json",
            "deepharness/deepharness.json",
        ] {
            assert!(themes.iter().any(|p| p.ends_with(family)), "{family}");
        }
        let fonts = Assets.list("fonts/").unwrap();
        assert!(fonts.iter().any(|p| p.ends_with("IBMPlexSans-Regular.ttf")));
        assert!(fonts.iter().any(|p| p.ends_with("Lilex-Regular.ttf")));
        assert!(Assets::text("keymaps/default-linux.json").is_some());
        assert!(Assets::text("keymaps/default-macos.json").is_some());
    }
}
