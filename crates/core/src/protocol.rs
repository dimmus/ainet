//! The wire protocol: one JSON object per line over the daemon's Unix socket.
//! Every request gets exactly one response, except `Subscribe`, after which
//! the connection carries `Response::Event` lines until it closes, and
//! `Attach`, after which it carries one terminal's screens and output.
//!
//! Requests made on behalf of an agent (the MCP server, `ainet msg …` run in
//! an agent's shell) carry that agent's id from `AINET_AGENT_ID` in their
//! `from` / `by` / `sender` fields; `None` means you, the operator.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{Agent, Decision, InboxItem, Runtime};
use crate::comms::{ChatMessage, Message, QItem, QState, Watchdog};
use crate::conversation::Conversation;
use crate::rig::{Discovered, Edge, Rig, SnapshotInfo, Whoami};
use crate::settings::{Effort, PermissionMode, SettingsReport};

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
        /// 0 (or left out): the daemon's default
        /// (`queue.watchdog.default_interval_secs`). Daemons from before it
        /// need one and refuse 0 as too short.
        #[serde(default)]
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

    // ---- permissions -------------------------------------------------------
    /// `/always` rules in force: an agent's (by any name `find_agent`
    /// takes), or every agent's and the global ones from settings.json.
    /// Daemons from before it answer "bad request".
    AlwaysList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// Revokes an agent's `/always` rule by its key (`AlwaysRule::key`),
    /// or all of its rules. Global rules are revoked in settings.json.
    AlwaysRevoke {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
    },

    // ---- settings ----------------------------------------------------------
    // Daemons from before these answer "bad request"; clients then read
    // or write settings.json themselves and say the daemon is older.
    /// The settings in force (`Response::Settings`): every one, or only
    /// `key` (a setting, or a group such as `queue`).
    SettingsGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    /// Sets one setting in settings.json, keeping the rest of the file as
    /// written, and puts it in force. Unknown keys and values that would
    /// not read are refused and nothing is written.
    SettingsSet {
        key: String,
        value: Value,
    },
    /// Removes a setting (or a group) from settings.json, so its default
    /// applies.
    SettingsUnset {
        key: String,
    },
    /// Reads settings.json again now (after an edit by hand), instead of
    /// within the second.
    SettingsReload,
    /// Whether ainet's hooks are in Claude Code's settings
    /// (`Response::Hooks`).
    HookStatus,
    /// Installs (or repairs) ainet's hooks in Claude Code's settings,
    /// keeping a backup. Only on your say-so: a button, a command.
    HookInstall,
    /// Removes ainet's hooks from Claude Code's settings, keeping a
    /// backup.
    HookUninstall,
    /// The paths ainetd uses, and its version (`Response::Paths`).
    Paths,
    /// The last `lines` lines of ainetd's log file (`Response::Log`).
    /// Daemons from before it answer "bad request"; clients then read the
    /// file themselves.
    LogTail {
        lines: usize,
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
    /// Reasoning effort, over `runtimes.<runtime>.effort`. Older daemons
    /// ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Claude Code's permission mode, over
    /// `runtimes.claude.permission_mode`. Older daemons ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// More environment variables for the agent (not ainet's own).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl SpawnSpec {
    /// An agent of `runtime` in `cwd`, with nothing else asked for.
    pub fn new(runtime: Runtime, cwd: PathBuf) -> SpawnSpec {
        SpawnSpec {
            runtime,
            cwd,
            role: None,
            task: None,
            parent: None,
            args: Vec::new(),
            effort: None,
            permission_mode: None,
            env: BTreeMap::new(),
        }
    }
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
    /// Reasoning effort. Older daemons ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Claude Code's permission mode, over the runtime's and a spec
    /// policy. Older daemons ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// More arguments for the agent CLI, after ainet's and the runtime's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// More environment variables (not ainet's own).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// An `/always` rule in force (`Request::AlwaysList`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlwaysRule {
    /// The agent it allows for; `None` for a global rule from
    /// settings.json (`permissions.global_rules`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// What names it: `Bash:<command>`, `<tool>:under:<dir>`,
    /// `WebFetch:<host>` or a tool name.
    pub key: String,
    /// What it allows, in words.
    pub scope: String,
    /// When it was made (ms since the epoch); 0 for a global rule.
    #[serde(default)]
    pub created: i64,
    /// Kept across sessions and restarts (`permissions.persist_always`).
    #[serde(default)]
    pub kept: bool,
}

