//! Files the app carries inside its binary, served to GPUI the way Zed's
//! `assets` crate serves Zed's: `ui::Icon` asks for `icons/<name>.svg`.
//! Phase 1 bundles only the icons it draws (Lucide, ISC: see
//! `assets/icons/LICENSES`); phase 2 adds themes and fonts.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((
            concat!("icons/", $name, ".svg"),
            include_bytes!(concat!("../assets/icons/", $name, ".svg")),
        )),*]
    };
}

const FILES: &[(&str, &[u8])] = icons![
    "bell",
    "chevron_down",
    "generic_close",
    "generic_maximize",
    "generic_minimize",
    "generic_restore",
    "list_tree",
    "server",
    "terminal",
    "user_group",
];

/// The bundled files.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // Unknown paths are not errors: GPUI then draws nothing, as Zed does
        // for a missing icon.
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use ui::IconName;

    use super::*;

    #[test]
    fn serves_the_icons_the_views_use() {
        for icon in [
            IconName::Bell,
            IconName::ChevronDown,
            IconName::GenericClose,
            IconName::GenericMaximize,
            IconName::GenericMinimize,
            IconName::GenericRestore,
            IconName::ListTree,
            IconName::Server,
            IconName::Terminal,
            IconName::UserGroup,
        ] {
            let path = icon.path();
            let svg = Assets.load(&path).unwrap().unwrap_or_else(|| panic!("{path}"));
            assert!(svg.starts_with(b"<svg"), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons/").unwrap().len(), FILES.len());
    }
}
