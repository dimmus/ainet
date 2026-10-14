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
