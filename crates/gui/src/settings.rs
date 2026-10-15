//! `~/.config/ainet/settings.json`: the desktop app's settings, in Zed's
//! style. JSON with comments and trailing commas; every field is optional.
//! Saved changes apply without a restart. A file that does not parse never
//! takes the app down: the last good settings stay, and the status bar says
//! what is wrong until it is fixed.
//!
//! The settings also back the [`ThemeSettingsProvider`] that Zed's `theme`
//! and `ui` crates read fonts, sizes and density from.

use std::path::PathBuf;

use gpui::{App, Font, Global, Pixels, SharedString, Task, font, px};
use serde::Deserialize;
use theme::{Appearance, ThemeSettingsProvider, UiDensity};

use crate::{problems, themes, watch};

/// Where the settings live: `$XDG_CONFIG_HOME/ainet/settings.json`.
pub fn settings_file() -> PathBuf {
    ainet_core::paths::config_dir().join("settings.json")
}

/// What a new settings file starts as when opened from the app.
pub const TEMPLATE: &str = r#"// ainet desktop settings. Every field is optional, and changes apply as
// soon as you save. Comments and trailing commas are fine.
{
  // A theme name ("Deepharness", "Omarchy", "One Dark", "Ayu Light", …),
  // or one per system appearance:
  // "theme": { "mode": "system", "light": "One Light", "dark": "Deepharness" },
  // "ui_font_family": ".ZedSans",
  // "ui_font_size": 16,
  // "ui_density": "default", // or "compact", "comfortable"
  // "terminal": { "font_family": ".ZedMono", "font_size": 15, "line_height": "standard",
  //               "cursor_shape": "block", "blinking": "on" },
  // "fleet_panel": { "dock": "left", "width": 260 },
  // "critters": { "size": "large" },
  // "confirm_quit": false,
}
"#;

/// Font sizes are kept within Zed's limits.
const MIN_FONT_SIZE: f32 = 6.;
const MAX_FONT_SIZE: f32 = 100.;

/// Everything `settings.json` can say.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The theme; unset means Omarchy when installed, else Deepharness.
    pub theme: Option<ThemeSelection>,
    pub ui_font_family: SharedString,
    pub ui_font_size: f32,
    pub ui_density: UiDensity,
    pub terminal: TerminalSettings,
    pub fleet_panel: FleetPanelSettings,
    pub critters: CritterSettings,
    /// Ask before quitting.
    pub confirm_quit: bool,
}

impl Default for Settings {
    /// Zed's fonts and sizes: its Plex UI font at 16 px, Lilex at 15.
    fn default() -> Self {
        Settings {
            theme: None,
            ui_font_family: ".ZedSans".into(),
            ui_font_size: 16.,
            ui_density: UiDensity::Default,
            terminal: TerminalSettings::default(),
            fleet_panel: FleetPanelSettings::default(),
            critters: CritterSettings::default(),
            confirm_quit: false,
        }
    }
}

/// A theme by name, or one for light and one for dark, as in Zed.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ThemeSelection {
    Static(String),
    Dynamic {
        #[serde(default)]
        mode: ThemeMode,
        light: String,
        dark: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    /// Follow the system's light or dark appearance.
    #[default]
    System,
}

impl ThemeSelection {
    /// The theme to use when the system looks like `appearance`.
    pub fn name(&self, appearance: Appearance) -> &str {
        match self {
            ThemeSelection::Static(name) => name,
            ThemeSelection::Dynamic { mode, light, dark } => match (mode, appearance) {
                (ThemeMode::Light, _) | (ThemeMode::System, Appearance::Light) => light,
                (ThemeMode::Dark, _) | (ThemeMode::System, Appearance::Dark) => dark,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub font_family: SharedString,
    pub font_size: f32,
    pub line_height: LineHeight,
    pub cursor_shape: CursorShape,
    pub blinking: Blinking,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings {
            font_family: ".ZedMono".into(),
            font_size: 15.,
            line_height: LineHeight::Standard,
            cursor_shape: CursorShape::Block,
            blinking: Blinking::On,
        }
    }
}

/// The cursor of a focused terminal, with Zed's names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
    Hollow,
}

/// Whether a focused terminal's cursor blinks, with Zed's names. ainetd
/// does not pass on a program's own blink request, so
/// `terminal_controlled` keeps the cursor still.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blinking {
    Off,
    #[default]
    On,
    TerminalControlled,
}

/// A terminal's line height, written as in Zed's settings.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineHeight {
    /// 1.618: easy reading.
    Comfortable,
    /// 1.3: box-drawing characters join up, which agents' TUIs rely on;
    /// so it is ainet's default, unlike Zed's.
    Standard,
    /// Any ratio of at least 1.
    Custom(f32),
}

