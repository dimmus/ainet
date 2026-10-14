//! `ainet hook`: Claude Code runs it for every hook event.

use std::io::Read;
use std::path::Path;
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
    let _ = client.set_timeout(Some(ainet_daemon::PERMISSION_HOLD + Duration::from_secs(10)));
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

/// Parses the payload and tags it with `$AINET_HOOK_SOURCE`, so the daemon
/// can tell its own per-seat hooks from your global ones.
pub fn prepare(raw: &str, source: Option<String>) -> Option<Value> {
    let mut payload: Value = serde_json::from_str(raw).ok()?;
    if let (Some(src), Value::Object(map)) = (source.filter(|s| !s.is_empty()), &mut payload) {
        map.insert("_ainet_source".into(), Value::String(src));
    }
    Some(payload)
}

/// The hooks block for `~/.claude/settings.json`.
pub fn print_hooks() {
    let cmd = std::env::current_exe()
        .ok()
        .map(|p| format!("{} hook", p.display()))
        .unwrap_or_else(|| "ainet hook".into());
    let entry = |timeout: u32| serde_json::json!([{ "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }] }]);
    let hooks = serde_json::json!({
        "hooks": {
            "SessionStart": entry(10),
            "SessionEnd": entry(10),
            "UserPromptSubmit": entry(10),
            "PreToolUse": entry(10),
            "PostToolUse": entry(10),
            "PostToolUseFailure": entry(10),
            "Notification": entry(10),
            "Stop": entry(10),
            "StopFailure": entry(10),
            // Held until you answer in ainet; must outlast PERMISSION_HOLD.
            "PermissionRequest": entry(600),
        }
    });
    eprintln!("Merge this into ~/.claude/settings.json (ainet does not edit it for you yet):\n");
    println!("{}", serde_json::to_string_pretty(&hooks).unwrap_or_default());
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
