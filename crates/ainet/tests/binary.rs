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

/// `ainet config` with no daemon: reads and writes settings.json itself,
/// keeping its comments, refusing what is not a setting, and `edit` runs
/// $EDITOR on the file and checks it.
#[test]
fn config_works_without_a_daemon() {
    let dir = std::env::temp_dir().join(format!("ainet-test-{}-config", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("ainet")).unwrap();
    let file = config.join("ainet/settings.json");
    std::fs::write(&file, "{\n  // mine\n  \"theme\": \"One Dark\",\n}\n").unwrap();
    let run = |args: &[&str], editor: Option<&str>| {
        let mut cmd = Command::new(exe());
        cmd.args(args)
            .env("AINET_SOCKET", no_daemon())
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("AINET_LOG")
            .env_remove("VISUAL")
            .env_remove("AINET_AGENT_ID");
        if let Some(e) = editor {
            cmd.env("EDITOR", e);
        }
        cmd.output().unwrap()
    };
    let ok = |args: &[&str]| {
        let out = run(args, None);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_eq!(ok(&["config", "path"]).trim(), file.display().to_string());
    assert_eq!(ok(&["config", "get", "queue.nudge"]).trim(), "true");
    assert_eq!(
        ok(&["config", "set", "queue.watchdog.message", "wake", "up"]).trim(),
        "queue.watchdog.message = \"wake up\" (applies now)"
    );
    assert_eq!(ok(&["config", "get", "queue.watchdog.message"]).trim(), "wake up");
    let set = ok(&["config", "set", "runtimes.claude.permission_mode", "acceptEdits"]);
    assert!(
        set.contains("applies to new seats") && set.contains("less safe"),
        "{set}"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("// mine") && text.contains("One Dark"), "{text}");
    // Refused, and nothing written.
    for bad in [
        &["config", "set", "queue.nudgy", "true"][..],
        &["config", "set", "runtimes.claude.permission_mode", "bypass"],
        &["config", "set", "theme", "Ayu"],
    ] {
        assert!(!run(bad, None).status.success(), "{bad:?}");
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    let list: serde_json::Value =
        serde_json::from_str(&ok(&["--json", "config", "list", "--changed"])).unwrap();
    let keys: Vec<&str> = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["key"].as_str().unwrap())
        .collect();
    // AINET_SOCKET wins over the file, so the socket is not its default.
    assert_eq!(
        keys,
        [
            "daemon.socket",
            "queue.watchdog.message",
            "runtimes.claude.permission_mode"
        ]
    );
    assert_eq!(list["entries"][0]["source"]["var"], "AINET_SOCKET");
    assert!(ok(&["config", "list", "queue"]).contains("* queue.watchdog.message"));
    let got: serde_json::Value =
        serde_json::from_str(&ok(&["--json", "config", "get", "queue.nudge"])).unwrap();
    assert_eq!(got["source"]["from"], "default");
    assert!(ok(&["config", "unset", "runtimes.claude"]).contains("runtimes.claude is unset"));
    assert_eq!(
        ok(&["config", "get", "runtimes.claude.permission_mode"]).trim(),
        "null"
    );
    let paths = ok(&["config", "path", "--all"]);
    assert!(
        paths.contains("ainetd not running") && paths.contains("settings.json"),
        "{paths}"
    );

    // edit: the editor's change is checked; a bad one exits non-zero.
    let good = dir.join("good.json");
    std::fs::write(&good, r#"{ "queue": { "nudge": false } }"#).unwrap();
    let out = run(&["config", "edit"], Some(&format!("cp {}", good.display())));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(ok(&["config", "get", "queue.nudge"]).trim(), "false");
    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{ "queue": { "nudge": "no" } }"#).unwrap();
    let out = run(&["config", "edit"], Some(&format!("cp {}", bad.display())));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("queue"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `ainet hooks` does what `install-hooks`' flags do, as JSON too.
#[test]
fn hooks_status_install_uninstall() {
    let dir = std::env::temp_dir().join(format!("ainet-test-{}-hooks2", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let claude = dir.join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    let settings = claude.join("settings.json");
    std::fs::write(&settings, r#"{"model":"opus"}"#).unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(exe())
            .args(args)
            .env("AINET_SOCKET", no_daemon())
            .env("CLAUDE_CONFIG_DIR", &claude)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let st: serde_json::Value = serde_json::from_str(&run(&["--json", "hooks", "status"])).unwrap();
    assert_eq!(
        (st["installed"].as_bool(), st["exists"].as_bool()),
        (Some(false), Some(true))
    );
    let done = run(&["hooks", "install"]);
    assert!(
        done.contains("installed ainet's hooks") && done.contains("the file as it was"),
        "{done}"
    );
    let st: serde_json::Value = serde_json::from_str(&run(&["--json", "hooks", "status"])).unwrap();
    assert_eq!(st["current"], true);
    assert!(run(&["hooks", "uninstall"]).contains("removed"));
    let left: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(left, serde_json::json!({"model": "opus"}));
    let _ = std::fs::remove_dir_all(&dir);
}

/// With ainetd running, `ainet config set` goes through it (the change is
/// in force at once, the reply says so), `ainet config path --all` is
/// the daemon's, and `ainet always list` reads its rules.
#[test]
fn config_goes_through_a_running_daemon() {
    let dir = std::env::temp_dir().join(format!("ainet-test-{}-cfgd", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("ainet")).unwrap();
    std::fs::write(
        config.join("ainet/settings.json"),
        r#"{ "discovery": { "claude_sessions": false, "tmux": false } }"#,
    )
    .unwrap();
    let socket = dir.join("d.sock");
    let env = |cmd: &mut Command| {
        cmd.env("AINET_SOCKET", &socket)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
            .env_remove("AINET_LOG")
            .env_remove("AINET_AGENT_ID");
    };
    let mut daemon = Command::new(exe());
    daemon.arg("daemon").stdout(Stdio::null()).stderr(Stdio::null());
    env(&mut daemon);
    let mut child = daemon.spawn().unwrap();
    let start = std::time::Instant::now();
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "ainetd never listened"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let ok = |args: &[&str]| {
        let mut cmd = Command::new(exe());
        cmd.args(args);
        env(&mut cmd);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_eq!(
        ok(&["config", "set", "rigs.keep_snapshots", "3"]).trim(),
        "rigs.keep_snapshots = 3 (applies now)"
    );
    assert_eq!(ok(&["config", "get", "rigs.keep_snapshots"]).trim(), "3");
    let paths = ok(&["config", "path", "--all"]);
    assert!(paths.contains(&format!("pid {}", child.id())), "{paths}");
    assert_eq!(ok(&["always", "list"]).trim(), "no /always rules");
    ok(&["shutdown"]);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mistake in one section of settings.json does not move the socket
/// or turn autostart back on (each read from `daemon` alone), and
/// `config get` says the file is not in force.
#[test]
fn a_typo_elsewhere_keeps_the_daemon_section() {
    let dir = std::env::temp_dir().join(format!("ainet-test-{}-section", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("ainet")).unwrap();
    let socket = dir.join("mine.sock");
    std::fs::write(
        config.join("ainet/settings.json"),
        format!(
            r#"{{ "daemon": {{ "socket": {:?}, "autostart": false }}, "queue": {{ "nudge": "yes" }} }}"#,
            socket.display().to_string()
        ),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(exe())
            .args(args)
            .env_remove("AINET_SOCKET")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_RUNTIME_DIR", dir.join("run"))
            .env_remove("AINET_AGENT_ID")
            .output()
            .unwrap()
    };
    let out = run(&["config", "path", "--all"]);
    let paths = String::from_utf8_lossy(&out.stdout);
    assert!(paths.contains(&socket.display().to_string()), "{paths}");
    // Not started: autostart is off.
    let out = run(&["ps"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("daemon.autostart is off"), "{err}");
    assert!(!socket.exists() && !dir.join("run/ainet.sock").exists());
    // The default is printed, and why the file's value is not in force.
    let out = run(&["config", "get", "queue.nudge"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not in force") && err.contains("queue"), "{err}");
    let out = run(&["--json", "config", "get", "queue.nudge"]);
    let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        got["error"].as_str().is_some_and(|e| e.contains("queue")),
        "{got}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
