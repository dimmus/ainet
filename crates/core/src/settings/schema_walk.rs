//! Finding a setting in the JSON schema: its description, and whether it
//! takes text, so words typed after `/set` or `ainet config set` become
//! the right JSON.

use std::sync::OnceLock;

use serde_json::Value;

/// The schema of ainet's sections, made once.
fn root() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(super::schema)
}

/// `s` with a `$ref` followed, and an optional value's `null` branch
/// dropped.
fn resolve<'a>(root: &'a Value, s: &'a Value) -> &'a Value {
    if let Some(name) = s
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
        && let Some(def) = root.get("$defs").and_then(|d| d.get(name))
    {
        return resolve(root, def);
    }
    for k in ["anyOf", "oneOf"] {
        if let Some(branches) = s.get(k).and_then(Value::as_array) {
            let real: Vec<&Value> = branches.iter().filter(|b| !is_null(b)).collect();
            if let [one] = real.as_slice() {
                return resolve(root, one);
            }
        }
    }
    s
}

fn is_null(s: &Value) -> bool {
    s.get("type").and_then(Value::as_str) == Some("null")
}

/// The schema of the setting at a dotted key, as its parent lists it (so
/// with its description); an entry of a map is the map's value schema.
fn property(key: &str) -> Option<&'static Value> {
    let root = root();
    let mut current = root;
    for part in key.split('.') {
        let object = resolve(root, current);
        current = object
            .get("properties")
            .and_then(|p| p.get(part))
            .or_else(|| object.get("additionalProperties").filter(|a| a.is_object()))?;
    }
    Some(current)
}

/// What the setting at `key` does, from its doc comment; empty for an
/// entry of a map or a key that is not a setting.
pub fn description(key: &str) -> String {
    property(key)
        .and_then(|p| p.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The JSON types a schema allows (`string` for a closed list of names).
fn types(root: &Value, s: &Value, out: &mut Vec<String>) {
    let s = resolve(root, s);
    match s.get("type") {
        Some(Value::String(t)) => out.push(t.clone()),
        Some(Value::Array(ts)) => out.extend(ts.iter().filter_map(Value::as_str).map(str::to_string)),
        _ => {}
    }
    if s.get("const").is_some_and(Value::is_string)
        || s.get("enum")
            .and_then(Value::as_array)
            .is_some_and(|e| e.iter().all(Value::is_string))
    {
        out.push("string".into());
    }
    for k in ["anyOf", "oneOf"] {
        for branch in s.get(k).and_then(Value::as_array).into_iter().flatten() {
            types(root, branch, out);
        }
    }
}

/// Whether the setting at `key` takes text and nothing but text (or
/// nothing): a name, a path, a model, a mode.
pub fn takes_text(key: &str) -> bool {
    let Some(p) = property(key) else { return false };
    let mut all = Vec::new();
    types(root(), p, &mut all);
    all.iter().any(|t| t == "string") && all.iter().all(|t| t == "string" || t == "null")
}
