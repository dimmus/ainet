//! The Settings view's categories and the fields on each: the app's own
//! (Appearance, Terminal, Workspace; Keymap is its own editor), then
//! ainet's, which ainetd and the console read.

use serde_json::{Value, json};

use super::field::{Choice, Field, Kind};

/// A category, in the order the list shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Appearance,
    Terminal,
    Keymap,
    Workspace,
    Daemon,
    Runtimes,
    Permissions,
    Messaging,
    Queue,
    Rigs,
    Discovery,
    Hooks,
    Retention,
    Cost,
    Diagnostics,
}

impl Page {
    pub const ALL: [Page; 15] = [
        Page::Appearance,
        Page::Terminal,
        Page::Keymap,
        Page::Workspace,
        Page::Daemon,
        Page::Runtimes,
        Page::Permissions,
        Page::Messaging,
        Page::Queue,
        Page::Rigs,
        Page::Discovery,
        Page::Hooks,
        Page::Retention,
        Page::Cost,
        Page::Diagnostics,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Terminal => "Terminal",
            Page::Keymap => "Keymap",
            Page::Workspace => "Workspace",
            Page::Daemon => "Daemon",
            Page::Runtimes => "Runtimes",
            Page::Permissions => "Permissions",
            Page::Messaging => "Messaging",
            Page::Queue => "Queue",
            Page::Rigs => "Rigs & Specs",
            Page::Discovery => "Discovery",
            Page::Hooks => "Hooks",
            Page::Retention => "Retention",
            Page::Cost => "Cost",
            Page::Diagnostics => "Diagnostics",
        }
    }

    /// Its name when saved with the workspace.
    pub fn name(self) -> &'static str {
        match self {
            Page::Rigs => "rigs",
            page => match page.section() {
                Some(section) => section,
                None => match page {
                    Page::Appearance => "appearance",
                    Page::Terminal => "terminal",
                    Page::Keymap => "keymap",
                    Page::Workspace => "workspace",
                    _ => "diagnostics",
                },
            },
        }
    }

    pub fn from_name(name: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Whether it is one of the app's own (else ainet's).
    pub fn is_app(self) -> bool {
        matches!(
            self,
            Page::Appearance | Page::Terminal | Page::Keymap | Page::Workspace
        )
    }

    /// The section of settings.json it shows, for ainet's.
    pub fn section(self) -> Option<&'static str> {
        Some(match self {
            Page::Daemon => "daemon",
            Page::Runtimes => "runtimes",
            Page::Permissions => "permissions",
            Page::Messaging => "messaging",
            Page::Queue => "queue",
            Page::Rigs => "rigs",
            Page::Discovery => "discovery",
            Page::Hooks => "hooks",
            Page::Retention => "retention",
            Page::Cost => "cost",
            _ => return None,
        })
    }
}

/// Every field, by category. ainet's categories are filled in as their
/// pages are built; until then they say where their settings are.
pub fn fields() -> Vec<(Page, Field)> {
    let mut out = Vec::new();
    out.extend(appearance().into_iter().map(|f| (Page::Appearance, f)));
    out.extend(terminal().into_iter().map(|f| (Page::Terminal, f)));
    out.extend(workspace().into_iter().map(|f| (Page::Workspace, f)));
    out
}

fn size(min: f64, max: f64, step: f64) -> Kind {
    Kind::Number {
        unit: "px",
        min,
        max,
        step,
        integer: false,
    }
}

fn appearance() -> Vec<Field> {
    vec![
        Field::app(
            "theme",
            "Theme",
            "The colours of the whole app. The selector previews each theme as you move; escape puts the one in use back. Unset: Omarchy when installed, else Deepharness.",
            Kind::Theme,
        ),
        Field::app(
            "ui_font_family",
            "UI Font",
            "The font of everything but terminals and code. .ZedSans is IBM Plex Sans, bundled.",
            Kind::Text {
                placeholder: ".ZedSans",
            },
        ),
        Field::app(
            "ui_font_size",
            "UI Font Size",
            "The size of the UI's text; it is also the unit everything else is sized in.",
            size(6., 100., 1.),
        ),
        Field::app(
            "ui_density",
            "UI Density",
            "How tightly the UI is spaced.",
            Kind::Dropdown(vec![
                Choice::text("Compact", "compact"),
                Choice::text("Default", "default"),
                Choice::text("Comfortable", "comfortable"),
            ]),
        ),
        Field::app(
            "critters.size",
            "Critter Size",
            "The agents' pixel critters in the Fleet (ctrl-e toggles it).",
            Kind::Dropdown(vec![
                Choice::text("Small", "small"),
                Choice::text("Large", "large"),
            ]),
        ),
    ]
}

