//! Requests about ainet itself on a real ainetd: the settings in force,
//! setting and unsetting them (the file edited in place and the change in
//! force at once), a reload after an edit by hand, hooks in a Claude Code
//! settings file of the test's own, and the paths in use.

mod common;

use ainet_core::protocol::{self, PathsReport};
use ainet_core::settings::{Applies, SettingsReport, Source};
use ainet_core::{Request, Response};
use common::Daemon;
use serde_json::json;

const QUIET: &str = r#"{
  // the test's own
  "theme": "One Dark",
  "discovery": { "claude_sessions": false },
}
"#;

fn settings(d: &Daemon, req: &Request) -> SettingsReport {
    match d.call(req) {
        Response::Settings { settings } => *settings,
        other => panic!("no settings: {other:?}"),
    }
}

fn error(d: &Daemon, req: &Request) -> String {
    match d.call(req) {
        Response::Error { message } => message,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn settings_are_read_set_and_unset_over_the_socket() {
    let d = Daemon::start("admin-set", Some(QUIET));
    let all = settings(&d, &Request::SettingsGet { key: None });
    assert_eq!(all.file, Some(d.dir.join("settings.json")));
    assert!(all.error.is_none() && all.unknown.is_empty());
    let tracked = all.get("discovery.claude_sessions").unwrap();
    assert_eq!(
        (tracked.source.clone(), tracked.value.clone()),
        (Source::File, json!(false))
    );
    assert_eq!(all.get("queue.nudge").unwrap().source, Source::Default);
    let queue = settings(
        &d,
        &Request::SettingsGet {
            key: Some("queue".into()),
        },
    );
    assert!(queue.entries.iter().all(|e| e.key.starts_with("queue.")));
    assert!(
        error(
            &d,
            &Request::SettingsGet {
                key: Some("queu".into())
            }
        )
        .contains("no setting")
    );

    // Set: the file keeps its comment and the desktop app's key, and the
    // value is in force in the reply.
    let set = settings(
        &d,
        &Request::SettingsSet {
            key: "queue.watchdog.min_interval_secs".into(),
            value: json!(60),
        },
    );
    assert_eq!(set.changed.as_deref(), Some("queue.watchdog.min_interval_secs"));
    let e = set.get("queue.watchdog.min_interval_secs").unwrap();
    assert_eq!(
        (e.value.clone(), e.source.clone(), e.applies),
        (json!(60), Source::File, Applies::Live)
    );
    let text = std::fs::read_to_string(d.dir.join("settings.json")).unwrap();
    assert!(
        text.contains("// the test's own") && text.contains("One Dark"),
        "{text}"
    );
    // In force: a watchdog shorter than the new minimum is refused.
    let short = error(
        &d,
        &Request::WatchdogRegister {
            target: "nobody".into(),
            interval_secs: 45,
            message: None,
            registered_by: None,
        },
    );
    assert!(short.contains("at least 60"), "{short}");

    // Refused, and nothing written: unknown keys, wrong types, out of
    // range, a misspelt permission mode.
    for (key, value) in [
        ("queue.nudgy", json!(true)),
        ("queue.nudge", json!("maybe")),
        ("queue.watchdog.tick_secs", json!(0)),
        ("runtimes.claude.permission_mode", json!("bypass")),
        ("theme", json!("Ayu")),
    ] {
        error(
            &d,
            &Request::SettingsSet {
                key: key.into(),
                value,
            },
        );
    }
    assert_eq!(
        std::fs::read_to_string(d.dir.join("settings.json")).unwrap(),
        text
    );

    let unset = settings(
        &d,
        &Request::SettingsUnset {
            key: "queue.watchdog.min_interval_secs".into(),
        },
    );
    let e = unset.get("queue.watchdog.min_interval_secs").unwrap();
    assert_eq!((e.value.clone(), e.source.clone()), (json!(30), Source::Default));
    assert!(
        std::fs::read_to_string(d.dir.join("settings.json"))
            .unwrap()
            .contains("// the test's own")
    );
}

#[test]
fn a_reload_reads_an_edit_at_once_and_keeps_the_last_good() {
    let d = Daemon::start("admin-reload", Some(QUIET));
    d.write_settings(r#"{ "discovery": { "claude_sessions": false }, "queue": { "nudge": false } }"#);
    let r = settings(&d, &Request::SettingsReload);
    assert_eq!(r.get("queue.nudge").unwrap().value, json!(false));
    assert!(r.error.is_none());
    d.write_settings(r#"{ "queue": { "nudge": "no" } }"#);
    let r = settings(&d, &Request::SettingsReload);
    assert!(r.error.as_deref().is_some_and(|e| e.contains("queue")), "{r:?}");
    assert_eq!(r.get("queue.nudge").unwrap().value, json!(false), "the last good");
    // A file with a problem is not edited over it.
    let e = error(
        &d,
        &Request::SettingsSet {
            key: "queue.nudge".into(),
            value: json!(true),
        },
    );
    assert!(e.contains("fix it first"), "{e}");
}

#[test]
fn a_daemon_without_a_file_says_so() {
    let d = Daemon::start("admin-nofile", None);
    let r = settings(&d, &Request::SettingsGet { key: None });
    assert!(r.entries.iter().all(|e| e.source != Source::File));
    // No file yet: setting makes one.
    let set = settings(
        &d,
        &Request::SettingsSet {
            key: "rigs.keep_snapshots".into(),
            value: json!(4),
        },
    );
    assert_eq!(set.get("rigs.keep_snapshots").unwrap().value, json!(4));
    assert!(d.dir.join("settings.json").exists());
}

#[test]
fn hooks_are_checked_installed_and_removed_with_backups() {
    let d = Daemon::start("admin-hooks", None);
    let claude = d.dir.join("claude-settings.json");
    std::fs::write(&claude, r#"{"model":"opus"}"#).unwrap();
    d.write_settings(&format!(
        r#"{{ "discovery": {{ "claude_sessions": false }}, "hooks": {{ "claude_settings": {:?} }} }}"#,
        claude.display().to_string()
    ));
    settings(&d, &Request::SettingsReload);
    let hooks = |req: &Request| match d.call(req) {
        Response::Hooks { hooks } => *hooks,
        other => panic!("no hooks: {other:?}"),
    };
    let st = hooks(&Request::HookStatus);
    assert_eq!(st.settings_path, claude);
    assert!(!st.installed && !st.current && !st.missing.is_empty());
    assert_eq!(
        std::fs::read_to_string(&claude).unwrap(),
        r#"{"model":"opus"}"#,
        "looking writes nothing"
    );

    let done = hooks(&Request::HookInstall);
    assert!(done.installed && done.current, "{done:?}");
    assert!(done.done.as_deref().is_some_and(|t| t.contains("installed")));
    let backup = done.backup.expect("a backup");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), r#"{"model":"opus"}"#);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
    assert_eq!(written["model"], "opus");
    // The daemon's own binary, at the hold's timeout.
    let cmd = written["hooks"]["PermissionRequest"][0]["hooks"][0].clone();
    assert_eq!(cmd["timeout"], 600);
    assert_eq!(cmd["command"], "ainet hook");

    let gone = hooks(&Request::HookUninstall);
    assert!(!gone.installed && gone.backup.is_some());
    let left: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
    assert_eq!(left, json!({"model": "opus"}));
}

#[test]
fn paths_and_version() {
    let d = Daemon::start("admin-paths", Some(QUIET));
    let Response::Paths { paths } = d.call(&Request::Paths) else {
        panic!("no paths")
    };
    let PathsReport {
        socket,
        database,
        settings,
        spec_dirs,
        protocol: version,
        pid,
        exe,
        ..
    } = *paths;
    assert_eq!(socket, d.socket);
    assert_eq!(database, d.dir.join("state.db"));
    assert_eq!(settings, Some(d.dir.join("settings.json")));
    assert!(spec_dirs[0].ends_with("ainet/specs"));
    assert_eq!(version, protocol::VERSION);
    assert_eq!(pid, std::process::id());
    assert_eq!(exe, std::path::PathBuf::from("ainet"));
}
