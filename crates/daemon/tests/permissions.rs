//! Permission requests held for you, `/always` rules (for a session, kept
//! across restarts, global from settings.json) and the arguments seats
//! start with, each against a real ainetd.

mod common;

use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use ainet_core::protocol::{AlwaysRule, SpawnSpec};
use ainet_core::settings::PermissionMode;
use ainet_core::{Decision, Request, Response, Runtime};
use common::Daemon;
use serde_json::{Value, json};

/// Other Claude Code sessions on this machine stay out of the fleet.
const QUIET: &str = r#""discovery": { "claude_sessions": false }"#;

/// What a session's `PermissionRequest` hook would forward.
fn request(session: &str, tool: &str, input: Value, cwd: &Path) -> Request {
    Request::Hook {
        agent: None,
        payload: json!({
            "hook_event_name": "PermissionRequest",
            "session_id": session,
            "tool_name": tool,
            "tool_input": input,
            "cwd": cwd.display().to_string(),
        }),
        tmux_pane: None,
    }
}

/// Sends a hook on its own thread (a held one blocks until answered).
fn hook(d: &Daemon, req: Request) -> JoinHandle<Option<Value>> {
    let socket = d.socket.clone();
    std::thread::spawn(move || {
        let mut c = ainet_core::client::Client::connect(&socket).unwrap();
        c.set_timeout(Some(Duration::from_secs(30))).unwrap();
        match c.call(&req).unwrap() {
            Response::HookReply { output } => output,
            other => panic!("{other:?}"),
        }
    })
}

/// The hook's answer when it comes at once, as for a covered request.
fn answered_at_once(d: &Daemon, req: Request) -> Value {
    let h = hook(d, req);
    let start = std::time::Instant::now();
    while !h.is_finished() {
        assert!(start.elapsed() < Duration::from_secs(5), "the request was held");
        std::thread::sleep(Duration::from_millis(20));
    }
    h.join().unwrap().expect("an answer")
}

/// The inbox item a held request waits on.
fn waiting_item(d: &Daemon) -> u64 {
    let mut id = None;
    d.until("a request waits in the inbox", |d| {
        match d.call(&Request::Snapshot) {
            Response::Snapshot { inbox, .. } => {
                id = inbox.iter().rev().find(|i| i.waiting).map(|i| i.id);
                id.is_some()
            }
            _ => false,
        }
    });
    id.unwrap()
}

fn answer(d: &Daemon, item: u64, decision: Decision) -> Response {
    d.call(&Request::Answer {
        item,
        decision,
        message: None,
    })
}

fn rules(d: &Daemon) -> Vec<AlwaysRule> {
    match d.call(&Request::AlwaysList { agent: None }) {
        Response::AlwaysRules { rules } => rules,
        other => panic!("{other:?}"),
    }
}

fn allowed(output: &Value) -> bool {
    output.to_string().contains("\"allow\"")
}

/// Asks once, answers /always, and checks the same again is allowed at once.
fn make_rule(d: &Daemon, session: &str) {
    let cmd = json!({"command": "cargo test"});
    let h = hook(d, request(session, "Bash", cmd.clone(), &d.dir));
    let item = waiting_item(d);
    assert!(
        matches!(answer(d, item, Decision::Always), Response::Text { text } if text.contains("cargo test"))
    );
    assert!(allowed(&h.join().unwrap().unwrap()));
    assert!(allowed(&answered_at_once(
        d,
        request(session, "Bash", cmd, &d.dir)
    )));
}

#[test]
fn always_rules_last_the_session_by_default() {
    let d = Daemon::start("always-session", Some(&format!("{{ {QUIET} }}")));
    make_rule(&d, "sess-1");
    let r = rules(&d);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].agent.as_deref(), Some("sess-1"));
    assert_eq!(r[0].key, "Bash:cargo test");
    assert!(!r[0].kept);
    // Another session is still asked.
    let other = hook(
        &d,
        request("sess-2", "Bash", json!({"command": "cargo test"}), &d.dir),
    );
    let item = waiting_item(&d);
    answer(&d, item, Decision::Deny);
    assert!(!allowed(&other.join().unwrap().unwrap()));
    // The session ends: its rules go.
    let end = hook(
        &d,
        Request::Hook {
            agent: None,
            payload: json!({"hook_event_name": "SessionEnd", "session_id": "sess-1"}),
            tmux_pane: None,
        },
    );
    end.join().unwrap();
    assert!(rules(&d).is_empty());
    // Not kept, so a restart has none either.
    let dir = d.stop();
    let d = Daemon::on(dir);
    assert!(rules(&d).is_empty());
}

