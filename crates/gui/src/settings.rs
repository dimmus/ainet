//! `~/.config/ainet/settings.json`: the desktop app's settings, in Zed's
//! style. JSON with comments and trailing commas; every field is optional.
//! Saved changes apply without a restart. A file that does not parse never
//! takes the app down: the last good settings stay, and the status bar says
//! what is wrong until it is fixed.
//!
//! The same file holds the sections ainetd and the console read (`daemon`,
//! `messaging`, …: `ainet_core::settings`); the app ignores them here and
//! keeps them when it saves, and the published schema covers both.
//!
//! The settings also back the [`ThemeSettingsProvider`] that Zed's `theme`
//! and `ui` crates read fonts, sizes and density from.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{App, Font, Global, Pixels, SharedString, Task, font, px};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use theme::{Appearance, SystemAppearance, ThemeSettingsProvider, UiDensity};

use crate::{problems, themes, watch};

/// Where the settings live: `$XDG_CONFIG_HOME/ainet/settings.json`.
pub fn settings_file() -> PathBuf {
    ainet_core::paths::settings_file()
}

/// What a new settings file starts as when opened from the app.
pub const TEMPLATE: &str = r#"// ainet desktop settings. Every field is optional, and changes apply as
// soon as you save. Comments and trailing commas are fine.
{
  // For editors that complete and check JSON (`just install-gui` also puts
  // a copy in ~/.local/share/ainet/settings-schema.json).
  "$schema": "https://raw.githubusercontent.com/dimmus/ainet/main/crates/gui/assets/settings-schema.json",
  // A theme name ("Deepharness", "Omarchy", "One Dark", "Ayu Light", …),
  // or one per system appearance:
  // "theme": { "mode": "system", "light": "One Light", "dark": "Deepharness" },
  // "ui_font_family": ".ZedSans",
  // "ui_font_size": 16,
  // "ui_density": "default", // or "compact", "comfortable"
  // "terminal": { "font_family": ".ZedMono", "font_size": 13, "line_height": "standard",
  //               "cursor_shape": "block", "blinking": "on" },
  // "fleet_panel": { "dock": "left", "width": 260 },
  // "critters": { "size": "large" },
  // "command_line": true, // the cmd line above the status bar
  // "confirm_quit": false,
  //
  // ainetd and the console read their own sections of this file:
  // "daemon", "runtimes", "permissions", "messaging", "queue", "rigs",
  // "discovery", "hooks", "retention" and "cost" (the schema has them all):
  // "messaging": { "hold_while_interacting": true, "flush_gap_ms": 400 },
  // "rigs": { "keep_snapshots": 20 },
}
"#;

/// Font sizes are kept within Zed's limits.
const MIN_FONT_SIZE: f32 = 6.;
const MAX_FONT_SIZE: f32 = 100.;

/// Everything `settings.json` can say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Settings {
    /// The theme: a name ("Deepharness", "Omarchy", "One Dark", …), or one
    /// per system appearance. Unset means Omarchy when installed, else
    /// Deepharness.
    pub theme: Option<ThemeSelection>,
    /// The font of everything but terminals and code (".ZedSans" is IBM
    /// Plex Sans, bundled).
    pub ui_font_family: SharedString,
    /// In pixels, 6 to 100.
    pub ui_font_size: f32,
    /// How tightly the UI is spaced.
    pub ui_density: UiDensity,
    /// Agents' terminals.
    pub terminal: TerminalSettings,
    /// The Fleet panel.
    pub fleet_panel: FleetPanelSettings,
    /// The agents' pixel critters.
    pub critters: CritterSettings,
    /// The command line above the status bar, always there. Off, it shows
    /// only while it has the keyboard (`ctrl-;`).
    pub command_line: bool,
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
            command_line: true,
            confirm_quit: false,
        }
    }
}

/// A theme by name, or one for light and one for dark, as in Zed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalSettings {
    /// ".ZedMono" is Lilex, bundled.
    pub font_family: SharedString,
    /// In pixels, 6 to 100. Unset, it follows the UI: 13 at the default
    /// `ui_font_size` of 16, so terminals read like the text around them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    /// The height of a line, as a ratio to the font size.
    pub line_height: LineHeight,
    /// The cursor of a focused terminal.
    pub cursor_shape: CursorShape,
    /// Whether that cursor blinks.
    pub blinking: Blinking,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings {
            font_family: ".ZedMono".into(),
            font_size: None,
            line_height: LineHeight::Standard,
            cursor_shape: CursorShape::Block,
            blinking: Blinking::On,
        }
    }
}

/// The cursor of a focused terminal, with Zed's names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Blinking {
    Off,
    #[default]
    On,
    TerminalControlled,
}

