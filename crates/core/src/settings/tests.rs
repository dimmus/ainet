use std::time::Duration;

use super::*;

/// With no file, every setting is what ainet did before it had settings.
#[test]
fn defaults_are_what_ainet_always_did() {
    let s = Settings::default();
    assert!(s.daemon.autostart);
    assert!(!s.daemon.restore_rigs);
    assert_eq!(s.daemon.socket, None);
    assert_eq!(
        s.daemon.log,
        LogSettings {
            level: "info".into(),
            file: None,
            append: false,
            max_size_mb: 0,
            keep: 1,
        }
    );
    let r = &s.runtimes;
    assert_eq!(r.claude, AgentRuntime::default());
    assert_eq!(r.codex, AgentRuntime::default());
    assert_eq!(
        (
            r.claude.binary.as_deref(),
            r.claude.permission_mode,
            r.claude.effort
        ),
        (None, None, None)
    );
    assert!(!r.claude.strict_mcp_config && r.claude.allowed_tools.is_empty());
    assert_eq!(r.shell, ShellRuntime::default());
    assert_eq!(
        (
            r.readiness.timeout_secs,
            r.readiness.trust_timeout_secs,
            r.readiness.poll_ms
        ),
        (60, 600, 300)
    );
    assert_eq!(
        (
            r.readiness.quiet_ms.claude,
            r.readiness.quiet_ms.codex,
            r.readiness.quiet_ms.shell
        ),
        (800, 1500, 400)
    );
    assert_eq!(r.readiness.startup_text_gap_ms, 1200);
    assert!(!r.claude.spec_effort && !r.codex.spec_effort);
    assert!(r.claude.disallowed_tools.is_empty() && r.claude.mcp_servers.is_empty());
    assert!(r.claude.args.is_empty() && r.claude.env.is_empty());
    assert_eq!(s.permissions.hold_secs, 580);
    // The hook's timeout and the hook client's wait, as they were.
    assert_eq!(s.permissions.hook_timeout_secs(), 600);
    assert_eq!(s.permissions.client_timeout_secs(), 590);
    assert!(!s.permissions.persist_always && !s.permissions.spec_policies);
    assert!(s.permissions.sensitive_paths.is_empty());
    assert!(s.permissions.global_rules.is_empty());
    let m = &s.messaging;
    assert_eq!(
        Duration::from_millis(m.paste_settle_ms),
        Duration::from_millis(60)
    );
    assert_eq!(m.flush_gap_ms, 400);
    assert!(m.hold_while_interacting && !m.typing_guard);
    assert_eq!(m.message_note, crate::conversation::MESSAGE_NOTE);
    assert_eq!(m.mention_note, crate::conversation::MENTION_NOTE);
    assert!(s.queue.nudge);
    let w = &s.queue.watchdog;
    assert_eq!(
        (w.min_interval_secs, w.tick_secs, w.default_interval_secs),
        (30, 5, 600)
    );
    assert_eq!(
        w.message,
        "[ainet] watchdog wake: check your queue (queue_list mine=true / ainet queue list --mine) and carry on with what you own."
    );
    assert_eq!(
        s.rigs,
        RigSettings {
            spec_dirs: vec![],
            default_cwd: None,
            snapshot_on_down: true,
            keep_snapshots: None,
        }
    );
    assert_eq!(
        s.discovery,
        DiscoverySettings {
            claude_sessions: true,
            tmux: true,
            interval_ms: 3000,
            transcripts_ms: 6000,
        }
    );
    assert_eq!(s.hooks.claude_settings, None);
    assert_eq!(
        s.retention,
        RetentionSettings {
            messages_in_memory: 5000,
            chat_in_memory: 5000,
            inbox_items: 200,
            scrollback_lines: 5000,
            events_max_age_days: None,
            events_max_count: None,
            inbox_expiry_mins: None,
        }
    );
    assert!(s.cost.prices.is_empty() && s.cost.context_windows.is_empty());
    assert!(s.problems().is_empty());
    // An empty file, comments only, and the desktop app's keys alone are
    // all the defaults.
    for text in [
        "",
        "  // nothing\n{}",
        r#"{ "theme": "One Dark", "terminal": { "font_size": 3 } }"#,
    ] {
        let loaded = parse(text).unwrap();
        assert_eq!(loaded.settings, s, "{text}");
        assert!(loaded.unknown.is_empty());
    }
}

