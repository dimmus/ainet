//! What a `PermissionRequest` hook prints for a decision. Byte-compatible
//! with Den's `den hook` (Go's `json.Marshal`: keys sorted, `<`, `>`, `&`
//! escaped, raw tool input compacted but otherwise kept byte for byte).
//!
//! Undocumented behaviour, checked by Den against Claude Code 2.1.289:
//!
//! - A plain decision is
//!   `{"hookSpecificOutput":{"decision":{"behavior":"allow","message":""},"hookEventName":"PermissionRequest"}}`
//!   (Den always includes `message`, empty for an allow).
//! - Claude Code ignores a bare allow for a tool that needs the user's
//!   interaction (`ExitPlanMode`, `AskUserQuestion`) and keeps its dialog
//!   open. An allow that carries `updatedInput` is taken. For a plan,
//!   `updatedInput` is the call's own `tool_input`, unchanged (it must hold a
//!   non-empty `plan`); Claude Code then leaves plan mode for the mode the
//!   session had before — what the terminal dialog calls "Yes, manually
//!   approve edits". A deny with a message sends the plan back (the message
//!   becomes the tool's error) and the session stays in plan mode.
//! - An `AskUserQuestion` call is answered with allow plus `updatedInput` =
//!   the call's input with an `answers` object added, keyed by question text
//!   (see [`crate::question`]).
//! - Printing nothing (and exiting 0) is "no decision": the terminal dialog
//!   stays.

use std::collections::BTreeMap;

use serde_json::value::RawValue;

use super::HookInput;
use crate::util;

/// The tool Claude Code asks with when a plan is ready (permission mode
/// `plan`).
pub const PLAN_TOOL: &str = "ExitPlanMode";

/// The message ainet denies with.
pub const DENY_MESSAGE: &str = "Denied from ainet by the user.";

/// The message Den denies with, for reference.
pub const DEN_DENY_MESSAGE: &str = "Denied from den by the user.";

/// The message of an allow given by an earlier /always rule (Den's is
/// "den: allowed by your earlier always-allow").
pub const ALWAYS_MESSAGE: &str = "ainet: allowed by your earlier always-allow";

/// A decision on a held `PermissionRequest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// No decision: print nothing; Claude Code's own dialog stays.
    NoDecision,
    /// Allow the call. For `ExitPlanMode` this is a plan approval (a bare
    /// allow would be ignored), as in Den.
    Allow { message: String },
    /// Deny the call; the message is what the agent reads.
    Deny { message: String },
    /// Approve a plan: allow with the call's own input as `updatedInput`.
    ApprovePlan,
    /// Send a plan back to keep planning, with a note (a deny).
    RejectPlan { note: String },
    /// Answer an `AskUserQuestion` call: answers keyed by question text,
    /// every question answered (multi-select answers already joined, see
    /// [`crate::question::join`]).
    Answer(BTreeMap<String, String>),
}

impl Decision {
    pub fn allow() -> Decision {
        Decision::Allow {
            message: String::new(),
        }
    }

    pub fn deny() -> Decision {
        Decision::Deny {
            message: DENY_MESSAGE.into(),
        }
    }
}

/// The line the hook prints for `decision` on a call of `tool_name` with
/// `tool_input` (raw JSON as Claude Code sent it), newline included; `None`
/// to print nothing (no decision, or a decision Claude Code would not take:
/// a plan without a plan in its input, an incomplete answer).
pub fn permission_output(tool_name: &str, tool_input: &str, decision: &Decision) -> Option<String> {
    let line = match decision {
        Decision::NoDecision => return None,
        Decision::Allow { .. } if tool_name == PLAN_TOOL => approve_plan(tool_input)?,
        Decision::Allow { message } => plain("allow", message),
        Decision::Deny { message } => plain("deny", message),
        Decision::ApprovePlan => approve_plan(tool_input)?,
        Decision::RejectPlan { note } => plain("deny", note),
        Decision::Answer(answers) => {
            if tool_name != crate::question::TOOL {
                return None;
            }
            return crate::question::output(tool_input, answers).map(|s| s + "\n");
        }
    };
    Some(line + "\n")
}

/// [`permission_output`] for a parsed payload.
pub fn permission_output_for(input: &HookInput, decision: &Decision) -> Option<String> {
    permission_output(&input.tool_name, input.tool_input_raw(), decision)
}

/// [`permission_output`] as a JSON value (without the byte-level
/// guarantees).
pub fn permission_output_value(
    tool_name: &str,
    tool_input: &str,
    decision: &Decision,
) -> Option<serde_json::Value> {
    serde_json::from_str(&permission_output(tool_name, tool_input, decision)?).ok()
}

/// The `updatedInput` that approves a plan: the call's own `tool_input`,
/// unchanged, as compact JSON; `None` when it is not a JSON object with a
/// non-empty `plan` string (then the plan cannot be approved from outside
/// the terminal). Den's `bridge.PlanApproval`.
pub fn plan_approval(tool_input: &str) -> Option<String> {
    let fields: BTreeMap<String, Box<RawValue>> = serde_json::from_str(tool_input).ok()?;
    let plan: String = serde_json::from_str(fields.get("plan")?.get()).ok()?;
    if plan.is_empty() {
        return None;
    }
    Some(util::compact_json(&serde_json::to_string(&fields).ok()?))
}

fn plain(behavior: &str, message: &str) -> String {
    util::go_escape(&format!(
        "{{\"hookSpecificOutput\":{{\"decision\":{{\"behavior\":\"{behavior}\",\"message\":{}}},\"hookEventName\":\"PermissionRequest\"}}}}",
        serde_json::to_string(message).unwrap_or_else(|_| "\"\"".into())
    ))
}

fn approve_plan(tool_input: &str) -> Option<String> {
    Some(allow_updated(&plan_approval(tool_input)?))
}

/// `{"hookSpecificOutput":{"decision":{"behavior":"allow","updatedInput":…},"hookEventName":"PermissionRequest"}}`
/// for a compact JSON object `updated`.
pub(crate) fn allow_updated(updated: &str) -> String {
    util::go_escape(&format!(
        "{{\"hookSpecificOutput\":{{\"decision\":{{\"behavior\":\"allow\",\"updatedInput\":{updated}}},\"hookEventName\":\"PermissionRequest\"}}}}"
    ))
}
