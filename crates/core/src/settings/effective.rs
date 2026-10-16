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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effective {
    /// Dotted: `messaging.paste_settle_ms`.
    pub key: String,
    pub value: Value,
    pub default: Value,
    pub source: Source,
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
fn leaves(v: &Value, key: String, f: &mut impl FnMut(String, &Value)) {
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
fn at<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(v, |v, k| v.get(k))
}
