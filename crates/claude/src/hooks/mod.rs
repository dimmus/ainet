//! Claude Code hooks: the payloads Claude Code pipes to a hook command, the
//! decisions a `PermissionRequest` hook prints, and installing the hook
//! command into `settings.json`.
//!
//! Ported from Den's `internal/bridge` (the hook input and plan approval),
//! `cmd/den/hook.go` (what the hook prints), `internal/events` (what each
//! event means) and `internal/hooks` (install), all MIT.
//!
//! # The hook contract, as Den runs it
//!
//! Claude Code runs the hook command once per subscribed event with the
//! payload as one JSON object on stdin (see [`HookInput`]). Den's hook:
//!
//! - reads all of stdin; empty input, input that is not JSON, or no
//!   `hook_event_name` means do nothing;
//! - skips payloads with a non-empty `agent_id` (a subagent's events);
//! - for every event but `PermissionRequest`, forwards the payload and
//!   prints nothing;
//! - for `PermissionRequest`, holds the request open (up to
//!   [`PERMISSION_WAIT_SECS`], inside the hook's own
//!   [`PERMISSION_TIMEOUT_SECS`] timeout) while the human decides, then
//!   prints one decision line ([`permission_output`]) or nothing;
//! - **always exits 0** ([`EXIT_CODE`]), even on a panic: Claude Code reads
//!   exit code 2 as blocking, which for `PermissionRequest` is a denial.
//!   Printing nothing leaves Claude Code's own dialog in charge.
//!
//! Claude Code shows its own permission dialog at once, beside the hook,
//! not after it: whichever answers first counts, and Claude Code ends (kills)
//! the hook when its dialog is answered or dismissed in the terminal. A hook
//! that hangs up before the wait ran out therefore means "answered in the
//! terminal".
//!
//! Observed with Claude Code 2.1.289 (Den's `docs/EVENTS.md`): payloads also
//! carry `prompt_id` and `effort`; `SessionStart` and `UserPromptSubmit`
//! carry `session_title`; `PostToolUse` carries `duration_ms`; `Stop`
//! carries `background_tasks` and `session_crons`. A `PermissionRequest` has
//! no `tool_use_id` (its `PreToolUse` and `PostToolUse` do), so a held call
//! is matched to its `PostToolUse` by tool name and input; an `ExitPlanMode`
//! `PostToolUse` comes with an empty `tool_input` (the plan is in
//! `tool_response`). A `Notification` of type `permission_prompt` follows a
//! `PermissionRequest` by about 6 s while the hook still holds (the same
//! wait); `idle_prompt` follows `Stop` by about 5 s; `AskUserQuestion` fires
//! `PreToolUse` about 60 ms before its `PermissionRequest`.

mod install;
mod output;

pub use install::{
    HookSpec, HookStatus, InstallReport, PROGRAM, SPECS, UninstallReport, hook_command, hooks_block, install,
    install_with, installed, is_ours, permission_timeout, quote, settings_path, settings_path_in, status,
    uninstall,
};
pub use output::{
    ALWAYS_MESSAGE, DEN_DENY_MESSAGE, DENY_MESSAGE, Decision, PLAN_TOOL, permission_output,
    permission_output_for, permission_output_value, plan_approval,
};

pub(crate) use output::allow_updated as output_allow_updated;

use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

/// What the hook process must exit with, always: 0. Exit 2 is "blocking",
/// which Claude Code reads as a denial of a `PermissionRequest`.
pub const EXIT_CODE: i32 = 0;

/// How long Den holds a permission request for the human by default
/// (`hooks.permission_wait`), seconds. It must stay below
/// [`PERMISSION_TIMEOUT_SECS`].
pub const PERMISSION_WAIT_SECS: u64 = 580;

/// The `PermissionRequest` hook's timeout in `settings.json`, seconds.
pub const PERMISSION_TIMEOUT_SECS: u64 = 600;

/// Den's limit for one line on its socket (1 MiB); [`fit_payload`] keeps
/// 4096 bytes of it for the envelope.
pub const MAX_LINE: usize = 1 << 20;

/// The hook events Den installs and reads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    Notification,
    Stop,
    PostToolUse,
    /// Installed with matcher `AskUserQuestion` only.
    PreToolUse,
    PermissionRequest,
    StopFailure,
    PostToolUseFailure,
    Other(String),
}

