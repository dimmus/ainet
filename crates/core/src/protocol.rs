//! The wire protocol: one JSON object per line over the daemon's Unix socket.
//! Every request gets exactly one response, except `Subscribe`, after which
//! the connection carries `Response::Event` lines until it closes, and
//! `Attach`, after which it carries one terminal's screens and output.
//!
//! Requests made on behalf of an agent (the MCP server, `ainet msg …` run in
//! an agent's shell) carry that agent's id from `AINET_AGENT_ID` in their
//! `from` / `by` / `sender` fields; `None` means you, the operator.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{Agent, Decision, InboxItem, Runtime};
use crate::comms::{ChatMessage, Message, QItem, QState, Watchdog};
use crate::conversation::Conversation;
use crate::rig::{Discovered, Edge, Rig, SnapshotInfo, Whoami};

/// Bumped on any incompatible change.
pub const VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
    },
    /// Agents, inbox, rigs, recent messages and chat, open queue items.
    Snapshot,
    Subscribe,
    Shutdown,

    // ---- agents and their terminals ------------------------------------
    /// Starts one agent outside any rig.
    Spawn(SpawnSpec),
    /// Types text into an agent's terminal as a paste, then presses Enter.
    Send {
        agent: String,
        text: String,
    },
    /// Raw keystrokes for an agent's terminal, as typed (no Enter added).
    Input {
        agent: String,
        data: String,
    },
    Resize {
        agent: String,
        rows: u16,
        cols: u16,
    },
    /// The screen of an agent's terminal: plain rows, and with `formatted`
    /// the whole state as escape sequences for a local `vt100` parser.
    /// `history` adds up to that many scrollback rows above the screen;
    /// with `formatted` too, a daemon that can also sends each of those rows
    /// with its colours (`Response::Screen::styled_rows`).
    Screen {
        agent: String,
        #[serde(default)]
        formatted: bool,
        #[serde(default)]
        history: usize,
    },
    /// Streams an agent's terminal on this connection, which carries
    /// nothing else from then on: first its screen (`Response::Screen`,
    /// formatted), then everything the program prints, in order, as
    /// `Response::Event { event: Event::Bytes }`, and the whole screen again
    /// after a resize, when the client fell behind, and once the output
    /// settles, until either side closes the connection (the daemon does
    /// when the program ends). An agent without a pane of ainetd's gets an
    /// error and the connection stays an ordinary one. Daemons from before
    /// it answer with an error too (`bad request: unknown variant`);
    /// clients then fetch `Screen` on `Event::Output`, as before.
    Attach {
        agent: String,
    },
    Close {
        agent: String,
    },
    /// You are (or stop being) at this agent's terminal. While on, messages
    /// for it are held so they never land under what you are typing.
    Interact {
        agent: String,
        on: bool,
    },
    Answer {
        item: u64,
        decision: Decision,
        /// With a deny: why, or what to do instead, for the agent to read.
        /// Left out of the line when empty, so older daemons and clients
        /// see the request they always did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Answers an agent's question (AskUserQuestion) waiting in the inbox:
    /// an option's number, several numbers ("1 3"), or your own words.
    AnswerQuestion {
        item: u64,
        answer: String,
    },
    /// From `ainet hook`: a Claude Code hook payload, verbatim. For a
    /// `PermissionRequest` the daemon holds the reply until you answer.
    Hook {
        agent: Option<String>,
        payload: Value,
        /// `$TMUX_PANE` of the hook's process, so tmux sessions can be adopted.
        #[serde(default)]
        tmux_pane: Option<String>,
    },
    Whoami {
        agent: String,
    },
    /// Terminal text of several agents: by name, seat, pod or whole rig.
    Capture {
        #[serde(default)]
        targets: Vec<String>,
        rig: Option<String>,
        pod: Option<String>,
        #[serde(default)]
        lines: usize,
    },
    /// The conversation from Claude Code's transcript file.
    Transcript {
        agent: String,
        #[serde(default)]
        tail: usize,
        grep: Option<String>,
    },
    /// The conversation in an agent's transcript as entries
    /// (`Response::Conversation`): the last `tail` (0: the daemon's
    /// choice), or with `after` only those further on than that entry
    /// id. Daemons from before it answer `bad request: unknown variant`;
    /// clients then ask for `Transcript` and show its text.
    Conversation {
        agent: String,
        #[serde(default)]
        tail: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<u64>,
    },

    // ---- rigs ------------------------------------------------------------
    /// Launches a rig from a spec file, a library name, or (restoring) the
    /// name of a rig ainet already knows.
    Up(UpSpec),
    Down {
        rig: String,
        /// Take a snapshot first (the default for `ainet down`).
        #[serde(default)]
        snapshot: bool,
        /// Forget the rig entirely after stopping it.
        #[serde(default)]
        delete: bool,
    },
    Ps,
    Status {
        rig: String,
    },
    SnapshotCreate {
        rig: String,
        label: Option<String>,
    },
    SnapshotList {
        rig: String,
    },
    Restore {
        rig: String,
        snapshot: Option<String>,
        /// Seats to start fresh instead of resuming.
        #[serde(default)]
        fresh: Vec<String>,
    },
    /// Adds seats (and edges) to a running rig: OpenRig's `grow`, `add` and
    /// `expand`.
    Grow {
        rig: String,
        seats: Vec<SeatSpec>,
        #[serde(default)]
        edges: Vec<Edge>,
    },
    /// Removes a whole pod.
    Shrink {
        rig: String,
        pod: String,
    },
    RemoveSeat {
        seat: String,
    },
    /// Launches (or relaunches) one seat.
    Launch {
        seat: String,
        #[serde(default)]
        fresh: bool,
    },
    /// A one-seat rig without writing a spec.
    Create {
        rig: String,
        seat: SeatSpec,
    },
    /// The running rig as RigSpec YAML.
    Export {
        rig: String,
    },
    /// Registers a rig from a spec without launching it.
    Import {
        path: PathBuf,
    },
    /// The spec library: built-in rigs and your own in the config directory.
    Specs,
    SpecShow {
        name: String,
    },
    /// Checks a spec without launching anything.
    Validate {
        path: PathBuf,
    },
    /// Agent sessions running outside ainet (tmux panes, hook-only sessions).
    Discover,
    /// Takes a discovered session into a rig seat.
    Adopt {
        id: String,
        rig: String,
        seat: SeatSpec,
    },
    /// Lets a seat go without stopping its process.
    Release {
        seat: String,
    },
    TypingGuard {
        seat: String,
        on: bool,
    },
    /// Messages a typing guard is holding for a seat.
    Held {
        seat: String,
    },

    // ---- messages, chat, queue --------------------------------------------
    /// Sends a message to agents. `to` takes names, seat addresses, logical
    /// ids (`pod.member`, within `rig`), agent ids; `rig`/`pod` broadcast.
    Message {
        from: Option<String>,
        #[serde(default)]
        to: Vec<String>,
        rig: Option<String>,
        pod: Option<String>,
        body: String,
        reply_to: Option<u64>,
        /// Deliver even through a typing guard.
        #[serde(default)]
        force: bool,
    },
    /// An agent's mailbox, newest last.
    Inbox {
        agent: String,
        /// Include messages already read.
        #[serde(default)]
        all: bool,
        #[serde(default)]
        limit: usize,
        /// Mark what is returned as read.
        #[serde(default)]
        mark_read: bool,
    },
    /// Every message in a conversation.
    Thread {
        id: u64,
    },
    /// Blocks until someone replies to message `id`.
    WaitReply {
        id: u64,
        timeout_secs: u64,
    },
    ChatSend {
        rig: String,
        topic: Option<String>,
        sender: Option<String>,
        body: String,
    },
    ChatHistory {
        rig: String,
        topic: Option<String>,
        after: Option<u64>,
        #[serde(default)]
        limit: usize,
    },
    /// Blocks until a chat message after `after` arrives.
    ChatWait {
        rig: String,
        topic: Option<String>,
        after: Option<u64>,
        timeout_secs: u64,
    },
    QueueCreate(QSpec),
    QueueList {
        rig: Option<String>,
        /// Items owned by or addressed to this agent or seat.
        who: Option<String>,
        state: Option<QState>,
        /// Include done and canceled items.
        #[serde(default)]
        all: bool,
    },
    QueueShow {
        id: String,
    },
    QueueClaim {
        id: String,
        by: Option<String>,
    },
    QueueUnclaim {
        id: String,
        by: Option<String>,
        reason: Option<String>,
    },
    QueueUpdate {
        id: String,
        by: Option<String>,
        note: Option<String>,
        state: Option<QState>,
    },
    /// Closes an item and opens its successor for another seat.
    QueueHandoff {
        id: String,
        by: Option<String>,
        to: String,
        body: String,
        summary: Option<String>,
    },
    /// Blocks an owned item on another item or `external:<name>`.
    QueueBlock {
        id: String,
        by: Option<String>,
        on: String,
        continuation: String,
        wake_after_secs: Option<u64>,
    },
    WatchdogRegister {
        target: String,
        interval_secs: u64,
        message: Option<String>,
        registered_by: Option<String>,
    },
    WatchdogList,
    WatchdogStop {
        id: String,
        reason: Option<String>,
    },
    /// Nudges a seat stuck at a prompt: presses Enter, or relaunches it if
    /// it has stopped.
    SeatContinue {
        seat: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpawnSpec {
    pub runtime: Runtime,
    pub cwd: PathBuf,
    pub role: Option<String>,
    pub task: Option<String>,
    pub parent: Option<String>,
    /// Extra arguments for the agent CLI.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpSpec {
    /// A spec file path, a library name (`starter`), or a known rig's name.
    pub source: String,
    /// Overrides every seat's working directory (OpenRig's `--cwd`).
    pub cwd: Option<PathBuf>,
    /// Seats to start fresh when restoring.
    #[serde(default)]
    pub fresh: Vec<String>,
    /// Rename the rig.
    pub name: Option<String>,
    /// Only describe what would launch (`--plan`); nothing starts.
    #[serde(default)]
    pub plan: bool,
}

/// One seat to add, in the terms of a RigSpec member.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeatSpec {
    pub pod: String,
    pub member: String,
    pub runtime: Option<Runtime>,
    pub role: Option<String>,
    pub label: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
    /// `local:` / `path:` agent spec reference, resolved against the rig root.
    pub agent_ref: Option<String>,
    /// Extra guidance for this seat's system prompt.
    pub guidance: Option<String>,
    /// Typed into the seat once it is ready.
    pub first_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QSpec {
    pub rig: Option<String>,
    pub source: Option<String>,
    pub destination: String,
    pub summary: String,
    #[serde(default)]
    pub body: String,
    pub priority: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub reply_to: Option<String>,
    /// Tell the destination about it right away (default).
    #[serde(default = "yes")]
    pub nudge: bool,
}

fn yes() -> bool {
    true
}

/// A library entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecEntry {
    pub name: String,
    pub summary: Option<String>,
    pub seats: usize,
    /// `builtin` or the file it came from.
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capture {
    pub agent: String,
    pub label: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error {
        message: String,
    },
    Hello {
        version: u32,
        pid: u32,
    },
    Snapshot {
        agents: Vec<Agent>,
        inbox: Vec<InboxItem>,
        #[serde(default)]
        rigs: Vec<Rig>,
        #[serde(default)]
        messages: Vec<Message>,
        #[serde(default)]
        chat: Vec<ChatMessage>,
        #[serde(default)]
        queue: Vec<QItem>,
    },
    Spawned {
        agent: Agent,
    },
    Screen {
        rows: Vec<String>,
        /// With `formatted`: the screen state as escape sequences.
        #[serde(default)]
        ansi: Option<String>,
        #[serde(default)]
        size: (u16, u16),
        /// With `formatted` and `history`: each of `rows` with its colours
        /// and attributes, as text with SGR sequences that starts from the
        /// default attributes and ends reset, to print at the start of a
        /// row. Absent from daemons that cannot (and for adopted tmux
        /// panes), which leaves the plain rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        styled_rows: Option<Vec<String>>,
    },
    /// What `ainet hook` prints for Claude Code, if anything.
    HookReply {
        output: Option<Value>,
    },
    Event {
        event: Event,
    },
    Rigs {
        rigs: Vec<Rig>,
    },
    Rig {
        rig: Rig,
    },
    Snapshots {
        snapshots: Vec<SnapshotInfo>,
    },
    Specs {
        specs: Vec<SpecEntry>,
    },
    /// YAML, a report, a transcript: anything that is just text.
    Text {
        text: String,
    },
    Discovered {
        sessions: Vec<Discovered>,
    },
    Whoami {
        whoami: Box<Whoami>,
    },
    Captures {
        captures: Vec<Capture>,
    },
    Messages {
        messages: Vec<Message>,
    },
    Chat {
        messages: Vec<ChatMessage>,
    },
    QItems {
        items: Vec<QItem>,
    },
    QItem {
        item: Box<QItem>,
    },
    Watchdogs {
        watchdogs: Vec<Watchdog>,
    },
    /// An agent's conversation (`Request::Conversation`).
    Conversation {
        conversation: Conversation,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Agent {
        agent: Agent,
    },
    AgentRemoved {
        id: String,
    },
    Inbox {
        item: InboxItem,
    },
    InboxResolved {
        id: u64,
    },
    /// New output in a pane; the TUI refetches the screen if it shows it.
    Output {
        agent: String,
    },
    /// What an attached terminal's program printed (`Request::Attach`), in
    /// order, as text: a character split between two reads is sent whole
    /// with the second, and bytes that are not UTF-8 become U+FFFD (the
    /// screen sent once the output settles mends what they spoilt). Sent
    /// only on the attaching connection, never to subscribers, whose
    /// clients may not know it.
    Bytes {
        agent: String,
        data: String,
    },
    Rig {
        rig: Rig,
    },
    RigRemoved {
        name: String,
    },
    Message {
        message: Message,
    },
    Chat {
        message: ChatMessage,
    },
    Queue {
        item: QItem,
    },
    Watchdog {
        watchdog: Watchdog,
    },
}

/// Encodes a message as one line, newline included.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut line = serde_json::to_vec(msg).unwrap_or_else(|_| b"{}".to_vec());
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_tagged() {
        let line = encode(&Request::Send {
            agent: "a".into(),
            text: "hi".into(),
        });
        assert_eq!(
            std::str::from_utf8(&line).unwrap(),
            "{\"op\":\"send\",\"agent\":\"a\",\"text\":\"hi\"}\n"
        );
        let back: Request = serde_json::from_slice(&line).unwrap();
        assert!(matches!(back, Request::Send { .. }));
    }

    #[test]
    fn answers_without_a_reason_are_the_old_line() {
        let plain = Request::Answer {
            item: 3,
            decision: Decision::Deny,
            message: None,
        };
        assert_eq!(
            std::str::from_utf8(&encode(&plain)).unwrap(),
            "{\"op\":\"answer\",\"item\":3,\"decision\":\"deny\"}\n"
        );
        let old: Request = serde_json::from_str(r#"{"op":"answer","item":3,"decision":"allow"}"#).unwrap();
        assert!(matches!(old, Request::Answer { message: None, .. }));
        let why: Request =
            serde_json::from_str(r#"{"op":"answer","item":3,"decision":"deny","message":"use cargo"}"#)
                .unwrap();
        assert!(matches!(why, Request::Answer { message: Some(ref m), .. } if m == "use cargo"));
    }

    #[test]
    fn old_screen_requests_still_parse() {
        let r: Request = serde_json::from_str(r#"{"op":"screen","agent":"a"}"#).unwrap();
        assert!(matches!(
            r,
            Request::Screen {
                formatted: false,
                history: 0,
                ..
            }
        ));
    }

    #[test]
    fn attaching_is_an_addition_old_peers_can_ignore() {
        let line = encode(&Request::Attach { agent: "a".into() });
        assert_eq!(
            std::str::from_utf8(&line).unwrap(),
            "{\"op\":\"attach\",\"agent\":\"a\"}\n"
        );
        // A screen without colours in its scrollback is the screen old
        // daemons send, and new ones send it unchanged to old clients.
        let old = r#"{"type":"screen","rows":["hi"],"ansi":null,"size":[2,10]}"#;
        let screen: Response = serde_json::from_str(old).unwrap();
        assert!(matches!(
            screen,
            Response::Screen {
                styled_rows: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_string(&screen).unwrap(), old);
        let bytes = encode(&Response::Event {
            event: Event::Bytes {
                agent: "a".into(),
                data: "\u{1b}[1mhi".into(),
            },
        });
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            "{\"type\":\"event\",\"event\":{\"kind\":\"bytes\",\"agent\":\"a\",\"data\":\"\\u001b[1mhi\"}}\n"
        );
    }

    #[test]
    fn conversations_are_an_addition_old_peers_can_ignore() {
        let ask = Request::Conversation {
            agent: "a".into(),
            tail: 50,
            after: None,
        };
        assert_eq!(
            std::str::from_utf8(&encode(&ask)).unwrap(),
            "{\"op\":\"conversation\",\"agent\":\"a\",\"tail\":50}\n"
        );
        let more: Request = serde_json::from_str(r#"{"op":"conversation","agent":"a","after":9}"#).unwrap();
        assert!(matches!(
            more,
            Request::Conversation {
                tail: 0,
                after: Some(9),
                ..
            }
        ));
        let mut entry = crate::ConversationEntry::new(4, crate::conversation::EntryKind::You, "hi");
        entry.at = Some(1);
        let reply = Response::Conversation {
            conversation: Conversation {
                entries: vec![entry],
                model: Some("Opus 5.5".into()),
                ..Conversation::default()
            },
        };
        let line = encode(&reply);
        assert_eq!(
            std::str::from_utf8(&line).unwrap(),
            "{\"type\":\"conversation\",\"conversation\":{\"entries\":[{\"id\":4,\"kind\":\"you\",\"at\":1,\"text\":\"hi\"}],\"model\":\"Opus 5.5\"}}\n"
        );
        let back: Response = serde_json::from_slice(&line).unwrap();
        assert!(matches!(back, Response::Conversation { conversation } if conversation.entries[0].id == 4));
    }

    #[test]
    fn runtimes_accept_openrig_names() {
        let r: Runtime = serde_json::from_str("\"claude-code\"").unwrap();
        assert_eq!(r, Runtime::ClaudeCode);
        let t: Runtime = serde_json::from_str("\"terminal\"").unwrap();
        assert_eq!(t, Runtime::Shell);
        assert_eq!("codex".parse::<Runtime>().unwrap(), Runtime::Codex);
    }
}
