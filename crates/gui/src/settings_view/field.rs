//! What the Settings view shows of one setting, without GPUI: its key,
//! label, kind of control, whose it is (the app's or ainetd's), its value
//! and default as JSON, how they read, and what a typed value becomes.

use ainet_core::settings::Applies;
use serde_json::Value;

/// Who reads a setting, and so how it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The desktop app (`theme`, `terminal`, …): written to settings.json
    /// here and applied at once.
    App,
    /// ainetd and the console (`daemon`, `runtimes`, …): written through
    /// ainetd when it runs, which puts it in force, else to the file.
    Daemon,
}

/// One choice of a dropdown: its words and the JSON it writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: &'static str,
    pub value: Value,
}

impl Choice {
    /// A choice written as the text `value`.
    pub fn text(label: &'static str, value: &str) -> Choice {
        Choice {
            label,
            value: Value::String(value.into()),
        }
    }
}

/// What the items of a list, or the values of a map, are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Values {
    /// Text, as typed.
    Text,
    /// Folders, typed or picked with Browse….
    Folder,
    /// Whole numbers of at least `min`.
    Count { min: u64 },
    /// JSON (a rule, an MCP server), checked as it is typed.
    Json,
}

impl Values {
    /// What typed `text` is as an item or value, or why it is none.
    pub fn parse(self, text: &str) -> Result<Value, String> {
        let text = text.trim();
        match self {
            Values::Text | Values::Folder => Ok(Value::String(text.to_string())),
            Values::Count { min } => match text.parse::<u64>() {
                Ok(n) if n >= min => Ok(Value::from(n)),
                _ => Err(format!("a whole number of at least {min}")),
            },
            Values::Json => serde_json::from_str(text).map_err(|e| format!("not JSON: {e}")),
        }
    }

    /// An item or value in words.
    pub fn show(self, value: &Value) -> String {
        match value {
            Value::String(s) if self != Values::Json => s.clone(),
            v => compact(v),
        }
    }
}

/// A part of a page that is not one setting: shown, and searched, among
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    /// The `/always` rules in force, each with Revoke.
    Always,
    /// ainet's hooks in Claude Code's settings: per event, and Install,
    /// Repair, Uninstall.
    Hooks,
    /// The spec library and where each spec comes from.
    Specs,
    /// Paths, versions, the daemon, the connection and its log.
    Diagnostics,
}

/// The control a setting gets.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// On or off.
    Toggle,
    /// One of a closed list (exact values only, so a typo is impossible).
    Dropdown(Vec<Choice>),
    /// A number within a range, in `unit`; empty means unset (the
    /// default).
    Number {
        unit: &'static str,
        min: f64,
        max: f64,
        step: f64,
        integer: bool,
    },
    /// Free text; empty means unset.
    Text { placeholder: &'static str },
    /// A file or folder, typed or picked with Browse…; empty means unset.
    #[cfg_attr(not(test), allow(dead_code))]
    Path { directory: bool },
    /// A list (of texts, folders, rules), added and removed one at a
    /// time.
    List {
        placeholder: &'static str,
        values: Values,
    },
    /// Names to values (environment variables, MCP servers), added and
    /// removed.
    Map {
        key_placeholder: &'static str,
        value_placeholder: &'static str,
        values: Values,
    },
    /// Model prices: a table of model prefixes and dollars per million
    /// tokens.
    Prices,
    /// The theme: the theme selector, which previews as it moves.
    Theme,
    /// Not a setting: a panel of its own.
    Panel(Panel),
}

/// Whether a part of a setting shows, from the setting's value.
pub type When = fn(&Value) -> bool;

