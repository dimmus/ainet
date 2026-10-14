//! Claude Code hook events → agent state and inbox items, and the answers
//! to held requests, byte-compatible with Den (`ainet_claude::hooks`).
//!
//! `ainet hook` forwards each hook's JSON payload. Seats ainet launches get
//! ainet's hooks through `--settings`, marked `_ainet_source: "seat"`; if
//! you also installed the hooks globally, the unmarked copy of an event from
//! such a seat is ignored. Any other Claude Code session with the hooks
//! installed shows up too, keyed by its session id.
//!
//! A `PermissionRequest` is held until you answer in the Inbox: allow, deny,
//! always (Den's `/always`: the same again this session), a plan approval
//! (Claude Code needs the plan's own call back, not a bare allow), or the
//! answers to an `AskUserQuestion`, one question at a time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ainet_claude::hooks::Decision as ClaudeDecision;
use ainet_claude::question::{self, Question};
use ainet_core::{AgentState, Decision, InboxItem, InboxKind, Response, Runtime, now_ms};
use serde_json::Value;

use crate::agents::new_agent;
use crate::{Daemon, PERMISSION_HOLD, Shared, err, lock};

/// What a held hook is sent when you answer.
pub(crate) enum HookAnswer {
    Decide(Decision),
    Answers(BTreeMap<String, String>),
}

