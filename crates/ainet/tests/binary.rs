//! The built binary, end to end, with no daemon: argv[0] naming, the MCP
//! server over real stdio, and the hook failing open.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ainet"))
}

/// A socket path nobody listens on.
fn no_daemon() -> PathBuf {
    std::env::temp_dir().join(format!("ainet-test-{}-none.sock", std::process::id()))
}

fn run_with_stdin(args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(exe())
        .args(args)
        .env("AINET_SOCKET", no_daemon())
        .env_remove("AINET_AGENT_ID")
        .env_remove("AINET_HOOK_SOURCE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn help_follows_argv0() {
    let dir = std::env::temp_dir().join(format!("ainet-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("rig");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(exe(), &link).unwrap();
    let out = Command::new(&link).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(help.contains("Usage: rig"), "{help}");
    for cmd in ["up", "down", "ps", "send", "queue", "chatroom", "whoami"] {
        assert!(help.contains(&format!("  {cmd} ")), "missing {cmd} in {help}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_over_stdio() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_messages","arguments":{}}}"#,
        "\n",
    );
    let out = run_with_stdin(&["mcp"], input);
    assert!(out.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "ainet");
    assert!(lines[1]["result"]["tools"].as_array().unwrap().len() >= 20);
    // No AINET_AGENT_ID: a tool error, reported as a result.
    assert_eq!(lines[2]["result"]["isError"], true);
}

#[test]
fn hook_fails_open() {
    let out = run_with_stdin(&["hook"], r#"{"hook_event_name":"Stop","session_id":"x"}"#);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn daemon_errors_exit_nonzero() {
    // `shutdown` never starts a daemon; with none running it fails cleanly.
    let out = run_with_stdin(&["shutdown"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not running"));
}

/// `ainet install-hooks` against a Claude Code config directory of its
/// own: prints by default, writes, checks and removes only when told, and
/// keeps a backup of what it changes.
#[test]
fn install_hooks_only_writes_when_told() {
    let dir = std::env::temp_dir().join(format!("ainet-test-{}-hooks", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let claude = dir.join("claude");
    let config = dir.join("config");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(config.join("ainet")).unwrap();
    let settings = claude.join("settings.json");
    std::fs::write(&settings, r#"{"model":"opus"}"#).unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(exe())
            .args(args)
            .env("AINET_SOCKET", no_daemon())
            .env("CLAUDE_CONFIG_DIR", &claude)
            .env("XDG_CONFIG_HOME", &config)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let printed: serde_json::Value = serde_json::from_str(&run(&["install-hooks"])).unwrap();
    assert_eq!(printed["hooks"]["PreToolUse"][0]["matcher"], "AskUserQuestion");
    assert_eq!(
        printed["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
        600
    );
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        r#"{"model":"opus"}"#,
        "printing writes nothing"
    );
    assert!(run(&["install-hooks", "--status"]).starts_with("not installed in"));

    assert!(run(&["install-hooks", "--write"]).contains("installed ainet's hooks"));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(written["model"], "opus");
    assert_eq!(
        written["hooks"], printed["hooks"],
        "what was printed is what is written"
    );
    let backups = std::fs::read_dir(&claude)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".bak.ainet.")
        })
        .count();
    assert_eq!(backups, 1);
    assert!(run(&["install-hooks", "--status"]).starts_with("installed in"));

    // A longer hold in settings.json makes the installed hooks outdated.
    std::fs::write(
        config.join("ainet/settings.json"),
        r#"{ "permissions": { "hold_secs": 900, } }"#,
    )
    .unwrap();
    let st: serde_json::Value = serde_json::from_str(&run(&["--json", "install-hooks", "--status"])).unwrap();
    assert_eq!(
        (st["installed"].as_bool(), st["current"].as_bool()),
        (Some(true), Some(false))
    );
    run(&["install-hooks", "--write"]);
    assert!(run(&["install-hooks", "--status"]).starts_with("installed in"));
    // Each change kept its own backup, the first one the file as it was.
    let mut backups: Vec<PathBuf> = std::fs::read_dir(&claude)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".bak.ainet."))
        .collect();
    backups.sort();
    assert_eq!(backups.len(), 2);
    assert!(
        backups
            .iter()
            .any(|b| std::fs::read_to_string(b).unwrap() == r#"{"model":"opus"}"#)
    );

    assert!(run(&["install-hooks", "--remove"]).contains("removed"));
    let left: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(left, serde_json::json!({"model": "opus"}));
    assert!(run(&["install-hooks", "--remove"]).starts_with("no ainet hooks"));
    let _ = std::fs::remove_dir_all(&dir);
}
