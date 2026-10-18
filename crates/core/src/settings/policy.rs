//! Settings that loosen permissions on purpose: rules that allow without
//! asking, and how a rig spec's `permission_policy` maps onto Claude
//! Code's permission modes. Both only take shapes and names from a closed
//! list, so a typo is an error, never a looser agent.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PermissionMode;

/// A rule that allows a request from any agent without asking, of the
/// same kinds as an `/always` answer makes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GlobalRule {
    /// `Bash` with exactly this command: one plain command, no shell
    /// operators.
    Command(String),
    /// Every call of this tool (an MCP tool by its full name,
    /// `mcp__server__tool`). Not `Bash`, `WebFetch` or the edit tools,
    /// which have their own kinds.
    Tool(String),
    /// `WebFetch` of URLs on this host.
    FetchHost(String),
    /// Edits by one edit tool of files under a directory, when the agent
    /// works in exactly that directory; sensitive files never.
    EditsUnder { tool: String, dir: String },
}

/// What a `Bash` command may not hold to take a rule (as `/always`).
const SHELL_OPERATORS: &[&str] = &[";", "&", "|", "$(", "`", "<", ">", "\n", "\r"];

/// The tools that edit files, which only `edits_under` may name.
const EDIT_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// Tools no rule may allow: each call is answered on its own.
const NEVER: &[&str] = &["ExitPlanMode", "AskUserQuestion"];

impl GlobalRule {
    /// What is wrong with the rule, if anything.
    pub fn problem(&self) -> Option<String> {
        match self {
            GlobalRule::Command(c) if c.trim().is_empty() => Some("a command rule is empty".into()),
            GlobalRule::Command(c) => {
                let ops: Vec<&str> = SHELL_OPERATORS
                    .iter()
                    .copied()
                    .filter(|op| c.contains(op))
                    .collect();
                (!ops.is_empty()).then(|| {
                    format!(
                        "the command rule {c:?} has shell operators ({}); a rule takes one plain command",
                        ops.join(" ")
                            .replace('\n', "newline")
                            .replace('\r', "carriage return")
                    )
                })
            }
            GlobalRule::Tool(t) if t.trim().is_empty() => Some("a tool rule is empty".into()),
            GlobalRule::Tool(t) if NEVER.contains(&t.as_str()) => {
                Some(format!("{t} is answered one at a time; no rule allows it"))
            }
            GlobalRule::Tool(t) if t == "Bash" => {
                Some("a tool rule cannot allow every Bash command; use a command rule".into())
            }
            GlobalRule::Tool(t) if t == "WebFetch" => {
                Some("a tool rule cannot allow every WebFetch; use a fetch_host rule".into())
            }
            GlobalRule::Tool(t) if EDIT_TOOLS.contains(&t.as_str()) => Some(format!(
                "a tool rule cannot allow every {t}; use an edits_under rule"
            )),
            GlobalRule::Tool(t)
                if t.starts_with("mcp__") && t.trim_start_matches("mcp__").split("__").any(str::is_empty) =>
            {
                Some(format!(
                    "{t:?} is not an MCP tool's full name (mcp__server__tool)"
                ))
            }
            GlobalRule::Tool(t)
                if t.starts_with("mcp__") && !t.trim_start_matches("mcp__").contains("__") =>
            {
                Some(format!(
                    "{t:?} names a whole MCP server; a rule allows one of its tools"
                ))
            }
            GlobalRule::FetchHost(h)
                if h.trim().is_empty() || h.contains(['/', ':', ' ', '*']) || h.to_lowercase() != *h =>
            {
                Some(format!("{h:?} is not a lower-case host name"))
            }
            GlobalRule::EditsUnder { tool, .. } if !EDIT_TOOLS.contains(&tool.as_str()) => Some(format!(
                "edits_under names {tool:?}; it must be one of {}",
                EDIT_TOOLS.join(", ")
            )),
            GlobalRule::EditsUnder { dir, .. }
                if !dir.starts_with('/') || dir.trim_end_matches('/').is_empty() =>
            {
                Some(format!(
                    "edits_under needs an absolute directory below /, not {dir:?}"
                ))
            }
            GlobalRule::EditsUnder { dir, .. } if dir.split('/').any(|p| p == "..") => {
                Some(format!("edits_under's directory {dir:?} has a `..`"))
            }
            _ => None,
        }
    }
}

/// The `permission_policy` values ainet knows, and the Claude Code
/// permission mode each maps to (`None`: Claude Code's own default).
pub const SPEC_POLICIES: &[(&str, Option<PermissionMode>)] = &[
    ("builtin:locked", Some(PermissionMode::Plan)),
    ("builtin:default", None),
    ("builtin:standard", None),
    ("builtin:yolo", Some(PermissionMode::BypassPermissions)),
];

/// The permission mode a spec's `permission_policy` asks for: `Ok(None)`
/// for Claude Code's default, `Err` for a policy ainet does not know
/// (reported and ignored, never guessed at).
pub fn spec_policy(policy: &str) -> Result<Option<PermissionMode>, String> {
    let p = policy.trim();
    SPEC_POLICIES
        .iter()
        .find(|(name, _)| *name == p)
        .map(|(_, mode)| *mode)
        .ok_or_else(|| {
            let known: Vec<&str> = SPEC_POLICIES.iter().map(|(n, _)| *n).collect();
            format!(
                "permission_policy {p:?} is not one ainet knows ({}); ignored",
                known.join(", ")
            )
        })
}