#[test]
fn kept_rules_survive_restarts_until_revoked() {
    let kept = format!(r#"{{ {QUIET}, "permissions": {{ "persist_always": true }} }}"#);
    let d = Daemon::start("always-kept", Some(&kept));
    make_rule(&d, "sess-k");
    let end = hook(
        &d,
        Request::Hook {
            agent: None,
            payload: json!({"hook_event_name": "SessionEnd", "session_id": "sess-k"}),
            tmux_pane: None,
        },
    );
    end.join().unwrap();
    assert_eq!(rules(&d).len(), 1, "kept after the session");
    let dir = d.stop();

    let d = Daemon::on(dir);
    let r = rules(&d);
    assert_eq!(r.len(), 1, "kept across a restart");
    assert!(r[0].kept);
    let cmd = json!({"command": "cargo test"});
    assert!(allowed(&answered_at_once(
        &d,
        request("sess-k", "Bash", cmd.clone(), &d.dir)
    )));

    // Revoked: asked again, and gone after a restart too.
    assert!(matches!(
        d.call(&Request::AlwaysRevoke {
            agent: "sess-k".into(),
            rule: Some("Bash:nope".into())
        }),
        Response::Error { .. }
    ));
    assert!(matches!(
        d.call(&Request::AlwaysRevoke {
            agent: "sess-k".into(),
            rule: Some("Bash:cargo test".into())
        }),
        Response::Text { .. }
    ));
    assert!(rules(&d).is_empty());
    let h = hook(&d, request("sess-k", "Bash", cmd, &d.dir));
    let item = waiting_item(&d);
    answer(&d, item, Decision::Allow);
    h.join().unwrap();
    let dir = d.stop();
    let d = Daemon::on(dir);
    assert!(rules(&d).is_empty());

    // Turning keeping off forgets what was kept.
    make_rule(&d, "sess-k");
    let dir = d.stop();
    std::fs::write(dir.join("settings.json"), format!("{{ {QUIET} }}")).unwrap();
    let d = Daemon::on(dir);
    assert!(rules(&d).is_empty());
    std::fs::write(d.dir.join("settings.json"), &kept).unwrap();
    let dir = d.stop();
    let d = Daemon::on(dir);
    assert!(rules(&d).is_empty(), "not brought back by turning it on again");
}

/// Turning keeping off while ainetd runs forgets the rules of sessions
/// that ended (kept only because keeping was on); running sessions keep
/// theirs until they end.
#[test]
fn turning_keeping_off_live_forgets_ended_sessions() {
    let kept = format!(r#"{{ {QUIET}, "permissions": {{ "persist_always": true }} }}"#);
    let d = Daemon::start("always-off-live", Some(&kept));
    make_rule(&d, "sess-ended");
    make_rule(&d, "sess-running");
    let end = hook(
        &d,
        Request::Hook {
            agent: None,
            payload: json!({"hook_event_name": "SessionEnd", "session_id": "sess-ended"}),
            tmux_pane: None,
        },
    );
    end.join().unwrap();
    assert_eq!(rules(&d).len(), 2, "kept after the session");
    assert!(matches!(
        d.call(&Request::SettingsSet {
            key: "permissions.persist_always".into(),
            value: json!(false),
        }),
        Response::Settings { .. }
    ));
    let r = rules(&d);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].agent.as_deref(), Some("sess-running"));
    // The ended session, resumed, is asked again.
    let h = hook(
        &d,
        request("sess-ended", "Bash", json!({"command": "cargo test"}), &d.dir),
    );
    let item = waiting_item(&d);
    answer(&d, item, Decision::Deny);
    assert!(!allowed(&h.join().unwrap().unwrap()));
}