fn terminal() -> Vec<Field> {
    vec![
        Field::app(
            "terminal.font_family",
            "Font",
            "The agents' terminals' font. .ZedMono is Lilex, bundled.",
            Kind::Text {
                placeholder: ".ZedMono",
            },
        ),
        Field::app(
            "terminal.font_size",
            "Font Size",
            "Unset, it follows the UI: 13 at the UI's 16, so terminals read like the text around them.",
            size(6., 100., 1.),
        )
        .when_unset(|s| format!("follows the UI ({} px)", s.terminal_font_size())),
        Field::app(
            "terminal.line_height",
            "Line Height",
            "The height of a line against the font size. Standard (1.3) joins box-drawing characters, which agents' screens rely on; comfortable is 1.618.",
            Kind::Dropdown(vec![
                Choice::text("Comfortable", "comfortable"),
                Choice::text("Standard", "standard"),
                Choice {
                    label: "Custom",
                    value: json!({ "custom": 1.3 }),
                },
            ]),
        ),
        Field::app(
            "terminal.line_height.custom",
            "Custom Line Height",
            "The ratio to the font size, at least 1.",
            Kind::Number {
                unit: "",
                min: 1.,
                max: 3.,
                step: 0.1,
                integer: false,
            },
        )
        .part_of("terminal.line_height", Value::is_object),
        Field::app(
            "terminal.cursor_shape",
            "Cursor Shape",
            "The cursor of a focused terminal.",
            Kind::Dropdown(vec![
                Choice::text("Block", "block"),
                Choice::text("Bar", "bar"),
                Choice::text("Underline", "underline"),
                Choice::text("Hollow", "hollow"),
            ]),
        ),
        Field::app(
            "terminal.blinking",
            "Cursor Blinking",
            "Whether that cursor blinks. ainetd does not pass on a program's own request, so Terminal Controlled keeps it still.",
            Kind::Dropdown(vec![
                Choice::text("Off", "off"),
                Choice::text("On", "on"),
                Choice::text("Terminal Controlled", "terminal_controlled"),
            ]),
        ),
    ]
}

fn workspace() -> Vec<Field> {
    vec![
        Field::app(
            "command_line",
            "Command Line",
            "The command line above the status bar, always there. Off, it shows only while it has the keyboard (ctrl-;).",
            Kind::Toggle,
        ),
        Field::app(
            "confirm_quit",
            "Confirm Quit",
            "Ask before quitting.",
            Kind::Toggle,
        ),
        Field::app(
            "fleet_panel.dock",
            "Fleet Panel Side",
            "The side of the window the Fleet panel docks on; the Agent panel takes the other.",
            Kind::Dropdown(vec![Choice::text("Left", "left"), Choice::text("Right", "right")]),
        ),
        Field::app(
            "fleet_panel.width",
            "Fleet Panel Width",
            "Its width until you drag its edge.",
            size(120., 1200., 10.),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::settings_view::field::{Owner, at};

    #[test]
    fn every_app_field_is_a_setting_with_a_default() {
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        let schema = serde_json::to_value(schemars::schema_for!(Settings)).unwrap();
        for (page, field) in fields() {
            assert!(page.is_app() && field.owner == Owner::App, "{}", field.key);
            assert!(!field.description().is_empty(), "{}", field.key);
            // Unset ones (the theme, the terminal's size) default to null.
            let top = field.key.split('.').next().unwrap();
            assert!(schema["properties"].get(top).is_some(), "{}", field.key);
            if let Kind::Dropdown(choices) = &field.kind
                && field.part_of.is_none()
            {
                let default = at(&defaults, field.key);
                assert!(
                    choices.iter().any(|c| c.value == default),
                    "{}'s default is a choice",
                    field.key
                );
            }
        }
    }

    #[test]
    fn pages_go_by_name_and_section() {
        for page in Page::ALL {
            assert_eq!(Page::from_name(page.name()), Some(page));
            if let Some(section) = page.section() {
                assert!(ainet_core::settings::SECTIONS.contains(&section));
            }
        }
        let ainets = Page::ALL.iter().filter(|p| p.section().is_some()).count();
        assert_eq!(ainets, ainet_core::settings::SECTIONS.len(), "a page per section");
    }
}
