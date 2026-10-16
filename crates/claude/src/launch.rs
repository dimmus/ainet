//! `claude` command lines, the way Den's spawn and orchestrator build them,
//! plus what ainet needs.
//!
//! Ported from Den's `internal/spawn` and `internal/orch` (MIT).
//!
//! Flags Den uses (so confirmed working with Claude Code 2.1.289): `--name`
//! (the session's peer name, `name` in its sessions file), `--resume <id>`,
//! `--fork-session` (with `--resume`: the copy gets a new session id),
//! `--worktree <name>` (a git worktree under `<repo>/.claude/worktrees/<name>`
//! on branch `worktree-<name>`), `--add-dir`, `--model`, `--mcp-config <JSON
//! string>`, `--strict-mcp-config`, `--allowedTools <comma list>`, `--tools
//! <comma list>`, `--append-system-prompt`, `--system-prompt-snapshot off`,
//! `--permission-mode` (README: `claude --permission-mode plan`), `-p`,
//! `--input-format stream-json`, `--output-format stream-json`,
//! `--include-partial-messages`, `--verbose`, and `--` before the first
//! prompt (`--allowedTools` takes a list, so a prompt after it would be read
//! as one more tool).
//!
//! Not confirmed from Den's source: `--session-id <uuid>` (ainetd has
//! started every seat with it since its first release), `--disallowedTools
//! <comma list>` (Claude Code's documented counterpart of
//! `--allowedTools`) and `--effort <level>` (Den never passes it; Den strips
//! `CLAUDE_EFFORT` from a child's environment, and hook payloads carry
//! `effort: {level}`), so ainetd passes an effort only when settings.json
//! asks for one.
//!
//! A new session started from inside another one must not inherit the
//! parent's session variables, or it takes itself for a child session and
//! stops registering in `~/.claude/sessions` and saving transcripts:
//! use [`child_env`].

use std::path::Path;

use serde_json::{Value, json};

use crate::util;

/// What to start `claude` with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaunchOptions {
    /// `--session-id`: the id the new session takes (unconfirmed flag).
    pub session_id: Option<String>,
    /// `--resume`: reopen this session's conversation.
    pub resume: Option<String>,
    /// `--fork-session` (with `resume`): a copy under a new session id.
    pub fork: bool,
    /// `--model`: an alias (`haiku`, `sonnet`, `opus`) or a full model id.
    pub model: Option<String>,
    /// `--effort` (unconfirmed flag).
    pub effort: Option<String>,
    /// `--permission-mode`: `default`, `acceptEdits`, `plan`, `auto`,
    /// `bypassPermissions`.
    pub permission_mode: Option<String>,
    /// `--append-system-prompt`.
    pub append_system_prompt: Option<String>,
    /// `--mcp-config`, passed as a JSON string, e.g. [`mcp_config`].
    pub mcp_config: Option<Value>,
    /// `--strict-mcp-config`: only the servers of `mcp_config`.
    pub strict_mcp_config: bool,
    /// `--allowedTools`, comma-joined.
    pub allowed_tools: Vec<String>,
    /// `--disallowedTools`, comma-joined: tools the session may not use.
    pub disallowed_tools: Vec<String>,
    /// `--tools`, comma-joined: the only built-in tools available.
    pub tools: Vec<String>,
    /// `--name`: the session's name, which peers message it by.
    pub name: Option<String>,
    /// `--worktree <name>` (not with `resume`).
    pub worktree: Option<String>,
    /// `--add-dir`, one per directory.
    pub add_dirs: Vec<String>,
    /// Further arguments, before the prompt.
    pub extra: Vec<String>,
    /// The first prompt, after `--` (not with `resume`: a resumed
    /// conversation carries on).
    pub prompt: Option<String>,
}

fn set(o: &Option<String>) -> Option<&str> {
    o.as_deref().filter(|s| !s.is_empty())
}

