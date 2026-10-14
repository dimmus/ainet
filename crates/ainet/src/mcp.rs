//! `ainet mcp`: a Model Context Protocol server over stdio, so agents can
//! talk to each other and to their rig through ainetd. JSON-RPC 2.0, one
//! object per line; logs go to stderr only.
//!
//! The daemon starts each agent with `AINET_AGENT_ID` (and `AINET_SOCKET`),
//! which is who the tools act as.

use std::io::{self, BufRead, Write};
use std::time::Duration;

use ainet_core::protocol::{QSpec, SeatSpec, UpSpec};
use ainet_core::{Request, Response, Runtime};
use serde_json::{Value, json};

use crate::conn::{Conn, DEFAULT_TIMEOUT, RIG_TIMEOUT};
use crate::output;

pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const ASK_DEFAULT: u64 = 600;
const CHAT_WAIT_DEFAULT: u64 = 120;
const MAX_WAIT: u64 = 3600;

const ADDRESSING: &str = "Address peers by seat logical id within your rig (`dev.review`), \
by full seat address (`dev-review@starter`), or by critter name (`nimble-otter`) for agents \
outside rigs; list_agents shows them all.";

/// Where tool calls go: ainetd, or a fake in tests.
pub trait Backend {
    fn call(&mut self, req: &Request, timeout: Duration) -> Result<Response, String>;
}

pub struct SocketBackend(pub Conn);

impl Backend for SocketBackend {
    fn call(&mut self, req: &Request, timeout: Duration) -> Result<Response, String> {
        self.0.call_for(req, timeout).map_err(|e| format!("{e:#}"))
    }
}

/// Runs the server on stdin/stdout until stdin closes.
pub fn run(conn: Conn) -> anyhow::Result<()> {
    let mut server = Server::new(SocketBackend(conn), crate::conn::me());
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(stdin.lock(), stdout.lock(), &mut server)?;
    Ok(())
}

/// The framing: one JSON-RPC message per line in, one reply per line out.
pub fn serve<R: BufRead, W: Write, B: Backend>(
    input: R,
    mut output: W,
    server: &mut Server<B>,
) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(error_reply(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut output, &reply)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn result_reply(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub struct Server<B> {
    backend: B,
    me: Option<String>,
    /// The caller's rig, looked up once with `Whoami`.
    my_rig: Option<Option<String>>,
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B, me: Option<String>) -> Self {
        Server {
            backend,
            me,
            my_rig: None,
        }
    }

    /// One incoming message; `None` for notifications and stray responses.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(error_reply(
                Value::Null,
                -32600,
                "invalid request: expected one JSON object",
            ));
        };
        let method = obj.get("method").and_then(Value::as_str);
        let id = obj.get("id").cloned();
        let (Some(method), Some(id)) = (method, id) else {
            // A notification (no id), or a response to us: nothing to say.
            if let Some(m) = method {
                eprintln!("ainet mcp: notification {m}");
            }
            return None;
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => result_reply(id, initialize(&params)),
            "ping" => result_reply(id, json!({})),
            "tools/list" => result_reply(id, json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                if !TOOL_NAMES.contains(&name) {
                    return Some(error_reply(id, -32602, &format!("unknown tool: {name:?}")));
                }
                let args = match params.get("arguments") {
                    None | Some(Value::Null) => json!({}),
                    Some(a) => a.clone(),
                };
                let (text, is_error) = self.call_tool(name, &args);
                result_reply(
                    id,
                    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
                )
            }
            other => error_reply(id, -32601, &format!("method not found: {other}")),
        })
    }

    fn my_rig(&mut self) -> Option<String> {
        if let Some(r) = &self.my_rig {
            return r.clone();
        }
        let me = self.me.clone()?;
        let rig = match self.backend.call(&Request::Whoami { agent: me }, DEFAULT_TIMEOUT) {
            Ok(Response::Whoami { whoami }) => whoami.seat.map(|s| s.rig),
            _ => return None, // Try again next time: the daemon may be starting.
        };
        self.my_rig = Some(rig.clone());
        rig
    }

    /// Runs one tool: the text to show and whether it failed.
    pub fn call_tool(&mut self, name: &str, args: &Value) -> (String, bool) {
        let my_rig = if uses_rig(name) && args.get("rig").and_then(Value::as_str).is_none() {
            self.my_rig()
        } else {
            None
        };
        let req = match tool_request(name, args, self.me.as_deref(), my_rig.as_deref()) {
            Ok(r) => r,
            Err(e) => return (e, true),
        };
        let wait = wait_secs(name, args);
        let timeout = match (&req, wait) {
            (Request::Up(_) | Request::Grow { .. } | Request::SnapshotCreate { .. }, _) => RIG_TIMEOUT,
            (Request::ChatWait { timeout_secs, .. }, _) => Duration::from_secs(timeout_secs + 10),
            _ => DEFAULT_TIMEOUT,
        };
        let resp = match self.backend.call(&req, timeout) {
            Ok(r) => r,
            Err(e) => return (format!("ainetd: {e}"), true),
        };
        if name == "ask_agent" {
            return self.await_reply(args, resp, wait.unwrap_or(ASK_DEFAULT));
        }
        format_tool(name, &resp)
    }

    fn await_reply(&mut self, args: &Value, sent: Response, secs: u64) -> (String, bool) {
        let to = args.get("to").and_then(Value::as_str).unwrap_or("?");
        let id = match &sent {
            Response::Messages { messages } if !messages.is_empty() => messages[0].id,
            Response::Messages { .. } => return (format!("No agent matched {to:?}. {ADDRESSING}"), true),
            Response::Error { message } => return (message.clone(), true),
            other => return (format!("unexpected reply from ainetd: {other:?}"), true),
        };
        let req = Request::WaitReply {
            id,
            timeout_secs: secs,
        };
        let resp = self.backend.call(&req, Duration::from_secs(secs + 10));
        format_reply(to, id, secs, resp)
    }
}