/// A terminal's line height, written as in Zed's settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FleetPanelSettings {
    /// Its side of the window; the Agent panel takes the other.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DockSide {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CritterSettings {
    /// Their size in the Fleet (`ctrl-e` toggles it).
    pub size: CritterSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// The terminal's size against the UI's when settings.json does not set
/// one. Zed's terminal takes its editor's 15 px, but ainet's UI is mostly
/// small labels (14 and 12 px at the default 16), and monospace text at 15
/// stood out beside them, most in the Terminals grid's tiles.
const TERMINAL_SCALE: f32 = 13. / 16.;

impl Settings {
    /// The terminal's font size in pixels: `terminal.font_size`, else the
    /// UI's scaled by [`TERMINAL_SCALE`] (13 at 16), whole pixels.
    pub fn terminal_font_size(&self) -> f32 {
        self.terminal
            .font_size
            .unwrap_or_else(|| (self.ui_font_size * TERMINAL_SCALE).round())
            .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }

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
        // A size that is not a number is as good as none: follow the UI.
        self.terminal.font_size = self
            .terminal
            .font_size
            .filter(|s| s.is_finite())
            .map(|s| s.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE));
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
    /// The file they come from and are saved to (none in tests).
    file: Option<PathBuf>,
    _watch: Option<Task<()>>,
}

impl Global for AppSettings {}

impl AppSettings {
    fn new(settings: Settings) -> Self {
        AppSettings {
            ui_font: font(settings.ui_font_family.clone()),
            buffer_font: font(settings.terminal.font_family.clone()),
            settings,
            file: None,
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
    let mut app_settings = AppSettings::new(settings);
    app_settings.file = file.clone();
    cx.set_global(app_settings);
    theme::set_theme_settings_provider(Box::new(Provider), cx);
    apply_theme(cx);
    if let Some(file) = file {
        let task = watch::watch(file, cx, |text, cx| reload(text.as_deref().unwrap_or(""), cx));
        cx.global_mut::<AppSettings>()._watch = Some(task);
    }
}

/// Saves the settings to `file` from now on, without following it (tests).
#[cfg(test)]
pub fn save_to(file: PathBuf, cx: &mut App) {
    cx.global_mut::<AppSettings>().file = Some(file);
}

/// Makes `name` the theme, in settings.json too: as Zed's theme selector,
/// only the `"theme"` value changes in the file's text (with a theme per
/// appearance, the one for the system's appearance now), and the rest
/// stays as written.
pub fn save_theme(name: &str, cx: &mut App) -> Result<(), String> {
    let file = cx.global::<AppSettings>().file.clone();
    let text = match &file {
        Some(file) => read_settings(file)?,
        None => String::new(),
    };
    let text = with_theme(&text, name, SystemAppearance::global(cx).0)?;
    if let Some(file) = &file {
        write_whole(file, &text).map_err(|e| format!("cannot save {}: {e}", file.display()))?;
        reload(&text, cx);
    } else {
        let mut next = get(cx).clone();
        next.theme = parse(&text)?.theme;
        set(next, cx);
    }
    Ok(())
}

/// settings.json's text, empty while there is none. Any other failure
/// is an error: saving over a file that could not be read would lose it.
fn read_settings(file: &Path) -> Result<String, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("cannot read {}: {e}", file.display())),
    }
}

