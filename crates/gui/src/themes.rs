//! Themes: Zed's bundled families (One, Ayu, Gruvbox), ainet's Deepharness,
//! when Omarchy is installed an Omarchy theme made from its current colours,
//! and the user's Zed theme files, all in Zed's own [`ThemeRegistry`]. Which
//! one is active follows the `theme` setting and the system's light or dark
//! appearance.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ainet_critter::Rgb;
use ainet_model::Palette;
use ainet_model::palette::mix;
use gpui::{App, Global, SharedString, Task};
use serde_json::{Map, Value, json};
use theme::{
    ActiveTheme as _, Appearance, GlobalTheme, LoadThemes, PlayerColor, SystemAppearance, Theme,
    ThemeRegistry,
};

use crate::assets::Assets;
use crate::settings::{self, ThemeSelection};
use crate::zed_theme::{ThemeFamilyContent, refine_theme_family};
use crate::{problems, watch};

/// ainet's own dark theme: the TUI's palette.
pub const DEEPHARNESS: &str = "Deepharness";
/// The theme made from the current Omarchy colours.
pub const OMARCHY: &str = "Omarchy";

/// Where the user's themes live: `$XDG_CONFIG_HOME/ainet/themes`.
pub fn user_dir() -> PathBuf {
    ainet_core::paths::config_dir().join("themes")
}

/// Sets up Zed's theme system with every bundled theme family and Omarchy
/// when it is installed. Returns what could not be loaded.
pub fn init(cx: &mut App) -> Vec<String> {
    theme::init(LoadThemes::All(Box::new(Assets)), cx);
    let registry = ThemeRegistry::global(cx);
    let mut problems = load_bundled(&registry);
    let mut palettes = Palettes::default();
    palettes.0.insert(DEEPHARNESS.into(), Palette::default());
    if let Some(palette) = Palette::omarchy() {
        match family(OMARCHY, "Omarchy (via ainet)", &palette) {
            Ok(family) => {
                registry.insert_theme_families([family]);
                palettes.0.insert(OMARCHY.into(), palette);
            }
            Err(e) => problems.push(format!("Omarchy theme: {e}")),
        }
    }
    cx.set_global(palettes);
    problems
}

/// The user's themes as loaded, by name, to take out again when their
/// directory changes; and the watch on it.
#[derive(Default)]
struct UserThemes {
    names: Vec<SharedString>,
    _watch: Option<Task<()>>,
}

impl Global for UserThemes {}

/// Loads the theme files in `dir` and follows it: a file added, changed or
/// removed reloads them all, and the theme in use is applied again (so an
/// edited theme shows at once). What does not load is in the status bar.
pub fn follow_user(dir: PathBuf, cx: &mut App) {
    reload_user(&dir, cx);
    let watch = watch::watch_dir(dir.clone(), cx, move |cx| {
        reload_user(&dir, cx);
        settings::apply_theme(cx);
    });
    cx.default_global::<UserThemes>()._watch = Some(watch);
}

/// Takes the user's themes loaded before out of the registry and loads
/// `dir`'s anew. Bundled themes a user file stood in for come back.
pub fn reload_user(dir: &Path, cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let old = std::mem::take(&mut cx.default_global::<UserThemes>().names);
    if !old.is_empty() {
        registry.remove_user_themes(&old);
        load_bundled(&registry);
        if let Some(palette) = cx.try_global::<Palettes>().and_then(|p| p.0.get(OMARCHY))
            && let Ok(family) = family(OMARCHY, "Omarchy (via ainet)", palette)
        {
            registry.insert_theme_families([family]);
        }
    }
    let (problems, names) = load_user(dir, &registry);
    cx.default_global::<UserThemes>().names = names;
    let problem = (!problems.is_empty()).then(|| problems.join("; "));
    problems::set(problems::Source::Themes, problem, cx);
}

/// Loads every theme family under `themes/`, as Zed's
/// `theme_settings::load_bundled_themes` does.
pub fn load_bundled(registry: &ThemeRegistry) -> Vec<String> {
    let mut problems = Vec::new();
    let paths = registry.assets().list("themes/").unwrap_or_default();
    for path in paths.iter().filter(|p| p.ends_with(".json")) {
        let parsed = registry
            .assets()
            .load(path)
            .ok()
            .flatten()
            .ok_or_else(|| "missing".to_string())
            .and_then(|bytes| parse_family(&bytes));
        match parsed {
            Ok(family) => registry.insert_theme_families([family]),
            Err(e) => problems.push(format!("theme {path}: {e}")),
        }
    }
    problems
}