/// What `ask_agent` shows once the wait ends.
pub fn format_reply(to: &str, id: u64, secs: u64, resp: Result<Response, String>) -> (String, bool) {
    let timeout = || {
        (
            format!(
                "No reply from {to} within {secs}s. Your question is message #{id}; the reply will \
                 arrive in your inbox, so check later with read_messages."
            ),
            false,
        )
    };
    match resp {
        Ok(Response::Messages { messages }) => match messages
            .iter()
            .find(|m| m.reply_to == Some(id))
            .or(messages.last())
        {
            Some(m) => (
                format!("Reply #{} from {}:\n{}", m.id, m.from_label, m.body),
                false,
            ),
            None => timeout(),
        },
        Ok(Response::Error { message }) if is_timeout(&message) => timeout(),
        Ok(Response::Error { message }) => (message, true),
        Ok(other) => (format!("unexpected reply from ainetd: {other:?}"), true),
        Err(e) if is_timeout(&e) => timeout(),
        Err(e) => (format!("ainetd: {e}"), true),
    }
}

fn is_timeout(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("timed out") || s.contains("timeout") || s.contains("did not answer")
}

fn initialize(params: &Value) -> Value {
    let asked = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let version = if PROTOCOL_VERSIONS.contains(&asked) {
        asked
    } else {
        PROTOCOL_VERSIONS[0]
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "ainet", "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!(
            "ainet connects you to the other coding agents in your rig (team). Use whoami to learn \
             your seat and peers, send_message / ask_agent to talk to one peer, broadcast or the \
             chat tools for the whole rig, and the queue tools for owned work items. {ADDRESSING}"
        ),
    })
}

pub const TOOL_NAMES: [&str; 23] = [
    "whoami",
    "list_agents",
    "send_message",
    "read_messages",
    "reply_message",
    "ask_agent",
    "broadcast",
    "chat_send",
    "chat_read",
    "chat_wait",
    "read_terminal",
    "queue_create",
    "queue_list",
    "queue_claim",
    "queue_update",
    "queue_handoff",
    "queue_block",
    "watchdog_register",
    "rig_status",
    "rig_up",
    "rig_grow",
    "snapshot_create",
    "list_specs",
];

/// Tools that act on the caller's rig unless given `rig`.
fn uses_rig(name: &str) -> bool {
    matches!(
        name,
        "broadcast"
            | "chat_send"
            | "chat_read"
            | "chat_wait"
            | "queue_create"
            | "queue_list"
            | "rig_status"
            | "rig_grow"
            | "snapshot_create"
    )
}

/// For tools that block: how long, in seconds.
fn wait_secs(name: &str, args: &Value) -> Option<u64> {
    let default = match name {
        "ask_agent" => ASK_DEFAULT,
        "chat_wait" => CHAT_WAIT_DEFAULT,
        _ => return None,
    };
    let secs = opt_u64(args, "timeout_secs").ok().flatten().unwrap_or(default);
    Some(secs.clamp(1, MAX_WAIT))
}

