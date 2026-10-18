//! The Settings view's categories and the fields on each: the app's own
//! (Appearance, Terminal, Workspace; Keymap is its own editor), then
//! ainet's, which ainetd and the console read.

use serde_json::{Value, json};

use super::field::{Choice, Field, Kind, Panel, Values};

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

/// Every field, by category, in the order shown.
pub fn fields() -> Vec<(Page, Field)> {
    let pages: [(Page, Vec<Field>); 14] = [
        (Page::Appearance, appearance()),
        (Page::Terminal, terminal()),
        (Page::Workspace, workspace()),
        (Page::Daemon, daemon()),
        (Page::Runtimes, runtimes()),
        (Page::Permissions, permissions()),
        (Page::Messaging, messaging()),
        (Page::Queue, queue()),
        (Page::Rigs, rigs()),
        (Page::Discovery, discovery()),
        (Page::Hooks, hooks()),
        (Page::Retention, retention()),
        (Page::Cost, cost()),
        (Page::Diagnostics, diagnostics()),
    ];
    pages
        .into_iter()
        .flat_map(|(page, fields)| fields.into_iter().map(move |f| (page, f)))
        .collect()
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

/// A whole number of `unit`, from `min` to `max`, stepped by `step`.
fn whole(unit: &'static str, min: f64, max: f64, step: f64) -> Kind {
    Kind::Number {
        unit,
        min,
        max,
        step,
        integer: true,
    }
}

fn list(placeholder: &'static str, values: Values) -> Kind {
    Kind::List { placeholder, values }
}

fn env() -> Kind {
    Kind::Map {
        key_placeholder: "NAME",
        value_placeholder: "value",
        values: Values::Text,
    }
}

/// What a permission mode loosens, said with the field and asked before
/// one is set.
const MODES_CAUTION: &str = "Every mode but Default and Plan lets Claude Code act without asking you; Bypass Permissions skips every permission check.";
const ARGS_CAUTION: &str =
    "Arguments such as --dangerously-skip-permissions turn the agent's own checks off.";

fn effort() -> Kind {
    Kind::Dropdown(vec![
        Choice {
            label: "Agent's Default",
            value: Value::Null,
        },
        Choice::text("Low", "low"),
        Choice::text("Medium", "medium"),
        Choice::text("High", "high"),
        Choice::text("Extra High", "xhigh"),
        Choice::text("Max", "max"),
    ])
}

fn daemon() -> Vec<Field> {
    vec![
        Field::daemon("daemon.autostart", "Start ainetd When Needed", Kind::Toggle),
        Field::daemon("daemon.restore_rigs", "Restore Rigs on Start", Kind::Toggle),
        Field::daemon("daemon.socket", "Socket", Kind::Path { directory: false }),
        Field::daemon(
            "daemon.log.level",
            "Log Level",
            Kind::Text { placeholder: "info" },
        )
        .group("Log"),
        Field::daemon("daemon.log.file", "Log File", Kind::Path { directory: false }).group("Log"),
        Field::daemon("daemon.log.append", "Append to the Log", Kind::Toggle).group("Log"),
        Field::daemon(
            "daemon.log.max_size_mb",
            "Rotate at",
            whole("MB", 0., 10_000., 10.),
        )
        .group("Log"),
        Field::daemon("daemon.log.keep", "Rotated Logs Kept", whole("", 1., 20., 1.)).group("Log"),
    ]
}

fn runtimes() -> Vec<Field> {
    let claude = "Claude Code";
    let mut out = vec![
        Field::daemon(
            "runtimes.claude.binary",
            "Program",
            Kind::Text {
                placeholder: "claude",
            },
        ),
        Field::daemon(
            "runtimes.claude.model",
            "Default Model",
            Kind::Text {
                placeholder: "Claude Code's default",
            },
        ),
        Field::daemon("runtimes.claude.effort", "Effort", effort()),
        Field::daemon(
            "runtimes.claude.spec_effort",
            "Pass a Spec's Effort",
            Kind::Toggle,
        ),
        Field::daemon(
            "runtimes.claude.permission_mode",
            "Permission Mode",
            Kind::Dropdown(vec![
                Choice {
                    label: "Claude Code's Default",
                    value: Value::Null,
                },
                Choice::text("Default: Ask", "default"),
                Choice::text("Plan Only", "plan"),
                Choice::text("Accept Edits (less safe)", "acceptEdits"),
                Choice::text("Auto (less safe)", "auto"),
                Choice::text("Don't Ask (less safe)", "dontAsk"),
                Choice::text("Bypass Permissions (unsafe)", "bypassPermissions"),
            ]),
        )
        .caution(MODES_CAUTION),
        Field::daemon(
            "runtimes.claude.allowed_tools",
            "Allowed Tools",
            list("a tool or pattern: Bash(cargo test:*)", Values::Text),
        )
        .caution("The tools listed run without asking you."),
        Field::daemon(
            "runtimes.claude.disallowed_tools",
            "Disallowed Tools",
            list("a tool or pattern", Values::Text),
        ),
        Field::daemon(
            "runtimes.claude.args",
            "More Arguments",
            list("an argument", Values::Text),
        )
        .caution(ARGS_CAUTION),
        Field::daemon("runtimes.claude.env", "Environment", env()),
        Field::daemon(
            "runtimes.claude.mcp_servers",
            "More MCP Servers",
            Kind::Map {
                key_placeholder: "name",
                value_placeholder: r#"{"command": "…", "args": []}"#,
                values: Values::Json,
            },
        ),
        Field::daemon(
            "runtimes.claude.strict_mcp_config",
            "Only These MCP Servers",
            Kind::Toggle,
        ),
    ];
    for f in &mut out {
        f.group = claude;
    }
    let codex = "Codex";
    out.extend(
        [
            Field::daemon(
                "runtimes.codex.binary",
                "Program",
                Kind::Text { placeholder: "codex" },
            ),
            Field::daemon(
                "runtimes.codex.model",
                "Default Model",
                Kind::Text {
                    placeholder: "Codex's default",
                },
            ),
            Field::daemon("runtimes.codex.effort", "Effort", effort()),
            Field::daemon("runtimes.codex.spec_effort", "Pass a Spec's Effort", Kind::Toggle),
            Field::daemon(
                "runtimes.codex.args",
                "More Arguments",
                list("an argument", Values::Text),
            )
            .caution(ARGS_CAUTION),
            Field::daemon("runtimes.codex.env", "Environment", env()),
            Field::daemon(
                "runtimes.codex.mcp_servers",
                "More MCP Servers",
                Kind::Map {
                    key_placeholder: "name",
                    value_placeholder: r#"{"command": "…", "args": []}"#,
                    values: Values::Json,
                },
            ),
        ]
        .map(|f| f.group(codex)),
    );
    out.extend(
        [
            Field::daemon(
                "runtimes.shell.binary",
                "Program",
                Kind::Text {
                    placeholder: "$SHELL, else /bin/sh",
                },
            ),
            Field::daemon(
                "runtimes.shell.args",
                "More Arguments",
                list("an argument", Values::Text),
            ),
            Field::daemon("runtimes.shell.env", "Environment", env()),
        ]
        .map(|f| f.group("Shell")),
    );
    out.extend(
        [
            Field::daemon(
                "runtimes.readiness.timeout_secs",
                "Ready Within",
                whole("s", 1., 3600., 5.),
            ),
            Field::daemon(
                "runtimes.readiness.trust_timeout_secs",
                "Ready Within, After a Trust Prompt",
                whole("s", 1., 86_400., 30.),
            ),
            Field::daemon(
                "runtimes.readiness.poll_ms",
                "Checked Every",
                whole("ms", 50., 10_000., 50.),
            ),
            Field::daemon(
                "runtimes.readiness.quiet_ms.claude",
                "Quiet Time, Claude Code",
                whole("ms", 0., 60_000., 100.),
            ),
            Field::daemon(
                "runtimes.readiness.quiet_ms.codex",
                "Quiet Time, Codex",
                whole("ms", 0., 60_000., 100.),
            ),
            Field::daemon(
                "runtimes.readiness.quiet_ms.shell",
                "Quiet Time, Shell",
                whole("ms", 0., 60_000., 100.),
            ),
            Field::daemon(
                "runtimes.readiness.startup_text_gap_ms",
                "Between Startup Texts",
                whole("ms", 0., 60_000., 100.),
            ),
        ]
        .map(|f| f.group("Readiness")),
    );
    out
}

fn permissions() -> Vec<Field> {
    vec![
        Field::daemon(
            "permissions.hold_secs",
            "Hold Requests For",
            whole("s", 10., 3600., 10.),
        ),
        Field::panel(
            "panel.always",
            "/always Rules in Force",
            "What agents may do without asking, because you answered Always (or a global rule says so). Revoke takes a rule back at once; the agent asks again next time.",
            Panel::Always,
        ),
        Field::daemon("permissions.persist_always", "Keep /always Rules", Kind::Toggle).caution(
            "Kept rules keep allowing after the session that made them has ended, and across restarts.",
        ),
        Field::daemon(
            "permissions.global_rules",
            "Global Rules",
            list(
                r#"{"command": "cargo test"}, {"tool": "WebSearch"}, {"fetch_host": "docs.rs"}"#,
                Values::Json,
            ),
        )
        .caution("Global rules allow every agent's matching requests without asking anyone."),
        Field::daemon(
            "permissions.spec_policies",
            "Apply Specs' Permission Policies",
            Kind::Toggle,
        )
        .caution("A spec's builtin:yolo then starts its seats with every permission check skipped."),
        Field::daemon(
            "permissions.sensitive_paths",
            "More Sensitive Paths",
            list("a name (secrets.toml) or a path (~/work/keys)", Values::Text),
        ),
    ]
}

fn messaging() -> Vec<Field> {
    vec![
        Field::daemon(
            "messaging.hold_while_interacting",
            "Hold While You Type",
            Kind::Toggle,
        ),
        Field::daemon(
            "messaging.typing_guard",
            "Typing Guard for New Seats",
            Kind::Toggle,
        ),
        Field::daemon(
            "messaging.paste_settle_ms",
            "Paste Settle",
            whole("ms", 0., 5000., 10.),
        ),
        Field::daemon(
            "messaging.flush_gap_ms",
            "Between Held Messages",
            whole("ms", 0., 60_000., 50.),
        ),
        Field::daemon(
            "messaging.message_note",
            "Message Note",
            Kind::Text { placeholder: "" },
        ),
        Field::daemon(
            "messaging.mention_note",
            "Mention Note",
            Kind::Text { placeholder: "" },
        ),
    ]
}

fn queue() -> Vec<Field> {
    vec![
        Field::daemon("queue.nudge", "Nudge on New Items", Kind::Toggle),
        Field::daemon(
            "queue.watchdog.default_interval_secs",
            "Default Interval",
            whole("s", 1., 604_800., 60.),
        )
        .group("Watchdogs"),
        Field::daemon(
            "queue.watchdog.min_interval_secs",
            "Shortest Interval",
            whole("s", 1., 86_400., 10.),
        )
        .group("Watchdogs"),
        Field::daemon(
            "queue.watchdog.message",
            "Default Message",
            Kind::Text { placeholder: "" },
        )
        .group("Watchdogs"),
        Field::daemon(
            "queue.watchdog.tick_secs",
            "Checked Every",
            whole("s", 1., 60., 1.),
        )
        .group("Watchdogs"),
    ]
}

fn rigs() -> Vec<Field> {
    vec![
        Field::daemon(
            "rigs.spec_dirs",
            "Spec Folders",
            list("a folder of RigSpec YAML", Values::Folder),
        ),
        Field::panel(
            "panel.specs",
            "Spec Library",
            "The specs `ainet up` can start by name, and where each comes from: built in, or a file in one of your folders (the first folder with a name wins).",
            Panel::Specs,
        ),
        Field::daemon(
            "rigs.default_cwd",
            "Default Folder",
            Kind::Path { directory: true },
        ),
        Field::daemon("rigs.snapshot_on_down", "Snapshot on Down", Kind::Toggle).group("Snapshots"),
        Field::daemon(
            "rigs.keep_snapshots",
            "Snapshots Kept per Rig",
            whole("", 1., 10_000., 1.),
        )
        .group("Snapshots"),
    ]
}

fn discovery() -> Vec<Field> {
    vec![
        Field::daemon(
            "discovery.claude_sessions",
            "Show Every Claude Code Session",
            Kind::Toggle,
        ),
        Field::daemon("discovery.tmux", "Look in tmux Panes", Kind::Toggle),
        Field::daemon(
            "discovery.interval_ms",
            "Sessions Checked Every",
            whole("ms", 500., 600_000., 500.),
        ),
        Field::daemon(
            "discovery.transcripts_ms",
            "Transcripts Read Every",
            whole("ms", 500., 600_000., 500.),
        ),
    ]
}

fn hooks() -> Vec<Field> {
    vec![
        Field::panel(
            "panel.hooks",
            "ainet's Hooks in Claude Code",
            "Hooks let Claude Code sessions ainet did not start report to it and send their permission requests to your Inbox. ainet's own seats get them without this.",
            Panel::Hooks,
        ),
        Field::daemon(
            "hooks.claude_settings",
            "Claude Code's Settings File",
            Kind::Path { directory: false },
        ),
    ]
}

fn retention() -> Vec<Field> {
    vec![
        Field::daemon(
            "retention.messages_in_memory",
            "Messages in Memory",
            whole("", 100., 1_000_000., 500.),
        )
        .group("Kept"),
        Field::daemon(
            "retention.chat_in_memory",
            "Chat Posts in Memory",
            whole("", 100., 1_000_000., 500.),
        )
        .group("Kept"),
        Field::daemon(
            "retention.inbox_items",
            "Inbox Items",
            whole("", 10., 100_000., 10.),
        )
        .group("Kept"),
        Field::daemon(
            "retention.scrollback_lines",
            "Terminal Scrollback",
            whole("lines", 0., 1_000_000., 1000.),
        )
        .group("Kept"),
        Field::daemon(
            "retention.events_max_age_days",
            "Delete Events After",
            whole("days", 1., 36_500., 1.),
        )
        .group("Pruned"),
        Field::daemon(
            "retention.events_max_count",
            "Keep at Most",
            whole("events", 1000., 1e12, 1000.),
        )
        .group("Pruned"),
        Field::daemon(
            "retention.inbox_expiry_mins",
            "Drop Answered Inbox Items After",
            whole("min", 1., 1e9, 10.),
        )
        .group("Pruned"),
    ]
}

fn cost() -> Vec<Field> {
    vec![
        Field::daemon("cost.prices", "Prices", Kind::Prices).describe(
            "Dollars per million tokens by model-id prefix (claude-opus-5), over the built-in table; the longest matching prefix wins. Cache writes default to 1.25 times input, cache reads to 0.1 times.",
        ),
        Field::daemon(
            "cost.context_windows",
            "Context Windows",
            Kind::Map {
                key_placeholder: "model prefix",
                value_placeholder: "tokens",
                values: Values::Count { min: 1 },
            },
        ),
    ]
}

fn diagnostics() -> Vec<Field> {
    vec![Field::panel(
        "panel.diagnostics",
        "Paths, Versions and the Log",
        "Paths, versions, the daemon's pid, uptime and log, and the connection: what to paste when something is wrong.",
        Panel::Diagnostics,
    )]
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
            assert_eq!(page.is_app(), field.owner == Owner::App, "{}", field.key);
            if !page.is_app() {
                continue;
            }
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
    fn every_ainet_field_is_a_setting_ainetd_takes() {
        let defaults = serde_json::to_value(ainet_core::settings::Settings::default()).unwrap();
        let mut keys = std::collections::HashSet::new();
        for (page, field) in fields().into_iter().filter(|(p, _)| !p.is_app()) {
            assert!(keys.insert(field.key), "{} twice", field.key);
            if !field.is_setting() {
                assert!(field.key.starts_with("panel."), "{}", field.key);
                assert!(!field.description().is_empty(), "{}", field.key);
                continue;
            }
            assert_eq!(
                Some(field.key.split('.').next().unwrap()),
                page.section(),
                "{} is on its section's page",
                field.key
            );
            ainet_core::settings::check_key(field.key).unwrap_or_else(|e| panic!("{e}"));
            assert!(!field.description().is_empty(), "{}", field.key);
            // Each choice is a value ainetd takes; the default is one.
            if let Kind::Dropdown(choices) = &field.kind {
                for c in choices.iter().filter(|c| !c.value.is_null()) {
                    ainet_core::settings::set("{}", field.key, &c.value)
                        .unwrap_or_else(|e| panic!("{}: {e}", field.key));
                }
                assert!(choices.iter().any(|c| c.value == at(&defaults, field.key)));
            }
            // A number's default is in its range.
            if let Kind::Number { min, max, .. } = field.kind
                && let Some(d) = at(&defaults, field.key).as_f64()
            {
                assert!((min..=max).contains(&d), "{}: {d}", field.key);
            }
            // Fields that can loosen safety say so.
            let loosening = [
                "runtimes.claude.permission_mode",
                "runtimes.claude.allowed_tools",
                "permissions.persist_always",
                "permissions.global_rules",
                "permissions.spec_policies",
            ];
            if loosening.contains(&field.key) {
                assert!(field.caution.is_some(), "{} has a caution", field.key);
            }
        }
        // Every setting of ainet's has a field, but the maps' entries.
        let report = ainet_core::settings::SettingsReport::new(
            &ainet_core::settings::parse("").unwrap(),
            None,
            &[],
            None,
        );
        // Codex has no permission modes or tool lists: setting them is an
        // error, so they get no field.
        let claude_only = [
            "runtimes.codex.permission_mode",
            "runtimes.codex.allowed_tools",
            "runtimes.codex.disallowed_tools",
            "runtimes.codex.strict_mcp_config",
        ];
        for e in report
            .entries
            .iter()
            .filter(|e| !claude_only.contains(&e.key.as_str()))
        {
            assert!(keys.contains(e.key.as_str()), "{} has no field", e.key);
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