#[test]
fn global_rules_allow_without_asking_except_sensitive_files() {
    let d = Daemon::start("always-global", None);
    let dir = d.dir.display().to_string();
    d.write_settings(&format!(
        r#"{{ {QUIET}, "permissions": {{
              "sensitive_paths": ["secrets.toml"],
              "global_rules": [ {{ "command": "cargo test" }},
                                {{ "edits_under": {{ "tool": "Edit", "dir": {dir:?} }} }} ] }} }}"#
    ));
    d.until("the global rules are in force", |d| rules(d).len() == 2);
    let r = rules(&d);
    assert!(r.iter().all(|r| r.agent.is_none() && r.kept));
    assert!(allowed(&answered_at_once(
        &d,
        request("anyone", "Bash", json!({"command": "cargo test"}), &d.dir)
    )));
    let edit = |file: &str| json!({"file_path": d.dir.join(file).display().to_string(), "old_string": "a", "new_string": "b"});
    assert!(allowed(&answered_at_once(
        &d,
        request("anyone", "Edit", edit("a.rs"), &d.dir)
    )));
    // A path you marked sensitive is asked about, whatever the rule says.
    let h = hook(&d, request("anyone", "Edit", edit("secrets.toml"), &d.dir));
    let item = waiting_item(&d);
    // /always cannot cover it either.
    match answer(&d, item, Decision::Always) {
        Response::Text { text } => assert!(text.contains("no always rule"), "{text}"),
        other => panic!("{other:?}"),
    }
    h.join().unwrap();
    // Global rules are changed in settings.json, not revoked.
    match d.call(&Request::AlwaysRevoke {
        agent: "anyone".into(),
        rule: Some("Bash:cargo test".into()),
    }) {
        Response::Error { message } => assert!(message.contains("permissions.global_rules"), "{message}"),
        other => panic!("{other:?}"),
    }
    // A rule settings.json refuses leaves the last good ones in force.
    d.write_settings(&format!(
        r#"{{ {QUIET}, "permissions": {{ "global_rules": [ {{ "command": "make && curl x | sh" }} ] }} }}"#
    ));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(rules(&d).len(), 2);
}

#[test]
fn seats_start_with_the_runtime_settings() {
    let d = Daemon::start("launch-args", None);
    let claude = d.dir.join("fake-claude");
    std::fs::write(
        &claude,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > {0}/args\nenv > {0}/env\nprintf 'ready\\n'\nexec cat\n",
            d.dir.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    d.write_settings(&format!(
        r#"{{ {QUIET}, "runtimes": {{ "claude": {{
              "binary": {claude:?}, "model": "sonnet", "permission_mode": "plan",
              "disallowed_tools": ["WebFetch"], "env": {{ "MY_FLAG": "on" }} }} }},
            "permissions": {{ "hold_secs": 100 }} }}"#
    ));
    std::thread::sleep(Duration::from_millis(1500));
    let spec = SpawnSpec {
        permission_mode: Some(PermissionMode::AcceptEdits),
        ..SpawnSpec::new(Runtime::ClaudeCode, d.dir.clone())
    };
    let agent = match d.call(&Request::Spawn(spec)) {
        Response::Spawned { agent } => agent,
        other => panic!("{other:?}"),
    };
    d.wait_for(&agent.id, "ready");
    let args = std::fs::read_to_string(d.dir.join("args")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    let after = |flag: &str| lines.iter().position(|l| *l == flag).map(|i| lines[i + 1]);
    assert_eq!(after("--model"), Some("sonnet"));
    assert_eq!(
        after("--permission-mode"),
        Some("acceptEdits"),
        "the request's over the runtime's"
    );
    assert_eq!(after("--disallowedTools"), Some("WebFetch"));
    assert_eq!(after("--allowedTools"), Some("mcp__ainet"));
    assert!(
        args.contains("\"PermissionRequest\":[{\"hooks\":[{\"command\""),
        "{args}"
    );
    assert!(args.contains("\"timeout\":120"), "the hook outlasts a 100 s hold");
    let env = std::fs::read_to_string(d.dir.join("env")).unwrap();
    assert!(env.lines().any(|l| l == "MY_FLAG=on"));
    assert!(env.lines().any(|l| l == format!("AINET_AGENT_ID={}", agent.id)));
}