/// The arguments after `claude`, in Den's order: resume (and fork) first,
/// then the name, worktree, directories, model and the rest, and the prompt
/// last after `--`.
pub fn args(o: &LaunchOptions) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let push = |a: &mut Vec<String>, flag: &str, v: &str| {
        a.push(flag.to_string());
        a.push(v.to_string());
    };
    let resume = set(&o.resume);
    if let Some(r) = resume {
        push(&mut a, "--resume", r);
        if o.fork {
            a.push("--fork-session".into());
        }
    }
    if let Some(id) = set(&o.session_id) {
        push(&mut a, "--session-id", id);
    }
    if let Some(n) = set(&o.name) {
        push(&mut a, "--name", n);
    }
    if resume.is_none()
        && let Some(w) = set(&o.worktree)
    {
        push(&mut a, "--worktree", w);
    }
    for d in o.add_dirs.iter().filter(|d| !d.is_empty()) {
        push(&mut a, "--add-dir", d);
    }
    if let Some(m) = set(&o.model) {
        push(&mut a, "--model", m);
    }
    if let Some(e) = set(&o.effort) {
        push(&mut a, "--effort", e);
    }
    if let Some(p) = set(&o.permission_mode) {
        push(&mut a, "--permission-mode", p);
    }
    if let Some(s) = set(&o.append_system_prompt) {
        push(&mut a, "--append-system-prompt", s);
    }
    if let Some(cfg) = &o.mcp_config {
        push(&mut a, "--mcp-config", &cfg.to_string());
        if o.strict_mcp_config {
            a.push("--strict-mcp-config".into());
        }
    }
    if !o.tools.is_empty() {
        push(&mut a, "--tools", &o.tools.join(","));
    }
    if !o.allowed_tools.is_empty() {
        push(&mut a, "--allowedTools", &o.allowed_tools.join(","));
    }
    if !o.disallowed_tools.is_empty() {
        push(&mut a, "--disallowedTools", &o.disallowed_tools.join(","));
    }
    a.extend(o.extra.iter().cloned());
    if resume.is_none()
        && let Some(p) = o.prompt.as_deref()
    {
        a.push("--".into());
        a.push(p.to_string());
    }
    a
}

/// `claude` and its arguments.
pub fn command(o: &LaunchOptions) -> Vec<String> {
    let mut c = vec!["claude".to_string()];
    c.extend(args(o));
    c
}

/// The arguments that reopen a session: `--resume <id>`, and
/// `--fork-session` for a copy under a new session id.
pub fn resume_args(session_id: &str, fork: bool) -> Vec<String> {
    args(&LaunchOptions {
        resume: Some(session_id.into()),
        fork,
        ..Default::default()
    })
}

/// The `--mcp-config` JSON for stdio servers: name → (command, args), as
/// `{"mcpServers":{"<name>":{"args":[…],"command":"…"}}}`.
pub fn mcp_config(servers: &[(&str, &str, &[&str])]) -> Value {
    let mut m = serde_json::Map::new();
    for (name, cmd, args) in servers {
        m.insert(name.to_string(), json!({"command": cmd, "args": args}));
    }
    json!({ "mcpServers": m })
}

/// Variables a Claude Code session sets for the processes it runs (its id,
/// its messaging socket, its child-session flag, its effort and entrypoint),
/// as prefixes of `NAME=value`. A session started with them takes itself
/// for a child and stops registering and saving transcripts. Den's own
/// markers are listed too.
pub const INHERITED_ENV_PREFIXES: &[&str] = &[
    "CLAUDECODE=",
    "CLAUDE_PID=",
    "CLAUDE_EFFORT=",
    "CLAUDE_CODE_SESSION_",
    "CLAUDE_CODE_MESSAGING_",
    "CLAUDE_CODE_CHILD_SESSION=",
    "CLAUDE_CODE_ENTRYPOINT=",
    "CLAUDE_CODE_EXECPATH=",
    "DEN_ORCHESTRATOR=",
    "DEN_PEER=",
];

/// The environment for a new session: `environ` without what a parent
/// session set ([`INHERITED_ENV_PREFIXES`]), plus `extra`.
pub fn child_env<I, K, V>(environ: I, extra: &[(String, String)]) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let mut out: Vec<(String, String)> = environ
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .filter(|(k, v)| {
            let kv = format!("{k}={v}");
            !INHERITED_ENV_PREFIXES.iter().any(|p| kv.starts_with(p))
        })
        .collect();
    out.extend(extra.iter().cloned());
    out
}

/// Whether the variable `name` must be removed from a new session's
/// environment (for `Command::env_remove`).
pub fn is_inherited_var(name: &str) -> bool {
    let kv = format!("{name}=");
    INHERITED_ENV_PREFIXES.iter().any(|p| kv.starts_with(p))
}