impl HookEvent {
    pub fn from_name(name: &str) -> HookEvent {
        match name {
            "SessionStart" => HookEvent::SessionStart,
            "SessionEnd" => HookEvent::SessionEnd,
            "UserPromptSubmit" => HookEvent::UserPromptSubmit,
            "Notification" => HookEvent::Notification,
            "Stop" => HookEvent::Stop,
            "PostToolUse" => HookEvent::PostToolUse,
            "PreToolUse" => HookEvent::PreToolUse,
            "PermissionRequest" => HookEvent::PermissionRequest,
            "StopFailure" => HookEvent::StopFailure,
            "PostToolUseFailure" => HookEvent::PostToolUseFailure,
            other => HookEvent::Other(other.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Notification => "Notification",
            HookEvent::Stop => "Stop",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PermissionRequest => "PermissionRequest",
            HookEvent::StopFailure => "StopFailure",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::Other(s) => s,
        }
    }
}

/// The fields of Claude Code's hook JSON that Den reads, plus the extra
/// ones 2.1.289 was seen to send. Strings that are absent or `null` are
/// empty. Raw fields keep the payload's own bytes (key order included), which
/// a plan approval and a question answer echo back.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HookInput {
    #[serde(deserialize_with = "de_str")]
    pub session_id: String,
    #[serde(deserialize_with = "de_str")]
    pub cwd: String,
    #[serde(deserialize_with = "de_str")]
    pub hook_event_name: String,
    /// `default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`, …
    #[serde(deserialize_with = "de_str")]
    pub permission_mode: String,
    #[serde(deserialize_with = "de_str")]
    pub transcript_path: String,
    /// Set on a subagent's events; Den ignores those.
    #[serde(deserialize_with = "de_str")]
    pub agent_id: String,
    #[serde(deserialize_with = "de_str")]
    pub tool_name: String,
    pub tool_input: Option<Box<RawValue>>,
    pub tool_response: Option<Box<RawValue>>,
    #[serde(deserialize_with = "de_str")]
    pub tool_use_id: String,
    #[serde(deserialize_with = "de_str")]
    pub permission_reason: String,
    pub permission_suggestions: Option<Box<RawValue>>,
    /// Notification: `permission_prompt`, `idle_prompt`, `agent_needs_input`,
    /// `elicitation_dialog`, `elicitation_url_dialog`, …
    #[serde(deserialize_with = "de_str")]
    pub notification_type: String,
    #[serde(deserialize_with = "de_str")]
    pub message: String,
    #[serde(deserialize_with = "de_str")]
    pub title: String,
    /// Stop: the reply that ended the turn.
    #[serde(deserialize_with = "de_str")]
    pub last_assistant_message: String,
    #[serde(deserialize_with = "de_str")]
    pub stop_reason: String,
    #[serde(deserialize_with = "de_bool")]
    pub stop_hook_active: bool,
    /// UserPromptSubmit: the prompt.
    #[serde(deserialize_with = "de_str")]
    pub prompt: String,
    #[serde(deserialize_with = "de_str")]
    pub prompt_id: String,
    /// SessionStart: `startup`, `resume`, `clear` or `compact`.
    #[serde(deserialize_with = "de_str")]
    pub source: String,
    #[serde(deserialize_with = "de_str")]
    pub session_title: String,
    /// SessionEnd: why (`prompt_input_exit`, `logout`, `clear`, `other`).
    /// Claude Code 2.1.289 sends `reason`; `end_reason` is read too.
    #[serde(deserialize_with = "de_str")]
    pub reason: String,
    #[serde(deserialize_with = "de_str")]
    pub end_reason: String,
    /// StopFailure: the documented error type.
    #[serde(deserialize_with = "de_str")]
    pub error_type: String,
    /// StopFailure: what 2.1.289 actually sends (a string such as
    /// `"rate_limit"`). Raw, since another event's error may have another
    /// shape.
    pub error: Option<Box<RawValue>>,
    /// PostToolUseFailure: the human interrupted the tool (Esc), which is
    /// not the tool failing.
    #[serde(deserialize_with = "de_bool")]
    pub is_interrupt: bool,
    pub effort: Option<serde_json::Value>,
    pub duration_ms: Option<serde_json::Value>,
}

