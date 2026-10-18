//! Installs ainet's hook command into Claude Code's user settings, the way
//! Den's `internal/hooks` (MIT) does.
//!
//! One entry per event in [`SPECS`], each running the hook command (such as
//! `/usr/local/bin/ainet hook`). Entries are recognised by that command, so
//! installing again replaces the old ones (also those of an ainet binary at
//! another path). The settings file is backed up first
//! (`settings.json.bak.ainet.<unix seconds>`, with `.1`, `.2`, … when that
//! second already has one), written to a temporary file
//! (mode 0600) and renamed into place. Every other key and hook entry is
//! kept. Like Den, the file is rewritten with keys sorted and two-space
//! indentation (Den's Go writer also escapes `<`, `>` and `&` as `<` and
//! the like; ainet writes them as they are, which Claude Code reads the
//! same). Hooks apply to sessions started after the change.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::util;

/// The program name whose hook entries are ours when no other is given.
pub const PROGRAM: &str = "ainet";

/// The suffix every hook command ends with.
const MARKER: &str = " hook";

/// One hook event to subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpec {
    pub event: &'static str,
    /// "" for every tool.
    pub matcher: &'static str,
    /// Seconds Claude Code lets the hook run.
    pub timeout: u64,
}

/// What Den installs: every event at 10 s (5 s for SessionEnd and the
/// failure events), `PreToolUse` only for `AskUserQuestion`, and
/// `PermissionRequest` at 600 s, held open while the human decides (the
/// daemon's hold of 580 s must stay inside it).
pub const SPECS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        matcher: "",
        timeout: 10,
    },
    HookSpec {
        event: "SessionEnd",
        matcher: "",
        timeout: 5,
    },
    HookSpec {
        event: "UserPromptSubmit",
        matcher: "",
        timeout: 10,
    },
    HookSpec {
        event: "Notification",
        matcher: "",
        timeout: 10,
    },
    HookSpec {
        event: "Stop",
        matcher: "",
        timeout: 10,
    },
    HookSpec {
        event: "PostToolUse",
        matcher: "",
        timeout: 10,
    },
    HookSpec {
        event: "PreToolUse",
        matcher: "AskUserQuestion",
        timeout: 10,
    },
    HookSpec {
        event: "PermissionRequest",
        matcher: "",
        timeout: super::PERMISSION_TIMEOUT_SECS,
    },
    HookSpec {
        event: "StopFailure",
        matcher: "",
        timeout: 5,
    },
    HookSpec {
        event: "PostToolUseFailure",
        matcher: "",
        timeout: 5,
    },
];

/// Claude Code's user settings file: `<config dir>/settings.json`.
pub fn settings_path() -> PathBuf {
    util::config_dir().join("settings.json")
}

/// [`settings_path`] under an explicit config directory.
pub fn settings_path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// The hook command for an executable: its path, quoted when it holds a
/// space, a tab or a quote, then " hook".
pub fn hook_command(exe: &Path) -> String {
    format!("{}{MARKER}", quote(&exe.to_string_lossy()))
}

/// Den's quoting of the executable path.
pub fn quote(p: &str) -> String {
    if p.contains([' ', '\t', '"', '\'']) {
        format!("\"{}\"", p.replace('"', "\\\""))
    } else {
        p.to_string()
    }
}

/// The program a hook command runs, by base name: `/usr/bin/ainet hook` →
/// `ainet`, `"/my dir/ainet" hook` → `ainet`.
fn program_of(command: &str) -> Option<String> {
    let exe = command.strip_suffix(MARKER)?;
    let exe = exe
        .strip_prefix('"')
        .and_then(|e| e.strip_suffix('"'))
        .map(|e| e.replace("\\\"", "\""))
        .unwrap_or_else(|| exe.to_string());
    if exe.is_empty() {
        return None;
    }
    Some(util::base(&exe))
}

/// Whether a hook command found in settings is ours: exactly `command`, or a
/// command of the same program ending in " hook" (an older install at
/// another path). With `command` empty, any `ainet … hook`.
pub fn is_ours(found: &str, command: &str) -> bool {
    if !command.is_empty() && found == command {
        return true;
    }
    let want = if command.is_empty() {
        Some(PROGRAM.to_string())
    } else {
        program_of(command)
    };
    match (program_of(found), want) {
        (Some(have), Some(want)) => have == want,
        _ => false,
    }
}

/// What [`install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub settings_path: PathBuf,
    /// The copy of the file before the change; `None` when there was no file.
    pub backup: Option<PathBuf>,
    /// Old entries of ours that were replaced.
    pub replaced: usize,
    /// Entries added, one per [`SPECS`] event.
    pub added: usize,
}

/// What [`uninstall`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReport {
    pub settings_path: PathBuf,
    /// `None` when nothing was installed (the file is not touched then).
    pub backup: Option<PathBuf>,
    pub removed: usize,
}

/// A [`SPECS`] event's timeout, with `PermissionRequest`'s as given (it
/// must outlast the time a request is held).
fn timeout_of(sp: &HookSpec, permission_timeout: u64) -> u64 {
    if sp.event == "PermissionRequest" {
        permission_timeout
    } else {
        sp.timeout
    }
}

