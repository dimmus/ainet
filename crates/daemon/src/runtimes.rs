//! The command line and environment each runtime starts with: what ainet
//! gives every agent (its MCP server, its hooks, its system prompt), then
//! settings.json's `runtimes`, then what the seat itself asks for. Pure
//! functions of their inputs, so every setting's effect on the arguments
//! is tested here; with nothing set, the arguments are what ainetd always
//! passed.

use std::collections::BTreeMap;
use std::path::Path;

use ainet_claude::launch::LaunchOptions;
use ainet_core::Runtime;
use ainet_core::settings::{AgentRuntime, Effort, PermissionMode, Settings, reserved_env, spec_policy};
use serde_json::{Value, json};

/// What ainet gives every agent it starts.
pub(crate) struct Base<'a> {
    pub id: &'a str,
    /// The `ainet` binary, for the MCP server and the hooks.
    pub exe: &'a Path,
    pub socket: &'a Path,
    /// The `PermissionRequest` hook's timeout, in seconds: longer than
    /// ainetd holds a request (`permissions.hold_secs`).
    pub permission_timeout: u64,
}

/// What a seat (a spec member, a seat added by hand, a loose agent) asks
/// for beyond its runtime's settings.
#[derive(Clone, Copy, Default)]
pub(crate) struct Ask<'a> {
    pub model: Option<&'a str>,
    /// A spec's `effort:` as written.
    pub effort: Option<&'a str>,
    /// An effort asked for in a request, passed whatever `spec_effort`
    /// says.
    pub effort_asked: Option<Effort>,
    pub permission_mode: Option<PermissionMode>,
    /// OpenRig's `permission_policy`, applied with
    /// `permissions.spec_policies`.
    pub permission_policy: Option<&'a str>,
    pub args: &'a [String],
    pub env: Option<&'a BTreeMap<String, String>>,
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The `ainet` MCP server and `extra` servers (`runtimes.<rt>.mcp_servers`),
/// as Claude Code's `--mcp-config` wants them. ainet's own always wins its
/// name.
pub(crate) fn mcp_config(base: &Base, extra: &BTreeMap<String, Value>) -> Value {
    let mut config = json!({
        "mcpServers": {
            "ainet": {
                "type": "stdio",
                "command": base.exe.display().to_string(),
                "args": ["mcp"],
                "env": { "AINET_AGENT_ID": base.id, "AINET_SOCKET": base.socket.display().to_string() }
            }
        }
    });
    if let Some(Value::Object(servers)) = config.get_mut("mcpServers") {
        for (name, server) in extra.iter().filter(|(n, _)| *n != "ainet") {
            servers.insert(name.clone(), server.clone());
        }
    }
    config
}

/// ainet's hooks for one session, passed with `--settings`. They are
/// marked so the daemon can tell them from your global ones.
pub(crate) fn hook_settings(base: &Base) -> Value {
    let cmd = format!(
        "AINET_HOOK_SOURCE=seat {} hook",
        shell_quote(&base.exe.display().to_string())
    );
    let entry =
        |timeout: u64| json!([{ "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }] }]);
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
    hooks.insert("PermissionRequest".into(), entry(base.permission_timeout));
    json!({ "hooks": hooks })
}

/// The runtime's settings, for Claude Code and Codex.
fn agent_runtime(settings: &Settings, runtime: Runtime) -> Option<&AgentRuntime> {
    match runtime {
        Runtime::ClaudeCode => Some(&settings.runtimes.claude),
        Runtime::Codex => Some(&settings.runtimes.codex),
        Runtime::Shell => None,
    }
}

/// The effort a seat starts with: one asked for in a request, else the
/// spec's (with `spec_effort`; a level the agent does not know is
/// dropped), else the runtime's.
pub(crate) fn effort(rt: &AgentRuntime, ask: &Ask) -> Option<Effort> {
    let from_spec = ask.effort.filter(|_| rt.spec_effort).and_then(Effort::parse);
    ask.effort_asked.or(from_spec).or(rt.effort)
}

/// The permission mode a spec's policy asks for, when settings.json says
/// to apply policies: `Ok(None)` when there is none to apply, `Err` with
/// why for a policy ainet does not know.
pub(crate) fn policy_mode(settings: &Settings, ask: &Ask) -> Result<Option<PermissionMode>, String> {
    match ask.permission_policy {
        Some(p) if settings.permissions.spec_policies => {
            spec_policy(p).map(|m| Some(m.unwrap_or(PermissionMode::Default)))
        }
        _ => Ok(None),
    }
}

/// Claude Code's permission mode for a seat: the seat's own, else its
/// spec's policy (with `permissions.spec_policies`), else the runtime's,
/// else none (Claude Code's default, as before).
pub(crate) fn permission_mode(settings: &Settings, ask: &Ask) -> Option<PermissionMode> {
    let rt = &settings.runtimes.claude;
    let policy = policy_mode(settings, ask).ok().flatten();
    let mode = ask.permission_mode.or(policy).or(rt.permission_mode);
    // A policy asking for Claude Code's default needs no flag unless it
    // overrides a runtime's mode.
    if mode == Some(PermissionMode::Default) && ask.permission_mode.is_none() && rt.permission_mode.is_none()
    {
        return None;
    }
    mode
}

/// Claude Code's arguments.
pub(crate) fn claude(
    base: &Base,
    settings: &Settings,
    ask: &Ask,
    resume: bool,
    prompt: &str,
    extra: &[String],
) -> Vec<String> {
    let rt = &settings.runtimes.claude;
    // Talking to the team never needs a permission prompt.
    let mut allowed = vec!["mcp__ainet".to_string()];
    for t in &rt.allowed_tools {
        if !allowed.contains(t) {
            allowed.push(t.clone());
        }
    }
    let mut rest = vec!["--settings".to_string(), hook_settings(base).to_string()];
    rest.extend(rt.args.iter().cloned());
    rest.extend(ask.args.iter().cloned());
    rest.extend(extra.iter().cloned());
    let options = LaunchOptions {
        resume: resume.then(|| base.id.to_string()),
        session_id: (!resume).then(|| base.id.to_string()),
        model: ask.model.or(rt.model.as_deref()).map(str::to_string),
        effort: effort(rt, ask).map(|e| e.as_str().to_string()),
        permission_mode: permission_mode(settings, ask).map(|m| m.as_str().to_string()),
        append_system_prompt: Some(prompt.to_string()),
        mcp_config: Some(mcp_config(base, &rt.mcp_servers)),
        strict_mcp_config: rt.strict_mcp_config,
        allowed_tools: allowed,
        disallowed_tools: rt.disallowed_tools.clone(),
        extra: rest,
        ..LaunchOptions::default()
    };
    ainet_claude::launch::args(&options)
}

/// Codex's name for an effort.
fn codex_effort(e: Effort) -> &'static str {
    match e {
        // Codex stops at xhigh.
        Effort::Max => "xhigh",
        e => e.as_str(),
    }
}

/// A TOML key: bare when it can be, else quoted.
fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        k.to_string()
    } else {
        serde_json::to_string(k).unwrap_or_default()
    }
}