/// ainet's hooks in Claude Code's settings (`Request::HookStatus`,
/// `HookInstall`, `HookUninstall`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookReport {
    /// Claude Code's settings file (`hooks.claude_settings`, else
    /// `settings.json` in its config directory).
    pub settings_path: PathBuf,
    pub exists: bool,
    /// Why the file could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Some hooks of ainet's are there.
    pub installed: bool,
    /// All of them are, as they should be: this binary, today's timeouts.
    pub current: bool,
    /// Hook events with no entry as it should be.
    #[serde(default)]
    pub missing: Vec<String>,
    /// Entries of ainet's that run another binary or another timeout.
    #[serde(default)]
    pub stale: usize,
    /// One line on it, for people.
    pub summary: String,
    /// After an install or removal: what it did, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// The file as it was before that change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// Every hook event ainet installs, in order; those not in `missing`
    /// are in place. Empty from daemons before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
}

/// Where ainetd keeps things, and which ainetd it is (`Request::Paths`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsReport {
    pub socket: PathBuf,
    /// The database and logs.
    pub state_dir: PathBuf,
    pub database: PathBuf,
    /// Where an autostarted ainetd writes its log.
    pub log: PathBuf,
    /// settings.json; none for a daemon that follows no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<PathBuf>,
    /// Where your own specs are looked for, in order.
    #[serde(default)]
    pub spec_dirs: Vec<PathBuf>,
    /// Claude Code's config directory (`$CLAUDE_CONFIG_DIR`, else
    /// `~/.claude`).
    pub claude_config_dir: PathBuf,
    /// The `ainet` binary seats' MCP server and hooks run.
    pub exe: PathBuf,
    /// ainet's version (the daemon's, from ainetd).
    pub version: String,
    pub protocol: u32,
    /// ainetd's process id; 0 when no daemon answered.
    #[serde(default)]
    pub pid: u32,
    /// When this ainetd started (ms since the epoch); 0 when no daemon
    /// answered, or one from before it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub started: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// The end of ainetd's log file (`Request::LogTail`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogTail {
    /// The file: where an autostarted ainetd writes its log.
    pub path: PathBuf,
    /// Whether it is there (an ainetd started by hand logs to its
    /// terminal instead).
    pub exists: bool,
    /// Its last lines, oldest first.
    #[serde(default)]
    pub lines: Vec<String>,
}

impl LogTail {
    /// The last `lines` lines of the log file at `path`, read from its
    /// end (it can be large). A file that is not there is said so.
    pub fn read(path: &std::path::Path, lines: usize) -> std::io::Result<LogTail> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LogTail {
                    path: path.to_path_buf(),
                    ..LogTail::default()
                });
            }
            Err(e) => return Err(e),
        };
        // A line is rarely longer than 512 bytes; reading this much from
        // the end is enough without reading a large log whole.
        let want = (lines as u64).saturating_mul(512).max(4096);
        let len = file.metadata()?.len();
        let from = len.saturating_sub(want);
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let mut all: Vec<&str> = text.lines().collect();
        // The first line read from the middle of the file is a part.
        if from > 0 && !all.is_empty() {
            all.remove(0);
        }
        let skip = all.len().saturating_sub(lines);
        Ok(LogTail {
            path: path.to_path_buf(),
            exists: true,
            lines: all[skip..].iter().map(|l| l.to_string()).collect(),
        })
    }
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
    /// Tell the destination about it right away. Left out, the daemon's
    /// default (`queue.nudge`, on unless settings.json says otherwise);
    /// older daemons take a missing value as yes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<bool>,
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
    /// `/always` rules (`Request::AlwaysList`).
    AlwaysRules {
        rules: Vec<AlwaysRule>,
    },
    /// The settings in force (`Request::SettingsGet`, and after a set,
    /// an unset or a reload).
    Settings {
        settings: Box<SettingsReport>,
    },
    /// ainet's hooks in Claude Code's settings.
    Hooks {
        hooks: Box<HookReport>,
    },
    /// The paths in use, and versions.
    Paths {
        paths: Box<PathsReport>,
    },
    /// The end of ainetd's log (`Request::LogTail`).
    Log {
        log: Box<LogTail>,
    },
}