/// A setting as the view lists it.
#[derive(Clone, Debug)]
pub struct Field {
    /// Dotted, as in settings.json: `terminal.font_size`.
    pub key: &'static str,
    pub label: &'static str,
    /// What it does. ainetd's settings may leave it empty: the schema's
    /// description is used.
    pub description: &'static str,
    pub kind: Kind,
    pub owner: Owner,
    /// Part of the setting above it (a custom line height's ratio): no
    /// reset of its own, shown only while the function holds of that
    /// setting's value.
    pub part_of: Option<(&'static str, When)>,
    /// What unset means, in words, from the app's settings in force (the
    /// terminal's size follows the UI's).
    pub when_unset: Option<fn(&crate::settings::Settings) -> String>,
    /// The sub-section of its page it is in (a runtime); empty for none.
    pub group: &'static str,
    /// A warning shown with it always: it can let agents do more without
    /// asking you. Such a change is confirmed before it is written.
    pub caution: Option<&'static str>,
}

impl Field {
    /// One of the app's own settings.
    pub fn app(key: &'static str, label: &'static str, description: &'static str, kind: Kind) -> Field {
        Field {
            key,
            label,
            description,
            kind,
            owner: Owner::App,
            part_of: None,
            when_unset: None,
            group: "",
            caution: None,
        }
    }

    /// One of ainetd's settings; its description is the schema's unless
    /// one is given with [`Field::describe`].
    pub fn daemon(key: &'static str, label: &'static str, kind: Kind) -> Field {
        Field {
            key,
            label,
            description: "",
            kind,
            owner: Owner::Daemon,
            part_of: None,
            when_unset: None,
            group: "",
            caution: None,
        }
    }

    /// A panel that is not a setting, under the pseudo-key `key`
    /// (`panel.always`), searched by its label and description.
    pub fn panel(key: &'static str, label: &'static str, description: &'static str, panel: Panel) -> Field {
        Field {
            description,
            ..Field::daemon(key, label, Kind::Panel(panel))
        }
    }

    /// In the sub-section `group` of its page.
    pub fn group(mut self, group: &'static str) -> Field {
        self.group = group;
        self
    }

    /// Warned about with `why`, and confirmed before a looser value is
    /// written.
    pub fn caution(mut self, why: &'static str) -> Field {
        self.caution = Some(why);
        self
    }

    /// Whether it is a setting (else a panel).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_setting(&self) -> bool {
        !matches!(self.kind, Kind::Panel(_))
    }

    /// With its own description.
    pub fn describe(mut self, description: &'static str) -> Field {
        self.description = description;
        self
    }

    /// Shown only while `parent`'s value is as `when` says; no reset.
    pub fn part_of(mut self, parent: &'static str, when: When) -> Field {
        self.part_of = Some((parent, when));
        self
    }

    /// Says what unset means with `words`, from the settings in force.
    pub fn when_unset(mut self, words: fn(&crate::settings::Settings) -> String) -> Field {
        self.when_unset = Some(words);
        self
    }

