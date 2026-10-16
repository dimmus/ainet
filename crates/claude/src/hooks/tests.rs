use super::*;
use crate::util::testutil::TempDir;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixture(dir: &str, name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir)
        .join(format!("{name}.json"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn cc(name: &str) -> HookInput {
    HookInput::parse(&fixture("claude-2.1.289", name)).expect(name)
}

#[test]
fn plain_decisions_are_byte_identical_to_den() {
    assert_eq!(
        permission_output(
            "Bash",
            r#"{"command":"ls"}"#,
            &Decision::Deny { message: "no".into() }
        )
        .unwrap(),
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"deny\",\"message\":\"no\"},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
    assert_eq!(
        permission_output("Bash", r#"{"command":"ls"}"#, &Decision::allow()).unwrap(),
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"allow\",\"message\":\"\"},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
    assert_eq!(
        permission_output(
            "Bash",
            "",
            &Decision::Allow {
                message: ALWAYS_MESSAGE.into()
            }
        )
        .unwrap(),
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"allow\",\"message\":\"ainet: allowed by your earlier always-allow\"},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
    // Go escapes HTML characters in strings.
    assert_eq!(
        permission_output(
            "Bash",
            "",
            &Decision::Deny {
                message: "a <b> & c".into()
            }
        )
        .unwrap(),
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"deny\",\"message\":\"a \\u003cb\\u003e \\u0026 c\"},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
    assert_eq!(permission_output("Bash", "", &Decision::NoDecision), None);
    let v = permission_output_value("Bash", "", &Decision::deny()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["decision"]["message"], DENY_MESSAGE);
}

/// Claude Code 2.1.289 keeps its plan dialog open on a bare allow for
/// ExitPlanMode: the approval carries the call's own input as updatedInput.
#[test]
fn plan_approval_and_rejection() {
    let input = cc("permission-exitplanmode");
    let want = concat!(
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"allow\",\"updatedInput\":",
        "{\"plan\":\"# Context\\nCreate a file `plan.txt` in /work/proj containing the word \\\"planned\\\".\\n\\n# Step\\n1. Write `/work/proj/plan.txt` with the content `planned`.\\n\\n# Verification\\nRead `plan.txt` and confirm it contains `planned`.\\n\",",
        "\"planFilePath\":\"/work/.claude/plans/plan-create-plan-txt-shimmering-turing.md\"}",
        "},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
    assert_eq!(
        permission_output_for(&input, &Decision::ApprovePlan).unwrap(),
        want
    );
    // An allow of ExitPlanMode is a plan approval, as in Den.
    assert_eq!(permission_output_for(&input, &Decision::allow()).unwrap(), want);
    let v: serde_json::Value =
        serde_json::from_str(&permission_output_for(&input, &Decision::ApprovePlan).unwrap()).unwrap();
    assert_eq!(
        v["hookSpecificOutput"]["decision"]["updatedInput"],
        input.tool_input_value()
    );
    assert!(v["hookSpecificOutput"]["decision"].get("message").is_none());

    assert_eq!(
        permission_output_for(
            &input,
            &Decision::RejectPlan {
                note: DEN_DENY_MESSAGE.into()
            }
        )
        .unwrap(),
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"deny\",\"message\":\"Denied from den by the user.\"},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
    // A call that cannot be echoed is left to the terminal, never a bare allow.
    for bad in [r#"{"planFilePath":"/work/p.md"}"#, r#""x""#, "", r#"{"plan":""}"#] {
        assert_eq!(
            permission_output(PLAN_TOOL, bad, &Decision::allow()),
            None,
            "{bad}"
        );
        assert_eq!(
            permission_output(PLAN_TOOL, bad, &Decision::ApprovePlan),
            None,
            "{bad}"
        );
    }
    // Key order inside values is kept; top-level keys are sorted; whitespace
    // goes; HTML characters are escaped.
    assert_eq!(
        plan_approval("{ \"z\": {\"b\": 1, \"a\": [1, 2]}, \"plan\": \"x < y\" }").unwrap(),
        r#"{"plan":"x < y","z":{"b":1,"a":[1,2]}}"#
    );
    assert_eq!(
        permission_output(PLAN_TOOL, r#"{"plan":"x < y"}"#, &Decision::ApprovePlan).unwrap(),
        "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"allow\",\"updatedInput\":{\"plan\":\"x \\u003c y\"}},\"hookEventName\":\"PermissionRequest\"}}\n"
    );
}

#[test]
fn question_answers() {
    let input = r#"{"questions":[{"question":"Which one?","header":"Pick","options":[{"label":"Warm","description":""},{"label":"Cool","description":""}],"multiSelect":false}]}"#;
    let answers = BTreeMap::from([("Which one?".to_string(), "Cool".to_string())]);
    assert_eq!(
        permission_output("AskUserQuestion", input, &Decision::Answer(answers.clone())).unwrap(),
        concat!(
            "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"allow\",\"updatedInput\":",
            "{\"answers\":{\"Which one?\":\"Cool\"},\"questions\":[{\"question\":\"Which one?\",\"header\":\"Pick\",\"options\":[{\"label\":\"Warm\",\"description\":\"\"},{\"label\":\"Cool\",\"description\":\"\"}],\"multiSelect\":false}]}",
            "},\"hookEventName\":\"PermissionRequest\"}}\n"
        )
    );
    // Answers only go with AskUserQuestion, and only complete ones.
    assert_eq!(permission_output("Bash", input, &Decision::Answer(answers)), None);
    let wrong = BTreeMap::from([("Another?".to_string(), "Cool".to_string())]);
    assert_eq!(
        permission_output("AskUserQuestion", input, &Decision::Answer(wrong)),
        None
    );

    // The real 2.1.289 payload, answered with option 2.
    let real = cc("permission-askuserquestion");
    let qs = crate::question::parse(real.tool_input_raw()).unwrap();
    let a = crate::question::pick(&qs[0], "2").unwrap();
    let out = permission_output_for(
        &real,
        &Decision::Answer(BTreeMap::from([(qs[0].text.clone(), a)])),
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["hookSpecificOutput"]["decision"]["updatedInput"]["answers"]["Which colour do you prefer?"],
        "Blue"
    );
    assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "allow");
    assert_eq!(
        v["hookSpecificOutput"]["decision"]["updatedInput"]["questions"],
        real.tool_input_value()["questions"]
    );
    assert!(out.ends_with("}}\n") && out.matches('\n').count() == 1);
}

#[test]
fn parses_every_fixture() {
    for dir in ["claude-2.1.289", "captured-2.1.289"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(dir);
        for e in std::fs::read_dir(&path).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            if stem.ends_with("-reply") {
                continue;
            }
            let raw = std::fs::read_to_string(e.path()).unwrap();
            let input = HookInput::parse(&raw).unwrap_or_else(|| panic!("{dir}/{name}"));
            assert!(!input.session_id.is_empty(), "{name}");
            assert!(
                !matches!(input.event(), HookEvent::Other(_)),
                "{name}: {}",
                input.hook_event_name
            );
        }
    }
}

#[test]
fn fields_and_stream_events() {
    let stop = cc("stop");
    assert_eq!(stop.event(), HookEvent::Stop);
    assert_eq!(stop.last_assistant_message, "Created check.txt.");
    assert_eq!(stop.permission_mode, "default");
    assert_eq!(stop.stream_event().unwrap().kind, StreamKind::Done);

    let end = cc("session-end");
    assert_eq!(end.session_end_reason(), "other");
    let ev = end.stream_event().unwrap();
    assert_eq!(
        (ev.kind, ev.reason.as_deref()),
        (StreamKind::SessionEnd, Some("other"))
    );

    let start = cc("session-start");
    assert_eq!(start.stream_event().unwrap().kind, StreamKind::SessionStart);

    let perm = cc("permission-bash");
    assert_eq!(perm.tool_name, "Bash");
    assert!(
        perm.tool_use_id.is_empty(),
        "a PermissionRequest has no tool_use_id"
    );
    let ev = perm.stream_event().unwrap();
    assert_eq!(
        (ev.kind, ev.request, ev.tool.as_deref()),
        (StreamKind::NeedsYou, Some(RequestKind::Permission), Some("Bash"))
    );

    let plan = cc("permission-exitplanmode").stream_event().unwrap();
    assert_eq!((plan.request, plan.tool), (Some(RequestKind::Plan), None));

    // A question is reported once, from its PreToolUse.
    assert_eq!(cc("permission-askuserquestion").stream_event(), None);
    assert_eq!(
        cc("pre-askuserquestion").stream_event().unwrap().request,
        Some(RequestKind::Question)
    );
    assert_eq!(cc("pre-bash").stream_event(), None);
    // A permission_prompt notice repeats a wait already reported.
    assert_eq!(cc("notification-permission-prompt-bash").stream_event(), None);
    assert_eq!(
        cc("notification-idle-prompt").stream_event().unwrap().kind,
        StreamKind::Idle
    );
    assert_eq!(
        cc("user-prompt-submit").stream_event().unwrap().kind,
        StreamKind::TurnStart
    );

    // StopFailure as 2.1.289 sends it ("error"), and as documented.
    let f = HookInput::parse(r#"{"hook_event_name":"StopFailure","session_id":"s","error":"rate_limit"}"#)
        .unwrap();
    assert_eq!(f.failure_type(), "rate_limit");
    assert_eq!(f.stream_event().unwrap().error.as_deref(), Some("rate_limit"));
    let f = HookInput::parse(
        r#"{"hook_event_name":"StopFailure","session_id":"s","error_type":"overloaded","error":{"x":1}}"#,
    )
    .unwrap();
    assert_eq!(f.failure_type(), "overloaded");
    let f =
        HookInput::parse(r#"{"hook_event_name":"StopFailure","session_id":"s","error":{"nested":[1,2]}}"#)
            .unwrap();
    assert_eq!(f.failure_type(), "unknown");
    assert_eq!(failure_title("rate_limit"), "Turn ended: rate limited");
    assert_eq!(
        failure_title("server_error"),
        "Turn ended on an API error: server error"
    );

    let t = HookInput::parse(
        r#"{"hook_event_name":"PostToolUseFailure","session_id":"s","tool_name":"Bash","is_interrupt":true}"#,
    )
    .unwrap();
    assert!(t.is_interrupt);
    assert_eq!(t.stream_event().unwrap().tool.as_deref(), Some("Bash"));

    let sub = HookInput::parse(r#"{"hook_event_name":"Stop","session_id":"s","agent_id":"a1"}"#).unwrap();
    assert!(sub.is_subagent());
    // Nulls are empty strings, as in Go.
    let n =
        HookInput::parse(r#"{"hook_event_name":"Stop","session_id":"s","message":null,"is_interrupt":null}"#)
            .unwrap();
    assert_eq!(n.message, "");

    for bad in ["", "not json", "{}", r#"{"session_id":"s"}"#, "[1]"] {
        assert!(HookInput::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn fit_payload_drops_tool_io_for_followed_events() {
    let big = "x".repeat(MAX_LINE);
    let post = serde_json::json!({"session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Read", "tool_input": {"file_path": "/x"}, "tool_response": big}).to_string();
    let out = fit_payload(&post, "PostToolUse", MAX_LINE).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["tool_name"], "Read");
    assert!(v.get("tool_response").is_none() && v.get("tool_input").is_none());
    let perm = serde_json::json!({"session_id": "s1", "hook_event_name": "PermissionRequest", "tool_name": "Write", "tool_input": {"content": big}}).to_string();
    assert_eq!(fit_payload(&perm, "PermissionRequest", MAX_LINE), None);
    assert_eq!(
        fit_payload(r#"{"session_id":"s1"}"#, "Stop", MAX_LINE).unwrap(),
        r#"{"session_id":"s1"}"#
    );
}

#[test]
fn summarize_reply_like_den() {
    assert_eq!(summarize_reply(""), "Turn finished.");
    assert_eq!(summarize_reply("```\ncode\n```\n# Heading"), "Turn finished.");
    assert_eq!(
        summarize_reply("## Done\n- **Fixed** the `login` redirect. Then more text."),
        "Fixed the login redirect."
    );
    assert_eq!(summarize_reply("Ok. Short."), "Ok. Short.");
    let long = "word ".repeat(80);
    let s = summarize_reply(&long);
    assert!(s.ends_with('…') && s.chars().count() == 160, "{s}");
}

// ---- install ----

fn read_json(p: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn install_keeps_other_hooks_and_uninstall_restores() {
    let tmp = TempDir::new();
    let path = settings_path_in(tmp.path());
    let orig = r#"{"model":"fable","permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"bash other.sh"}]}],"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/opt/den hook"}]}]}}"#;
    std::fs::write(&path, orig).unwrap();
    let cmd = hook_command(Path::new("/usr/local/bin/ainet"));
    assert_eq!(cmd, "/usr/local/bin/ainet hook");

    let r = install(&path, &cmd).unwrap();
    let backup = r.backup.clone().expect("a backup");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), orig);
    assert!(backup.to_string_lossy().contains("settings.json.bak.ainet."));
    assert_eq!((r.replaced, r.added), (0, SPECS.len()));
    assert!(installed(&path, &cmd) && installed(&path, ""));
    assert!(!installed(&path, "/elsewhere/ainet hook"));

    let s = read_json(&path);
    assert_eq!(s["model"], "fable");
    assert_eq!(s["permissions"]["allow"][0], "Bash(ls)");
    let hooks = s["hooks"].as_object().unwrap();
    assert_eq!(
        hooks["Stop"].as_array().unwrap().len(),
        2,
        "keeps the other Stop hook"
    );
    assert_eq!(hooks["Stop"][0]["hooks"][0]["command"], "bash other.sh");
    // Den's own hook is someone else's: kept.
    assert_eq!(hooks["PreToolUse"].as_array().unwrap().len(), 2);
    for sp in SPECS {
        let list = hooks[sp.event].as_array().unwrap();
        let ours: Vec<_> = list
            .iter()
            .filter(|e| e["hooks"][0]["command"] == cmd.as_str())
            .collect();
        assert_eq!(ours.len(), 1, "{}", sp.event);
        assert_eq!(ours[0]["hooks"][0]["type"], "command");
        assert_eq!(ours[0]["hooks"][0]["timeout"], sp.timeout);
        assert_eq!(
            ours[0].get("matcher").and_then(|m| m.as_str()).unwrap_or(""),
            sp.matcher
        );
    }
    // The permission hook outlasts the daemon's hold.
    assert!(
        hooks["PermissionRequest"][0]["hooks"][0]["timeout"]
            .as_u64()
            .unwrap()
            > PERMISSION_WAIT_SECS
    );
    assert!(hooks["PermissionRequest"][0].get("matcher").is_none());

    // The file is sorted, indented by two spaces, newline-terminated, 0600.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("{\n  \"hooks\": {") && text.ends_with("}\n"),
        "{text}"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // Installing again, also from another path, does not duplicate.
    let other = hook_command(Path::new("/home/u/.cargo/bin/ainet"));
    let r = install(&path, &other).unwrap();
    assert_eq!(r.replaced, SPECS.len());
    let s = read_json(&path);
    assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(s["hooks"]["PermissionRequest"].as_array().unwrap().len(), 1);
    assert!(installed(&path, &other) && !installed(&path, &cmd));

    let u = uninstall(&path, &other).unwrap();
    assert_eq!(u.removed, SPECS.len());
    assert!(u.backup.is_some());
    let s = read_json(&path);
    let hooks = s["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 2, "{hooks:?}");
    assert_eq!(hooks["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["command"], "/opt/den hook");
    assert_eq!(s["model"], "fable");
    assert!(!installed(&path, ""));

    // Nothing to remove: the file is left alone.
    let before = std::fs::read_to_string(&path).unwrap();
    let u = uninstall(&path, "").unwrap();
    assert_eq!((u.removed, u.backup), (0, None));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn install_into_a_missing_file_and_remove_all() {
    let tmp = TempDir::new();
    let path = tmp.join("nested/dir/settings.json");
    let cmd = hook_command(Path::new("/my tools/ainet"));
    assert_eq!(cmd, "\"/my tools/ainet\" hook");
    let r = install(&path, &cmd).unwrap();
    assert_eq!(r.backup, None);
    assert!(installed(&path, &cmd));
    assert!(installed(&path, ""), "a quoted path is ours too");
    let u = uninstall(&path, "").unwrap();
    assert_eq!(u.removed, SPECS.len());
    let s = read_json(&path);
    assert!(s.get("hooks").is_none(), "{s}");
}

#[test]
fn install_refuses_broken_settings() {
    let tmp = TempDir::new();
    let path = tmp.join("settings.json");
    std::fs::write(&path, "{ not json").unwrap();
    assert!(install(&path, "/x/ainet hook").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    std::fs::write(&path, "[1]").unwrap();
    assert!(install(&path, "/x/ainet hook").is_err());
    std::fs::write(&path, "null").unwrap();
    assert!(install(&path, "/x/ainet hook").is_ok());
}

#[test]
fn ours_is_by_program() {
    assert!(is_ours("/usr/bin/ainet hook", "/usr/local/bin/ainet hook"));
    assert!(is_ours("ainet hook", ""));
    assert!(is_ours("\"/a b/ainet\" hook", ""));
    assert!(!is_ours("/usr/bin/den hook", ""));
    assert!(!is_ours("/usr/bin/ainet hook --x", ""));
    assert!(!is_ours("bash other.sh", "/usr/bin/ainet hook"));
    assert!(!is_ours("/usr/bin/ainetx hook", "/usr/bin/ainet hook"));
}

#[test]
fn the_block_to_merge_is_what_install_writes() {
    let tmp = TempDir::new();
    let path = settings_path_in(tmp.path());
    let cmd = hook_command(Path::new("/opt/my apps/ainet"));
    install(&path, &cmd).unwrap();
    // What a hand merge of the printed block gives equals what install
    // writes into an empty file: one entry per SPECS event, the same
    // matchers and timeouts.
    assert_eq!(read_json(&path), hooks_block(&cmd, PERMISSION_TIMEOUT_SECS));
    let block = hooks_block(&cmd, PERMISSION_TIMEOUT_SECS);
    let hooks = block["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), SPECS.len());
    for sp in SPECS {
        let e = &hooks[sp.event][0];
        assert_eq!(e["hooks"][0]["timeout"], sp.timeout, "{}", sp.event);
        assert_eq!(e["hooks"][0]["command"], cmd.as_str());
        assert_eq!(
            e.get("matcher").and_then(|m| m.as_str()).unwrap_or(""),
            sp.matcher
        );
    }
    assert_eq!(hooks["PreToolUse"][0]["matcher"], "AskUserQuestion");
    assert_eq!(hooks["SessionEnd"][0]["hooks"][0]["timeout"], 5);
}

#[test]
fn status_says_what_is_there_and_install_repairs_it() {
    let tmp = TempDir::new();
    let path = settings_path_in(tmp.path());
    let cmd = hook_command(Path::new("/usr/bin/ainet"));
    let st = status(&path, &cmd, 600);
    assert!(!st.exists && st.ours == 0 && !st.current());
    assert_eq!(st.missing.len(), SPECS.len());
    assert!(st.summary().starts_with("not installed in"));

    install(&path, &cmd).unwrap();
    let st = status(&path, &cmd, 600);
    assert!(st.current(), "{st:?}");
    assert_eq!(st.ours, SPECS.len());
    assert!(st.summary().starts_with("installed in"));

    // A longer hold needs a longer PermissionRequest timeout: outdated.
    let st = status(&path, &cmd, 1220);
    assert!(!st.current());
    assert_eq!((st.stale, st.missing.clone()), (1, vec!["PermissionRequest"]));
    assert!(st.summary().contains("needs repair"), "{}", st.summary());
    let r = install_with(&path, &cmd, 1220).unwrap();
    assert!(r.backup.is_some());
    assert_eq!(r.replaced, SPECS.len());
    assert!(status(&path, &cmd, 1220).current());
    assert_eq!(
        read_json(&path)["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
        1220
    );

    // An ainet at another path counts as ours, and outdated.
    let moved = hook_command(Path::new("/home/u/.cargo/bin/ainet"));
    let st = status(&path, &moved, 1220);
    assert_eq!((st.ours, st.stale), (SPECS.len(), SPECS.len()));

    // Status only reads; a file that does not parse is said so.
    std::fs::write(&path, "{ not json").unwrap();
    let st = status(&path, &cmd, 600);
    assert!(st.error.is_some() && !st.current());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}
