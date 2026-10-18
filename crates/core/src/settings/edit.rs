//! Changing one setting in settings.json: by its dotted key, keeping the
//! rest of the file (comments, order, the desktop app's keys) as written.
//! A key that is not one of ainet's settings, or a value that would not
//! read, is refused before anything is written.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::effective::{at, leaves};
use super::{SECTIONS, Settings, parse};

/// Whether `key` names one of ainet's settings, or an entry of one that
/// is a map of your own names (`runtimes.claude.env.RUST_LOG`). Groups
/// (`queue.watchdog`) are refused: set what is in them.
pub fn check_key(key: &str) -> Result<(), String> {
    let section = key.split('.').next().unwrap_or_default();
    if key.is_empty() || key.split('.').any(str::is_empty) {
        return Err(format!(
            "{key:?} is not a setting: give a dotted key such as queue.nudge"
        ));
    }
    if !SECTIONS.contains(&section) {
        return Err(format!(
            "{key} is not one of ainet's settings (ainet config list shows them); the desktop app's own keys are set in its Settings or with ainet config edit"
        ));
    }
    let default = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    let mut found = false;
    leaves(&default, String::new(), &mut |leaf, d| {
        let is_map = d.as_object().is_some_and(|o| o.is_empty());
        if leaf == key
            || (is_map
                && key
                    .strip_prefix(leaf.as_str())
                    .is_some_and(|r| r.starts_with('.')))
        {
            found = true;
        }
    });
    if found {
        return Ok(());
    }
    if at(&default, key).is_some_and(Value::is_object) {
        return Err(format!(
            "{key} is a group of settings: set one of them ({key}.<name>)"
        ));
    }
    Err(format!("no setting {key}; ainet config list shows them"))
}

/// Whether `key` is a setting or a group of them, as `unset` takes.
fn check_unset_key(key: &str) -> Result<(), String> {
    let default = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    match check_key(key) {
        Err(_)
            if SECTIONS.contains(&key.split('.').next().unwrap_or_default())
                && at(&default, key).is_some_and(Value::is_object) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// The JSON for words typed after a key: text for a setting that takes
/// text (unless quoted, or `null`), else the words read as JSON (`true`,
/// `600`, `["a","b"]`), else text.
pub fn value_from_text(key: &str, words: &str) -> Value {
    let words = words.trim();
    let json = crate::jsonc::to_value(words).ok();
    if super::schema_walk::takes_text(key) {
        return match json {
            Some(v @ (Value::String(_) | Value::Null)) => v,
            _ => Value::String(words.to_string()),
        };
    }
    json.unwrap_or_else(|| Value::String(words.to_string()))
}

/// settings.json's `text` with `key` set to `value`. Refused, with why,
/// when the key is not a setting, the file does not read now, or the
/// value would not (a wrong type, out of range, a mode misspelt).
pub fn set(text: &str, key: &str, value: &Value) -> Result<String, String> {
    check_key(key)?;
    readable(text)?;
    let json = serde_json::to_string(value).map_err(|e| e.to_string())?;
    let path: Vec<&str> = key.split('.').collect();
    let edited = crate::jsonc::set(text, &path, &json).map_err(|e| format!("settings.json: {e}"))?;
    parse(&edited).map_err(|e| format!("{key} cannot be {json}: {e}"))?;
    Ok(edited)
}

/// settings.json's `text` without `key` (a setting or a whole group), so
/// its default applies. Text without it comes back as it was.
pub fn unset(text: &str, key: &str) -> Result<String, String> {
    check_unset_key(key)?;
    readable(text)?;
    let path: Vec<&str> = key.split('.').collect();
    let edited = crate::jsonc::unset(text, &path).map_err(|e| format!("settings.json: {e}"))?;
    parse(&edited).map_err(|e| format!("without {key}: {e}"))?;
    Ok(edited)
}

/// A file that does not read now is not edited: the change would be
/// lost among its problems, or hide them.
fn readable(text: &str) -> Result<(), String> {
    parse(text)
        .map(drop)
        .map_err(|e| format!("settings.json has a problem already: {e}; fix it first (ainet config edit)"))
}

/// One change to a file at a time in this process: two at once would
/// each read the file before the other wrote it, and one would be lost.
static EDITING: Mutex<()> = Mutex::new(());

/// Sets `key` to `value` in the settings file at `path` (made if there is
/// none). True when the file changed.
pub fn set_in_file(path: &Path, key: &str, value: &Value) -> Result<bool, String> {
    edit_file(path, |text| set(text, key, value))
}

/// Removes `key` from the settings file at `path`. True when the file
/// changed (false: it was not set).
pub fn unset_in_file(path: &Path, key: &str) -> Result<bool, String> {
    edit_file(path, |text| unset(text, key))
}

fn edit_file(path: &Path, edit: impl FnOnce(&str) -> Result<String, String>) -> Result<bool, String> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let edited = edit(&text)?;
    if edited == text {
        return Ok(false);
    }
    write_whole(path, &edited).map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    Ok(true)
}

/// Writes a file whole or not at all: into a file beside it, then renamed
/// over it, so a crash halfway never leaves settings.json cut short.
/// Through a symlink (settings kept with your dotfiles), the file it
/// points at is written, so the link stays.
pub fn write_whole(file: &Path, text: &str) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let file = &std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut part = file.as_os_str().to_owned();
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    part.push(format!(".{}.{n}.saving", std::process::id()));
    let part = std::path::PathBuf::from(part);
    let written = std::fs::File::create(&part).and_then(|mut out| {
        out.write_all(text.as_bytes())?;
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