impl LineHeight {
    /// The ratio to the font size.
    pub fn ratio(self) -> f32 {
        match self {
            LineHeight::Comfortable => 1.618,
            LineHeight::Standard => 1.3,
            LineHeight::Custom(ratio) => ratio.max(1.),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct FleetPanelSettings {
    pub dock: DockSide,
    /// In pixels; Zed's default side-dock width.
    pub width: f32,
}

impl Default for FleetPanelSettings {
    fn default() -> Self {
        FleetPanelSettings {
            dock: DockSide::Left,
            width: 260.,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockSide {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CritterSettings {
    pub size: CritterSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CritterSize {
    Small,
    #[default]
    Large,
}

impl CritterSize {
    pub fn toggled(self) -> Self {
        match self {
            CritterSize::Small => CritterSize::Large,
            CritterSize::Large => CritterSize::Small,
        }
    }
}

/// Reads settings from the file's text. An empty file means the defaults;
/// fields the app does not know are ignored; out-of-range sizes are brought
/// into range.
pub fn parse(text: &str) -> Result<Settings, String> {
    if text.trim().is_empty() {
        return Ok(Settings::default());
    }
    let settings: Settings = serde_json_lenient::from_str(text).map_err(|e| e.to_string())?;
    Ok(settings.in_range())
}

impl Settings {
    fn in_range(mut self) -> Self {
        let size = |s: f32, default: f32| {
            if s.is_finite() {
                s.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
            } else {
                default
            }
        };
        let defaults = Settings::default();
        self.ui_font_size = size(self.ui_font_size, defaults.ui_font_size);
        self.terminal.font_size = size(self.terminal.font_size, defaults.terminal.font_size);
        if !self.fleet_panel.width.is_finite() {
            self.fleet_panel.width = defaults.fleet_panel.width;
        }
        self.fleet_panel.width = self.fleet_panel.width.clamp(120., 1200.);
        self
    }
}

/// Opens a config file with the system's handler for JSON, creating it from
/// `template` first if it does not exist yet.
pub fn open_file(file: &std::path::Path, template: &str, cx: &App) -> Result<(), String> {
    if !file.exists() {
        file.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(file, template))
            .map_err(|e| format!("cannot create {}: {e}", file.display()))?;
    }
    cx.open_with_system(file);
    Ok(())
}

/// The settings in force, as a GPUI global.
pub struct AppSettings {
    pub settings: Settings,
    ui_font: Font,
    buffer_font: Font,
    _watch: Option<Task<()>>,
}

impl Global for AppSettings {}

impl AppSettings {
    fn new(settings: Settings) -> Self {
        AppSettings {
            ui_font: font(settings.ui_font_family.clone()),
            buffer_font: font(settings.terminal.font_family.clone()),
            settings,
            _watch: None,
        }
    }
}

/// The settings in force.
pub fn get(cx: &App) -> &Settings {
    &cx.global::<AppSettings>().settings
}

/// Loads the settings from `file` (none in tests), registers them with the
/// theme crate, applies the theme, and follows the file from then on.
pub fn init(file: Option<PathBuf>, cx: &mut App) {
    let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
    let settings = match text.as_deref().map(parse) {
        None => Settings::default(),
        Some(Ok(settings)) => settings,
        Some(Err(e)) => {
            problems::set(problems::Source::Settings, Some(e), cx);
            Settings::default()
        }
    };
    cx.set_global(AppSettings::new(settings));
    theme::set_theme_settings_provider(Box::new(Provider), cx);
    apply_theme(cx);
    if let Some(file) = file {
        let task = watch::watch(file, cx, |text, cx| reload(text.as_deref().unwrap_or(""), cx));
        cx.global_mut::<AppSettings>()._watch = Some(task);
    }
}

/// Takes new settings from the file's text, or keeps the last good ones and
/// reports why not.
pub fn reload(text: &str, cx: &mut App) {
    match parse(text) {
        Ok(settings) => {
            problems::set(problems::Source::Settings, None, cx);
            set(settings, cx);
        }
        Err(e) => problems::set(problems::Source::Settings, Some(e), cx),
    }
}

/// Puts `settings` in force and redraws.
pub fn set(settings: Settings, cx: &mut App) {
    if *get(cx) == settings {
        return;
    }
    let watch = cx.global_mut::<AppSettings>()._watch.take();
    let mut next = AppSettings::new(settings);
    next._watch = watch;
    cx.set_global(next);
    apply_theme(cx);
    cx.refresh_windows();
}

/// Makes the configured theme active, for the current system appearance.
pub fn apply_theme(cx: &mut App) {
    let selection = get(cx).theme.clone();
    let problem = themes::apply(selection.as_ref(), cx);
    problems::set(problems::Source::Theme, problem, cx);
}

/// The UI font and its size; the size is also the window's rem size, as in
/// Zed.
pub fn ui_font(cx: &App) -> (Font, Pixels) {
    let settings = theme::theme_settings(cx);
    (settings.ui_font(cx).clone(), settings.ui_font_size(cx))
}

/// What Zed's `theme` and `ui` crates see: ainet's settings.
struct Provider;

impl ThemeSettingsProvider for Provider {
    fn ui_font<'a>(&'a self, cx: &'a App) -> &'a Font {
        &cx.global::<AppSettings>().ui_font
    }

    fn buffer_font<'a>(&'a self, cx: &'a App) -> &'a Font {
        &cx.global::<AppSettings>().buffer_font
    }

    fn ui_font_size(&self, cx: &App) -> Pixels {
        px(get(cx).ui_font_size)
    }

    fn buffer_font_size(&self, cx: &App) -> Pixels {
        px(get(cx).terminal.font_size)
    }

    fn ui_density(&self, cx: &App) -> UiDensity {
        get(cx).ui_density
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_zed() {
        let s = Settings::default();
        assert_eq!(s.ui_font_family.as_ref(), ".ZedSans");
        assert_eq!(s.terminal.font_family.as_ref(), ".ZedMono");
        assert_eq!((s.ui_font_size, s.terminal.font_size), (16., 15.));
        assert_eq!(s.ui_density, UiDensity::Default);
        assert_eq!(
            s.fleet_panel,
            FleetPanelSettings {
                dock: DockSide::Left,
                width: 260.
            }
        );
        assert_eq!(s.critters.size, CritterSize::Large);
        assert!(!s.confirm_quit && s.theme.is_none());
        assert_eq!(parse("").unwrap(), s);
        assert_eq!(parse("  // nothing yet\n{}").unwrap(), s);
    }

    #[test]
    fn the_template_is_valid_and_says_nothing() {
        assert_eq!(parse(TEMPLATE).unwrap(), Settings::default());
    }

    #[test]
    fn reads_comments_trailing_commas_and_partial_objects() {
        let s = parse(
            r#"
            // A comment.
            {
              "theme": "One Dark", /* inline */
              "ui_font_size": 14,
              "terminal": { "line_height": { "custom": 1.5 }, "cursor_shape": "bar", "blinking": "off" },
              "fleet_panel": { "dock": "right" },
              "ui_density": "compact",
              "critters": { "size": "small" },
              "confirm_quit": true,
            }
            "#,
        )
        .unwrap();
        assert_eq!(s.theme, Some(ThemeSelection::Static("One Dark".into())));
        assert_eq!(s.ui_font_size, 14.);
        assert_eq!(
            s.terminal.font_family.as_ref(),
            ".ZedMono",
            "unset fields keep defaults"
        );
        assert_eq!(s.terminal.line_height.ratio(), 1.5);
        assert_eq!(s.terminal.cursor_shape, CursorShape::Bar);
        assert_eq!(s.terminal.blinking, Blinking::Off);
        assert_eq!(
            s.fleet_panel,
            FleetPanelSettings {
                dock: DockSide::Right,
                width: 260.
            }
        );
        assert_eq!(s.ui_density, UiDensity::Compact);
        assert_eq!(s.critters.size, CritterSize::Small);
        assert!(s.confirm_quit);
    }

    #[test]
    fn ignores_unknown_fields() {
        let s = parse(r#"{ "vim_mode": true, "terminal": { "copy_on_select": true }, "ui_font_size": 18 }"#)
            .unwrap();
        assert_eq!(s.ui_font_size, 18.);
    }

    #[test]
    fn rejects_bad_values_and_clamps_sizes() {
        for bad in [
            r#"{ "ui_font_size": "big" }"#,
            r#"{ "fleet_panel": { "dock": "top" } }"#,
            r#"{ "theme": 3 }"#,
            r#"{ "ui_density": "dense" }"#,
            r#"{ "terminal": { "line_height": "tall" } }"#,
            r#"{ "terminal": { "cursor_shape": "beam" } }"#,
            r#"{ "ui_font_size": 14 "#,
            "[1, 2]",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let s = parse(
            r#"{ "ui_font_size": 1, "terminal": { "font_size": 500 }, "fleet_panel": { "width": 5 } }"#,
        )
        .unwrap();
        assert_eq!(
            (s.ui_font_size, s.terminal.font_size),
            (MIN_FONT_SIZE, MAX_FONT_SIZE)
        );
        assert_eq!(s.fleet_panel.width, 120.);
        assert_eq!(LineHeight::Custom(0.5).ratio(), 1.);
    }

    #[test]
    fn a_theme_per_appearance() {
        let s = parse(r#"{ "theme": { "light": "One Light", "dark": "Deepharness" } }"#).unwrap();
        let theme = s.theme.unwrap();
        assert_eq!(theme.name(Appearance::Light), "One Light");
        assert_eq!(theme.name(Appearance::Dark), "Deepharness");
        let dark = parse(r#"{ "theme": { "mode": "dark", "light": "One Light", "dark": "Ayu Dark" } }"#)
            .unwrap()
            .theme
            .unwrap();
        assert_eq!(dark.name(Appearance::Light), "Ayu Dark");
    }

    #[test]
    fn lives_in_the_xdg_config_dir() {
        assert!(settings_file().ends_with("ainet/settings.json"));
    }
}