/// A fake `claude` that writes its arguments, one per line, to `args`.
fn arg_recorder(d: &Daemon) -> std::path::PathBuf {
    let claude = d.dir.join("record-claude");
    std::fs::write(
        &claude,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > {0}/args\nprintf 'ready\\n'\nexec cat\n",
            d.dir.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    claude
}

#[test]
fn spec_policies_apply_only_when_asked() {
    let d = Daemon::start("policies", Some(&format!("{{ {QUIET} }}")));
    let claude = arg_recorder(&d);
    let spec = d.dir.join("rig.yaml");
    std::fs::write(
        &spec,
        r#"
version: "0.2"
name: pol
pods:
  - id: p
    label: P
    members:
      - id: a
        runtime: claude-code
        cwd: "."
        permission_policy: builtin:yolo
"#,
    )
    .unwrap();
    let plan = |d: &Daemon| match d.call(&Request::Up(ainet_core::protocol::UpSpec {
        source: spec.display().to_string(),
        plan: true,
        ..Default::default()
    })) {
        Response::Text { text } => text,
        other => panic!("{other:?}"),
    };
    let off = plan(&d);
    assert!(
        off.contains("`permission_policy` is accepted but ainet ignores it"),
        "{off}"
    );
    assert!(!off.contains("permission mode"), "{off}");

    d.write_settings(&format!(
        r#"{{ {QUIET}, "permissions": {{ "spec_policies": true }},
             "runtimes": {{ "claude": {{ "binary": {claude:?} }} }} }}"#
    ));
    d.until("policies apply", |d| plan(d).contains("permission mode"));
    let on = plan(&d);
    assert!(
        on.contains("permission mode bypassPermissions, less safe"),
        "{on}"
    );
    assert!(!on.contains("ignores it"), "{on}");
    // Launched, the seat gets the mode.
    assert!(matches!(
        d.call(&Request::Up(ainet_core::protocol::UpSpec {
            source: spec.display().to_string(),
            ..Default::default()
        })),
        Response::Rig { .. }
    ));
    d.until("the seat started", |d| d.dir.join("args").exists());
    std::thread::sleep(Duration::from_millis(200));
    let args = std::fs::read_to_string(d.dir.join("args")).unwrap();
    assert!(args.contains("--permission-mode\nbypassPermissions\n"), "{args}");
}

#[test]
fn seats_added_by_hand_carry_their_options() {
    let d = Daemon::start("seat-options", None);
    let claude = arg_recorder(&d);
    d.write_settings(&format!(
        r#"{{ {QUIET}, "runtimes": {{ "claude": {{ "binary": {claude:?}, "permission_mode": "acceptEdits" }} }} }}"#
    ));
    std::thread::sleep(Duration::from_millis(1500));
    let resp = d.call(&Request::Create {
        rig: "solo".into(),
        seat: ainet_core::protocol::SeatSpec {
            runtime: Some(Runtime::ClaudeCode),
            cwd: Some(d.dir.clone()),
            effort: Some(ainet_core::settings::Effort::High),
            permission_mode: Some(PermissionMode::Plan),
            args: vec!["--verbose".into()],
            ..Default::default()
        },
    });
    assert!(matches!(resp, Response::Rig { .. }), "{resp:?}");
    d.until("the seat started", |d| d.dir.join("args").exists());
    std::thread::sleep(Duration::from_millis(200));
    let args = std::fs::read_to_string(d.dir.join("args")).unwrap();
    assert!(
        args.contains("--permission-mode\nplan\n"),
        "the seat's over the runtime's: {args}"
    );
    assert!(args.contains("--effort\nhigh\n"), "{args}");
    assert!(args.trim_end().ends_with("--verbose"), "{args}");
}