fn de_str<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

fn de_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or_default())
}

impl HookInput {
    /// Parses a hook payload. `None` for input that is not a JSON object of
    /// the expected shape or has no `hook_event_name`: the hook then does
    /// nothing.
    pub fn parse(raw: &str) -> Option<HookInput> {
        let input: HookInput = serde_json::from_str(raw).ok()?;
        (!input.hook_event_name.is_empty()).then_some(input)
    }

    pub fn event(&self) -> HookEvent {
        HookEvent::from_name(&self.hook_event_name)
    }

    /// A subagent's event (non-empty `agent_id`): not a fleet member.
    pub fn is_subagent(&self) -> bool {
        !self.agent_id.is_empty()
    }

    /// The tool input as raw JSON text, "" when absent.
    pub fn tool_input_raw(&self) -> &str {
        self.tool_input.as_deref().map(RawValue::get).unwrap_or("")
    }

    /// The tool input parsed, `Null` when absent or not JSON.
    pub fn tool_input_value(&self) -> serde_json::Value {
        serde_json::from_str(self.tool_input_raw()).unwrap_or_default()
    }

    /// Why a session ended, "" when Claude Code gave no reason.
    pub fn session_end_reason(&self) -> &str {
        if self.reason.is_empty() {
            &self.end_reason
        } else {
            &self.reason
        }
    }

    /// A StopFailure's error type: `rate_limit`, `server_error`,
    /// `authentication_failed`, … ; `unknown` when none came.
    pub fn failure_type(&self) -> String {
        if !self.error_type.is_empty() {
            return self.error_type.clone();
        }
        if let Some(e) = &self.error
            && let Ok(s) = serde_json::from_str::<String>(e.get())
            && !s.is_empty()
        {
            return s;
        }
        "unknown".into()
    }

    /// What the event means for the human, as Den's event stream
    /// (`events.FromHook`) reads it; `None` for events that add nothing
    /// (a `permission_prompt` notice follows a request that already said it;
    /// the `PermissionRequest` of an `AskUserQuestion` follows its
    /// `PreToolUse`, which already reported the question).
    pub fn stream_event(&self) -> Option<StreamEvent> {
        if self.session_id.is_empty() {
            return None;
        }
        let mut ev = StreamEvent {
            kind: StreamKind::Done,
            reason: None,
            request: None,
            tool: None,
            error: None,
        };
        let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
        match self.event() {
            HookEvent::SessionStart => {
                ev.kind = StreamKind::SessionStart;
                ev.reason = some(&self.source);
            }
            HookEvent::SessionEnd => {
                ev.kind = StreamKind::SessionEnd;
                ev.reason = some(self.session_end_reason());
            }
            HookEvent::UserPromptSubmit => ev.kind = StreamKind::TurnStart,
            HookEvent::Stop => ev.kind = StreamKind::Done,
            HookEvent::StopFailure => {
                ev.kind = StreamKind::TurnFailed;
                ev.error = Some(self.failure_type());
            }
            HookEvent::PostToolUseFailure => {
                ev.kind = StreamKind::ToolFailed;
                ev.tool = some(&self.tool_name);
            }
            HookEvent::PermissionRequest => {
                if self.tool_name == crate::question::TOOL {
                    return None;
                }
                ev.kind = StreamKind::NeedsYou;
                if self.tool_name == PLAN_TOOL {
                    ev.request = Some(RequestKind::Plan);
                } else {
                    ev.request = Some(RequestKind::Permission);
                    ev.tool = some(&self.tool_name);
                }
            }
            HookEvent::PreToolUse => {
                if self.tool_name != crate::question::TOOL {
                    return None;
                }
                ev.kind = StreamKind::NeedsYou;
                ev.request = Some(RequestKind::Question);
            }
            HookEvent::Notification => match self.notification_type.as_str() {
                "idle_prompt" => ev.kind = StreamKind::Idle,
                "agent_needs_input" | "elicitation_dialog" | "elicitation_url_dialog" => {
                    ev.kind = StreamKind::NeedsYou;
                    ev.request = Some(RequestKind::Question);
                }
                _ => return None,
            },
            _ => return None,
        }
        Some(ev)
    }
}