/// Loads every `*.json` in `dir`: Zed theme files, such as the ones Zed's
/// theme extensions ship. A missing directory is fine; a file that does
/// not parse is reported and skipped. They come after the bundled themes,
/// so a file can replace one by name. Returns what could not be loaded,
/// and the names of the themes loaded.
pub fn load_user(dir: &Path, registry: &ThemeRegistry) -> (Vec<String>, Vec<SharedString>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    let (mut problems, mut names) = (Vec::new(), Vec::new());
    for path in paths {
        let parsed = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| parse_family(&bytes));
        match parsed {
            Ok(family) => {
                names.extend(family.themes.iter().map(|t| t.name.clone()));
                registry.insert_theme_families([family]);
            }
            Err(e) => problems.push(format!("theme {}: {e}", path.display())),
        }
    }
    (problems, names)
}

/// A theme family from Zed theme JSON (comments and trailing commas
/// allowed, as in Zed).
fn parse_family(bytes: &[u8]) -> Result<theme::ThemeFamily, String> {
    let content: ThemeFamilyContent = serde_json_lenient::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(refine_theme_family(content))
}

/// The dark theme when none is set: Omarchy when it is installed, as the
/// TUI follows it, else Deepharness.
pub fn default_dark(registry: &ThemeRegistry) -> &'static str {
    if registry.get(OMARCHY).is_ok() {
        OMARCHY
    } else {
        DEEPHARNESS
    }
}

/// The theme `selection` names for the system's appearance; with none,
/// the default dark theme whatever the system looks like, as the TUI is
/// always dark. An unknown name falls back to the default and is reported.
pub fn resolve(
    selection: Option<&ThemeSelection>,
    appearance: Appearance,
    registry: &ThemeRegistry,
) -> (Arc<Theme>, Option<String>) {
    let default = default_dark(registry);
    let name = selection.map_or(default, |s| s.name(appearance));
    match registry.get(name) {
        Ok(theme) => (theme, None),
        Err(_) => {
            let theme = registry
                .get(default)
                .or_else(|_| registry.get(DEEPHARNESS))
                .unwrap_or_else(|_| fallback(registry));
            let problem = format!("unknown theme \"{name}\"; using {}", theme.name);
            (theme, Some(problem))
        }
    }
}

/// Makes the theme the settings name active. Returns a problem to show if
/// the name is unknown.
pub fn apply(selection: Option<&ThemeSelection>, cx: &mut App) -> Option<String> {
    let registry = ThemeRegistry::global(cx);
    let (theme, problem) = resolve(selection, SystemAppearance::global(cx).0, &registry);
    // By identity, not name: `theme::init` makes Zed's built-in "One Dark"
    // active, and the bundled One Dark that replaces it in the registry
    // has the same name but other colours.
    if !Arc::ptr_eq(cx.theme(), &theme) {
        GlobalTheme::update_theme(cx, theme);
        cx.refresh_windows();
    }
    problem
}

/// An agent's colour in the active theme: one of the theme's player
/// colours, stable for the agent. In Deepharness and Omarchy these are the
/// TUI's agent hues in the TUI's order, so an agent looks the same in both.
pub fn agent_color(id: &str, cx: &App) -> PlayerColor {
    cx.theme()
        .players()
        .color_for_participant(ainet_critter::hash(id) as u32)
}

/// The palettes ainet's own themes were made from, by theme name, for
/// what a Zed theme has no colour for (the critters' cheeks and ink).
#[derive(Default)]
struct Palettes(HashMap<String, Palette>);

impl Global for Palettes {}

/// The palette the active theme was made from, if it is one of ainet's
/// own (Deepharness, Omarchy).
pub fn palette(cx: &App) -> Option<&Palette> {
    cx.try_global::<Palettes>()?.0.get(cx.theme().name.as_ref())
}

/// Zed's built-in dark theme, which is always registered.
fn fallback(registry: &ThemeRegistry) -> Arc<Theme> {
    registry
        .get(theme::DEFAULT_DARK_THEME)
        .ok()
        .or_else(|| {
            let first = registry.list().into_iter().next()?;
            registry.get(&first.name).ok()
        })
        .expect("the theme registry always has Zed's base theme")
}

