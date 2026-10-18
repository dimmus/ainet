//! Which value each setting has, and why: a default, the file, or an
//! environment variable that wins over the file.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{SECTIONS, Settings};

/// Where a setting's value comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum Source {
    Default,
    File,
    /// An environment variable, which wins over the file.
    Env {
        var: String,
    },
}

/// One setting as it is in force.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effective {
    /// Dotted: `messaging.paste_settle_ms`.
    pub key: String,
    pub value: Value,
    pub default: Value,
    pub source: Source,
    /// When a change to it takes effect.
    #[serde(default)]
    pub applies: Applies,
    /// What it does, from the schema.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// When a change to a setting takes effect in ainetd.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    /// At once: the daemon (or the console) reads it on each use.
    #[default]
    Live,
    /// For seats and terminals started from then on.
    NewSeats,
    /// When ainetd next starts.
    Restart,
}

impl Applies {
    /// In words, after "applies": `now`, `to new seats`, `after a daemon
    /// restart`.
    pub fn label(self) -> &'static str {
        match self {
            Applies::Live => "now",
            Applies::NewSeats => "to new seats",
            Applies::Restart => "after a daemon restart",
        }
    }
}

/// When a change to `key` takes effect: the first rule whose key is
/// `key` or a group above it.
const APPLIES: &[(&str, Applies)] = &[
    // Read by the console and the desktop app each time they need ainetd.
    ("daemon.autostart", Applies::Live),
    ("daemon", Applies::Restart),
    ("runtimes", Applies::NewSeats),
    // The hold is in force at once, but seats keep the hook timeout they
    // started with.
    ("permissions.hold_secs", Applies::NewSeats),
    ("permissions.spec_policies", Applies::NewSeats),
    ("messaging.typing_guard", Applies::NewSeats),
    ("retention.scrollback_lines", Applies::NewSeats),
];

/// When a change to the setting `key` takes effect.
pub fn applies(key: &str) -> Applies {
    APPLIES
        .iter()
        .find(|(k, _)| key == *k || key.strip_prefix(k).is_some_and(|rest| rest.starts_with('.')))
        .map_or(Applies::Live, |(_, a)| *a)
}

/// Whether setting `key` to `value` lets agents do more without asking
/// you than ainet's defaults do: a permission mode that loosens, tools
/// allowed without asking, rules that keep allowing, a spec's policy
/// applied, or arguments that skip the agent's own checks. Front ends
/// say so when such a value is set.
pub fn less_safe(key: &str, value: &Value) -> bool {
    let last = key.rsplit('.').next().unwrap_or(key);
    let in_runtime = key.starts_with("runtimes.");
    match last {
        "permission_mode" if in_runtime => value
            .as_str()
            .and_then(|m| m.parse::<super::PermissionMode>().ok())
            .is_some_and(super::PermissionMode::loosens),
        "allowed_tools" if in_runtime => value.as_array().is_some_and(|a| !a.is_empty()),
        "args" if in_runtime => value
            .as_array()
            .is_some_and(|a| args_loosen(&a.iter().filter_map(Value::as_str).collect::<Vec<_>>())),
        _ => match key {
            "permissions.persist_always" | "permissions.spec_policies" => value == &Value::Bool(true),
            "permissions.global_rules" => value.as_array().is_some_and(|a| !a.is_empty()),
            _ => false,
        },
    }
}

/// Whether command-line arguments for an agent skip its checks or allow
/// more without asking: `--dangerously-*`, `--yolo`, a permission mode
/// that loosens (`--permission-mode bypassPermissions`, or with `=`),
/// tools allowed up front (`--allowedTools`), and Codex's `--full-auto`
/// and `danger-full-access` sandbox. They come after ainet's own flags,
/// so they win.
pub fn args_loosen(args: &[&str]) -> bool {
    let loosening_mode = |m: &str| {
        m.parse::<super::PermissionMode>()
            .is_ok_and(super::PermissionMode::loosens)
    };
    args.iter().enumerate().any(|(i, arg)| {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (*arg, None),
        };
        let value = || inline.or_else(|| args.get(i + 1).copied()).unwrap_or_default();
        arg.contains("dangerously")
            || arg.contains("danger-full-access")
            || matches!(flag, "--yolo" | "--full-auto")
            || matches!(flag, "--allowedTools" | "--allowed-tools")
            || (flag == "--permission-mode" && loosening_mode(value()))
            || (matches!(flag, "--ask-for-approval" | "-a") && value() == "never")
    })
}