/// Whether Claude Code has recorded the human's answer to its "do you trust
/// this folder?" dialog for `dir` or a folder above it, from the `projects`
/// map of `.claude.json` (`<CLAUDE_CONFIG_DIR>/.claude.json`, else
/// `~/.claude.json`): `projects.<path>.hasTrustDialogAccepted`. Without it
/// a new session waits on that dialog before anything else.
pub fn trusted(dir: &str) -> bool {
    let path = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => Path::new(&d).join(".claude.json"),
        None => util::home_dir().unwrap_or_default().join(".claude.json"),
    };
    trusted_in(&path, dir)
}

/// [`trusted`] with an explicit `.claude.json`.
pub fn trusted_in(claude_json: &Path, dir: &str) -> bool {
    let Ok(raw) = std::fs::read(claude_json) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
        return false;
    };
    let Some(projects) = v.get("projects").and_then(Value::as_object) else {
        return false;
    };
    let mut d = util::clean(dir);
    loop {
        if projects
            .get(&d)
            .and_then(|p| p.get("hasTrustDialogAccepted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return true;
        }
        let parent = util::dir(&d);
        if parent == d {
            return false;
        }
        d = parent;
    }
}

/// The branch Claude Code gives a worktree called `name`.
pub fn worktree_branch(name: &str) -> String {
    format!("worktree-{name}")
}

/// Where Claude Code puts the worktree `name` of the repository at `root`.
pub fn worktree_path(root: &Path, name: &str) -> std::path::PathBuf {
    root.join(".claude").join("worktrees").join(name)
}

/// A session name no running session uses: `want` cleaned to `[a-z0-9-]`,
/// with "-2", "-3", … when taken; with no `want`, "<folder>-<4 hex>" from
/// `random` (the caller's randomness). Den's `PeerName`.
pub fn peer_name(want: &str, dir: &str, taken: &[&str], mut random: impl FnMut() -> u16) -> String {
    let taken: std::collections::HashSet<&str> = taken.iter().copied().collect();
    let base = slug(want);
    if !base.is_empty() {
        if !taken.contains(base.as_str()) {
            return base;
        }
        for i in 2.. {
            let n = format!("{base}-{i}");
            if !taken.contains(n.as_str()) {
                return n;
            }
        }
    }
    let mut base = slug(&util::base(dir));
    if base.is_empty() {
        base = "agent".into();
    }
    loop {
        let n = format!("{base}-{:04x}", random());
        if !taken.contains(n.as_str()) {
            return n;
        }
    }
}

/// Lower case with every run of characters outside `[a-z0-9]` made one "-",
/// trimmed of "-".
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