fn rig_prop() -> Value {
    json!({ "type": "string", "description": "Rig name. Defaults to your own rig." })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

/// The tool catalogue, with JSON Schemas.
pub fn tools() -> Vec<Value> {
    let qstates = json!(["pending", "in-progress", "blocked", "done", "canceled"]);
    vec![
        tool(
            "whoami",
            "Who you are in ainet: your agent name, your seat (address, logical id, rig, role), the \
             seats you delegate to and are delegated by, your peers, and how many unread messages \
             and open queue items you have. Call this first to learn how to address your team.",
            json!({}),
            &[],
        ),
        tool(
            "list_agents",
            "Every agent ainet knows, one per line: critter name, seat address, role, state \
             (working, idle, needs you, ...) and unread message count.",
            json!({}),
            &[],
        ),
        tool(
            "send_message",
            &format!(
                "Send a direct message to another agent. It is typed into their terminal and kept \
                 in their inbox until read. Does not wait for an answer; use ask_agent for that. \
                 {ADDRESSING}"
            ),
            json!({
                "to": { "type": "string", "description": "Recipient: logical id (dev.review), seat address (dev-review@starter) or critter name. Several may be separated by commas." },
                "body": { "type": "string", "description": "The message text." },
            }),
            &["to", "body"],
        ),
        tool(
            "read_messages",
            "Read your inbox: messages sent to you, oldest first, each with its id (use the id with \
             reply_message). Returned messages are marked read.",
            json!({
                "all": { "type": "boolean", "description": "Include messages already read. Default false: unread only." },
            }),
            &[],
        ),
        tool(
            "reply_message",
            "Reply to a message you received, by its id. The reply goes to whoever sent it, in the \
             same conversation thread, and wakes anyone waiting on it with ask_agent.",
            json!({
                "id": { "type": "integer", "minimum": 1, "description": "The id of the message you are answering (from read_messages)." },
                "body": { "type": "string", "description": "Your reply." },
            }),
            &["id", "body"],
        ),
        tool(
            "ask_agent",
            &format!(
                "Ask another agent a question and wait for their reply (they answer with \
                 reply_message). Returns the reply text. If nobody answers in time, returns a note \
                 with the message id; the reply will still land in your inbox (read_messages). \
                 {ADDRESSING}"
            ),
            json!({
                "to": { "type": "string", "description": "Who to ask: logical id (dev.review), seat address (dev-review@starter) or critter name." },
                "question": { "type": "string", "description": "The question." },
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT, "default": ASK_DEFAULT, "description": "How long to wait for the reply, in seconds (default 600, at most 3600)." },
            }),
            &["to", "question"],
        ),
        tool(
            "broadcast",
            "Send the same message to every seat in your rig, or in one pod of it. You do not \
             receive your own broadcast.",
            json!({
                "body": { "type": "string", "description": "The message text." },
                "pod": { "type": "string", "description": "Only this pod (e.g. dev). Default: the whole rig." },
                "rig": rig_prop(),
            }),
            &["body"],
        ),
        tool(
            "chat_send",
            "Post to your rig's chatroom, a shared log every seat can read. Good for status updates \
             and decisions the whole team should see; it is not typed into anyone's terminal.",
            json!({
                "body": { "type": "string", "description": "The chat message." },
                "topic": { "type": "string", "description": "Topic channel (default: general)." },
                "rig": rig_prop(),
            }),
            &["body"],
        ),
        tool(
            "chat_read",
            "Read your rig's chatroom history, oldest first. Each line starts with the message id; \
             pass the last id as `after` to read only newer messages.",
            json!({
                "topic": { "type": "string", "description": "Only this topic." },
                "after": { "type": "integer", "minimum": 0, "description": "Only messages with a larger id." },
                "limit": { "type": "integer", "minimum": 1, "default": 50, "description": "At most this many (the newest). Default 50." },
                "rig": rig_prop(),
            }),
            &[],
        ),
        tool(
            "chat_wait",
            "Wait until a new message is posted in your rig's chatroom, then return it. Returns an \
             empty result after the timeout.",
            json!({
                "topic": { "type": "string", "description": "Only this topic." },
                "after": { "type": "integer", "minimum": 0, "description": "Wait for a message with an id larger than this (default: the latest)." },
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT, "default": CHAT_WAIT_DEFAULT, "description": "How long to wait, in seconds (default 120, at most 3600)." },
                "rig": rig_prop(),
            }),
            &[],
        ),
        tool(
            "read_terminal",
            &format!(
                "Read the last lines of another agent's terminal, to see what it is doing. {ADDRESSING}"
            ),
            json!({
                "agent": { "type": "string", "description": "Whose terminal: logical id, seat address or critter name." },
                "lines": { "type": "integer", "minimum": 1, "default": 50, "description": "How many lines from the bottom (default 50)." },
            }),
            &["agent"],
        ),
        tool(
            "queue_create",
            "Hand a piece of work to a seat as a queue item: it has one owner and a state \
             (pending, in-progress, blocked, done, canceled). The destination is told right away. \
             Returns the item id (q-N).",
            json!({
                "destination": { "type": "string", "description": "The seat that should do it: logical id (dev.impl) or seat address." },
                "summary": { "type": "string", "description": "One line: what needs doing." },
                "body": { "type": "string", "description": "Details, acceptance criteria, links." },
                "priority": { "type": "string", "enum": ["routine", "urgent", "critical"], "description": "Default routine." },
                "rig": rig_prop(),
            }),
            &["destination", "summary"],
        ),
        tool(
            "queue_list",
            "List queue items: by default the open items in your rig; with mine, the ones owned by \
             or addressed to you.",
            json!({
                "mine": { "type": "boolean", "description": "Only items owned by or addressed to you." },
                "state": { "type": "string", "enum": qstates, "description": "Only items in this state." },
                "all": { "type": "boolean", "description": "Include done and canceled items." },
                "rig": rig_prop(),
            }),
            &[],
        ),
        tool(
            "queue_claim",
            "Claim a queue item: you become its owner and it moves to in-progress.",
            json!({ "id": { "type": "string", "description": "Item id, e.g. q-12." } }),
            &["id"],
        ),
        tool(
            "queue_update",
            "Add a progress note to a queue item and/or change its state. Set state done when \
             finished (with a note saying what you did), blocked when stuck.",
            json!({
                "id": { "type": "string", "description": "Item id, e.g. q-12." },
                "note": { "type": "string", "description": "What happened." },
                "state": { "type": "string", "enum": qstates, "description": "New state." },
            }),
            &["id"],
        ),
        tool(
            "queue_handoff",
            "Close a queue item you own and open its successor for another seat, carrying your \
             notes forward. Use when your part is done and someone else continues.",
            json!({
                "id": { "type": "string", "description": "Item id, e.g. q-12." },
                "to": { "type": "string", "description": "The seat that takes over: logical id or seat address." },
                "body": { "type": "string", "description": "What they need to know to continue." },
                "summary": { "type": "string", "description": "One line for the new item (default: the old summary)." },
            }),
            &["id", "to", "body"],
        ),
        tool(
            "queue_block",
            "Mark a queue item you own as blocked on another item (q-N) or something outside \
             (external:<name>), with what to do once unblocked. You are woken when the blocker \
             is done, or after wake_after_secs.",
            json!({
                "id": { "type": "string", "description": "Item id, e.g. q-12." },
                "on": { "type": "string", "description": "The blocker: another item id (q-7) or external:<name>." },
                "continuation": { "type": "string", "description": "What to do once unblocked." },
                "wake_after_secs": { "type": "integer", "minimum": 1, "description": "Wake you after this many seconds even if still blocked." },
            }),
            &["id", "on", "continuation"],
        ),
        tool(
            "watchdog_register",
            "Schedule a periodic reminder: every interval_secs the message is typed into the \
             target seat (for example to make it check its queue).",
            json!({
                "target": { "type": "string", "description": "The seat to remind: logical id or seat address." },
                "interval_secs": { "type": "integer", "minimum": 60, "description": "Seconds between reminders." },
                "message": { "type": "string", "description": "What to type (default: a nudge to check the queue)." },
            }),
            &["target", "interval_secs"],
        ),
        tool(
            "rig_status",
            "The seats of your rig: address, runtime, model, the agent's state, the seat's status, \
             and messages held for it. Outside a rig: every rig ainet knows.",
            json!({ "rig": rig_prop() }),
            &[],
        ),
        tool(
            "rig_up",
            "Launch a rig: from a spec file path, a library name (see list_specs), or the name of a \
             stopped rig to bring it back.",
            json!({
                "source": { "type": "string", "description": "Spec file path, library name, or known rig name." },
            }),
            &["source"],
        ),
        tool(
            "rig_grow",
            "Add a seat to your rig and launch it.",
            json!({
                "pod": { "type": "string", "description": "Pod to add it to (new or existing), e.g. dev." },
                "member": { "type": "string", "description": "Member name within the pod, e.g. qa. The seat becomes pod.member." },
                "runtime": { "type": "string", "enum": ["claude-code", "codex", "terminal"], "description": "Default claude-code." },
                "role": { "type": "string", "description": "What this seat is for." },
                "model": { "type": "string", "description": "Model for the agent CLI." },
                "rig": rig_prop(),
            }),
            &["pod", "member"],
        ),
        tool(
            "snapshot_create",
            "Snapshot your rig so it can be restored later with every seat's conversation.",
            json!({
                "label": { "type": "string", "description": "A label to recognise it by." },
                "rig": rig_prop(),
            }),
            &[],
        ),
        tool(
            "list_specs",
            "The rig spec library: built-in rigs and your own, with seat counts.",
            json!({}),
            &[],
        ),
    ]
}