/// A held `PermissionRequest`, kept to build its answer.
pub(crate) struct Held {
    pub agent: String,
    pub tool: String,
    pub input_raw: String,
    pub cwd: String,
    pub questions: Vec<Question>,
    pub answers: BTreeMap<String, String>,
    pub tx: tokio::sync::oneshot::Sender<HookAnswer>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// First line, at most `max` characters.
fn line(s: &str, max: usize) -> String {
    let first = s.trim().lines().next().unwrap_or("");
    if first.chars().count() > max {
        let cut: String = first.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        first.to_string()
    }
}

/// "Bash  npm run build", "Edit  src/main.rs", "WebFetch  example.com".
fn tool_summary(payload: &Value) -> (String, String) {
    let tool = text(payload, "tool_name").unwrap_or_else(|| "tool".into());
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    let target = ["command", "file_path", "url", "pattern", "path", "description"]
        .iter()
        .find_map(|k| text(&input, k))
        .unwrap_or_default();
    let detail = match (
        input.get("command").and_then(Value::as_str),
        input.get("plan").and_then(Value::as_str),
    ) {
        (Some(cmd), _) => cmd.to_string(),
        (_, Some(plan)) => plan.to_string(),
        _ => serde_json::to_string_pretty(&input).unwrap_or_default(),
    };
    (format!("{tool}  {}", line(&target, 80)), detail)
}

/// What the Fleet shows from a Claude transcript.
pub(crate) struct Summary {
    pub context: Option<f32>,
    pub model: Option<String>,
    pub last_tool: Option<String>,
}

pub(crate) fn summarize(path: &Path) -> Option<Summary> {
    if !path.is_file() {
        return None;
    }
    let usage = ainet_claude::transcript::read_usage(path);
    let facts = ainet_claude::transcript::read_facts(path);
    let last_tool = ainet_claude::transcript::tail(path, 8)
        .into_iter()
        .rev()
        .find(|e| e.kind == ainet_claude::transcript::EntryKind::Tool)
        .map(|e| format!("{}  {}", e.tool, line(&e.code, 80)));
    let model = Some(facts.model)
        .filter(|m| !m.is_empty())
        .or_else(|| usage.as_ref().map(|u| u.model.clone()).filter(|m| !m.is_empty()));
    Some(Summary {
        context: usage.map(|u| u.percent() as f32 / 100.0),
        model: model.map(|m| ainet_claude::transcript::pretty_model(&m)),
        last_tool,
    })
}

fn question_item(agent: &str, q: &Question, step: usize, total: usize) -> InboxItem {
    let of = if total > 1 {
        format!(" (question {} of {total})", step + 1)
    } else {
        String::new()
    };
    InboxItem {
        id: 0,
        agent: agent.to_string(),
        kind: InboxKind::Question,
        summary: format!("{}{of}", line(&q.text, 100)),
        detail: if q.header.is_empty() {
            q.text.clone()
        } else {
            format!("{}\n\n{}", q.header, q.text)
        },
        created: now_ms(),
        waiting: true,
        options: q
            .options
            .iter()
            .map(|o| {
                if o.description.is_empty() {
                    o.label.clone()
                } else {
                    format!("{} — {}", o.label, o.description)
                }
            })
            .collect(),
        multi_select: q.multi,
    }
}

pub(crate) async fn handle(
    d: &Shared,
    agent: Option<String>,
    payload: Value,
    tmux_pane: Option<String>,
) -> Response {
    let event = text(&payload, "hook_event_name").unwrap_or_default();
    let Some(id) = agent.or_else(|| text(&payload, "session_id")) else {
        return err("hook payload has no session_id");
    };
    let from_seat = text(&payload, "_ainet_source").as_deref() == Some("seat");

    let held = {
        let mut d = lock(d);
        // A seat ainet launched gets its own hooks; your global ones would
        // report the same event twice.
        if d.panes.contains_key(&id) && !from_seat && d.agents.get(&id).is_some_and(|a| a.seat.is_some()) {
            return Response::HookReply { output: None };
        }
        let now = now_ms();
        d.store.log(now, Some(&id), &event, &payload);
        let cwd = text(&payload, "cwd").unwrap_or_default();
        if !d.agents.contains_key(&id) {
            // A session we did not start: track it without a pane.
            let mut a = new_agent(&id, Runtime::ClaudeCode, Path::new(&cwd));
            a.state = AgentState::Idle;
            a.writable = true;
            d.agents.insert(id.clone(), a);
        }
        d.touch(&id);
        if let Some(a) = d.agents.get_mut(&id) {
            if let Some(t) = text(&payload, "transcript_path") {
                a.transcript = Some(PathBuf::from(t));
            }
            if let Some(m) = text(&payload, "model") {
                a.model = Some(ainet_claude::transcript::pretty_model(&m));
            }
        }
        if let Some(pane) = tmux_pane.filter(|_| !d.panes.contains_key(&id)) {
            tracing::debug!("{id} runs in tmux pane {pane}");
        }

        let mut held = None;
        match event.as_str() {
            "SessionStart" => d.set_state(&id, AgentState::Idle),
            "UserPromptSubmit" => {
                d.release_held(&id);
                d.set_state(&id, AgentState::Working);
                if let (Some(a), Some(prompt)) = (d.agents.get_mut(&id), text(&payload, "prompt"))
                    && a.task.is_none()
                    && !prompt.starts_with("[ainet]")
                {
                    a.task = Some(line(&prompt, 72));
                }
            }
            "PreToolUse" if text(&payload, "tool_name").as_deref() == Some(question::TOOL) => {
                d.set_state(&id, AgentState::NeedsYou);
            }
            "PreToolUse" | "PostToolUse" | "PostToolUseFailure" => {
                if event != "PreToolUse" {
                    // The tool ran, so any request about it was answered.
                    d.release_held(&id);
                }
                d.set_state(&id, AgentState::Working);
                let (summary, _) = tool_summary(&payload);
                if let Some(a) = d.agents.get_mut(&id) {
                    a.last_tool = Some(summary);
                }
            }
            "Notification" => d.set_state(&id, AgentState::NeedsYou),
            "PermissionRequest" => {
                let tool = text(&payload, "tool_name").unwrap_or_default();
                let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
                let input_raw = input.to_string();
                // An earlier /always covers it: allow without asking.
                let covered = d
                    .always
                    .get(&id)
                    .is_some_and(|rules| rules.iter().any(|r| r.matches(&tool, &input, &cwd)));
                if covered {
                    let decision = ClaudeDecision::Allow {
                        message: ainet_claude::hooks::ALWAYS_MESSAGE.into(),
                    };
                    d.publish(&id);
                    return Response::HookReply {
                        output: ainet_claude::hooks::permission_output_value(&tool, &input_raw, &decision),
                    };
                }
                let questions = if tool == question::TOOL {
                    question::parse(&input_raw).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let item = if let Some(q) = questions.first() {
                    question_item(&id, q, 0, questions.len())
                } else {
                    let (summary, detail) = tool_summary(&payload);
                    InboxItem {
                        id: 0,
                        agent: id.clone(),
                        kind: if tool == "ExitPlanMode" {
                            InboxKind::Plan
                        } else {
                            InboxKind::Permission
                        },
                        summary,
                        detail,
                        created: now,
                        waiting: true,
                        options: vec![],
                        multi_select: false,
                    }
                };
                let item_id = d.add_item(item);
                let (tx, rx) = tokio::sync::oneshot::channel();
                d.held_req.insert(
                    item_id,
                    Held {
                        agent: id.clone(),
                        tool: tool.clone(),
                        input_raw: input_raw.clone(),
                        cwd: cwd.clone(),
                        questions,
                        answers: BTreeMap::new(),
                        tx,
                    },
                );
                d.set_state(&id, AgentState::NeedsYou);
                held = Some((item_id, rx, tool, input_raw));
            }
            "Stop" => {
                d.release_held(&id);
                d.set_state(&id, AgentState::Idle);
                let msg = text(&payload, "last_assistant_message").unwrap_or_default();
                d.add_item(InboxItem {
                    id: 0,
                    agent: id.clone(),
                    kind: InboxKind::Finished,
                    summary: ainet_claude::hooks::summarize_reply(&msg),
                    detail: msg,
                    created: now,
                    waiting: false,
                    options: vec![],
                    multi_select: false,
                });
            }
            "StopFailure" => {
                d.set_state(&id, AgentState::Failed);
                let err_type = text(&payload, "error_type")
                    .or_else(|| text(&payload, "error"))
                    .unwrap_or_default();
                d.add_item(InboxItem {
                    id: 0,
                    agent: id.clone(),
                    kind: InboxKind::Failed,
                    summary: ainet_claude::hooks::failure_title(&err_type),
                    detail: String::new(),
                    created: now,
                    waiting: false,
                    options: vec![],
                    multi_select: false,
                });
            }
            "SessionEnd" => {
                d.release_held(&id);
                d.always.remove(&id);
                if !d.panes.contains_key(&id) {
                    d.set_state(&id, AgentState::Gone);
                }
            }
            _ => {}
        }
        d.publish(&id);
        held
    };

    let Some((item, rx, tool, input_raw)) = held else {
        return Response::HookReply { output: None };
    };
    // Hold the hook open until you answer; on timeout or release, print
    // nothing and Claude Code's own dialog stays in charge.
    let answer = match tokio::time::timeout(PERMISSION_HOLD, rx).await {
        Ok(Ok(a)) => a,
        _ => {
            let mut g = lock(d);
            g.held_req.remove(&item);
            g.resolve(item);
            return Response::HookReply { output: None };
        }
    };
    let decision = match answer {
        HookAnswer::Decide(Decision::Deny) if tool == "ExitPlanMode" => ClaudeDecision::RejectPlan {
            note: "Your human sent the plan back from ainet: keep planning.".into(),
        },
        HookAnswer::Decide(Decision::Deny) => ClaudeDecision::Deny {
            message: "Denied from ainet by your human.".into(),
        },
        HookAnswer::Decide(_) if tool == "ExitPlanMode" => ClaudeDecision::ApprovePlan,
        HookAnswer::Decide(_) => ClaudeDecision::allow(),
        HookAnswer::Answers(a) => ClaudeDecision::Answer(a),
    };
    Response::HookReply {
        output: ainet_claude::hooks::permission_output_value(&tool, &input_raw, &decision),
    }
}

/// ^Y / ^A / ^N on a held request.
pub(crate) fn answer(d: &Shared, item: u64, decision: Decision) -> Response {
    let mut g = lock(d);
    let Some(held) = g.held_req.remove(&item) else {
        return err("that request is no longer waiting");
    };
    let mut note = None;
    if decision == Decision::Always {
        let input: Value = serde_json::from_str(&held.input_raw).unwrap_or(Value::Null);
        match ainet_claude::always::derive(&held.tool, &input, &held.cwd) {
            Ok(rule) => {
                note = Some(format!("always allowing {} for this session", rule.scope()));
                g.always.entry(held.agent.clone()).or_default().push(rule);
            }
            Err(refusal) => note = Some(format!("allowed once; no always rule: {refusal:?}")),
        }
    }
    g.resolve(item);
    let _ = held.tx.send(HookAnswer::Decide(decision));
    g.set_state(
        &held.agent,
        if decision == Decision::Deny {
            AgentState::Idle
        } else {
            AgentState::Working
        },
    );
    g.publish(&held.agent);
    match note {
        Some(text) => Response::Text { text },
        None => Response::Ok,
    }
}

/// An answer typed for a held `AskUserQuestion`.
pub(crate) fn answer_question(d: &Shared, item: u64, typed: &str) -> Response {
    let mut g = lock(d);
    let Some(mut held) = g.held_req.remove(&item) else {
        return err("that question is no longer waiting");
    };
    let step = held.answers.len();
    let Some(q) = held.questions.get(step).cloned() else {
        g.held_req.insert(item, held);
        return err("this request is not a question");
    };
    let Some(answer) = question::pick(&q, typed) else {
        g.held_req.insert(item, held);
        return err(if q.multi {
            "pick option numbers (\"1 3\") or type your own answer"
        } else {
            "pick one option number, or type your own answer"
        });
    };
    held.answers.insert(q.text.clone(), answer);
    g.resolve(item);
    if let Some(next) = held.questions.get(step + 1).cloned() {
        // The next question gets its own inbox item and keeps the hook held.
        let next_item = g.add_item(question_item(&held.agent, &next, step + 1, held.questions.len()));
        g.held_req.insert(next_item, held);
        return Response::Ok;
    }
    let agent = held.agent.clone();
    let _ = held.tx.send(HookAnswer::Answers(held.answers));
    g.set_state(&agent, AgentState::Working);
    g.publish(&agent);
    Response::Ok
}

impl Daemon {
    /// Drops held requests whose agent moved on (answered in its terminal).
    pub(crate) fn forget_held(&mut self, item: u64) {
        self.held_req.remove(&item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summaries_name_the_target() {
        let p = json!({"tool_name": "Bash", "tool_input": {"command": "npm run build\nnpm test"}});
        let (summary, detail) = tool_summary(&p);
        assert_eq!(summary, "Bash  npm run build");
        assert_eq!(detail, "npm run build\nnpm test");
    }

    #[test]
    fn long_lines_are_cut() {
        assert_eq!(line("abcdef", 4), "abc…");
        assert_eq!(line("  ab\ncd", 4), "ab");
    }
}
