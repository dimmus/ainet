//! `ainet hook`: Claude Code runs it for every hook event.

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use ainet_core::client::Client;
use ainet_core::{Request, Response};
use anyhow::Result;
use serde_json::Value;

/// Forwards one hook payload. Fails open: with no daemon, Claude Code carries
/// on exactly as if the hook were not installed.
pub fn run(socket: &Path) -> Result<()> {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return Ok(());
    }
    let Some(payload) = prepare(&raw, std::env::var("AINET_HOOK_SOURCE").ok()) else {
        return Ok(());
    };
    let Ok(mut client) = Client::connect(socket) else {
        return Ok(());
    };
    // A permission request may be held until you answer.
    let _ = client.set_timeout(Some(hold_timeout()));
    let agent = crate::conn::me();
    let req = Request::Hook {
        agent,
        payload,
        tmux_pane: std::env::var("TMUX_PANE").ok().filter(|s| !s.is_empty()),
    };
    if let Ok(Response::HookReply { output: Some(out) }) = client.call(&req) {
        println!("{out}");
    }
    Ok(())
}

/// How long to wait for ainetd's answer: the hold settings.json sets
/// (`permissions.hold_secs`) and a little more. Read silently: the hook
/// prints only what Claude Code reads. A file that does not read gets
/// the longest hold allowed, so a hold is never cut short here (Claude
/// Code's own timeout still ends the hook).
fn hold_timeout() -> Duration {
    let path = ainet_core::paths::settings_file();
    let secs = match ainet_core::settings::load(&path) {
        Ok(loaded) => loaded
            .unwrap_or_default()
            .settings
            .permissions
            .client_timeout_secs(),
        Err(_) => 3600 + 10,
    };
    Duration::from_secs(secs)
}

/// Parses the payload and tags it with `$AINET_HOOK_SOURCE`, so the daemon
/// can tell its own per-seat hooks from your global ones.
pub fn prepare(raw: &str, source: Option<String>) -> Option<Value> {
    let mut payload: Value = serde_json::from_str(raw).ok()?;
    if let (Some(src), Value::Object(map)) = (source.filter(|s| !s.is_empty()), &mut payload) {
        map.insert("_ainet_source".into(), Value::String(src));
    }
    Some(payload)
}

/// The hook command for this binary: `<path> hook`.
fn command() -> String {
    std::env::current_exe()
        .map(|p| ainet_claude::hooks::hook_command(&p))
        .unwrap_or_else(|_| "ainet hook".into())
}

/// `ainet install-hooks`: prints the block to merge (nothing written),
/// or with `write`, `remove` or `status` installs, removes or checks
/// ainet's hooks in Claude Code's settings. Writing keeps a backup.
pub fn install_hooks(json: bool, write: bool, remove: bool, status: bool) -> Result<ExitCode> {
    let settings = crate::settings::get();
    let path = ainet_daemon::claude_settings_file(settings);
    let cmd = command();
    let timeout = settings.permissions.hook_timeout_secs();
    if status {
        let st = ainet_claude::hooks::status(&path, &cmd, timeout);
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "settings_path": st.settings_path,
                    "exists": st.exists,
                    "error": st.error,
                    "installed": st.ours > 0,
                    "current": st.current(),
                    "missing": st.missing,
                    "stale": st.stale,
                })
            );
        } else {
            println!("{}", st.summary());
        }
        return Ok(ExitCode::SUCCESS);
    }
    if write {
        let r = ainet_claude::hooks::install_with(&path, &cmd, timeout)?;
        let backup = r
            .backup
            .map(|b| format!("; the old file is {}", b.display()))
            .unwrap_or_default();
        println!(
            "installed ainet's hooks in {} ({} events{}){backup}",
            r.settings_path.display(),
            r.added,
            if r.replaced > 0 {
                format!(", {} old entries replaced", r.replaced)
            } else {
                String::new()
            }
        );
        println!("They apply to Claude Code sessions started from now on.");
        return Ok(ExitCode::SUCCESS);
    }
    if remove {
        let r = ainet_claude::hooks::uninstall(&path, "")?;
        match r.backup {
            Some(b) if r.removed > 0 => println!(
                "removed {} ainet hook entries from {}; the old file is {}",
                r.removed,
                r.settings_path.display(),
                b.display()
            ),
            _ => println!("no ainet hooks in {}", r.settings_path.display()),
        }
        return Ok(ExitCode::SUCCESS);
    }
    let block = ainet_claude::hooks::hooks_block(&cmd, timeout);
    eprintln!(
        "Merge this into {}, or run `ainet install-hooks --write` to have ainet do it (it keeps a backup):\n",
        path.display()
    );
    println!("{}", serde_json::to_string_pretty(&block).unwrap_or_default());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_the_source() {
        let p = prepare(r#"{"hook_event_name":"Stop"}"#, Some("seat".into())).unwrap();
        assert_eq!(p["_ainet_source"], "seat");
        let p = prepare(r#"{"hook_event_name":"Stop"}"#, None).unwrap();
        assert!(p.get("_ainet_source").is_none());
        assert!(prepare("not json", Some("x".into())).is_none());
    }
}
