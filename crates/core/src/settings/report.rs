//! The settings in force as a front end shows them: every setting with
//! its value, default, source and when a change applies, and what is
//! wrong with the file. ainetd sends it (`Request::SettingsGet`); the
//! console and the desktop app make the same from the file when there is
//! no daemon to ask.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Effective, Loaded, effective, env_overrides};

/// Every setting in force, and the state of the file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsReport {
    /// The file they come from; none for a daemon that follows no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Each setting, ordered by its dotted key.
    pub entries: Vec<Effective>,
    /// Why the file's text is not in force (the last good settings are).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Keys in ainet's sections that mean nothing to it, ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
    /// After a set or unset: the key changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<String>,
    /// Something the reader should know about where this came from (an
    /// older daemon that was not asked, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

impl SettingsReport {
    /// The report on `loaded` (the last good file), with environment
    /// overrides `env` applied over it.
    pub fn new(
        loaded: &Loaded,
        error: Option<String>,
        env: &[(String, String, String)],
        file: Option<PathBuf>,
    ) -> SettingsReport {
        let settings = loaded.settings.clone().with_env(env);
        SettingsReport {
            file,
            entries: effective(&settings, &loaded.raw, env),
            error,
            unknown: loaded.unknown.clone(),
            changed: None,
            notice: None,
        }
    }

    /// The report a process makes for itself from the file at `path`,
    /// with its own environment: a file that does not read is the
    /// defaults, and the error.
    pub fn read(path: &Path) -> SettingsReport {
        let (loaded, error) = match super::load(path) {
            Ok(loaded) => (
                loaded.unwrap_or_else(|| super::parse("").unwrap_or_default()),
                None,
            ),
            Err(e) => (super::parse("").unwrap_or_default(), Some(e)),
        };
        SettingsReport::new(&loaded, error, &env_overrides(), Some(path.to_path_buf()))
    }

    /// The setting at `key`, or the map an entry of it is in.
    pub fn get(&self, key: &str) -> Option<&Effective> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .or_else(|| self.entries.iter().find(|e| under(key, &e.key)))
    }

    /// Only `key`: the setting, every setting in the group it names, or
    /// the map an entry of it is in. An error when it names none.
    pub fn only(mut self, key: &str) -> Result<SettingsReport, String> {
        let key = key.trim();
        if key.is_empty() {
            return Ok(self);
        }
        self.entries
            .retain(|e| e.key == key || under(&e.key, key) || under(key, &e.key));
        match self.entries.is_empty() {
            true => Err(format!("no setting {key}; ainet config list shows them")),
            false => Ok(self),
        }
    }

    /// The entries whose value is not their default.
    pub fn modified(&self) -> impl Iterator<Item = &Effective> {
        self.entries.iter().filter(|e| e.value != e.default)
    }
}

/// Whether `key` is inside the group (or map) `group`.
fn under(key: &str, group: &str) -> bool {
    key.strip_prefix(group).is_some_and(|rest| rest.starts_with('.'))
}