/// A one-theme family named `name` made from a palette.
pub fn family(name: &str, author: &str, palette: &Palette) -> anyhow::Result<theme::ThemeFamily> {
    let content: ThemeFamilyContent = serde_json::from_value(family_json(name, author, palette))?;
    Ok(refine_theme_family(content))
}

/// A palette as a Zed theme family: Den's rule of near-black surfaces, text
/// in steps of grey, and hue only for status and agents. Deepharness is this
/// for the default palette, bundled as `themes/deepharness/deepharness.json`
/// (a test keeps the two equal).
pub fn family_json(name: &str, author: &str, p: &Palette) -> Value {
    let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}ff", c[0], c[1], c[2]);
    let alpha = |c: Rgb, a: u8| format!("#{:02x}{:02x}{:02x}{a:02x}", c[0], c[1], c[2]);
    let toward = |a: Rgb, b: Rgb, t: f32| hex(mix(a, b, t));
    let white = [0xff; 3];

    let raised = p.surface2();
    let border = mix(p.bg, p.sel, 0.8);
    let elevated = mix(p.surface, p.sel, 0.35);
    // Zed's terminal ANSI colours, from the palette's status hues; magenta
    // and cyan borrow two agent hues.
    let magenta = p.hues.get(9).copied().unwrap_or(p.code);
    let cyan = p.hues.get(5).copied().unwrap_or(p.info);
    let ansi = [
        ("black", p.sel),
        ("red", p.err),
        ("green", p.ok),
        ("yellow", p.attn),
        ("blue", p.info),
        ("magenta", magenta),
        ("cyan", cyan),
        ("white", p.fg),
    ];

    let mut colors = Map::new();
    let mut set = |key: &str, value: String| {
        colors.insert(key.to_string(), Value::String(value));
    };
    set("border", hex(border));
    set("border.variant", hex(raised));
    set("border.focused", toward(p.bg, p.info, 0.6));
    set("border.selected", toward(p.bg, p.info, 0.35));
    set("border.transparent", "#00000000".into());
    set("border.disabled", hex(raised));
    set("elevated_surface.background", hex(elevated));
    set("surface.background", hex(elevated));
    set("background", hex(p.surface));
    set("element.background", hex(raised));
    set("element.hover", toward(raised, p.sel, 0.5));
    set("element.active", hex(p.sel));
    set("element.selected", hex(p.sel));
    set("element.disabled", hex(raised));
    set("drop_target.background", alpha(p.info, 0x30));
    set("ghost_element.background", "#00000000".into());
    set("ghost_element.hover", hex(raised));
    set("ghost_element.active", hex(p.sel));
    set("ghost_element.selected", hex(p.sel));
    set("ghost_element.disabled", hex(raised));
    set("text", hex(p.fg));
    set("text.muted", hex(p.fg_dim));
    set("text.placeholder", hex(p.muted));
    set("text.disabled", hex(p.muted));
    set("text.accent", hex(p.info));
    set("icon", hex(p.fg));
    set("icon.muted", hex(p.fg_dim));
    set("icon.disabled", hex(p.muted));
    set("icon.placeholder", hex(p.fg_dim));
    set("icon.accent", hex(p.info));
    set("status_bar.background", hex(p.surface));
    set("title_bar.background", hex(p.surface));
    set("title_bar.inactive_background", hex(p.bg));
    set("toolbar.background", hex(p.bg));
    set("tab_bar.background", hex(p.surface));
    set("tab.inactive_background", hex(p.surface));
    set("tab.active_background", hex(p.bg));
    set("search.match_background", alpha(p.info, 0x40));
    set("search.active_match_background", alpha(p.attn, 0x40));
    set("panel.background", hex(p.surface));
    set("panel.focused_border", toward(p.bg, p.info, 0.6));
    set("pane.focused_border", toward(p.bg, p.info, 0.6));
    set("scrollbar.thumb.background", alpha(p.fg, 0x30));
    set("scrollbar.thumb.hover_background", hex(p.sel));
    set("scrollbar.thumb.border", hex(raised));
    set("scrollbar.track.background", "#00000000".into());
    set("scrollbar.track.border", hex(raised));
    set("editor.foreground", hex(p.fg));
    set("editor.background", hex(p.bg));
    set("editor.gutter.background", hex(p.bg));
    set("editor.subheader.background", hex(p.surface));
    set("editor.active_line.background", alpha(raised, 0xbf));
    set("editor.highlighted_line.background", hex(raised));
    set("editor.line_number", hex(p.muted));
    set("editor.active_line_number", hex(p.fg));
    set("editor.hover_line_number", hex(p.fg_dim));
    set("editor.invisible", hex(p.muted));
    set("editor.wrap_guide", alpha(p.fg, 0x0d));
    set("editor.active_wrap_guide", alpha(p.fg, 0x1a));
    set("editor.document_highlight.read_background", alpha(p.info, 0x1a));
    set("editor.document_highlight.write_background", alpha(p.sel, 0x66));
    set("terminal.background", hex(p.bg));
    set("terminal.foreground", hex(p.fg));
    set("terminal.bright_foreground", hex(p.bright));
    set("terminal.dim_foreground", hex(p.fg_dim));
    for (name, c) in ansi {
        let bright = if name == "black" {
            p.muted
        } else {
            mix(c, white, 0.25)
        };
        set(&format!("terminal.ansi.{name}"), hex(c));
        set(&format!("terminal.ansi.bright_{name}"), hex(bright));
        set(&format!("terminal.ansi.dim_{name}"), toward(c, p.bg, 0.3));
    }
    set("link_text.hover", hex(p.info));
    set("version_control.added", hex(p.ok));
    set("version_control.modified", hex(p.attn));
    set("version_control.deleted", hex(p.err));

    // Status hues: the colour, a faint wash of it, and a dim border.
    let status = [
        ("conflict", p.attn),
        ("created", p.ok),
        ("deleted", p.err),
        ("error", p.err),
        ("hidden", p.muted),
        ("hint", p.code),
        ("ignored", p.muted),
        ("info", p.info),
        ("modified", p.attn),
        ("predictive", p.fg_dim),
        ("renamed", p.info),
        ("success", p.ok),
        ("unreachable", p.fg_dim),
        ("warning", p.attn),
    ];
    for (name, c) in status {
        set(name, hex(c));
        set(&format!("{name}.background"), alpha(c, 0x1a));
        set(&format!("{name}.border"), toward(p.bg, c, 0.35));
    }

    // Player 0 is you (Zed's local player); the rest are the agents' hues,
    // in the TUI's order, which `agent_color` relies on.
    let player = |c: Rgb| json!({ "cursor": hex(c), "background": hex(c), "selection": alpha(c, 0x3d) });
    let players: Vec<Value> = std::iter::once(player(p.info))
        .chain(p.hues.iter().map(|&h| player(h)))
        .collect();

    let syntax = |c: Rgb| json!({ "color": hex(c) });
    json!({
        "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
        "name": name,
        "author": author,
        "themes": [{
            "name": name,
            "appearance": "dark",
            "style": Value::Object({
                let mut style = colors;
                style.insert("players".into(), Value::Array(players));
                style.insert("syntax".into(), json!({
                    "comment": syntax(p.muted),
                    "constant": syntax(p.code),
                    "function": syntax(p.info),
                    "keyword": syntax(p.code),
                    "number": syntax(p.attn),
                    "string": syntax(p.ok),
                    "type": syntax(p.info),
                    "variable": syntax(p.fg),
                    "punctuation": syntax(p.fg_dim),
                }));
                style
            }),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLED: &str = include_str!("../assets/themes/deepharness/deepharness.json");

    #[test]
    fn bundled_deepharness_is_the_default_palette() {
        let made = family_json(DEEPHARNESS, "ainet", &Palette::default());
        // `AINET_WRITE_THEME=1 cargo test -p ainet-gui` rewrites the file
        // after a palette change.
        if std::env::var_os("AINET_WRITE_THEME").is_some() {
            let path = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/themes/deepharness/deepharness.json"
            );
            std::fs::write(path, serde_json::to_string_pretty(&made).unwrap() + "\n").unwrap();
            return;
        }
        let bundled: Value = serde_json::from_str(BUNDLED).unwrap();
        assert_eq!(bundled, made, "run with AINET_WRITE_THEME=1 to regenerate");
    }

    #[gpui::test]
    fn one_dark_is_the_bundled_one_not_zeds_fallback(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            // `theme::init` starts on Zed's built-in One Dark, which has the
            // same name as the bundled one that replaces it in the registry.
            init(cx);
            let fallback = cx.theme().clone();
            assert_eq!(fallback.name, "One Dark");
            apply(Some(&ThemeSelection::Static("One Dark".into())), cx);
            let bundled = ThemeRegistry::global(cx).get("One Dark").unwrap();
            assert!(!Arc::ptr_eq(&fallback, &bundled));
            assert!(Arc::ptr_eq(cx.theme(), &bundled));
        });
    }

    #[gpui::test]
    fn user_theme_files_are_loaded_and_bad_ones_reported(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("ainet-gui-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine = family_json("Mine", "me", &Palette::default());
        std::fs::write(dir.join("mine.json"), mine.to_string()).unwrap();
        std::fs::write(dir.join("broken.json"), "{ \"name\": ").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        let problems = cx.update(|cx| {
            theme::init(LoadThemes::JustBase, cx);
            let registry = ThemeRegistry::global(cx);
            let (problems, names) = load_user(&dir, &registry);
            assert!(registry.get("Mine").is_ok());
            assert_eq!(names, ["Mine"]);
            problems
        });
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            problems.len() == 1 && problems[0].contains("broken.json"),
            "{problems:?}"
        );
        cx.update(|cx| {
            let registry = ThemeRegistry::global(cx);
            assert!(
                load_user(&dir, &registry).0.is_empty(),
                "no directory, no problem"
            );
        });
    }

    #[gpui::test]
    async fn user_themes_reload_when_their_directory_changes(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);
        let dir = std::env::temp_dir().join(format!("ainet-gui-themes-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        cx.update(|cx| follow_user(dir.clone(), cx));
        let settle = |cx: &mut gpui::TestAppContext| {
            cx.executor().advance_clock(watch::POLL * 2);
            cx.run_until_parked();
        };

        // A new file: its theme is there to use.
        std::fs::create_dir_all(&dir).unwrap();
        let mine = |bg: &str| {
            family_json(
                "Mine",
                "me",
                &Palette::from_omarchy(&format!("background = \"{bg}\"\n")),
            )
        };
        std::fs::write(dir.join("mine.json"), mine("#101418").to_string()).unwrap();
        settle(cx);
        cx.update(|cx| {
            assert!(ThemeRegistry::global(cx).get("Mine").is_ok());
            settings::reload(r#"{ "theme": "Mine" }"#, cx);
            assert_eq!(cx.theme().name, "Mine");
        });
        let first = cx.update(|cx| cx.theme().colors().editor_background);

        // Edited: the theme in use shows the change at once.
        std::fs::write(dir.join("mine.json"), mine("#203040").to_string()).unwrap();
        settle(cx);
        let second = cx.update(|cx| cx.theme().colors().editor_background);
        assert_ne!(first, second);
        cx.update(|cx| assert_eq!(cx.theme().name, "Mine"));

        // A broken file is reported; removed, its theme is gone.
        std::fs::write(dir.join("broken.json"), "{ \"name\": ").unwrap();
        settle(cx);
        cx.update(|cx| {
            let lines = crate::problems::lines(cx);
            assert!(
                lines
                    .iter()
                    .any(|l| l.starts_with("themes:") && l.contains("broken.json")),
                "{lines:?}"
            );
        });
        std::fs::remove_dir_all(&dir).unwrap();
        settle(cx);
        cx.update(|cx| {
            assert!(ThemeRegistry::global(cx).get("Mine").is_err());
            assert_ne!(cx.theme().name, "Mine", "back to the default");
            assert!(
                !crate::problems::lines(cx)
                    .iter()
                    .any(|l| l.starts_with("themes:"))
            );
            // The bundled ones are all still there.
            assert!(ThemeRegistry::global(cx).get("One Dark").is_ok());
        });
    }

    #[test]
    fn a_palette_makes_a_dark_theme_with_its_colours() {
        let palette = Palette::from_omarchy("background = \"#101418\"\nred = \"#ff0000\"\n");
        let family = family(OMARCHY, "test", &palette).unwrap();
        let theme = &family.themes[0];
        assert_eq!(theme.name, OMARCHY);
        assert_eq!(theme.appearance, Appearance::Dark);
        let rgb = |c: gpui::Hsla| {
            let c = gpui::Rgba::from(c);
            [c.r, c.g, c.b].map(|v| (v * 255.).round() as u8)
        };
        assert_eq!(rgb(theme.colors().editor_background), [0x10, 0x14, 0x18]);
        assert_eq!(rgb(theme.status().error), [0xff, 0, 0]);
        // You, then one player per agent hue.
        assert_eq!(theme.players().0.len(), 1 + palette.hues.len());
    }
}
