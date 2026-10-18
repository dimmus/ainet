//! Reading settings.json, and following it: a file that does not parse or
//! says something out of range is reported, and the last good settings
//! stay in force.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::{SECTIONS, Settings, unknown_keys};

/// Settings read from a file, with what the file itself said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// The file as JSON (an empty object without one), to tell what it
    /// sets from what is a default.
    pub raw: Value,
    /// Keys in ainet's sections that mean nothing to this version
    /// (misspelt, or from a newer one). They are ignored.
    pub unknown: Vec<String>,
}

/// Reads settings from a file's text. An empty file is the defaults; the
/// desktop app's keys and unknown keys are ignored; a wrong type or an
/// out-of-range value is an error naming it.
pub fn parse(text: &str) -> Result<Loaded, String> {
    if text.trim().is_empty() {
        return Ok(Loaded {
            raw: Value::Object(Default::default()),
            ..Loaded::default()
        });
    }
    let raw = crate::jsonc::to_value(text)?;
    let Value::Object(map) = &raw else {
        return Err("settings.json is not a JSON object".into());
    };
    // Only ainet's sections, so an error names one of them, not a key of
    // the desktop app's.
    let mut ours = serde_json::Map::new();
    for key in SECTIONS {
        if let Some(v) = map.get(*key) {
            let section: Value = v.clone();
            if !section.is_object() {
                return Err(format!("{key} must be an object"));
            }
            ours.insert((*key).to_string(), section);
        }
    }
    let settings: Settings = section_errors(&ours)?;
    let problems = settings.problems();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(Loaded {
        unknown: unknown_keys(&raw),
        settings,
        raw,
    })
}

/// The settings from `ours`, or the first section that does not read,
/// with its name.
fn section_errors(ours: &serde_json::Map<String, Value>) -> Result<Settings, String> {
    match serde_json::from_value::<Settings>(Value::Object(ours.clone())) {
        Ok(s) => Ok(s),
        Err(whole) => {
            for (key, v) in ours {
                let mut one = serde_json::Map::new();
                one.insert(key.clone(), v.clone());
                if let Err(e) = serde_json::from_value::<Settings>(Value::Object(one)) {
                    return Err(format!("{key}: {e}"));
                }
            }
            Err(whole.to_string())
        }
    }
}

/// Reads settings from `path`: `Ok(None)` when there is no file.
pub fn load(path: &Path) -> Result<Option<Loaded>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// One section of settings.json (`daemon`, `rigs`, ...) read on its own,
/// the others left at their defaults, so a mistake in another section
/// does not change it. For what must not follow someone else's typo:
/// where ainetd's socket is, whether it is started. The defaults when
/// there is no file, or this section does not read or is out of range.
pub fn load_section(path: &Path, key: &str) -> Settings {
    std::fs::read_to_string(path)
        .map(|text| parse_section(&text, key))
        .unwrap_or_default()
}

/// [`load_section`] on a file's text.
pub fn parse_section(text: &str, key: &str) -> Settings {
    let section = || -> Option<Settings> {
        let raw = crate::jsonc::to_value(text).ok()?;
        let Some(section) = raw.get(key) else {
            return Some(Settings::default());
        };
        if !section.is_object() {
            return None;
        }
        let mut one = serde_json::Map::new();
        one.insert(key.to_string(), section.clone());
        let settings: Settings = serde_json::from_value(Value::Object(one)).ok()?;
        // The other sections are defaults, which have no problems: any
        // problem is this section's.
        settings.problems().is_empty().then_some(settings)
    };
    section().unwrap_or_default()
}

/// The JSON schema of ainet's sections, for editors and the desktop app's
/// published schema.
pub fn schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Settings)).unwrap_or(Value::Null)
}

/// What tells one version of a file from the next: modification time,
/// size and a hash of the contents (a quick re-save of the same size can
/// keep the time on a coarse file system); `None` when it is gone.
type Stamp = Option<(SystemTime, u64, u64)>;

fn stamp_of(path: &Path, bytes: &[u8]) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some((meta.modified().ok()?, meta.len(), hasher.finish()))
}

/// Follows settings.json: [`Follower::poll`] re-reads it when it changed.
/// Calls block on the file system, so callers on an async runtime run
/// them on a blocking thread.
#[derive(Debug)]
pub struct Follower {
    path: Option<PathBuf>,
    stamp: Stamp,
    /// The settings in force: the last good ones, the defaults before any.
    pub loaded: Loaded,
    /// Why the file's current text is not in force, if it is not.
    pub error: Option<String>,
}

impl Follower {
    /// Reads `path` now (no path: the defaults, for tests).
    pub fn new(path: Option<PathBuf>) -> Follower {
        let mut f = Follower {
            path,
            stamp: None,
            loaded: parse("").unwrap_or_default(),
            error: None,
        };
        f.read();
        f
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn settings(&self) -> &Settings {
        &self.loaded.settings
    }

    /// Re-reads the file if it changed since the last look. True when the
    /// settings in force or the error changed.
    pub fn poll(&mut self) -> bool {
        let Some(path) = &self.path else { return false };
        let bytes = std::fs::read(path).ok();
        let now = bytes.as_deref().and_then(|b| stamp_of(path, b));
        if now == self.stamp {
            return false;
        }
        self.read()
    }

    fn read(&mut self) -> bool {
        let Some(path) = &self.path else { return false };
        let bytes = std::fs::read(path).ok();
        self.stamp = bytes.as_deref().and_then(|b| stamp_of(path, b));
        let before = (self.loaded.clone(), self.error.clone());
        match bytes {
            // Gone: the defaults, as with no file at all.
            None => {
                self.loaded = parse("").unwrap_or_default();
                self.error = None;
            }
            Some(bytes) => match String::from_utf8(bytes)
                .map_err(|_| "settings.json is not UTF-8".to_string())
                .and_then(|text| parse(&text))
            {
                Ok(loaded) => {
                    self.loaded = loaded;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            },
        }
        (self.loaded.clone(), self.error.clone()) != before
    }
}
