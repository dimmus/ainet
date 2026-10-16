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