    /// The key as a path of object keys.
    pub fn path(&self) -> Vec<&'static str> {
        self.key.split('.').collect()
    }

    /// The description to show: its own, else ainetd's schema's.
    pub fn description(&self) -> String {
        match (self.description, self.owner) {
            ("", Owner::Daemon) => ainet_core::settings::description(self.key),
            (d, _) => d.to_string(),
        }
    }

    /// When a change takes effect: the app's at once, ainetd's by its
    /// table.
    pub fn applies(&self) -> Applies {
        match self.owner {
            Owner::App => Applies::Live,
            Owner::Daemon => ainet_core::settings::applies(self.key),
        }
    }

    /// Whether every word of `query` is in its label, description or key
    /// (any case): the search across categories.
    pub fn matches(&self, query: &str) -> bool {
        let haystack = format!("{} {} {}", self.label, self.description(), self.key).to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }

    /// `value` in words, for the default and the dropdown's button.
    pub fn show(&self, value: &Value) -> String {
        match (&self.kind, value) {
            // A dropdown may name what unset means ("Claude Code's
            // default").
            (Kind::Dropdown(choices), v) => match chosen(choices, v) {
                Some(c) => c.label.to_string(),
                None if v.is_null() => "unset".into(),
                None => compact(v),
            },
            (_, Value::Null) => "unset".into(),
            (Kind::Toggle, Value::Bool(on)) => if *on { "on" } else { "off" }.into(),
            (Kind::Number { unit, .. }, Value::Number(n)) => {
                let n = number_text(n.as_f64().unwrap_or_default());
                match unit {
                    &"" => n,
                    unit => format!("{n} {unit}"),
                }
            }
            (Kind::List { .. }, Value::Array(items)) if items.is_empty() => "none".into(),
            (Kind::Map { .. } | Kind::Prices, Value::Object(map)) if map.is_empty() => "none".into(),
            (_, Value::String(s)) if s.is_empty() => "empty".into(),
            (_, Value::String(s)) => s.clone(),
            (_, v) => compact(v),
        }
    }

    /// The text an input shows for `value`: empty for unset.
    pub fn input_text(&self, value: &Value) -> String {
        match value {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            Value::Number(n) => number_text(n.as_f64().unwrap_or_default()),
            v => compact(v),
        }
    }

    /// What typed `text` sets: `None` to unset (empty), or the JSON value;
    /// an error, in words, for a number that is not one or out of range.
    pub fn parse_input(&self, text: &str) -> Result<Option<Value>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        match &self.kind {
            Kind::Number {
                unit,
                min,
                max,
                integer,
                ..
            } => {
                let n: f64 = text
                    .parse()
                    .ok()
                    .filter(|n: &f64| n.is_finite())
                    .ok_or_else(|| format!("{text:?} is not a number"))?;
                let unit = if unit.is_empty() {
                    String::new()
                } else {
                    format!(" {unit}")
                };
                if n < *min || n > *max {
                    return Err(format!(
                        "from {} to {}{unit}",
                        number_text(*min),
                        number_text(*max)
                    ));
                }
                if *integer && n.fract() != 0. {
                    return Err("a whole number".into());
                }
                Ok(Some(number_value(n, *integer)))
            }
            _ => Ok(Some(Value::String(text.to_string()))),
        }
    }

    /// The number one `step` (times `by`) from `value`, kept in range;
    /// from the default when unset.
    pub fn stepped(&self, value: &Value, default: &Value, by: f64) -> Option<Value> {
        let Kind::Number {
            min,
            max,
            step,
            integer,
            ..
        } = &self.kind
        else {
            return None;
        };
        let from = value.as_f64().or_else(|| default.as_f64()).unwrap_or(*min);
        let to = ((from + step * by) / step).round() * step;
        Some(number_value(to.clamp(*min, *max), *integer))
    }
}

/// Whether changing ainetd's setting `key` from `current` to `new` lets
/// agents do more without asking you: a looser value, or items added to
/// a list that loosen (a tool allowed, a global rule, a dangerous
/// argument). Taking something away never does, but from the lists that
/// hold agents back: tools disallowed, paths kept sensitive.
pub fn loosens(key: &str, current: &Value, new: &Value) -> bool {
    use ainet_core::settings::less_safe;
    let restricts = key.ends_with(".disallowed_tools") || key == "permissions.sensitive_paths";
    match (current, new) {
        (Value::Array(old), new) if restricts => {
            let kept = new.as_array().map(Vec::as_slice).unwrap_or_default();
            old.iter().any(|v| !kept.contains(v))
        }
        // Arguments go in pairs (`--permission-mode bypassPermissions`),
        // so the list is judged whole: looser now and not before.
        (old, new) if key.ends_with(".args") => less_safe(key, new) && !less_safe(key, old),
        (Value::Array(old), Value::Array(items)) => {
            let added: Vec<Value> = items.iter().filter(|v| !old.contains(v)).cloned().collect();
            !added.is_empty() && less_safe(key, &Value::Array(added))
        }
        _ => !same(current, new) && less_safe(key, new),
    }
}

/// The choice `value` is: the same value, or an object of the same keys
/// (a custom line height of any ratio is the Custom choice).
pub fn chosen<'a>(choices: &'a [Choice], value: &Value) -> Option<&'a Choice> {
    choices.iter().find(|c| match (&c.value, value) {
        (Value::Object(a), Value::Object(b)) => a.keys().eq(b.keys()),
        (a, b) => same(a, b),
    })
}