#[test]
fn reads_comments_trailing_commas_and_partial_sections() {
    let loaded = parse(
        r#"// Mine.
        {
          "theme": "One Dark", // the desktop app's
          "messaging": { "paste_settle_ms": 120, /* slow box */ },
          "queue": { "watchdog": { "min_interval_secs": 10, }, },
          "runtimes": { "claude": { "permission_mode": "plan", "env": { "A": "1" } } },
          "cost": { "context_windows": { "claude-opus-5": 1000000 } },
        }"#,
    )
    .unwrap();
    let s = &loaded.settings;
    assert_eq!(s.messaging.paste_settle_ms, 120);
    assert_eq!(s.messaging.flush_gap_ms, 400, "the rest stays default");
    assert_eq!(s.queue.watchdog.min_interval_secs, 10);
    assert_eq!(s.queue.watchdog.tick_secs, 5);
    assert_eq!(s.runtimes.claude.permission_mode, Some(PermissionMode::Plan));
    assert_eq!(s.runtimes.claude.env["A"], "1");
    assert_eq!(s.cost.context_window("claude-opus-5-5"), Some(1_000_000));
    assert_eq!(s.cost.context_window("claude-sonnet-5"), None);
    assert_eq!(loaded.raw["theme"], "One Dark");
}

#[test]
fn bad_values_are_errors_that_name_them() {
    for (text, says) in [
        (r#"{ "daemon": { "autostart": "yes" } }"#, "daemon:"),
        (
            r#"{ "runtimes": { "claude": { "permission_mode": "bypass" } } }"#,
            "runtimes:",
        ),
        (
            r#"{ "runtimes": { "claude": { "permission_mode": "BypassPermissions" } } }"#,
            "runtimes:",
        ),
        (
            r#"{ "runtimes": { "codex": { "effort": "huge" } } }"#,
            "runtimes:",
        ),
        (
            r#"{ "messaging": { "paste_settle_ms": 99999 } }"#,
            "messaging.paste_settle_ms",
        ),
        (r#"{ "messaging": { "paste_settle_ms": -1 } }"#, "messaging:"),
        (
            r#"{ "queue": { "watchdog": { "min_interval_secs": 0 } } }"#,
            "min_interval_secs",
        ),
        (
            r#"{ "queue": { "watchdog": { "min_interval_secs": 60, "default_interval_secs": 30 } } }"#,
            "default_interval_secs",
        ),
        (r#"{ "rigs": { "keep_snapshots": 0 } }"#, "rigs.keep_snapshots"),
        // Huge values are typos, and would overflow the time arithmetic.
        (
            r#"{ "retention": { "inbox_expiry_mins": 18446744073709551615 } }"#,
            "retention.inbox_expiry_mins",
        ),
        (
            r#"{ "queue": { "watchdog": { "default_interval_secs": 18446744073709551615 } } }"#,
            "default_interval_secs",
        ),
        (
            r#"{ "retention": { "events_max_age_days": 4000000000 } }"#,
            "retention.events_max_age_days",
        ),
        (
            r#"{ "retention": { "events_max_count": 18446744073709551615 } }"#,
            "retention.events_max_count",
        ),
        (
            r#"{ "retention": { "inbox_items": 1 } }"#,
            "retention.inbox_items",
        ),
        (
            r#"{ "daemon": { "log": { "level": "infoo" } } }"#,
            "daemon.log.level",
        ),
        (
            r#"{ "cost": { "prices": { "x": { "input": -1, "output": 1 } } } }"#,
            "cost.prices.x",
        ),
        (r#"{ "discovery": 3 }"#, "discovery must be an object"),
        (
            r#"{ "discovery": { "interval_ms": 10 } }"#,
            "discovery.interval_ms",
        ),
        ("[1]", "not a JSON object"),
        (r#"{ "daemon": "#, "EOF"),
    ] {
        let e = parse(text).unwrap_err();
        assert!(e.contains(says), "{text}: {e}");
    }
    // Every permission mode by Claude Code's own name.
    for mode in [
        "default",
        "plan",
        "acceptEdits",
        "auto",
        "dontAsk",
        "bypassPermissions",
    ] {
        let text = format!(r#"{{ "runtimes": {{ "claude": {{ "permission_mode": "{mode}" }} }} }}"#);
        assert!(parse(&text).is_ok(), "{mode}");
    }
    for level in ["debug", "info,ainet_daemon=debug", "WARN", "off"] {
        let text = format!(r#"{{ "daemon": {{ "log": {{ "level": "{level}" }} }} }}"#);
        assert!(parse(&text).is_ok(), "{level}");
    }
}

#[test]
fn unknown_keys_are_reported_not_fatal() {
    let loaded = parse(
        r#"{ "vim_mode": true,
             "daemon": { "autostrat": false, "log": { "colour": true } },
             "runtimes": { "claude": { "env": { "ANY_NAME": "x" } } },
             "cost": { "prices": { "claude-x": { "input": 1, "output": 2 } } } }"#,
    )
    .unwrap();
    assert_eq!(loaded.unknown, ["daemon.autostrat", "daemon.log.colour"]);
    assert!(loaded.settings.daemon.autostart, "a misspelt key changes nothing");
}

#[test]
fn effective_values_say_where_they_come_from() {
    let loaded =
        parse(r#"{ "messaging": { "flush_gap_ms": 900 }, "daemon": { "log": { "level": "debug" } } }"#)
            .unwrap();
    let env = vec![(
        "daemon.log.level".to_string(),
        "AINET_LOG".to_string(),
        "trace".to_string(),
    )];
    let settings = loaded.settings.clone().with_env(&env);
    assert_eq!(settings.daemon.log.level, "trace", "the environment wins");
    let all = effective(&settings, &loaded.raw, &env);
    let get = |key: &str| {
        all.iter()
            .find(|e| e.key == key)
            .unwrap_or_else(|| panic!("{key}"))
    };
    let gap = get("messaging.flush_gap_ms");
    assert_eq!((gap.value.clone(), gap.default.clone()), (900.into(), 400.into()));
    assert_eq!(gap.source, Source::File);
    assert_eq!(get("messaging.paste_settle_ms").source, Source::Default);
    let level = get("daemon.log.level");
    assert_eq!(level.value, "trace");
    assert_eq!(
        level.source,
        Source::Env {
            var: "AINET_LOG".into()
        }
    );
    // Maps of your own names are one setting each.
    assert_eq!(get("runtimes.claude.env").value, serde_json::json!({}));
    assert!(all.iter().all(|e| !e.key.starts_with("runtimes.claude.env.")));
    assert!(
        all.iter()
            .any(|e| e.key == "rigs.keep_snapshots" && e.value.is_null())
    );
    // Sources survive the wire.
    let json = serde_json::to_string(&level.source).unwrap();
    assert_eq!(json, r#"{"from":"env","var":"AINET_LOG"}"#);
    assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), level.source);
}

#[test]
fn a_follower_keeps_the_last_good_settings() {
    let dir = std::env::temp_dir().join(format!("ainet-core-follow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    let mut f = Follower::new(Some(file.clone()));
    assert_eq!(f.settings(), &Settings::default(), "no file");
    assert!(!f.poll(), "still none");

    std::fs::write(&file, r#"{ "messaging": { "flush_gap_ms": 50 } }"#).unwrap();
    assert!(f.poll());
    assert_eq!(f.settings().messaging.flush_gap_ms, 50);
    assert!(!f.poll(), "unchanged");

    std::fs::write(&file, r#"{ "messaging": { "flush_gap_ms": "fast" } }"#).unwrap();
    assert!(f.poll());
    assert_eq!(f.settings().messaging.flush_gap_ms, 50, "the last good stays");
    assert!(
        f.error.as_deref().is_some_and(|e| e.contains("messaging")),
        "{:?}",
        f.error
    );

    std::fs::write(&file, r#"{ "messaging": { "flush_gap_ms": 70 } }"#).unwrap();
    assert!(f.poll());
    assert_eq!((f.settings().messaging.flush_gap_ms, f.error.clone()), (70, None));

    std::fs::remove_file(&file).unwrap();
    assert!(f.poll());
    assert_eq!(f.settings(), &Settings::default(), "gone: the defaults");
    assert!(Follower::new(None).settings() == &Settings::default());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_schema_covers_every_section() {
    let schema = schema();
    for section in SECTIONS {
        assert!(schema["properties"].get(*section).is_some(), "{section}");
    }
    let text = schema.to_string();
    for word in ["bypassPermissions", "paste_settle_ms", "keep_snapshots", "Unsafe"] {
        assert!(text.contains(word), "{word}");
    }
}

#[test]
fn loosening_settings_take_only_what_they_know() {
    let bad = |text: &str| parse(text).expect_err(text);
    // Permission modes and global rules are closed lists.
    assert!(bad(r#"{"runtimes":{"claude":{"permission_mode":"bypass"}}}"#).contains("runtimes"));
    assert!(bad(r#"{"permissions":{"global_rules":[{"commands":"x"}]}}"#).contains("permissions"));
    assert!(
        bad(r#"{"permissions":{"global_rules":[{"edits_under":{"tool":"Edit","dir":"/w","x":1}}]}}"#)
            .contains("permissions")
    );
    for (rule, why) in [
        (r#"{"command":"make && curl x | sh"}"#, "shell operators"),
        (r#"{"command":" "}"#, "empty"),
        (r#"{"tool":"Bash"}"#, "command rule"),
        (r#"{"tool":"WebFetch"}"#, "fetch_host"),
        (r#"{"tool":"Edit"}"#, "edits_under"),
        (r#"{"tool":"ExitPlanMode"}"#, "one at a time"),
        (r#"{"tool":"mcp__github"}"#, "whole MCP server"),
        (r#"{"fetch_host":"https://docs.rs"}"#, "host name"),
        (r#"{"fetch_host":"*.rs"}"#, "host name"),
        (r#"{"edits_under":{"tool":"Bash","dir":"/w"}}"#, "must be one of"),
        (r#"{"edits_under":{"tool":"Edit","dir":"/"}}"#, "absolute"),
        (r#"{"edits_under":{"tool":"Edit","dir":"w"}}"#, "absolute"),
        (r#"{"edits_under":{"tool":"Edit","dir":"/w/../x"}}"#, ".."),
    ] {
        let e = bad(&format!(r#"{{"permissions":{{"global_rules":[{rule}]}}}}"#));
        assert!(
            e.contains("permissions.global_rules[0]") && e.contains(why),
            "{rule}: {e}"
        );
    }
    let ok = parse(
        r#"{"permissions":{"global_rules":[{"command":"cargo test"},{"tool":"mcp__github__get_issue"},
            {"fetch_host":"docs.rs"},{"edits_under":{"tool":"Edit","dir":"/work/app"}}]}}"#,
    )
    .unwrap();
    assert_eq!(ok.settings.permissions.global_rules.len(), 4);
    assert_eq!(
        ok.settings.permissions.global_rules[3],
        GlobalRule::EditsUnder {
            tool: "Edit".into(),
            dir: "/work/app".into()
        }
    );
    // ainet's own variables and server are not yours to set.
    assert!(bad(r#"{"runtimes":{"claude":{"env":{"AINET_SOCKET":"/x"}}}}"#).contains("ainet sets"));
    assert!(bad(r#"{"runtimes":{"shell":{"env":{"OPENRIG_RIG":"x"}}}}"#).contains("ainet sets"));
    assert!(bad(r#"{"runtimes":{"claude":{"mcp_servers":{"ainet":{}}}}}"#).contains("ainet's own"));
    assert!(bad(r#"{"runtimes":{"claude":{"mcp_servers":{"docs":"x"}}}}"#).contains("must be an object"));
    assert!(bad(r#"{"runtimes":{"claude":{"mcp_servers":{"a.b":{}}}}}"#).contains("letters"));
    assert!(bad(r#"{"runtimes":{"claude":{"allowed_tools":["Read,Bash"]}}}"#).contains("without commas"));
    assert!(bad(r#"{"runtimes":{"codex":{"permission_mode":"plan"}}}"#).contains("Claude Code only"));
    assert!(bad(r#"{"runtimes":{"codex":{"disallowed_tools":["x"]}}}"#).contains("Claude Code only"));
    assert!(bad(r#"{"permissions":{"sensitive_paths":["~/a/../b"]}}"#).contains("sensitive_paths"));
    assert!(bad(r#"{"runtimes":{"claude":{"effort":"extreme"}}}"#).contains("runtimes"));
}

#[test]
fn spec_policies_map_onto_permission_modes() {
    assert_eq!(spec_policy("builtin:locked"), Ok(Some(PermissionMode::Plan)));
    assert_eq!(spec_policy(" builtin:default "), Ok(None));
    assert_eq!(spec_policy("builtin:standard"), Ok(None));
    assert_eq!(
        spec_policy("builtin:yolo"),
        Ok(Some(PermissionMode::BypassPermissions))
    );
    for unknown in ["builtin:YOLO", "yolo", "builtin:trusted", ""] {
        assert!(spec_policy(unknown).unwrap_err().contains("ignored"), "{unknown}");
    }
    assert!(PermissionMode::BypassPermissions.loosens() && !PermissionMode::Plan.loosens());
    assert_eq!(
        "acceptEdits".parse::<PermissionMode>(),
        Ok(PermissionMode::AcceptEdits)
    );
    assert!(
        "acceptedits".parse::<PermissionMode>().is_err(),
        "exact names only"
    );
    assert_eq!(Effort::parse(" High"), Some(Effort::High));
    assert_eq!(Effort::parse("extreme"), None);
}

/// Words typed after a key become the JSON the setting takes.
#[test]
fn words_become_the_settings_json() {
    use serde_json::json;
    assert_eq!(value_from_text("queue.nudge", "false"), json!(false));
    assert_eq!(value_from_text("permissions.hold_secs", "900"), json!(900));
    // Text needs no quotes, even when it looks like something else.
    assert_eq!(value_from_text("daemon.log.level", "debug"), json!("debug"));
    assert_eq!(value_from_text("messaging.message_note", "true"), json!("true"));
    assert_eq!(
        value_from_text("runtimes.claude.model", "\"opus\""),
        json!("opus")
    );
    assert_eq!(value_from_text("runtimes.claude.model", "null"), Value::Null);
    assert_eq!(value_from_text("rigs.default_cwd", "~/work"), json!("~/work"));
    assert_eq!(
        value_from_text("runtimes.claude.permission_mode", "plan"),
        json!("plan")
    );
    assert_eq!(value_from_text("runtimes.claude.env.RUST_LOG", "1"), json!("1"));
    assert_eq!(
        value_from_text("runtimes.claude.args", r#"["--verbose"]"#),
        json!(["--verbose"])
    );
    assert_eq!(value_from_text("rigs.keep_snapshots", "5"), json!(5));
    assert!(takes_text("daemon.socket") && !takes_text("queue.nudge"));
}

/// Only ainet's settings may be set; groups and the desktop app's keys
/// are refused with a word on why.
#[test]
fn keys_are_checked() {
    assert!(check_key("queue.nudge").is_ok());
    assert!(check_key("rigs.keep_snapshots").is_ok());
    assert!(check_key("runtimes.claude.env.RUST_LOG").is_ok());
    assert!(check_key("cost.prices.claude-opus").is_ok());
    assert!(
        check_key("queue.nudgy")
            .unwrap_err()
            .contains("no setting queue.nudgy")
    );
    assert!(check_key("queue.watchdog").unwrap_err().contains("group"));
    assert!(check_key("theme").unwrap_err().contains("desktop app"));
    assert!(check_key("").is_err() && check_key("queue..nudge").is_err());
}

/// Setting and unsetting edit the text: comments, the desktop app's keys
/// and trailing commas stay; bad values are refused and say why.
#[test]
fn set_and_unset_keep_the_file_as_written() {
    use serde_json::json;
    let text = "{\n  // my theme\n  \"theme\": \"One Dark\",\n  \"queue\": { \"nudge\": true, },\n}\n";
    let set_ = set(text, "queue.nudge", &json!(false)).unwrap();
    assert!(
        set_.contains("// my theme") && set_.contains("One Dark"),
        "{set_}"
    );
    assert!(!parse(&set_).unwrap().settings.queue.nudge);
    let more = set(&set_, "runtimes.claude.permission_mode", &json!("plan")).unwrap();
    assert_eq!(
        parse(&more).unwrap().settings.runtimes.claude.permission_mode,
        Some(PermissionMode::Plan)
    );
    // A mode misspelt, a wrong type, out of range: refused.
    assert!(set(text, "runtimes.claude.permission_mode", &json!("bypass")).is_err());
    assert!(set(text, "queue.nudge", &json!("maybe")).is_err());
    let e = set(text, "queue.watchdog.tick_secs", &json!(0)).unwrap_err();
    assert!(e.contains("tick_secs"), "{e}");
    assert!(set(text, "theme", &json!("x")).is_err());
    // A file that does not read now is not edited.
    assert!(
        set("{ \"queue\": { \"nudge\": 3 } }", "queue.nudge", &json!(true))
            .unwrap_err()
            .contains("fix it first")
    );
    let back = unset(&more, "runtimes.claude.permission_mode").unwrap();
    assert_eq!(
        parse(&back).unwrap().settings.runtimes.claude.permission_mode,
        None
    );
    assert!(back.contains("// my theme"));
    // A whole group goes back to its defaults; what is not set stays as is.
    let none = unset(&back, "queue").unwrap();
    assert!(parse(&none).unwrap().settings.queue.nudge);
    assert_eq!(unset(&none, "queue.nudge").unwrap(), none);
    // A file that is not there yet is made.
    let fresh = set("", "rigs.keep_snapshots", &json!(3)).unwrap();
    assert_eq!(parse(&fresh).unwrap().settings.rigs.keep_snapshots, Some(3));
}

#[test]
fn files_are_edited_whole() {
    let dir = std::env::temp_dir().join(format!("ainet-settings-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("ainet/settings.json");
    assert!(set_in_file(&file, "queue.nudge", &serde_json::json!(false)).unwrap());
    assert!(!set_in_file(&file, "queue.nudge", &serde_json::json!(false)).unwrap());
    assert!(!SettingsReport::read(&file).entries.is_empty());
    assert_eq!(
        SettingsReport::read(&file).get("queue.nudge").unwrap().value,
        serde_json::json!(false)
    );
    assert!(unset_in_file(&file, "queue.nudge").unwrap());
    assert!(!unset_in_file(&file, "queue.nudge").unwrap());
    // Nothing left beside it.
    assert_eq!(std::fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each setting says when a change applies; most are live.
#[test]
fn settings_say_when_they_apply() {
    assert_eq!(applies("queue.nudge"), Applies::Live);
    assert_eq!(applies("daemon.autostart"), Applies::Live);
    assert_eq!(applies("daemon.socket"), Applies::Restart);
    assert_eq!(applies("daemon.log.level"), Applies::Restart);
    assert_eq!(applies("runtimes.claude.model"), Applies::NewSeats);
    assert_eq!(applies("permissions.hold_secs"), Applies::NewSeats);
    assert_eq!(applies("permissions.global_rules"), Applies::Live);
    assert_eq!(applies("messaging.typing_guard"), Applies::NewSeats);
    assert_eq!(applies("messaging.paste_settle_ms"), Applies::Live);
    assert_eq!(applies("retention.scrollback_lines"), Applies::NewSeats);
    assert_eq!(applies("daemonic"), Applies::Live, "a whole word");
    let report = SettingsReport::new(&parse("").unwrap(), None, &[], None);
    let e = report.get("runtimes.claude.permission_mode").unwrap();
    assert_eq!(e.applies, Applies::NewSeats);
    assert!(e.description.contains("bypassPermissions"), "{}", e.description);
    let bare: Vec<&str> = report
        .entries
        .iter()
        .filter(|e| e.description.is_empty())
        .map(|e| e.key.as_str())
        .collect();
    assert!(bare.is_empty(), "not described: {bare:?}");
    assert_eq!(report.modified().count(), 0, "no file is all defaults");
}

/// Values that let agents do more without asking are told apart.
#[test]
fn less_safe_values_are_told_apart() {
    use serde_json::json;
    assert!(less_safe(
        "runtimes.claude.permission_mode",
        &json!("bypassPermissions")
    ));
    assert!(less_safe(
        "runtimes.claude.permission_mode",
        &json!("acceptEdits")
    ));
    assert!(!less_safe("runtimes.claude.permission_mode", &json!("plan")));
    assert!(!less_safe("runtimes.claude.permission_mode", &Value::Null));
    assert!(less_safe("runtimes.claude.allowed_tools", &json!(["Bash"])));
    assert!(!less_safe("runtimes.claude.allowed_tools", &json!([])));
    assert!(less_safe(
        "runtimes.claude.args",
        &json!(["--dangerously-skip-permissions"])
    ));
    for args in [
        json!(["--permission-mode", "bypassPermissions"]),
        json!(["--permission-mode=acceptEdits"]),
        json!(["--allowedTools", "Bash"]),
        json!(["--allowed-tools=Bash"]),
        json!(["--yolo"]),
        json!(["--full-auto"]),
        json!(["--sandbox", "danger-full-access"]),
        json!(["-a", "never"]),
    ] {
        assert!(less_safe("runtimes.codex.args", &args), "{args}");
    }
    for args in [
        json!(["--permission-mode", "plan"]),
        json!(["--permission-mode=default"]),
        json!(["--verbose", "--model", "opus"]),
        json!(["-a", "on-request"]),
        json!([]),
    ] {
        assert!(!less_safe("runtimes.claude.args", &args), "{args}");
    }
    assert!(less_safe("permissions.persist_always", &json!(true)));
    assert!(less_safe(
        "permissions.global_rules",
        &json!([{"tool": "WebSearch"}])
    ));
    assert!(!less_safe("permissions.persist_always", &json!(false)));
    assert!(!less_safe("queue.nudge", &json!(false)));
}

/// A report narrowed to a group, a setting or a map's entry.
#[test]
fn reports_narrow_to_a_key() {
    let report = SettingsReport::new(&parse("").unwrap(), None, &[], None);
    let queue = report.clone().only("queue").unwrap();
    assert!(queue.entries.iter().all(|e| e.key.starts_with("queue.")));
    assert!(queue.entries.len() > 3);
    assert_eq!(report.clone().only("queue.nudge").unwrap().entries.len(), 1);
    let env = report.clone().only("runtimes.claude.env.X").unwrap();
    assert_eq!(env.entries[0].key, "runtimes.claude.env");
    assert!(report.clone().only("queu").is_err());
    assert_eq!(
        report.clone().only("").unwrap().entries.len(),
        report.entries.len()
    );
}

/// One section read on its own: a mistake in another section does not
/// change it, one in itself means its defaults.
#[test]
fn a_section_reads_without_the_others() {
    let text = r#"{
        // The socket, and a typo elsewhere.
        "daemon": { "socket": "/tmp/x.sock", "autostart": false, },
        "queue": { "nudge": "yes" },
    }"#;
    assert!(parse(text).is_err(), "the whole file does not read");
    let s = parse_section(text, "daemon");
    assert_eq!(
        s.daemon.socket.as_deref(),
        Some(std::path::Path::new("/tmp/x.sock"))
    );
    assert!(!s.daemon.autostart);
    assert_eq!(s.queue, QueueSettings::default());
    // The broken section itself: its defaults.
    assert_eq!(parse_section(text, "queue"), Settings::default());
    let bad = r#"{ "daemon": { "socket": "/tmp/x.sock", "log": { "keep": 0 } } }"#;
    assert_eq!(parse_section(bad, "daemon"), Settings::default());
    assert_eq!(parse_section(r#"{ "daemon": 3 }"#, "daemon"), Settings::default());
    // No section, no file, no JSON: the defaults.
    assert_eq!(
        parse_section(r#"{ "theme": "One Dark" }"#, "daemon"),
        Settings::default()
    );
    assert_eq!(parse_section("", "daemon"), Settings::default());
    assert_eq!(parse_section("{", "daemon"), Settings::default());
    let missing = std::env::temp_dir().join("ainet-no-such-settings.json");
    assert_eq!(load_section(&missing, "daemon"), Settings::default());
}