/// One settings entry running `command` for `sp`.
fn entry_for(sp: &HookSpec, command: &str, permission_timeout: u64) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "hooks".into(),
        json!([{"type": "command", "command": command, "timeout": timeout_of(sp, permission_timeout)}]),
    );
    if !sp.matcher.is_empty() {
        entry.insert("matcher".into(), Value::String(sp.matcher.into()));
    }
    Value::Object(entry)
}

/// The `hooks` block [`install`] writes, as `{"hooks": {…}}`: what to
/// merge into Claude Code's settings by hand.
pub fn hooks_block(command: &str, permission_timeout: u64) -> Value {
    let mut hooks = Map::new();
    for sp in SPECS {
        hooks.insert(
            sp.event.into(),
            json!([entry_for(sp, command, permission_timeout)]),
        );
    }
    json!({ "hooks": hooks })
}

/// Adds the hook entries for `command` (see [`hook_command`]) to the settings
/// file. Installing twice is safe: our old entries are replaced.
pub fn install(settings_path: &Path, command: &str) -> Result<InstallReport> {
    install_with(settings_path, command, super::PERMISSION_TIMEOUT_SECS)
}

/// [`install`] with the `PermissionRequest` hook's timeout.
pub fn install_with(settings_path: &Path, command: &str, permission_timeout: u64) -> Result<InstallReport> {
    if command.trim().is_empty() {
        bail!("no hook command");
    }
    let (mut settings, raw) = load(settings_path)?;
    let mut hooks = match settings.remove("hooks") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let replaced = strip(&mut hooks, command);
    for sp in SPECS {
        let entry = entry_for(sp, command, permission_timeout);
        let list = hooks.entry(sp.event).or_insert_with(|| Value::Array(Vec::new()));
        if !list.is_array() {
            *list = Value::Array(Vec::new());
        }
        if let Value::Array(l) = list {
            l.push(entry);
        }
    }
    settings.insert("hooks".into(), Value::Object(hooks));
    let backup = save(settings_path, raw.as_deref(), &settings)?;
    Ok(InstallReport {
        settings_path: settings_path.to_path_buf(),
        backup,
        replaced,
        added: SPECS.len(),
    })
}

/// Removes every entry of ours (see [`is_ours`]; `command` may be "" for any
/// ainet hook). An event left with no entries is removed, and `hooks` too
/// when it is left empty.
pub fn uninstall(settings_path: &Path, command: &str) -> Result<UninstallReport> {
    let none = UninstallReport {
        settings_path: settings_path.to_path_buf(),
        backup: None,
        removed: 0,
    };
    let (mut settings, raw) = load(settings_path)?;
    let Some(Value::Object(hooks)) = settings.get_mut("hooks") else {
        return Ok(none);
    };
    let removed = strip(hooks, command);
    if removed == 0 {
        return Ok(none);
    }
    if hooks.is_empty() {
        settings.remove("hooks");
    }
    let backup = save(settings_path, raw.as_deref(), &settings)?;
    Ok(UninstallReport {
        removed,
        backup,
        ..none
    })
}

/// Whether the settings file has a hook entry running exactly `command`
/// (or, with `command` empty, any ainet hook).
pub fn installed(settings_path: &Path, command: &str) -> bool {
    let Ok((settings, _)) = load(settings_path) else {
        return false;
    };
    let Some(Value::Object(hooks)) = settings.get("hooks") else {
        return false;
    };
    hooks.values().filter_map(Value::as_array).flatten().any(|e| {
        commands(e).any(|c| {
            if command.is_empty() {
                is_ours(c, "")
            } else {
                c == command
            }
        })
    })
}

/// Where ainet's hooks stand in a settings file ([`status`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookStatus {
    pub settings_path: PathBuf,
    /// The file exists.
    pub exists: bool,
    /// Why the file could not be read, if it could not.
    pub error: Option<String>,
    /// Entries of ours (any ainet binary's `… hook`).
    pub ours: usize,
    /// [`SPECS`] events with no entry running exactly `command` at the
    /// expected timeout and matcher.
    pub missing: Vec<&'static str>,
    /// Entries of ours that run another command (an ainet binary at
    /// another path) or have another timeout: installing again replaces
    /// them.
    pub stale: usize,
}

impl HookStatus {
    /// Every event is installed as it should be, and nothing stale.
    pub fn current(&self) -> bool {
        self.error.is_none() && self.missing.is_empty() && self.stale == 0
    }

    /// One line on it, for people.
    pub fn summary(&self) -> String {
        let path = self.settings_path.display();
        if let Some(e) = &self.error {
            return format!("{path}: cannot read it: {e}");
        }
        if self.ours == 0 {
            return format!("not installed in {path}");
        }
        if self.current() {
            return format!("installed in {path}");
        }
        let mut why = Vec::new();
        if !self.missing.is_empty() {
            why.push(format!("missing {}", self.missing.join(", ")));
        }
        if self.stale > 0 {
            why.push(format!(
                "{} outdated entr{}",
                self.stale,
                if self.stale == 1 { "y" } else { "ies" }
            ));
        }
        format!("installed in {path}, needs repair ({})", why.join("; "))
    }
}