/// Writes a file (settings.json, the saved workspace) whole or not at
/// all: into a file beside it, then renamed over it, so a crash halfway
/// never leaves it cut short.
pub fn write_whole(file: &Path, text: &str) -> std::io::Result<()> {
    // Every write gets a file of its own: two windows (a demo one and a
    // real one) save the same gui.json, and a shared one would let one
    // rename the other's half-written text into place.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    // Through a symlink (settings kept with your dotfiles), the file it
    // points at, so the link stays.
    let file = &std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut part = file.as_os_str().to_owned();
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    part.push(format!(".{}.{write}.saving", std::process::id()));
    let part = PathBuf::from(part);
    let written = std::fs::File::create(&part).and_then(|mut out| {
        out.write_all(text.as_bytes())?;
        // On disk before the rename, so a crash cannot leave it empty.
        out.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    // Keep who may read it, as an edit in place would.
    if let Ok(meta) = std::fs::metadata(file) {
        let _ = std::fs::set_permissions(&part, meta.permissions());
    }
    std::fs::rename(&part, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// settings.json's text with `name` as its theme for `appearance`.
pub fn with_theme(text: &str, name: &str, appearance: Appearance) -> Result<String, String> {
    let current = parse(text).map_err(|e| format!("settings.json does not parse: {e}"))?;
    let path: &[&str] = match &current.theme {
        Some(ThemeSelection::Dynamic { mode, .. }) => match (mode, appearance) {
            (ThemeMode::Light, _) | (ThemeMode::System, Appearance::Light) => &["theme", "light"],
            _ => &["theme", "dark"],
        },
        _ => &["theme"],
    };
    let value = serde_json::to_string(name).map_err(|e| e.to_string())?;
    let edited = ainet_core::jsonc::set(text, path, &value).map_err(|e| format!("settings.json: {e}"))?;
    // What was written must read back as asked.
    match parse(&edited)?.theme {
        Some(theme) if theme.name(appearance) == name => Ok(edited),
        _ => Err("settings.json: the theme could not be set".into()),
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
    let file = cx.global_mut::<AppSettings>().file.take();
    let mut next = AppSettings::new(settings);
    next._watch = watch;
    next.file = file;
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
        px(get(cx).terminal_font_size())
    }

    fn ui_density(&self, cx: &App) -> UiDensity {
        get(cx).ui_density
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writers_at_once_never_mix_their_files() {
        let dir = std::env::temp_dir().join(format!("ainet-gui-write-whole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("gui.json");
        let texts: Vec<String> = (0..4u8)
            .map(|i| char::from(b'a' + i).to_string().repeat(64 * 1024))
            .collect();
        std::thread::scope(|scope| {
            for text in &texts {
                let file = &file;
                scope.spawn(move || {
                    for _ in 0..50 {
                        write_whole(file, text).unwrap();
                    }
                });
            }
        });
        let left = std::fs::read_to_string(&file).unwrap();
        assert!(texts.contains(&left), "a mix of writes was left");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["gui.json"], "nothing left beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn defaults_match_zed() {
        let s = Settings::default();
        assert_eq!(s.ui_font_family.as_ref(), ".ZedSans");
        assert_eq!(s.terminal.font_family.as_ref(), ".ZedMono");
        assert_eq!((s.ui_font_size, s.terminal.font_size), (16., None));
        assert_eq!(s.terminal_font_size(), 13., "follows the UI");
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
        assert!(s.command_line);
        assert_eq!(parse("").unwrap(), s);
        assert_eq!(parse("  // nothing yet\n{}").unwrap(), s);
    }

    #[test]
    fn the_terminal_follows_the_ui_unless_told() {
        let at = |json: &str| parse(json).unwrap().terminal_font_size();
        assert_eq!(at("{}"), 13.);
        assert_eq!(at(r#"{ "ui_font_size": 20 }"#), 16.);
        assert_eq!(at(r#"{ "ui_font_size": 12 }"#), 10.);
        assert_eq!(
            at(r#"{ "ui_font_size": 20, "terminal": { "font_size": 15 } }"#),
            15.
        );
        assert_eq!(
            at(r#"{ "ui_font_size": 6 }"#),
            MIN_FONT_SIZE,
            "never below the floor"
        );
    }

    #[test]
    fn the_template_is_valid_and_says_nothing() {
        assert_eq!(parse(TEMPLATE).unwrap(), Settings::default());
    }

    /// The JSON schema for settings.json, as `assets/settings-schema.json`
    /// holds it: the app's keys and ainetd's sections in one.
    fn schema() -> String {
        let app = serde_json::to_value(schemars::schema_for!(Settings)).unwrap();
        let schema = compose(app, ainet_core::settings::schema());
        serde_json::to_string_pretty(&schema).unwrap() + "\n"
    }

    /// `app`'s schema with `core`'s properties and definitions added.
    fn compose(mut app: serde_json::Value, core: serde_json::Value) -> serde_json::Value {
        for part in ["properties", "$defs"] {
            let Some(from) = core.get(part).and_then(|v| v.as_object()).cloned() else {
                continue;
            };
            let into = app
                .as_object_mut()
                .unwrap()
                .entry(part)
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .unwrap();
            for (k, v) in from {
                assert!(
                    into.insert(k.clone(), v).is_none(),
                    "{part}/{k} is in both schemas"
                );
            }
        }
        app
    }

    #[test]
    fn the_schema_file_is_the_settings_types() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/settings-schema.json");
        // `AINET_WRITE_SCHEMA=1 cargo test --manifest-path crates/gui/Cargo.toml
        // settings_types` rewrites it after a change to the settings.
        if std::env::var_os("AINET_WRITE_SCHEMA").is_some() {
            std::fs::write(path, schema()).unwrap();
            return;
        }
        let file = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            file == schema(),
            "run with AINET_WRITE_SCHEMA=1 to regenerate {path}"
        );
        let properties = serde_json::from_str::<serde_json::Value>(&file).unwrap()["properties"].clone();
        for field in [
            "theme",
            "ui_font_size",
            "terminal",
            "fleet_panel",
            "critters",
            "command_line",
            "confirm_quit",
        ]
        .iter()
        .chain(ainet_core::settings::SECTIONS)
        {
            assert!(properties.get(field).is_some(), "{field} in the schema");
        }
        // The template points editors at it, where the repository keeps it.
        assert!(TEMPLATE.contains("/crates/gui/assets/settings-schema.json\","));
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
              "command_line": false,
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
        assert!(!s.command_line);
        assert!(s.confirm_quit);
    }

    #[test]
    fn the_daemons_sections_are_not_the_apps_and_stay_when_it_saves() {
        let text = "{\n  // ainetd's\n  \"messaging\": { \"flush_gap_ms\": \"not a number\" },\n  \"ui_font_size\": 15\n}\n";
        assert_eq!(parse(text).unwrap().ui_font_size, 15., "another reader's section");
        let saved = with_theme(text, "One Dark", Appearance::Dark).unwrap();
        assert!(saved.contains("// ainetd's\n  \"messaging\": { \"flush_gap_ms\": \"not a number\" }"));
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
            (MIN_FONT_SIZE, Some(MAX_FONT_SIZE))
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
    fn the_theme_is_saved_into_the_text_as_written() {
        let text = "// Mine.\n{\n  \"ui_font_size\": 15, // big\n  \"theme\": \"One Dark\",\n}\n";
        let saved = with_theme(text, "Ayu Light", Appearance::Dark).unwrap();
        assert_eq!(saved, text.replace("One Dark", "Ayu Light"));
        assert_eq!(
            with_theme("", "Ayu Light", Appearance::Dark).unwrap(),
            "{\n  \"theme\": \"Ayu Light\"\n}\n"
        );
        assert_eq!(
            parse(&with_theme(TEMPLATE, "One Light", Appearance::Dark).unwrap())
                .unwrap()
                .theme,
            Some(ThemeSelection::Static("One Light".into()))
        );
        // A theme per appearance: the one in use now changes.
        let dynamic = r#"{ "theme": { "mode": "system", "light": "One Light", "dark": "One Dark" } }"#;
        let light = with_theme(dynamic, "Ayu Light", Appearance::Light).unwrap();
        assert!(
            light.contains(r#""light": "Ayu Light", "dark": "One Dark""#),
            "{light}"
        );
        let dark = with_theme(dynamic, "Ayu Dark", Appearance::Dark).unwrap();
        assert!(
            dark.contains(r#""light": "One Light", "dark": "Ayu Dark""#),
            "{dark}"
        );
        assert!(with_theme("{ \"theme\": ", "X", Appearance::Dark).is_err());
    }

    #[test]
    fn lives_in_the_xdg_config_dir() {
        assert!(settings_file().ends_with("ainet/settings.json"));
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod app_tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::tests::init_test;

    #[gpui::test]
    fn saving_a_theme_never_loses_the_file(cx: &mut TestAppContext) {
        init_test(cx);
        let dir = std::env::temp_dir().join(format!("ainet-gui-save-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");

        // Unreadable (not UTF-8): an error, and the file as it was.
        let bytes = b"{ \"ui_font_size\": 15, \"note\": \"\xff\" }".to_vec();
        std::fs::write(&file, &bytes).unwrap();
        let saved = cx.update(|cx| {
            save_to(file.clone(), cx);
            save_theme("One Light", cx)
        });
        assert!(saved.is_err_and(|e| e.starts_with("cannot read")));
        assert_eq!(std::fs::read(&file).unwrap(), bytes);

        // Missing: written anew, nothing left beside it.
        std::fs::remove_file(&file).unwrap();
        cx.update(|cx| save_theme("One Light", cx)).unwrap();
        assert_eq!(
            parse(&std::fs::read_to_string(&file).unwrap()).unwrap().theme,
            Some(ThemeSelection::Static("One Light".into()))
        );
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["settings.json"]);

        // Through a symlink: the file it points at changes, the link stays.
        #[cfg(unix)]
        {
            let real = dir.join("real.json");
            std::fs::write(&real, "{ \"ui_font_size\": 15 }").unwrap();
            std::fs::remove_file(&file).unwrap();
            std::os::unix::fs::symlink(&real, &file).unwrap();
            cx.update(|cx| save_theme("One Dark", cx)).unwrap();
            assert!(std::fs::symlink_metadata(&file).unwrap().file_type().is_symlink());
            let text = std::fs::read_to_string(&real).unwrap();
            assert!(
                text.contains("\"ui_font_size\": 15") && text.contains("One Dark"),
                "{text}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