// ---- arguments ---------------------------------------------------------

fn req_str(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) | None | Some(Value::Null) => {
            Err(format!("missing required argument `{key}`"))
        }
        Some(_) => Err(format!("argument `{key}` must be a string")),
    }
}

fn opt_str(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("argument `{key}` must be a string")),
    }
}

fn opt_u64(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("argument `{key}` must be a non-negative integer")),
        Some(Value::String(s)) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| format!("argument `{key}` must be a non-negative integer")),
        Some(_) => Err(format!("argument `{key}` must be a non-negative integer")),
    }
}

fn req_u64(args: &Value, key: &str) -> Result<u64, String> {
    opt_u64(args, key)?.ok_or_else(|| format!("missing required argument `{key}`"))
}

fn opt_bool(args: &Value, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" => Ok(true),
        Some(Value::String(s)) if s == "false" => Ok(false),
        Some(_) => Err(format!("argument `{key}` must be true or false")),
    }
}

fn opt_state(args: &Value) -> Result<Option<ainet_core::QState>, String> {
    opt_str(args, "state")?
        .map(|s| crate::cli::parse_qstate(&s))
        .transpose()
}

fn rig_or(args: &Value, my_rig: Option<&str>) -> Result<Option<String>, String> {
    Ok(opt_str(args, "rig")?.or_else(|| my_rig.map(str::to_string)))
}

fn need_rig(args: &Value, my_rig: Option<&str>) -> Result<String, String> {
    rig_or(args, my_rig)?.ok_or_else(|| "you are not in a rig; pass `rig` to name one".to_string())
}

fn need_me(me: Option<&str>) -> Result<String, String> {
    me.map(str::to_string).ok_or_else(|| {
        "this session is not an ainet agent (AINET_AGENT_ID is not set), so it has no identity or inbox"
            .to_string()
    })
}