/// Whether a daemon's error says it does not know the request: it is
/// older than the client.
pub fn unknown_request(message: &str) -> bool {
    message.starts_with("bad request")
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
    fn nudge_and_watchdog_intervals_may_be_left_to_the_daemon() {
        // An old client's line: nudge said outright.
        let old: Request = serde_json::from_str(
            r#"{"op":"queue_create","rig":null,"source":null,"destination":"d","summary":"s","priority":null,"reply_to":null,"nudge":true}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            Request::QueueCreate(QSpec {
                nudge: Some(true),
                ..
            })
        ));
        // A new client leaving it to the daemon sends no nudge, which an
        // old daemon reads as yes, its only default.
        let line = encode(&Request::QueueCreate(QSpec {
            destination: "d".into(),
            summary: "s".into(),
            ..QSpec::default()
        }));
        let text = std::str::from_utf8(&line).unwrap();
        assert!(!text.contains("nudge"), "{text}");
        let no = encode(&Request::QueueCreate(QSpec {
            nudge: Some(false),
            ..QSpec::default()
        }));
        assert!(std::str::from_utf8(&no).unwrap().contains(r#""nudge":false"#));
        let dog: Request = serde_json::from_str(r#"{"op":"watchdog_register","target":"t"}"#).unwrap();
        assert!(matches!(dog, Request::WatchdogRegister { interval_secs: 0, .. }));
        let line = encode(&Request::WatchdogRegister {
            target: "t".into(),
            interval_secs: 600,
            message: None,
            registered_by: None,
        });
        assert!(
            std::str::from_utf8(&line)
                .unwrap()
                .contains(r#""interval_secs":600"#)
        );
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

    #[test]
    fn seat_and_spawn_options_are_additive() {
        // Unset, the lines are what they always were.
        let seat = SeatSpec {
            pod: "p".into(),
            member: "m".into(),
            ..SeatSpec::default()
        };
        let line = String::from_utf8(encode(&seat)).unwrap();
        assert!(!line.contains("effort") && !line.contains("permission_mode") && !line.contains("env"));
        let spawn = SpawnSpec::new(Runtime::Shell, "/w".into());
        assert_eq!(
            String::from_utf8(encode(&spawn)).unwrap(),
            "{\"runtime\":\"shell\",\"cwd\":\"/w\",\"role\":null,\"task\":null,\"parent\":null,\"args\":[]}\n"
        );
        // Old lines read; new ones carry their options.
        let old: SeatSpec = serde_json::from_str(r#"{"pod":"p","member":"m","runtime":null,"role":null,"label":null,"model":null,"cwd":null,"agent_ref":null,"guidance":null,"first_message":null}"#).unwrap();
        assert_eq!(old.effort, None);
        assert!(old.args.is_empty() && old.env.is_empty());
        let new: SeatSpec = serde_json::from_str(
            r#"{"pod":"p","member":"m","effort":"high","permission_mode":"acceptEdits","args":["--verbose"],"env":{"A":"1"}}"#,
        )
        .unwrap();
        assert_eq!(new.effort, Some(Effort::High));
        assert_eq!(new.permission_mode, Some(PermissionMode::AcceptEdits));
        assert_eq!(new.args, vec!["--verbose".to_string()]);
        // A misspelt mode is refused, never taken as some mode.
        assert!(
            serde_json::from_str::<SeatSpec>(r#"{"pod":"p","member":"m","permission_mode":"bypass"}"#)
                .is_err()
        );
        let old_spawn: SpawnSpec =
            serde_json::from_str(r#"{"runtime":"codex","cwd":"/w","role":null,"task":null,"parent":null}"#)
                .unwrap();
        assert_eq!((old_spawn.effort, old_spawn.permission_mode), (None, None));
    }

    #[test]
    fn always_rules_round_trip() {
        let line = encode(&Request::AlwaysList { agent: None });
        assert_eq!(std::str::from_utf8(&line).unwrap(), "{\"op\":\"always_list\"}\n");
        let revoke: Request = serde_json::from_str(r#"{"op":"always_revoke","agent":"a1"}"#).unwrap();
        assert!(matches!(revoke, Request::AlwaysRevoke { rule: None, .. }));
        let reply = Response::AlwaysRules {
            rules: vec![AlwaysRule {
                agent: Some("a1".into()),
                key: "Bash:cargo test".into(),
                scope: "this exact command, cargo test".into(),
                created: 5,
                kept: true,
            }],
        };
        let back: Response = serde_json::from_slice(&encode(&reply)).unwrap();
        assert!(
            matches!(back, Response::AlwaysRules { rules } if rules[0].kept && rules[0].key == "Bash:cargo test")
        );
    }

    #[test]
    fn settings_requests_are_additions_old_peers_can_ignore() {
        // New requests, as lines: what an older daemon reads as "bad
        // request", which clients take as "older; restart it".
        for (req, line) in [
            (Request::SettingsGet { key: None }, r#"{"op":"settings_get"}"#),
            (
                Request::SettingsGet {
                    key: Some("queue".into()),
                },
                r#"{"op":"settings_get","key":"queue"}"#,
            ),
            (
                Request::SettingsSet {
                    key: "queue.nudge".into(),
                    value: serde_json::json!(false),
                },
                r#"{"op":"settings_set","key":"queue.nudge","value":false}"#,
            ),
            (
                Request::SettingsUnset { key: "queue".into() },
                r#"{"op":"settings_unset","key":"queue"}"#,
            ),
            (Request::SettingsReload, r#"{"op":"settings_reload"}"#),
            (Request::HookStatus, r#"{"op":"hook_status"}"#),
            (Request::HookInstall, r#"{"op":"hook_install"}"#),
            (Request::HookUninstall, r#"{"op":"hook_uninstall"}"#),
            (Request::Paths, r#"{"op":"paths"}"#),
        ] {
            let encoded = String::from_utf8(encode(&req)).unwrap();
            assert_eq!(encoded.trim_end(), line);
            let back: Request = serde_json::from_str(line).unwrap();
            assert_eq!(String::from_utf8(encode(&back)).unwrap(), encoded);
        }
        assert!(unknown_request("bad request: unknown variant `paths`"));
        assert!(!unknown_request("no agent x"));

        let report = crate::settings::SettingsReport::new(
            &crate::settings::parse(r#"{"queue": {"nudge": false}}"#).unwrap(),
            None,
            &[],
            Some("/c/settings.json".into()),
        );
        let reply = Response::Settings {
            settings: Box::new(report.clone()),
        };
        let line = encode(&reply);
        let text = std::str::from_utf8(&line).unwrap();
        assert!(text.starts_with(r#"{"type":"settings","settings":{"file":"/c/settings.json","entries":["#));
        assert!(
            !text.contains("\"changed\"") && !text.contains("\"notice\""),
            "{text}"
        );
        match serde_json::from_slice::<Response>(&line).unwrap() {
            Response::Settings { settings } => {
                assert_eq!(*settings, report);
                let nudge = settings.get("queue.nudge").unwrap();
                assert_eq!(nudge.source, crate::settings::Source::File);
                assert_eq!(nudge.applies, crate::settings::Applies::Live);
            }
            other => panic!("{other:?}"),
        }
        // An entry from a client that knows no `applies` or description.
        let old: crate::settings::Effective =
            serde_json::from_str(r#"{"key":"a.b","value":1,"default":1,"source":{"from":"default"}}"#)
                .unwrap();
        assert_eq!(old.applies, crate::settings::Applies::Live);

        let hooks = Response::Hooks {
            hooks: Box::new(HookReport {
                settings_path: "/c/claude.json".into(),
                summary: "not installed in /c/claude.json".into(),
                missing: vec!["Stop".into()],
                ..HookReport::default()
            }),
        };
        let back: Response = serde_json::from_slice(&encode(&hooks)).unwrap();
        assert!(
            matches!(back, Response::Hooks { hooks } if hooks.missing == ["Stop"] && hooks.done.is_none())
        );
        let paths = Response::Paths {
            paths: Box::new(PathsReport {
                version: "0.1.0".into(),
                protocol: VERSION,
                pid: 7,
                ..PathsReport::default()
            }),
        };
        let back: Response = serde_json::from_slice(&encode(&paths)).unwrap();
        assert!(matches!(back, Response::Paths { paths } if paths.pid == 7 && paths.settings.is_none()));
    }

    #[test]
    fn log_tails_uptime_and_hook_events_are_additions_old_peers_can_ignore() {
        let req = Request::LogTail { lines: 50 };
        let line = String::from_utf8(encode(&req)).unwrap();
        assert_eq!(line.trim_end(), r#"{"op":"log_tail","lines":50}"#);
        let log = Response::Log {
            log: Box::new(LogTail {
                path: "/s/ainetd.log".into(),
                exists: true,
                lines: vec!["INFO listening".into()],
            }),
        };
        let back: Response = serde_json::from_slice(&encode(&log)).unwrap();
        assert!(matches!(back, Response::Log { log } if log.exists && log.lines == ["INFO listening"]));

        // From a daemon before them: no start time, no list of events.
        let old_paths: PathsReport = serde_json::from_str(
            r#"{"socket":"/s","state_dir":"/d","database":"/d/a.db","log":"/d/l","claude_config_dir":"/c","exe":"/a","version":"0.1.0","protocol":2,"pid":9}"#,
        )
        .unwrap();
        assert_eq!((old_paths.pid, old_paths.started), (9, 0));
        let old_hooks: HookReport = serde_json::from_str(
            r#"{"settings_path":"/c/s.json","exists":true,"installed":false,"current":false,"summary":"not installed"}"#,
        )
        .unwrap();
        assert!(old_hooks.events.is_empty());
        // And to one: unset, they are not sent.
        let text = serde_json::to_string(&PathsReport::default()).unwrap();
        assert!(!text.contains("started"), "{text}");
        let text = serde_json::to_string(&HookReport::default()).unwrap();
        assert!(!text.contains("events"), "{text}");
    }

    #[test]
    fn a_log_tail_is_its_last_lines() {
        let dir = std::env::temp_dir().join(format!("ainet-core-logtail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ainetd.log");
        let missing = LogTail::read(&path, 3).unwrap();
        assert!(!missing.exists && missing.lines.is_empty());
        let text: String = (1..=5000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, text).unwrap();
        let tail = LogTail::read(&path, 3).unwrap();
        assert!(tail.exists);
        assert_eq!(tail.lines, ["line 4998", "line 4999", "line 5000"]);
        std::fs::write(&path, "only\n").unwrap();
        assert_eq!(LogTail::read(&path, 3).unwrap().lines, ["only"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