/// The orchestrator-style headless session Den runs: `-p` with stream-json
/// in and out, partial messages, verbose, a name, a tool list, an appended
/// system prompt and `--system-prompt-snapshot off` (Claude Code records a
/// session's system prompt and by default sends that record again on every
/// resume, whatever a later launch passes).
pub fn headless_args(name: &str, tools: &[&str], system_prompt: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
        "--name",
        name,
        "--tools",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(tools.join(","));
    a.extend(["--append-system-prompt".to_string(), system_prompt.to_string()]);
    a.extend(["--system-prompt-snapshot".to_string(), "off".to_string()]);
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn new_agent_like_den() {
        let cfg = mcp_config(&[("den", "/usr/bin/den", &["mcp"])]);
        assert_eq!(
            cfg.to_string(),
            r#"{"mcpServers":{"den":{"args":["mcp"],"command":"/usr/bin/den"}}}"#
        );
        let o = LaunchOptions {
            name: Some("mossy".into()),
            worktree: Some("feat".into()),
            add_dirs: vec!["/skills".into()],
            model: Some("sonnet".into()),
            mcp_config: Some(cfg.clone()),
            allowed_tools: s(&["mcp__den__spawn_agent", "mcp__den__open_terminal"]),
            prompt: Some("do it".into()),
            ..Default::default()
        };
        assert_eq!(
            command(&o),
            s(&[
                "claude",
                "--name",
                "mossy",
                "--worktree",
                "feat",
                "--add-dir",
                "/skills",
                "--model",
                "sonnet",
                "--mcp-config",
                r#"{"mcpServers":{"den":{"args":["mcp"],"command":"/usr/bin/den"}}}"#,
                "--allowedTools",
                "mcp__den__spawn_agent,mcp__den__open_terminal",
                "--",
                "do it",
            ])
        );
    }

    #[test]
    fn resume_and_fork() {
        assert_eq!(resume_args("abc", false), s(&["--resume", "abc"]));
        assert_eq!(
            resume_args("abc", true),
            s(&["--resume", "abc", "--fork-session"])
        );
        // Den: resume, fork, then the name; no worktree and no prompt.
        let o = LaunchOptions {
            resume: Some("abc".into()),
            fork: true,
            name: Some("mossy".into()),
            worktree: Some("x".into()),
            model: Some("claude-opus-5-5".into()),
            prompt: Some("ignored".into()),
            ..Default::default()
        };
        assert_eq!(
            args(&o),
            s(&[
                "--resume",
                "abc",
                "--fork-session",
                "--name",
                "mossy",
                "--model",
                "claude-opus-5-5"
            ])
        );
    }

    #[test]
    fn everything_else() {
        let o = LaunchOptions {
            session_id: Some("11111111-2222-4333-8444-555555555555".into()),
            effort: Some("high".into()),
            permission_mode: Some("plan".into()),
            append_system_prompt: Some("be brief".into()),
            mcp_config: Some(json!({"mcpServers": {}})),
            strict_mcp_config: true,
            tools: s(&["ListAgents", "SendMessage"]),
            disallowed_tools: s(&["WebFetch", "Bash(rm:*)"]),
            extra: s(&["--verbose"]),
            ..Default::default()
        };
        assert_eq!(
            args(&o),
            s(&[
                "--session-id",
                "11111111-2222-4333-8444-555555555555",
                "--effort",
                "high",
                "--permission-mode",
                "plan",
                "--append-system-prompt",
                "be brief",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
                "--strict-mcp-config",
                "--tools",
                "ListAgents,SendMessage",
                "--disallowedTools",
                "WebFetch,Bash(rm:*)",
                "--verbose",
            ])
        );
        assert!(args(&LaunchOptions::default()).is_empty());
        let h = headless_args("orch", &["ListAgents", "SendMessage"], "P");
        assert_eq!(h[0], "-p");
        assert_eq!(&h[h.len() - 2..], &s(&["--system-prompt-snapshot", "off"])[..]);
    }

    #[test]
    fn child_env_drops_the_parents_session() {
        let env = vec![
            ("PATH", "/bin"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "x"),
            ("CLAUDE_CODE_MESSAGING_SOCKET", "/s"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CONFIG_DIR", "/c"),
            ("CLAUDE_PID", "5"),
            ("CLAUDE_EFFORT", "high"),
        ];
        let out = child_env(env, &[("AINET_AGENT".into(), "a1".into())]);
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["PATH", "CLAUDE_CONFIG_DIR", "AINET_AGENT"]);
        assert!(is_inherited_var("CLAUDE_CODE_SESSION_ID"));
        assert!(is_inherited_var("CLAUDECODE"));
        assert!(!is_inherited_var("CLAUDE_CONFIG_DIR"));
        assert!(!is_inherited_var("CLAUDECODE_X"));
    }

    #[test]
    fn trust_and_names() {
        let tmp = TempDir::new();
        let cj = tmp.join(".claude.json");
        std::fs::write(&cj, r#"{"projects":{"/work":{"hasTrustDialogAccepted":true},"/other":{"hasTrustDialogAccepted":false}}}"#)
            .unwrap();
        assert!(trusted_in(&cj, "/work/proj/sub"));
        assert!(trusted_in(&cj, "/work"));
        assert!(!trusted_in(&cj, "/other/x"));
        assert!(!trusted_in(&cj, "/"));
        assert!(!trusted_in(&tmp.join("none.json"), "/work"));

        assert_eq!(peer_name("UI Designer!", "/w", &[], || 0), "ui-designer");
        assert_eq!(
            peer_name("ui designer", "/w", &["ui-designer", "ui-designer-2"], || 0),
            "ui-designer-3"
        );
        assert_eq!(peer_name("", "/home/u/My Site", &[], || 0xab), "my-site-00ab");
        assert_eq!(
            peer_name("", "/", &["agent-0001"], {
                let mut n = 0;
                move || {
                    n += 1;
                    n
                }
            }),
            "agent-0002"
        );
        assert_eq!(worktree_branch("feat"), "worktree-feat");
        assert_eq!(
            worktree_path(Path::new("/r"), "w"),
            Path::new("/r/.claude/worktrees/w")
        );
    }
}