/// The environment variables that set a setting, and the setting: they
/// meant something before settings.json did, so they still win.
const ENV: &[(&str, &str)] = &[
    ("AINET_SOCKET", "daemon.socket"),
    ("AINET_LOG", "daemon.log.level"),
];

/// The environment variables set now that override a setting: (setting,
/// variable, value).
pub fn env_overrides() -> Vec<(String, String, String)> {
    ENV.iter()
        .filter_map(|(var, key)| {
            let value = std::env::var(var).ok().filter(|v| !v.trim().is_empty())?;
            Some((key.to_string(), var.to_string(), value))
        })
        .collect()
}

impl Settings {
    /// These settings with environment overrides applied.
    pub fn with_env(mut self, overrides: &[(String, String, String)]) -> Settings {
        for (key, _, value) in overrides {
            match key.as_str() {
                "daemon.socket" => self.daemon.socket = Some(value.into()),
                "daemon.log.level" => self.daemon.log.level = value.clone(),
                _ => {}
            }
        }
        self
    }
}

/// Every setting with its value in force, its default and where the value
/// comes from. `settings` are those in force (environment applied), `raw`
/// the file as JSON, `env` what [`env_overrides`] found.
pub fn effective(settings: &Settings, raw: &Value, env: &[(String, String, String)]) -> Vec<Effective> {
    let value = serde_json::to_value(settings).unwrap_or(Value::Null);
    let default = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    let mut out = Vec::new();
    leaves(&default, String::new(), &mut |key, d| {
        let source = match env.iter().find(|(k, _, _)| *k == key) {
            Some((_, var, _)) => Source::Env { var: var.clone() },
            None if at(raw, &key).is_some() => Source::File,
            None => Source::Default,
        };
        out.push(Effective {
            value: at(&value, &key).cloned().unwrap_or(Value::Null),
            default: d.clone(),
            applies: applies(&key),
            description: super::schema_walk::description(&key),
            key,
            source,
        });
    });
    out
}

/// Keys in ainet's sections of `raw` that no setting has: misspelt, or
/// from a newer version.
pub fn unknown_keys(raw: &Value) -> Vec<String> {
    let default = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    let mut out = Vec::new();
    for section in SECTIONS {
        if let (Some(r), Some(d)) = (raw.get(*section), default.get(*section)) {
            unknown_in(r, d, section.to_string(), &mut out);
        }
    }
    out
}

fn unknown_in(raw: &Value, default: &Value, key: String, out: &mut Vec<String>) {
    let (Value::Object(r), Value::Object(d)) = (raw, default) else {
        return;
    };
    // An object that is empty by default is a map of your own names.
    if d.is_empty() {
        return;
    }
    for (k, v) in r {
        let child = format!("{key}.{k}");
        match d.get(k) {
            Some(dv) => unknown_in(v, dv, child, out),
            None => out.push(child),
        }
    }
}

/// Calls `f` with each setting's dotted key and default: values that are
/// not objects, and objects that are empty by default (maps).
pub(super) fn leaves(v: &Value, key: String, f: &mut impl FnMut(String, &Value)) {
    match v {
        Value::Object(map) if !map.is_empty() => {
            for (k, child) in map {
                let child_key = if key.is_empty() {
                    k.clone()
                } else {
                    format!("{key}.{k}")
                };
                leaves(child, child_key, f);
            }
        }
        _ => f(key, v),
    }
}

/// The value at a dotted key.
pub(super) fn at<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(v, |v, k| v.get(k))
}
