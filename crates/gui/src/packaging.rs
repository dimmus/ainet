//! What installing the app puts beside it (`just install-gui`,
//! `packaging/arch/PKGBUILD`): the desktop entry `assets/ainet-gui.desktop`
//! and its icon `assets/ainet-gui.svg`, a critter drawn by
//! `ainet-critter`. The icon is written once and kept in the repository
//! (no build script); a test keeps it the critter and the entry in step
//! with the app.

use ainet_critter::{Colors, DEFAULT_BODIES, N, Pose, hash, pixels, to_hex};
use ainet_model::Palette;

const ICON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ainet-gui.svg");
const DESKTOP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ainet-gui.desktop");

/// The icon: ainet's own critter (seeded by its name, in the first of the
/// default bodies) on a rounded square in Deepharness's surface colour,
/// one rectangle per run of a colour, pixels kept crisp at any size.
fn icon() -> String {
    let palette = Palette::default();
    let colors = Colors {
        bodies: vec![DEFAULT_BODIES[0]],
        dark: palette.bg,
        blush: palette.blush,
    };
    let grid = pixels(hash("ainet"), &colors, Pose::default());
    let margin = 2;
    let side = N + 2 * margin;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" width=\"256\" height=\"256\">\n\
         <rect width=\"{side}\" height=\"{side}\" rx=\"4\" fill=\"{}\"/>\n\
         <g shape-rendering=\"crispEdges\">\n",
        to_hex(palette.surface)
    );
    for (y, row) in grid.iter().enumerate() {
        let mut x = 0;
        while x < N {
            let Some(color) = row[x] else {
                x += 1;
                continue;
            };
            let start = x;
            while x < N && row[x] == Some(color) {
                x += 1;
            }
            svg += &format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"1\" fill=\"{}\"/>\n",
                start + margin,
                y + margin,
                x - start,
                to_hex(color)
            );
        }
    }
    svg + "</g>\n</svg>\n"
}

#[test]
fn the_icon_is_ainets_critter() {
    // `AINET_WRITE_ICON=1 cargo test --manifest-path crates/gui/Cargo.toml
    // ainets_critter` draws it anew, after a change to the critters.
    if std::env::var_os("AINET_WRITE_ICON").is_some() {
        std::fs::write(ICON, icon()).unwrap();
        return;
    }
    let file = std::fs::read_to_string(ICON).unwrap_or_default();
    assert!(file == icon(), "run with AINET_WRITE_ICON=1 to draw {ICON} again");
    assert!(
        file.matches("<rect").count() > 10,
        "a critter, not an empty square"
    );
}

#[test]
fn the_desktop_entry_names_the_app() {
    let text = std::fs::read_to_string(DESKTOP).unwrap();
    assert_eq!(text.lines().next(), Some("[Desktop Entry]"));
    let value = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_else(|| panic!("{key} in {DESKTOP}"))
    };
    assert_eq!(value("Type"), "Application");
    // The binary, its icon (`ainet-gui.svg`) and the window's app id must
    // agree, or the desktop shows a second, iconless window in the dock.
    assert_eq!(value("Exec"), "ainet-gui");
    assert_eq!(value("Icon"), "ainet-gui");
    assert_eq!(value("StartupWMClass"), crate::APP_ID);
    assert_eq!(value("Terminal"), "false");
    assert!(value("Categories").ends_with(';'));
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('[')) {
        assert!(line.contains('='), "{line:?} is not key=value");
    }
}