/// One entry of Den's event stream (`docs/EVENTS.md`, schema version 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    pub kind: StreamKind,
    /// session_start: `startup`/`resume`/`clear`/`compact`; session_end: why.
    pub reason: Option<String>,
    pub request: Option<RequestKind>,
    pub tool: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    SessionStart,
    SessionEnd,
    TurnStart,
    Done,
    NeedsYou,
    Idle,
    TurnFailed,
    ToolFailed,
}

impl StreamKind {
    /// The `kind` string of Den's stream.
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::SessionStart => "session_start",
            StreamKind::SessionEnd => "session_end",
            StreamKind::TurnStart => "turn_start",
            StreamKind::Done => "done",
            StreamKind::NeedsYou => "needs_you",
            StreamKind::Idle => "idle",
            StreamKind::TurnFailed => "turn_failed",
            StreamKind::ToolFailed => "tool_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Permission,
    Plan,
    Question,
}

impl RequestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestKind::Permission => "permission",
            RequestKind::Plan => "plan",
            RequestKind::Question => "question",
        }
    }
}

/// Makes a hook payload fit a line limit (`max_line`, e.g. [`MAX_LINE`]):
/// an event that is only followed loses `tool_input` and `tool_response`;
/// a permission request is never cut, since it must be shown whole: `None`,
/// and the hook leaves it to the terminal. Den's `fitLine`.
pub fn fit_payload(raw: &str, event: &str, max_line: usize) -> Option<String> {
    let room = max_line.saturating_sub(4096);
    if raw.len() <= room {
        return Some(raw.to_string());
    }
    if event == "PermissionRequest" {
        return None;
    }
    let mut m: std::collections::BTreeMap<String, Box<RawValue>> = serde_json::from_str(raw).ok()?;
    m.remove("tool_input");
    m.remove("tool_response");
    let out = serde_json::to_string(&m).ok()?;
    (out.len() <= room).then_some(out)
}

/// Why a turn ended, from a StopFailure error type (Den's inbox title).
pub fn failure_title(error_type: &str) -> String {
    match error_type {
        "rate_limit" => "Turn ended: rate limited".into(),
        "overloaded" => "Turn ended: the API is overloaded".into(),
        "authentication_failed" | "oauth_org_not_allowed" | "cloud_credential_error" => {
            "Turn ended: not logged in or credentials failed".into()
        }
        "billing_error" | "account_on_hold" => "Turn ended: a billing or account problem".into(),
        "max_output_tokens" => "Turn ended: the reply hit the output limit".into(),
        "" | "unknown" => "Turn ended on an API error".into(),
        other => format!("Turn ended on an API error: {}", other.replace('_', " ")),
    }
}

/// An assistant reply as one plain sentence for a "done" notice: no code
/// blocks, no headings or markdown furniture, cut at the first sentence end
/// (after at least 12 bytes), at most 160 characters. Den's
/// `inbox.Summarize`.
pub fn summarize_reply(text: &str) -> String {
    let text = crate::sanitize::text(text);
    let text = text.trim();
    if text.is_empty() {
        return "Turn finished.".into();
    }
    let mut kept = Vec::new();
    let mut in_fence = false;
    for l in text.split('\n') {
        let t = l.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || t.starts_with('#') {
            continue;
        }
        let t = t.trim_start_matches(['>', '-', '*', '•', ' ']);
        let t = t.replace("**", "").replace("__", "").replace('`', "");
        let t = t.trim();
        if !t.is_empty() {
            kept.push(t.to_string());
        }
    }
    if kept.is_empty() {
        return "Turn finished.".into();
    }
    let mut flat = kept.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    let b = flat.as_bytes();
    for i in 0..b.len().saturating_sub(1) {
        if matches!(b[i], b'.' | b'!' | b'?') && b[i + 1] == b' ' && i >= 12 {
            flat.truncate(i + 1);
            break;
        }
    }
    if flat.chars().count() > 160 {
        let cut: String = flat.chars().take(159).collect();
        flat = format!("{}…", cut.trim());
    }
    flat
}

#[cfg(test)]
mod tests;
