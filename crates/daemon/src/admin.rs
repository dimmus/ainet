//! Requests about ainet itself rather than its agents: the settings in
//! force and changing them, ainet's hooks in Claude Code's settings, and
//! the paths and version of this ainetd.
//!
//! Files are read and written on a blocking thread, never under the
//! daemon's lock.

use std::path::{Path, PathBuf};

use ainet_core::Response;
use ainet_core::protocol::{self, HookReport, PathsReport};
use ainet_core::settings::{self, Settings};
use serde_json::Value;

use crate::config::Config;
use crate::{Shared, err, lock};

fn settings_reply(report: settings::SettingsReport) -> Response {
    Response::Settings {
        settings: Box::new(report),
    }
}

/// The settings in force, or only `key` (a setting or a group).
pub(crate) fn settings_get(d: &Shared, key: Option<&str>) -> Response {
    let report = lock(d).config.report();
    match report.only(key.unwrap_or_default()) {
        Ok(report) => settings_reply(report),
        Err(e) => err(e),
    }
}

/// Sets `key` to `value` in settings.json (`None`: removes it), then puts
/// the file in force at once.
pub(crate) async fn settings_set(d: &Shared, key: String, value: Option<Value>) -> Response {
    let Some(file) = lock(d).config.file.clone() else {
        return err("this ainetd follows no settings file");
    };
    let edit = {
        let (file, key) = (file.clone(), key.clone());
        tokio::task::spawn_blocking(move || match &value {
            Some(v) => settings::set_in_file(&file, &key, v),
            None => settings::unset_in_file(&file, &key),
        })
        .await
    };
    match edit {
        Ok(Ok(_)) => settings_reload(d, Some(key)).await,
        Ok(Err(e)) => err(e),
        Err(e) => err(format!("settings: {e}")),
    }
}

/// Reads settings.json now and puts it in force (a file that does not
/// read keeps the last good settings, and says why). `changed` is the key
/// a set or unset just changed, to say so in the reply.
pub(crate) async fn settings_reload(d: &Shared, changed: Option<String>) -> Response {
    let file = lock(d).config.file.clone();
    let Some(path) = file.clone() else {
        let mut report = lock(d).config.report();
        report.changed = changed;
        return settings_reply(report);
    };
    let Ok(read) = tokio::task::spawn_blocking(move || settings::load(&path)).await else {
        return err("settings: the read did not finish");
    };
    let mut g = lock(d);
    let config = match read {
        Ok(Some(loaded)) => Config::of(loaded, None, file),
        Ok(None) => Config::of(settings::parse("").unwrap_or_default(), None, file),
        Err(e) => Config::of(g.config.loaded.clone(), Some(e), file),
    };
    config.log();
    g.config = config;
    g.settings_changed();
    let mut report = g.config.report();
    report.changed = changed;
    settings_reply(report)
}

/// What to do with ainet's hooks in Claude Code's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAction {
    /// Look; change nothing.
    Status,
    /// Install or repair them, keeping a backup.
    Install,
    /// Remove them, keeping a backup.
    Uninstall,
}

/// Claude Code's settings file the hooks go in: `hooks.claude_settings`,
/// else `settings.json` in `$CLAUDE_CONFIG_DIR` or `~/.claude`.
pub fn claude_settings_file(settings: &Settings) -> PathBuf {
    match &settings.hooks.claude_settings {
        Some(p) => ainet_core::paths::expand(p),
        None => ainet_claude::hooks::settings_path(),
    }
}

/// Does `action` to the hooks running `command` in the Claude Code
/// settings file at `path`, with the `PermissionRequest` hook's
/// `timeout`, and reports on them after. Blocks on the file system.
pub fn hook_action(
    path: &Path,
    command: &str,
    timeout: u64,
    action: HookAction,
) -> Result<HookReport, String> {
    let (done, backup) = match action {
        HookAction::Status => (None, None),
        HookAction::Install => {
            let r =
                ainet_claude::hooks::install_with(path, command, timeout).map_err(|e| format!("{e:#}"))?;
            let replaced = match r.replaced {
                0 => String::new(),
                n => format!(", {n} old entries replaced"),
            };
            let done = format!(
                "installed ainet's hooks in {} ({} events{replaced}); they apply to Claude Code sessions started from now on",
                r.settings_path.display(),
                r.added
            );
            (Some(done), r.backup)
        }
        HookAction::Uninstall => {
            let r = ainet_claude::hooks::uninstall(path, "").map_err(|e| format!("{e:#}"))?;
            let done = match r.removed {
                0 => format!("no ainet hooks in {}", r.settings_path.display()),
                n => format!(
                    "removed {n} ainet hook entries from {}",
                    r.settings_path.display()
                ),
            };
            (Some(done), r.backup.filter(|_| r.removed > 0))
        }
    };
    let st = ainet_claude::hooks::status(path, command, timeout);
    Ok(HookReport {
        current: st.current(),
        summary: st.summary(),
        installed: st.ours > 0,
        settings_path: st.settings_path,
        exists: st.exists,
        error: st.error,
        missing: st.missing.iter().map(|m| m.to_string()).collect(),
        stale: st.stale,
        done,
        backup,
    })
}

/// Hook status, install or removal for the `ainet` binary this daemon
/// gives its seats, in the file settings.json names.
pub(crate) async fn hooks(d: &Shared, action: HookAction) -> Response {
    let (path, command, timeout) = {
        let g = lock(d);
        let s = g.settings();
        (
            claude_settings_file(s),
            ainet_claude::hooks::hook_command(&g.exe),
            s.permissions.hook_timeout_secs(),
        )
    };
    match tokio::task::spawn_blocking(move || hook_action(&path, &command, timeout, action)).await {
        Ok(Ok(report)) => Response::Hooks {
            hooks: Box::new(report),
        },
        Ok(Err(e)) => err(e),
        Err(e) => err(format!("hooks: {e}")),
    }
}

/// The paths this ainetd uses, and its version.
pub(crate) fn paths(d: &Shared) -> Response {
    let g = lock(d);
    let report = PathsReport {
        socket: g.socket.clone(),
        state_dir: ainet_core::paths::state_dir(),
        database: g.database.clone(),
        log: g.settings().daemon.log.path(),
        settings: g.config.file.clone(),
        spec_dirs: g.spec_dirs(),
        claude_config_dir: ainet_claude::config_dir(),
        exe: g.exe.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: protocol::VERSION,
        pid: std::process::id(),
    };
    Response::Paths {
        paths: Box::new(report),
    }
}