fn split_to(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The daemon request for one tool call. `me` is the caller's agent id,
/// `my_rig` the rig its seat is in.
pub fn tool_request(
    name: &str,
    args: &Value,
    me: Option<&str>,
    my_rig: Option<&str>,
) -> Result<Request, String> {
    if !args.is_object() {
        return Err("arguments must be a JSON object".into());
    }
    let from = me.map(str::to_string);
    Ok(match name {
        "whoami" => Request::Whoami { agent: need_me(me)? },
        "list_agents" => Request::Snapshot,
        "send_message" => Request::Message {
            from,
            to: split_to(&req_str(args, "to")?),
            rig: None,
            pod: None,
            body: req_str(args, "body")?,
            reply_to: None,
            force: false,
        },
        "read_messages" => Request::Inbox {
            agent: need_me(me)?,
            all: opt_bool(args, "all")?,
            limit: 0,
            mark_read: true,
        },
        "reply_message" => Request::Message {
            from,
            to: vec![],
            rig: None,
            pod: None,
            body: req_str(args, "body")?,
            reply_to: Some(req_u64(args, "id")?),
            force: false,
        },
        "ask_agent" => Request::Message {
            from,
            to: split_to(&req_str(args, "to")?),
            rig: None,
            pod: None,
            body: req_str(args, "question")?,
            reply_to: None,
            force: false,
        },
        "broadcast" => Request::Message {
            from,
            to: vec![],
            rig: Some(need_rig(args, my_rig)?),
            pod: opt_str(args, "pod")?,
            body: req_str(args, "body")?,
            reply_to: None,
            force: false,
        },
        "chat_send" => Request::ChatSend {
            rig: need_rig(args, my_rig)?,
            topic: opt_str(args, "topic")?,
            sender: from,
            body: req_str(args, "body")?,
        },
        "chat_read" => Request::ChatHistory {
            rig: need_rig(args, my_rig)?,
            topic: opt_str(args, "topic")?,
            after: opt_u64(args, "after")?,
            limit: opt_u64(args, "limit")?.unwrap_or(50) as usize,
        },
        "chat_wait" => Request::ChatWait {
            rig: need_rig(args, my_rig)?,
            topic: opt_str(args, "topic")?,
            after: opt_u64(args, "after")?,
            timeout_secs: wait_secs(name, args).unwrap_or(CHAT_WAIT_DEFAULT),
        },
        "read_terminal" => Request::Capture {
            targets: vec![req_str(args, "agent")?],
            rig: None,
            pod: None,
            lines: opt_u64(args, "lines")?.unwrap_or(50) as usize,
        },
        "queue_create" => Request::QueueCreate(QSpec {
            rig: rig_or(args, my_rig)?,
            source: from,
            destination: req_str(args, "destination")?,
            summary: req_str(args, "summary")?,
            body: opt_str(args, "body")?.unwrap_or_default(),
            priority: opt_str(args, "priority")?,
            tags: vec![],
            reply_to: None,
            nudge: true,
        }),
        "queue_list" => {
            let mine = opt_bool(args, "mine")?;
            Request::QueueList {
                rig: if mine {
                    opt_str(args, "rig")?
                } else {
                    rig_or(args, my_rig)?
                },
                who: if mine { Some(need_me(me)?) } else { None },
                state: opt_state(args)?,
                all: opt_bool(args, "all")?,
            }
        }
        "queue_claim" => Request::QueueClaim {
            id: req_str(args, "id")?,
            by: from,
        },
        "queue_update" => {
            let id = req_str(args, "id")?;
            let (note, state) = (opt_str(args, "note")?, opt_state(args)?);
            if note.is_none() && state.is_none() {
                return Err("give a `note`, a `state`, or both".into());
            }
            Request::QueueUpdate {
                id,
                by: from,
                note,
                state,
            }
        }
        "queue_handoff" => Request::QueueHandoff {
            id: req_str(args, "id")?,
            by: from,
            to: req_str(args, "to")?,
            body: req_str(args, "body")?,
            summary: opt_str(args, "summary")?,
        },
        "queue_block" => Request::QueueBlock {
            id: req_str(args, "id")?,
            by: from,
            on: req_str(args, "on")?,
            continuation: req_str(args, "continuation")?,
            wake_after_secs: opt_u64(args, "wake_after_secs")?,
        },
        "watchdog_register" => Request::WatchdogRegister {
            target: req_str(args, "target")?,
            interval_secs: req_u64(args, "interval_secs")?.max(1),
            message: opt_str(args, "message")?,
            registered_by: from,
        },
        "rig_status" => match rig_or(args, my_rig)? {
            Some(rig) => Request::Status { rig },
            None => Request::Ps,
        },
        "rig_up" => Request::Up(UpSpec {
            source: req_str(args, "source")?,
            ..UpSpec::default()
        }),
        "rig_grow" => {
            let runtime = opt_str(args, "runtime")?
                .map(|r| r.parse::<Runtime>())
                .transpose()?;
            Request::Grow {
                rig: need_rig(args, my_rig)?,
                seats: vec![SeatSpec {
                    pod: req_str(args, "pod")?,
                    member: req_str(args, "member")?,
                    runtime,
                    role: opt_str(args, "role")?,
                    model: opt_str(args, "model")?,
                    ..SeatSpec::default()
                }],
                edges: vec![],
            }
        }
        "snapshot_create" => Request::SnapshotCreate {
            rig: need_rig(args, my_rig)?,
            label: opt_str(args, "label")?,
        },
        "list_specs" => Request::Specs,
        other => return Err(format!("unknown tool: {other}")),
    })
}

/// A daemon reply as tool output: the text and whether it is an error.
pub fn format_tool(name: &str, resp: &Response) -> (String, bool) {
    let now = ainet_core::now_ms();
    let text = match (name, resp) {
        (_, Response::Error { message }) => return (message.clone(), true),
        ("list_agents", Response::Snapshot { agents, .. }) => {
            if agents.is_empty() {
                "No agents.".to_string()
            } else {
                agents
                    .iter()
                    .map(|a| {
                        let mut line = format!("- {}", a.seat.as_deref().unwrap_or(&a.name));
                        if a.seat.is_some() {
                            line.push_str(&format!(" ({})", a.name));
                        }
                        line.push_str(&format!(": {}", a.state.label()));
                        if let Some(r) = &a.role {
                            line.push_str(&format!(", role {r}"));
                        }
                        if a.unread > 0 {
                            line.push_str(&format!(", {} unread", a.unread));
                        }
                        line
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        ("send_message" | "reply_message" | "broadcast", Response::Messages { messages }) => {
            if messages.is_empty() {
                return (format!("No agent matched, nothing was sent. {ADDRESSING}"), true);
            }
            messages
                .iter()
                .map(|m| {
                    let held = if m.held {
                        " (held: they are busy typing; it is delivered when they stop)"
                    } else {
                        ""
                    };
                    format!("Sent message #{} to {}{held}.", m.id, m.to_label)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        ("read_messages", Response::Messages { messages }) if messages.is_empty() => {
            "No new messages.".into()
        }
        ("chat_wait", Response::Chat { messages }) if messages.is_empty() => {
            "No new chat messages before the timeout.".into()
        }
        ("chat_read", Response::Chat { messages }) if messages.is_empty() => "The chatroom is empty.".into(),
        ("queue_list", Response::QItems { items }) if items.is_empty() => "No queue items.".into(),
        (_, Response::Ok) => "Done.".into(),
        (_, Response::Messages { messages }) => output::messages_text(messages, now),
        (_, other) => output::human(other),
    };
    (text.trim_end().to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainet_core::{Message, QState};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        replies: VecDeque<Response>,
        seen: Vec<(Request, Duration)>,
    }

    impl Backend for Fake {
        fn call(&mut self, req: &Request, timeout: Duration) -> Result<Response, String> {
            self.seen.push((req.clone(), timeout));
            self.replies.pop_front().ok_or_else(|| "no daemon".to_string())
        }
    }

    fn msg(id: u64, reply_to: Option<u64>, body: &str) -> Message {
        Message {
            id,
            from: "b".into(),
            from_label: "dev-review@starter".into(),
            to: "a".into(),
            to_label: "dev-impl@starter".into(),
            body: body.into(),
            reply_to,
            thread: 1,
            created: 0,
            delivered: None,
            read: None,
            held: false,
        }
    }

    fn drive(server: &mut Server<Fake>, input: &str) -> Vec<Value> {
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, server).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn treq(name: &str, args: Value) -> Request {
        tool_request(name, &args, Some("me-id"), Some("starter")).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn terr(name: &str, args: Value) -> String {
        tool_request(name, &args, Some("me-id"), Some("starter")).unwrap_err()
    }

    #[test]
    fn initialize_negotiates_the_version() {
        let mut s = Server::new(Fake::default(), None);
        let out = drive(
            &mut s,
            concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
                "\n",
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "\n\n",
                r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
                "\n",
            ),
        );
        assert_eq!(out.len(), 2, "the notification gets no reply: {out:?}");
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "ainet");
        assert_eq!(
            out[0]["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(out[0]["result"]["capabilities"]["tools"].is_object());
        assert_eq!(out[1]["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn tools_list_has_every_tool() {
        let mut s = Server::new(Fake::default(), None);
        let out = drive(
            &mut s,
            "{\"jsonrpc\":\"2.0\",\"id\":\"a\",\"method\":\"tools/list\"}\n",
        );
        let tools = out[0]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{t}");
            assert!(t["description"].as_str().unwrap().len() > 20);
            for r in t["inputSchema"]["required"].as_array().unwrap() {
                assert!(
                    t["inputSchema"]["properties"].get(r.as_str().unwrap()).is_some(),
                    "{t}"
                );
            }
        }
    }

    #[test]
    fn unknown_methods_and_garbage() {
        let mut s = Server::new(Fake::default(), None);
        let out = drive(
            &mut s,
            concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"resources/list\"}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n",
                "not json\n",
                "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"nope\"}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{}}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n",
            ),
        );
        assert_eq!(out.len(), 4, "{out:?}");
        assert_eq!(out[0]["error"]["code"], -32601);
        assert_eq!(out[0]["id"], 3);
        assert_eq!(out[1]["result"], json!({}));
        assert_eq!(out[2]["error"]["code"], -32700);
        assert_eq!(out[3]["error"]["code"], -32602);
    }

    #[test]
    fn tool_errors_are_results() {
        let mut s = Server::new(Fake::default(), Some("me-id".into()));
        let out = drive(
            &mut s,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"send_message\",\"arguments\":{\"to\":\"x\"}}}\n",
        );
        assert_eq!(out[0]["result"]["isError"], true);
        assert!(
            out[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("`body`")
        );
        // No daemon: still a result, not a JSON-RPC error.
        let out = drive(
            &mut s,
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_agents\"}}\n",
        );
        assert_eq!(out[0]["result"]["isError"], true);
        assert!(
            out[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("ainetd:")
        );
    }

    #[test]
    fn tool_call_round_trip_resolves_my_rig() {
        let whoami = serde_json::from_value::<Response>(json!({
            "type": "whoami",
            "whoami": {
                "agent": "me-id", "name": "nimble-otter",
                "seat": {
                    "address": "dev-impl@starter", "logical": "dev.impl", "rig": "starter", "pod": "dev",
                    "member": "impl", "label": null, "role": null, "runtime": "claude_code", "model": null,
                    "cwd": "/w", "agent": "me-id", "status": "running", "transport": {"kind": "pane"},
                    "restore_policy": "resume_if_possible"
                },
                "delegates_to": [], "delegated_by": [], "peers": [], "edges": [], "unread": 0, "open_queue": 0
            }
        }))
        .unwrap();
        let fake = Fake {
            replies: VecDeque::from([whoami, Response::Ok, Response::Ok]),
            ..Fake::default()
        };
        let mut s = Server::new(fake, Some("me-id".into()));
        let (text, err) = s.call_tool("chat_send", &json!({ "body": "hi" }));
        assert_eq!((text.as_str(), err), ("Done.", false));
        let (_, err) = s.call_tool("snapshot_create", &json!({}));
        assert!(!err);
        let seen = &s.backend.seen;
        assert_eq!(seen.len(), 3, "whoami is asked once: {seen:?}");
        assert!(
            matches!(&seen[1].0, Request::ChatSend { rig, sender: Some(me), .. } if rig == "starter" && me == "me-id")
        );
        assert!(matches!(&seen[2].0, Request::SnapshotCreate { rig, .. } if rig == "starter"));
        assert_eq!(seen[2].1, RIG_TIMEOUT);
    }

    #[test]
    fn ask_agent_sends_then_waits() {
        let fake = Fake {
            replies: VecDeque::from([
                Response::Messages {
                    messages: vec![msg(41, None, "ok?")],
                },
                Response::Messages {
                    messages: vec![msg(42, Some(41), "yes, ship it")],
                },
            ]),
            ..Fake::default()
        };
        let mut s = Server::new(fake, Some("me-id".into()));
        let (text, err) = s.call_tool(
            "ask_agent",
            &json!({ "to": "dev.review", "question": "ok?", "timeout_secs": 30 }),
        );
        assert!(!err);
        assert_eq!(text, "Reply #42 from dev-review@starter:\nyes, ship it");
        let (req, timeout) = &s.backend.seen[1];
        assert!(matches!(
            req,
            Request::WaitReply {
                id: 41,
                timeout_secs: 30
            }
        ));
        assert_eq!(*timeout, Duration::from_secs(40));
    }

    #[test]
    fn ask_agent_timeout_mentions_the_id() {
        let (text, err) = format_reply("dev.review", 41, 30, Ok(Response::Messages { messages: vec![] }));
        assert!(!err);
        assert!(text.contains("#41") && text.contains("read_messages"), "{text}");
        let (text, _) = format_reply("x", 7, 5, Err("ainetd did not answer within 15s".into()));
        assert!(text.contains("#7"));
        let (_, err) = format_reply(
            "x",
            7,
            5,
            Ok(Response::Error {
                message: "no such agent".into(),
            }),
        );
        assert!(err);
    }

    #[test]
    fn identity_and_simple_tools() {
        assert!(matches!(treq("whoami", json!({})), Request::Whoami { agent } if agent == "me-id"));
        assert!(
            tool_request("whoami", &json!({}), None, None)
                .unwrap_err()
                .contains("AINET_AGENT_ID")
        );
        assert!(matches!(treq("list_agents", json!({})), Request::Snapshot));
        assert!(matches!(treq("list_specs", json!({})), Request::Specs));
        assert!(tool_request("list_agents", &json!([]), None, None).is_err());
        assert!(tool_request("nope", &json!({}), None, None).is_err());
    }

    #[test]
    fn messaging_tools() {
        match treq(
            "send_message",
            json!({ "to": "dev.review, dev.qa", "body": "hi" }),
        ) {
            Request::Message {
                from,
                to,
                rig: None,
                pod: None,
                body,
                reply_to: None,
                force: false,
            } => {
                assert_eq!(from.as_deref(), Some("me-id"));
                assert_eq!(to, ["dev.review", "dev.qa"]);
                assert_eq!(body, "hi");
            }
            other => panic!("{other:?}"),
        }
        // Outside ainet you send as the operator.
        assert!(matches!(
            tool_request("send_message", &json!({ "to": "a", "body": "b" }), None, None).unwrap(),
            Request::Message { from: None, .. }
        ));
        assert!(terr("send_message", json!({ "body": "b" })).contains("`to`"));
        assert!(terr("send_message", json!({ "to": "a", "body": "  " })).contains("`body`"));
        assert!(terr("send_message", json!({ "to": 3, "body": "b" })).contains("string"));

        assert!(matches!(
            treq("read_messages", json!({})),
            Request::Inbox { agent, all: false, limit: 0, mark_read: true } if agent == "me-id"
        ));
        assert!(matches!(
            treq("read_messages", json!({ "all": true })),
            Request::Inbox { all: true, .. }
        ));
        assert!(tool_request("read_messages", &json!({}), None, None).is_err());

        assert!(matches!(
            treq("reply_message", json!({ "id": 12, "body": "yes" })),
            Request::Message { reply_to: Some(12), ref to, .. } if to.is_empty()
        ));
        assert!(matches!(
            treq("reply_message", json!({ "id": "12", "body": "yes" })),
            Request::Message {
                reply_to: Some(12),
                ..
            }
        ));
        assert!(terr("reply_message", json!({ "body": "yes" })).contains("`id`"));
        assert!(terr("reply_message", json!({ "id": -1, "body": "yes" })).contains("integer"));

        assert!(matches!(
            treq("ask_agent", json!({ "to": "dev.review", "question": "ok?" })),
            Request::Message { ref body, reply_to: None, .. } if body == "ok?"
        ));
        assert!(terr("ask_agent", json!({ "to": "dev.review" })).contains("`question`"));
        assert_eq!(wait_secs("ask_agent", &json!({})), Some(600));
        assert_eq!(
            wait_secs("ask_agent", &json!({ "timeout_secs": 99999 })),
            Some(3600)
        );
        assert_eq!(wait_secs("send_message", &json!({})), None);

        assert!(matches!(
            treq("broadcast", json!({ "body": "all hands", "pod": "dev" })),
            Request::Message { rig: Some(r), pod: Some(p), .. } if r == "starter" && p == "dev"
        ));
        assert!(matches!(
            treq("broadcast", json!({ "body": "x", "rig": "other" })),
            Request::Message { rig: Some(r), .. } if r == "other"
        ));
        assert!(
            tool_request("broadcast", &json!({ "body": "x" }), Some("me"), None)
                .unwrap_err()
                .contains("not in a rig")
        );
    }

    #[test]
    fn chat_and_terminal_tools() {
        assert!(matches!(
            treq("chat_send", json!({ "body": "done", "topic": "status" })),
            Request::ChatSend { rig, topic: Some(t), sender: Some(s), body } if rig == "starter" && t == "status" && s == "me-id" && body == "done"
        ));
        assert!(matches!(
            treq("chat_read", json!({})),
            Request::ChatHistory {
                topic: None,
                after: None,
                limit: 50,
                ..
            }
        ));
        assert!(matches!(
            treq("chat_read", json!({ "after": 9, "limit": 5 })),
            Request::ChatHistory {
                after: Some(9),
                limit: 5,
                ..
            }
        ));
        assert!(matches!(
            treq("chat_wait", json!({})),
            Request::ChatWait {
                timeout_secs: 120,
                after: None,
                ..
            }
        ));
        assert!(matches!(
            treq("chat_wait", json!({ "timeout_secs": 7, "after": 3 })),
            Request::ChatWait {
                timeout_secs: 7,
                after: Some(3),
                ..
            }
        ));
        assert!(tool_request("chat_read", &json!({}), None, None).is_err());
        assert!(matches!(
            treq("read_terminal", json!({ "agent": "dev.review" })),
            Request::Capture { ref targets, lines: 50, rig: None, pod: None } if targets == &["dev.review"]
        ));
        assert!(matches!(
            treq("read_terminal", json!({ "agent": "a", "lines": 10 })),
            Request::Capture { lines: 10, .. }
        ));
        assert!(terr("read_terminal", json!({})).contains("`agent`"));
    }

    #[test]
    fn queue_tools() {
        match treq(
            "queue_create",
            json!({ "destination": "dev.impl", "summary": "fix", "priority": "urgent" }),
        ) {
            Request::QueueCreate(q) => {
                assert_eq!(q.rig.as_deref(), Some("starter"));
                assert_eq!(q.source.as_deref(), Some("me-id"));
                assert_eq!(
                    (q.destination.as_str(), q.summary.as_str(), q.body.as_str()),
                    ("dev.impl", "fix", "")
                );
                assert_eq!(q.priority.as_deref(), Some("urgent"));
                assert!(q.nudge);
            }
            other => panic!("{other:?}"),
        }
        assert!(terr("queue_create", json!({ "destination": "x" })).contains("`summary`"));
        assert!(matches!(
            treq("queue_list", json!({})),
            Request::QueueList { rig: Some(r), who: None, state: None, all: false } if r == "starter"
        ));
        assert!(matches!(
            treq("queue_list", json!({ "mine": true, "state": "in-progress", "all": true })),
            Request::QueueList { rig: None, who: Some(w), state: Some(QState::InProgress), all: true } if w == "me-id"
        ));
        assert!(terr("queue_list", json!({ "state": "bogus" })).contains("unknown state"));
        assert!(matches!(
            treq("queue_claim", json!({ "id": "q-3" })),
            Request::QueueClaim { id, by: Some(b) } if id == "q-3" && b == "me-id"
        ));
        assert!(matches!(
            treq(
                "queue_update",
                json!({ "id": "q-3", "state": "done", "note": "merged" })
            ),
            Request::QueueUpdate {
                state: Some(QState::Done),
                note: Some(_),
                by: Some(_),
                ..
            }
        ));
        assert!(terr("queue_update", json!({ "id": "q-3" })).contains("note"));
        assert!(matches!(
            treq("queue_handoff", json!({ "id": "q-3", "to": "dev.qa", "body": "test it" })),
            Request::QueueHandoff { to, summary: None, by: Some(_), .. } if to == "dev.qa"
        ));
        assert!(terr("queue_handoff", json!({ "id": "q-3", "to": "dev.qa" })).contains("`body`"));
        assert!(matches!(
            treq("queue_block", json!({ "id": "q-3", "on": "external:ci", "continuation": "rerun", "wake_after_secs": 3600 })),
            Request::QueueBlock { wake_after_secs: Some(3600), on, .. } if on == "external:ci"
        ));
        assert!(terr("queue_block", json!({ "id": "q-3", "on": "q-1" })).contains("`continuation`"));
        assert!(matches!(
            treq(
                "watchdog_register",
                json!({ "target": "dev.impl", "interval_secs": 600 })
            ),
            Request::WatchdogRegister {
                interval_secs: 600,
                message: None,
                registered_by: Some(_),
                ..
            }
        ));
        assert!(terr("watchdog_register", json!({ "target": "dev.impl" })).contains("`interval_secs`"));
    }

    #[test]
    fn rig_tools() {
        assert!(matches!(treq("rig_status", json!({})), Request::Status { rig } if rig == "starter"));
        assert!(matches!(
            tool_request("rig_status", &json!({}), None, None).unwrap(),
            Request::Ps
        ));
        assert!(matches!(
            treq("rig_up", json!({ "source": "starter" })),
            Request::Up(UpSpec { ref source, cwd: None, plan: false, .. }) if source == "starter"
        ));
        assert!(terr("rig_up", json!({})).contains("`source`"));
        match treq(
            "rig_grow",
            json!({ "pod": "dev", "member": "qa", "runtime": "codex", "role": "tester" }),
        ) {
            Request::Grow { rig, seats, edges } => {
                assert_eq!(rig, "starter");
                assert!(edges.is_empty());
                assert_eq!((seats[0].pod.as_str(), seats[0].member.as_str()), ("dev", "qa"));
                assert_eq!(seats[0].runtime, Some(Runtime::Codex));
                assert_eq!(seats[0].role.as_deref(), Some("tester"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            terr(
                "rig_grow",
                json!({ "pod": "dev", "member": "qa", "runtime": "vim" })
            )
            .contains("unknown runtime")
        );
        assert!(terr("rig_grow", json!({ "pod": "dev" })).contains("`member`"));
        assert!(matches!(
            treq("snapshot_create", json!({ "label": "before refactor" })),
            Request::SnapshotCreate { label: Some(_), .. }
        ));
        assert!(tool_request("snapshot_create", &json!({}), Some("me"), None).is_err());
    }

    #[test]
    fn every_tool_has_a_request() {
        for name in TOOL_NAMES {
            let err = tool_request(name, &json!({}), Some("me"), Some("r"));
            if let Err(e) = err {
                assert!(e.contains("missing required argument"), "{name}: {e}");
            }
        }
    }

    #[test]
    fn formatting() {
        let (t, e) = format_tool(
            "send_message",
            &Response::Messages {
                messages: vec![msg(5, None, "x")],
            },
        );
        assert_eq!((t.as_str(), e), ("Sent message #5 to dev-impl@starter.", false));
        let (_, e) = format_tool("send_message", &Response::Messages { messages: vec![] });
        assert!(e);
        let (t, _) = format_tool("read_messages", &Response::Messages { messages: vec![] });
        assert_eq!(t, "No new messages.");
        let (t, _) = format_tool(
            "read_messages",
            &Response::Messages {
                messages: vec![msg(5, None, "hello")],
            },
        );
        assert!(
            t.starts_with("#5 dev-review@starter -> dev-impl@starter") && t.ends_with("  hello"),
            "{t}"
        );
        let (t, e) = format_tool(
            "queue_claim",
            &Response::Error {
                message: "already claimed".into(),
            },
        );
        assert_eq!((t.as_str(), e), ("already claimed", true));
        let (t, _) = format_tool("chat_wait", &Response::Chat { messages: vec![] });
        assert!(t.contains("timeout"));
        let (t, _) = format_tool(
            "list_agents",
            &Response::Snapshot {
                agents: vec![],
                inbox: vec![],
                rigs: vec![],
                messages: vec![],
                chat: vec![],
                queue: vec![],
            },
        );
        assert_eq!(t, "No agents.");
    }
}
