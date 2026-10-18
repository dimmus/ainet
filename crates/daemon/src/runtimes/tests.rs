use std::path::PathBuf;

use ainet_core::settings::{Effort, PermissionMode, Settings};
use serde_json::json;

use super::*;

fn base() -> Base<'static> {
    static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    static SOCKET: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    Base {
        id: "11111111-2222-4333-8444-555555555555",
        exe: EXE.get_or_init(|| PathBuf::from("/opt/ainet bin/ainet")),
        socket: SOCKET.get_or_init(|| PathBuf::from("/run/user/1000/ainet.sock")),
        permission_timeout: Settings::default().permissions.hook_timeout_secs(),
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn settings(json: serde_json::Value) -> Settings {
    let loaded = ainet_core::settings::parse(&json.to_string()).unwrap();
    loaded.settings
}

/// ainetd's argument building before settings.json could change it,
/// kept word for word: with nothing set, every runtime must still get
/// exactly this.
mod today {
    use super::*;

    fn shell_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', r"'\''"))
    }

    fn mcp_config(b: &Base) -> serde_json::Value {
        json!({
            "mcpServers": {
                "ainet": {
                    "type": "stdio",
                    "command": b.exe.display().to_string(),
                    "args": ["mcp"],
                    "env": { "AINET_AGENT_ID": b.id, "AINET_SOCKET": b.socket.display().to_string() }
                }
            }
        })
    }

    fn hook_settings(b: &Base) -> serde_json::Value {
        let cmd = format!(
            "AINET_HOOK_SOURCE=seat {} hook",
            shell_quote(&b.exe.display().to_string())
        );
        let entry =
            |timeout: u32| json!([{ "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }] }]);
        let mut hooks = serde_json::Map::new();
        for event in [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "Notification",
            "Stop",
            "StopFailure",
            "PostToolUse",
            "PostToolUseFailure",
        ] {
            hooks.insert(event.into(), entry(10));
        }
        hooks.insert(
            "PreToolUse".into(),
            json!([{ "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": cmd, "timeout": 10 }] }]),
        );
        hooks.insert("PermissionRequest".into(), entry(600));
        json!({ "hooks": hooks })
    }

    pub fn claude(
        b: &Base,
        resume: bool,
        model: Option<&str>,
        prompt: &str,
        extra: &[String],
    ) -> Vec<String> {
        let mut args = vec![
            if resume { "--resume" } else { "--session-id" }.to_string(),
            b.id.to_string(),
        ];
        if let Some(m) = model {
            args.extend(["--model".into(), m.into()]);
        }
        args.extend(["--append-system-prompt".into(), prompt.to_string()]);
        args.extend(["--mcp-config".into(), mcp_config(b).to_string()]);
        args.extend(["--allowedTools".into(), "mcp__ainet".into()]);
        args.extend(["--settings".into(), hook_settings(b).to_string()]);
        args.extend(extra.iter().cloned());
        args
    }

    pub fn codex(b: &Base, model: Option<&str>, prompt: Option<&str>, extra: &[String]) -> Vec<String> {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let mut args = Vec::new();
        if let Some(m) = model {
            args.extend(["-m".into(), m.into()]);
        }
        args.extend([
            "-c".into(),
            format!("mcp_servers.ainet.command={}", q(&b.exe.display().to_string())),
            "-c".into(),
            "mcp_servers.ainet.args=[\"mcp\"]".into(),
            "-c".into(),
            format!(
                "mcp_servers.ainet.env={{AINET_AGENT_ID={}, AINET_SOCKET={}}}",
                q(b.id),
                q(&b.socket.display().to_string())
            ),
        ]);
        args.extend(extra.iter().cloned());
        if let Some(p) = prompt {
            args.push(p.to_string());
        }
        args
    }
}

/// With no settings (or a file of the desktop app's keys alone), every
/// runtime starts with exactly the arguments it always did, whatever the
/// spec says about effort and policies.
#[test]
fn nothing_set_is_byte_for_byte_today() {
    let b = base();
    let prompt = "# You are a seat\nwith 'quotes' and \"more\"";
    let extra = s(&["--verbose"]);
    for set in [
        Settings::default(),
        settings(json!({"theme": "One Dark", "terminal": {"font_size": 13}})),
    ] {
        for resume in [false, true] {
            for model in [None, Some("opus")] {
                let ask = Ask {
                    model,
                    effort: Some("high"),
                    permission_policy: Some("builtin:yolo"),
                    ..Ask::default()
                };
                assert_eq!(
                    claude(&b, &set, &ask, resume, prompt, &extra),
                    today::claude(&b, resume, model, prompt, &extra)
                );
                assert_eq!(
                    claude(&b, &set, &ask, resume, prompt, &[]),
                    today::claude(&b, resume, model, prompt, &[])
                );
                assert_eq!(
                    codex(&b, &set, &ask, Some(prompt), &[]),
                    today::codex(&b, model, Some(prompt), &[])
                );
                assert_eq!(
                    codex(&b, &set, &ask, None, &extra),
                    today::codex(&b, model, None, &extra)
                );
            }
        }
        assert!(shell(&set, &Ask::default(), &[]).is_empty());
        assert_eq!(shell(&set, &Ask::default(), &extra), extra);
        assert!(env(&set, Runtime::ClaudeCode, &Ask::default()).is_empty());
        assert!(env(&set, Runtime::Shell, &Ask::default()).is_empty());
        assert_eq!(permission_mode(&set, &Ask::default()), None);
    }
    assert_eq!(b.permission_timeout, 600, "the hook's timeout, as before");
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

#[test]
fn claude_settings_reach_the_command_line() {
    let b = base();
    let set = settings(json!({
        "runtimes": { "claude": {
            "model": "sonnet",
            "effort": "high",
            "permission_mode": "acceptEdits",
            "args": ["--verbose"],
            "allowed_tools": ["Bash(cargo test:*)", "mcp__ainet"],
            "disallowed_tools": ["WebFetch"],
            "mcp_servers": { "docs": { "command": "docs-mcp", "args": ["--stdio"] } },
            "strict_mcp_config": true
        } }
    }));
    let ask = Ask {
        args: &["--debug".to_string()],
        ..Ask::default()
    };
    let a = claude(&b, &set, &ask, false, "P", &s(&["--x"]));
    assert_eq!(value_after(&a, "--model"), Some("sonnet"));
    assert_eq!(value_after(&a, "--effort"), Some("high"));
    assert_eq!(value_after(&a, "--permission-mode"), Some("acceptEdits"));
    assert_eq!(
        value_after(&a, "--allowedTools"),
        Some("mcp__ainet,Bash(cargo test:*)"),
        "ainet's own first, once"
    );
    assert_eq!(value_after(&a, "--disallowedTools"), Some("WebFetch"));
    assert!(a.contains(&"--strict-mcp-config".to_string()));
    let mcp: serde_json::Value = serde_json::from_str(value_after(&a, "--mcp-config").unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["docs"]["command"], "docs-mcp");
    assert_eq!(mcp["mcpServers"]["ainet"]["args"], json!(["mcp"]));
    // ainet's hooks, then the runtime's args, the seat's, the request's.
    let settings_at = a.iter().position(|x| x == "--settings").unwrap();
    assert_eq!(&a[settings_at + 2..], &s(&["--verbose", "--debug", "--x"])[..]);
    // The seat's model wins over the runtime's.
    let mine = Ask {
        model: Some("opus"),
        ..Ask::default()
    };
    assert_eq!(
        value_after(&claude(&b, &set, &mine, false, "P", &[]), "--model"),
        Some("opus")
    );
}

#[test]
fn ainets_own_server_and_tool_cannot_be_taken() {
    let b = base();
    let mut extra = BTreeMap::new();
    extra.insert("ainet".to_string(), json!({"command": "evil"}));
    let c = mcp_config(&b, &extra);
    assert_eq!(c["mcpServers"]["ainet"]["args"], json!(["mcp"]));
    assert_ne!(c["mcpServers"]["ainet"]["command"], "evil");
    // settings.json refuses the name outright.
    assert!(
        ainet_core::settings::parse(r#"{"runtimes":{"claude":{"mcp_servers":{"ainet":{"command":"x"}}}}}"#)
            .is_err()
    );
}

#[test]
fn effort_from_a_spec_only_when_asked() {
    let mut rt = ainet_core::settings::AgentRuntime::default();
    let spec = Ask {
        effort: Some("High"),
        ..Ask::default()
    };
    assert_eq!(effort(&rt, &spec), None, "dropped, as before");
    rt.spec_effort = true;
    assert_eq!(effort(&rt, &spec), Some(Effort::High));
    let odd = Ask {
        effort: Some("extreme"),
        ..Ask::default()
    };
    assert_eq!(
        effort(&rt, &odd),
        None,
        "a level Claude Code does not know is dropped"
    );
    rt.effort = Some(Effort::Low);
    assert_eq!(effort(&rt, &odd), Some(Effort::Low));
    assert_eq!(effort(&rt, &spec), Some(Effort::High), "the spec's wins");
    rt.spec_effort = false;
    let asked = Ask {
        effort_asked: Some(Effort::Max),
        ..spec
    };
    assert_eq!(
        effort(&rt, &asked),
        Some(Effort::Max),
        "a request's is always passed"
    );
}

#[test]
fn spec_policies_map_conservatively_and_only_when_on() {
    let ask = |p: &'static str| Ask {
        permission_policy: Some(p),
        ..Ask::default()
    };
    let off = Settings::default();
    for p in ["builtin:yolo", "builtin:locked", "builtin:nonsense"] {
        assert_eq!(permission_mode(&off, &ask(p)), None, "{p} is ignored by default");
        assert_eq!(policy_mode(&off, &ask(p)), Ok(None));
    }
    let on = settings(json!({"permissions": {"spec_policies": true}}));
    assert_eq!(
        permission_mode(&on, &ask("builtin:locked")),
        Some(PermissionMode::Plan)
    );
    assert_eq!(
        permission_mode(&on, &ask("builtin:yolo")),
        Some(PermissionMode::BypassPermissions)
    );
    assert_eq!(
        permission_mode(&on, &ask("builtin:default")),
        None,
        "Claude Code's default: no flag"
    );
    assert_eq!(permission_mode(&on, &ask("builtin:standard")), None);
    assert!(policy_mode(&on, &ask("builtin:Yolo")).is_err(), "names are exact");
    assert_eq!(
        permission_mode(&on, &ask("builtin:nonsense")),
        None,
        "unknown: ignored"
    );
    // A policy wins over the runtime's mode; the seat's own over both.
    let both = settings(json!({
        "permissions": {"spec_policies": true},
        "runtimes": {"claude": {"permission_mode": "acceptEdits"}}
    }));
    assert_eq!(
        permission_mode(&both, &ask("builtin:default")),
        Some(PermissionMode::Default)
    );
    assert_eq!(
        permission_mode(&both, &ask("builtin:locked")),
        Some(PermissionMode::Plan)
    );
    assert_eq!(
        permission_mode(&both, &Ask::default()),
        Some(PermissionMode::AcceptEdits)
    );
    let own = Ask {
        permission_mode: Some(PermissionMode::Default),
        ..ask("builtin:yolo")
    };
    assert_eq!(permission_mode(&both, &own), Some(PermissionMode::Default));
    let a = claude(&base(), &on, &ask("builtin:yolo"), false, "P", &[]);
    assert_eq!(value_after(&a, "--permission-mode"), Some("bypassPermissions"));
}

#[test]
fn codex_settings_reach_the_command_line() {
    let b = base();
    let set = settings(json!({
        "runtimes": { "codex": {
            "model": "gpt-5",
            "effort": "max",
            "args": ["--search"],
            "mcp_servers": { "docs": { "type": "stdio", "command": "docs-mcp", "args": ["--stdio"], "env": {"K": "v w"} } }
        } }
    }));
    let a = codex(&b, &set, &Ask::default(), Some("P"), &s(&["--x"]));
    assert_eq!(value_after(&a, "-m"), Some("gpt-5"));
    assert!(
        a.contains(&"model_reasoning_effort=\"xhigh\"".to_string()),
        "{a:?}"
    );
    assert!(
        a.contains(&"mcp_servers.docs.command=\"docs-mcp\"".to_string()),
        "{a:?}"
    );
    assert!(
        a.contains(&"mcp_servers.docs.args=[\"--stdio\"]".to_string()),
        "{a:?}"
    );
    assert!(
        a.contains(&"mcp_servers.docs.env={K = \"v w\"}".to_string()),
        "{a:?}"
    );
    assert!(!a.iter().any(|x| x.contains("docs.type")), "Codex has no type");
    assert_eq!(
        &a[a.len() - 3..],
        &s(&["--search", "--x", "P"])[..],
        "the prompt last"
    );
    // Claude Code's own restrictions are refused for Codex, never ignored.
    for bad in [
        json!({"permission_mode": "plan"}),
        json!({"disallowed_tools": ["Bash"]}),
        json!({"strict_mcp_config": true}),
    ] {
        let text = json!({"runtimes": {"codex": bad}}).to_string();
        assert!(ainet_core::settings::parse(&text).is_err(), "{text}");
    }
}

#[test]
fn shells_and_environment() {
    let set = settings(json!({
        "runtimes": {
            "shell": { "args": ["-l"], "env": {"EDITOR": "vi"} },
            "claude": { "env": {"CLAUDE_CODE_MAX_OUTPUT_TOKENS": "8000", "SHARED": "rt"} }
        }
    }));
    let seat_env: BTreeMap<String, String> = [("SHARED", "seat"), ("AINET_SOCKET", "/evil")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let ask = Ask {
        args: &["-c".to_string(), "echo hi".to_string()],
        env: Some(&seat_env),
        ..Ask::default()
    };
    assert_eq!(shell(&set, &ask, &s(&["x"])), s(&["-l", "-c", "echo hi", "x"]));
    assert_eq!(
        env(&set, Runtime::Shell, &Ask::default()),
        vec![("EDITOR".into(), "vi".into())]
    );
    assert_eq!(
        env(&set, Runtime::ClaudeCode, &ask),
        vec![
            ("CLAUDE_CODE_MAX_OUTPUT_TOKENS".to_string(), "8000".to_string()),
            ("SHARED".to_string(), "seat".to_string()),
        ],
        "the seat's over the runtime's; ainet's own never"
    );
    assert!(ainet_core::settings::parse(r#"{"runtimes":{"shell":{"env":{"AINET_AGENT_ID":"x"}}}}"#).is_err());
}

#[test]
fn the_hook_outlasts_the_hold() {
    let set = settings(json!({"permissions": {"hold_secs": 1200}}));
    let p = &set.permissions;
    assert!(p.client_timeout_secs() > p.hold_secs && p.hook_timeout_secs() > p.client_timeout_secs());
    let b = Base {
        permission_timeout: p.hook_timeout_secs(),
        ..base()
    };
    let hooks = hook_settings(&b);
    assert_eq!(
        hooks["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
        1220
    );
    assert_eq!(hooks["hooks"]["Stop"][0]["hooks"][0]["timeout"], 10);
}