/// A JSON value as an inline TOML value, for Codex's `-c key=value`.
/// TOML has no null: a null is an empty string.
fn toml(v: &Value) -> String {
    match v {
        Value::Null => "\"\"".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        // JSON's string escapes are TOML's basic-string escapes.
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::Array(a) => format!("[{}]", a.iter().map(toml).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{} = {}", toml_key(k), toml(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Codex's arguments; `prompt` (the system prompt) goes last, as its first
/// message.
pub(crate) fn codex(
    base: &Base,
    settings: &Settings,
    ask: &Ask,
    prompt: Option<&str>,
    extra: &[String],
) -> Vec<String> {
    let rt = &settings.runtimes.codex;
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut args = Vec::new();
    if let Some(m) = ask.model.or(rt.model.as_deref()) {
        args.extend(["-m".into(), m.into()]);
    }
    if let Some(e) = effort(rt, ask) {
        args.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", q(codex_effort(e))),
        ]);
    }
    args.extend([
        "-c".into(),
        format!("mcp_servers.ainet.command={}", q(&base.exe.display().to_string())),
        "-c".into(),
        "mcp_servers.ainet.args=[\"mcp\"]".into(),
        "-c".into(),
        format!(
            "mcp_servers.ainet.env={{AINET_AGENT_ID={}, AINET_SOCKET={}}}",
            q(base.id),
            q(&base.socket.display().to_string())
        ),
    ]);
    for (name, server) in rt.mcp_servers.iter().filter(|(n, _)| *n != "ainet") {
        let Value::Object(keys) = server else { continue };
        // Claude Code's `type` means nothing to Codex.
        for (key, value) in keys.iter().filter(|(k, _)| *k != "type") {
            args.extend([
                "-c".into(),
                format!("mcp_servers.{}.{}={}", toml_key(name), toml_key(key), toml(value)),
            ]);
        }
    }
    args.extend(rt.args.iter().cloned());
    args.extend(ask.args.iter().cloned());
    args.extend(extra.iter().cloned());
    if let Some(p) = prompt {
        args.push(p.to_string());
    }
    args
}

/// A shell's arguments: `runtimes.shell.args`, the seat's, then `extra`.
pub(crate) fn shell(settings: &Settings, ask: &Ask, extra: &[String]) -> Vec<String> {
    let mut args = settings.runtimes.shell.args.clone();
    args.extend(ask.args.iter().cloned());
    args.extend(extra.iter().cloned());
    args
}

/// The arguments for `runtime`.
pub(crate) fn args(
    runtime: Runtime,
    base: &Base,
    settings: &Settings,
    ask: &Ask,
    claude_resume: bool,
    prompt: Option<&str>,
    extra: &[String],
) -> Vec<String> {
    match runtime {
        Runtime::ClaudeCode => claude(
            base,
            settings,
            ask,
            claude_resume,
            prompt.unwrap_or_default(),
            extra,
        ),
        Runtime::Codex => codex(base, settings, ask, prompt, extra),
        Runtime::Shell => shell(settings, ask, extra),
    }
}

/// The environment settings.json and the seat add, before ainet's own
/// (which come after and so win; settings cannot name them anyway).
pub(crate) fn env(settings: &Settings, runtime: Runtime, ask: &Ask) -> Vec<(String, String)> {
    let configured = match agent_runtime(settings, runtime) {
        Some(rt) => &rt.env,
        None => &settings.runtimes.shell.env,
    };
    let mut env: BTreeMap<&str, &str> = configured.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    for (k, v) in ask.env.into_iter().flatten() {
        env.insert(k, v);
    }
    env.into_iter()
        .filter(|(k, _)| !k.is_empty() && !k.contains('=') && !reserved_env(k))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests;