/// Whether two values are the same setting: numbers by value (16 and
/// 16.0 are one size).
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// The value at a dotted `key` in `root`, `Null` when it is not there.
pub fn at(root: &Value, key: &str) -> Value {
    key.split('.')
        .try_fold(root, |v, k| v.get(k))
        .cloned()
        .unwrap_or(Value::Null)
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// A number as typed: no `.0` on whole ones, no float noise (`1.3`, not
/// `1.2999999523`, from an `f32` setting).
fn number_text(n: f64) -> String {
    let rounded = (n * 1e4).round() / 1e4;
    if rounded.fract() == 0. && rounded.abs() < 1e15 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// A number as JSON: whole ones without `.0`, as they are typed.
fn number_value(n: f64, integer: bool) -> Value {
    if integer || n.fract() == 0. {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64((n * 1e4).round() / 1e4).map_or(Value::Null, Value::Number)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn size() -> Field {
        Field::app(
            "ui_font_size",
            "UI Font Size",
            "The size of the UI's text.",
            Kind::Number {
                unit: "px",
                min: 6.,
                max: 100.,
                step: 1.,
                integer: false,
            },
        )
    }

    #[test]
    fn numbers_read_with_their_unit_and_range() {
        let f = size();
        assert_eq!(f.show(&json!(16.0)), "16 px");
        assert_eq!(f.input_text(&json!(1.2999999523162842)), "1.3");
        assert_eq!(f.parse_input("14").unwrap(), Some(json!(14)));
        assert_eq!(f.parse_input("14.5").unwrap(), Some(json!(14.5)));
        assert_eq!(f.parse_input("  ").unwrap(), None, "empty unsets");
        assert_eq!(f.parse_input("big").unwrap_err(), "\"big\" is not a number");
        assert_eq!(f.parse_input("200").unwrap_err(), "from 6 to 100 px");
        assert_eq!(f.stepped(&json!(16.), &json!(16.), 1.), Some(json!(17)));
        assert_eq!(f.stepped(&Value::Null, &json!(16.), -1.), Some(json!(15)));
        assert_eq!(
            f.stepped(&json!(100.), &json!(16.), 1.),
            Some(json!(100)),
            "kept in range"
        );
        let ms = Field::daemon(
            "messaging.flush_gap_ms",
            "Flush Gap",
            Kind::Number {
                unit: "ms",
                min: 0.,
                max: 60_000.,
                step: 50.,
                integer: true,
            },
        );
        assert_eq!(ms.parse_input("1.5").unwrap_err(), "a whole number");
        assert_eq!(ms.parse_input("450").unwrap(), Some(json!(450)));
        assert!(!ms.description().is_empty(), "the schema's description");
        assert_eq!(ms.applies(), Applies::Live);
        assert_eq!(size().applies(), Applies::Live);
    }

    #[test]
    fn values_read_as_words() {
        let dock = Field::app(
            "fleet_panel.dock",
            "Fleet Panel Side",
            "",
            Kind::Dropdown(vec![Choice::text("Left", "left"), Choice::text("Right", "right")]),
        );
        assert_eq!(dock.show(&json!("right")), "Right");
        assert_eq!(dock.show(&Value::Null), "unset");
        let toggle = Field::app("confirm_quit", "Confirm Quit", "", Kind::Toggle);
        assert_eq!(
            (toggle.show(&json!(true)), toggle.show(&json!(false))),
            ("on".into(), "off".into())
        );
        let list = Field::daemon(
            "rigs.spec_dirs",
            "Spec Folders",
            Kind::List {
                placeholder: "",
                values: Values::Folder,
            },
        );
        assert_eq!(list.show(&json!([])), "none");
        assert_eq!(list.show(&json!(["/a"])), "[\"/a\"]");
        let mode = Field::daemon(
            "runtimes.claude.permission_mode",
            "Permission Mode",
            Kind::Dropdown(vec![
                Choice {
                    label: "Claude Code's Default",
                    value: Value::Null,
                },
                Choice::text("Plan", "plan"),
            ]),
        );
        assert_eq!(mode.show(&Value::Null), "Claude Code's Default");
        assert_eq!(mode.show(&json!("plan")), "Plan");
        assert!(same(&json!(16), &json!(16.0)));
        assert!(!same(&json!("16"), &json!(16)));
    }

    #[test]
    fn search_finds_words_in_label_description_and_key() {
        let f = size();
        for query in ["font", "UI SIZE", "ui_font", "text size", ""] {
            assert!(f.matches(query), "{query}");
        }
        assert!(!f.matches("terminal"));
        assert!(!f.matches("font colour"), "every word");
        let d = Field::daemon("queue.nudge", "Nudge", Kind::Toggle);
        assert!(d.matches("queue"), "by key");
    }

    #[test]
    fn only_looser_changes_loosen() {
        let mode = "runtimes.claude.permission_mode";
        assert!(loosens(mode, &Value::Null, &json!("bypassPermissions")));
        assert!(loosens(mode, &json!("acceptEdits"), &json!("bypassPermissions")));
        assert!(!loosens(
            mode,
            &json!("bypassPermissions"),
            &json!("bypassPermissions")
        ));
        assert!(!loosens(mode, &json!("bypassPermissions"), &json!("plan")));
        let tools = "runtimes.claude.allowed_tools";
        assert!(loosens(tools, &json!([]), &json!(["Bash"])));
        assert!(loosens(tools, &json!(["Bash"]), &json!(["Bash", "Edit"])));
        assert!(
            !loosens(tools, &json!(["Bash", "Edit"]), &json!(["Bash"])),
            "taking away"
        );
        let args = "runtimes.claude.args";
        assert!(!loosens(args, &json!([]), &json!(["--verbose"])));
        assert!(loosens(
            args,
            &json!(["--verbose"]),
            &json!(["--verbose", "--dangerously-skip-permissions"])
        ));
        assert!(
            !loosens(
                args,
                &json!(["--dangerously-skip-permissions"]),
                &json!(["--dangerously-skip-permissions", "--verbose"])
            ),
            "only what is added counts"
        );
        assert!(loosens(
            args,
            &json!(["--verbose"]),
            &json!(["--verbose", "--permission-mode", "bypassPermissions"])
        ));
        assert!(loosens(
            args,
            &json!(["--permission-mode"]),
            &json!(["--permission-mode", "acceptEdits"])
        ));
        assert!(loosens(args, &json!([]), &json!(["--allowedTools", "Bash"])));
        assert!(!loosens(args, &json!([]), &json!(["--permission-mode=plan"])));
        assert!(loosens("permissions.persist_always", &json!(false), &json!(true)));
        assert!(!loosens(
            "permissions.persist_always",
            &json!(true),
            &json!(false)
        ));
        assert!(!loosens("queue.nudge", &json!(true), &json!(false)));
        // Lists that hold agents back loosen as they shrink.
        let denied = "runtimes.claude.disallowed_tools";
        assert!(!loosens(denied, &json!([]), &json!(["WebFetch"])));
        assert!(loosens(denied, &json!(["WebFetch"]), &json!([])));
        assert!(
            loosens(denied, &json!(["WebFetch"]), &Value::Null),
            "reset to none"
        );
        assert!(loosens(
            "permissions.sensitive_paths",
            &json!(["~/keys"]),
            &json!([])
        ));
    }

    #[test]
    fn items_and_values_are_read_as_their_kind() {
        assert_eq!(Values::Text.parse(" a b ").unwrap(), json!("a b"));
        assert_eq!(Values::Count { min: 1 }.parse("200000").unwrap(), json!(200000));
        assert_eq!(
            Values::Count { min: 1 }.parse("0").unwrap_err(),
            "a whole number of at least 1"
        );
        assert_eq!(
            Values::Json.parse(r#"{"command": "cargo test"}"#).unwrap(),
            json!({ "command": "cargo test" })
        );
        assert!(
            Values::Json
                .parse("{command")
                .unwrap_err()
                .starts_with("not JSON")
        );
        assert_eq!(
            Values::Json.show(&json!({ "tool": "WebSearch" })),
            r#"{"tool":"WebSearch"}"#
        );
        assert_eq!(Values::Text.show(&json!("x")), "x");
    }

    #[test]
    fn values_are_found_by_dotted_key() {
        let root = json!({ "terminal": { "line_height": { "custom": 1.4 } } });
        assert_eq!(at(&root, "terminal.line_height.custom"), json!(1.4));
        assert_eq!(at(&root, "terminal.font_size"), Value::Null);
        assert_eq!(at(&root, "nowhere.at.all"), Value::Null);
    }
}