/// How ainet's hooks running `command` stand in a settings file: what is
/// there, what is missing and what is outdated. Reads only.
pub fn status(settings_path: &Path, command: &str, permission_timeout: u64) -> HookStatus {
    let mut st = HookStatus {
        settings_path: settings_path.to_path_buf(),
        exists: settings_path.exists(),
        error: None,
        ours: 0,
        missing: Vec::new(),
        stale: 0,
    };
    let settings = match load(settings_path) {
        Ok((s, _)) => s,
        Err(e) => {
            st.error = Some(format!("{e:#}"));
            return st;
        }
    };
    let hooks = match settings.get("hooks") {
        Some(Value::Object(h)) => h.clone(),
        _ => Map::new(),
    };
    let wanted: Vec<(&'static str, Value)> = SPECS
        .iter()
        .map(|sp| (sp.event, entry_for(sp, command, permission_timeout)))
        .collect();
    for (event, list) in &hooks {
        let Value::Array(list) = list else { continue };
        for e in list.iter().filter(|e| entry_is_ours(e, command)) {
            st.ours += 1;
            let exact = wanted.iter().any(|(ev, w)| ev == event && w == e);
            if !exact {
                st.stale += 1;
            }
        }
    }
    for (event, w) in &wanted {
        let present = hooks
            .get(*event)
            .and_then(Value::as_array)
            .is_some_and(|l| l.iter().any(|e| e == w));
        if !present {
            st.missing.push(event);
        }
    }
    st
}

/// The timeout of ainet's `PermissionRequest` hook running `command` in
/// a settings file: how long Claude Code lets it hold a request before
/// ending it. `None` when the hook is not there or gives none.
pub fn permission_timeout(settings_path: &Path, command: &str) -> Option<u64> {
    let (settings, _) = load(settings_path).ok()?;
    settings
        .get("hooks")?
        .get("PermissionRequest")?
        .as_array()?
        .iter()
        .flat_map(|e| e.get("hooks").and_then(Value::as_array).into_iter().flatten())
        .filter(|h| {
            h.get("command")
                .and_then(Value::as_str)
                .is_some_and(|c| is_ours(c, command))
        })
        .filter_map(|h| h.get("timeout").and_then(Value::as_u64))
        .min()
}

/// The settings as an object, and the file's bytes when it existed.
type Loaded = (Map<String, Value>, Option<Vec<u8>>);

fn load(path: &Path) -> Result<Loaded> {
    let raw = match fs::read(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Map::new(), None)),
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    let v: Value = serde_json::from_slice(&raw).with_context(|| path.display().to_string())?;
    let settings = match v {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        _ => bail!("{}: not a JSON object", path.display()),
    };
    Ok((settings, Some(raw)))
}

fn save(path: &Path, raw: Option<&[u8]>, settings: &Map<String, Value>) -> Result<Option<PathBuf>> {
    let mut backup = None;
    if let Some(raw) = raw {
        let secs = util::now_ms() / 1000;
        // Two changes within a second must not overwrite the first backup
        // (the file as you had it): the second takes a suffix.
        let b = (0..)
            .map(|n| {
                let mut b = path.as_os_str().to_owned();
                b.push(format!(".bak.ainet.{secs}"));
                if n > 0 {
                    b.push(format!(".{n}"));
                }
                PathBuf::from(b)
            })
            .find(|b| !b.exists())
            .unwrap_or_default();
        write_private(&b, raw)?;
        backup = Some(b);
    }
    let mut out = serde_json::to_string_pretty(settings)?;
    out.push('\n');
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".ainet.tmp");
    let tmp = PathBuf::from(tmp);
    write_private(&tmp, out.as_bytes())?;
    fs::rename(&tmp, path)?;
    Ok(backup)
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| path.display().to_string())?;
    f.write_all(data)?;
    Ok(())
}

/// The hook commands of one settings entry.
fn commands(entry: &Value) -> impl Iterator<Item = &str> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|h| h.get("command").and_then(Value::as_str))
}

fn entry_is_ours(entry: &Value, command: &str) -> bool {
    commands(entry).any(|c| is_ours(c, command))
}

/// Removes our entries from every event list and returns how many. An event
/// list that only held ours is removed. Values that are not lists are kept
/// (Den drops them; that would lose a user's setting).
fn strip(hooks: &mut Map<String, Value>, command: &str) -> usize {
    let mut found = 0;
    let mut emptied = Vec::new();
    for (event, v) in hooks.iter_mut() {
        let Value::Array(list) = v else { continue };
        let before = list.len();
        list.retain(|e| !entry_is_ours(e, command));
        let n = before - list.len();
        found += n;
        if n > 0 && list.is_empty() {
            emptied.push(event.clone());
        }
    }
    for e in emptied {
        hooks.remove(&e);
    }
    found
}
