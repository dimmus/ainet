//! The sections of `~/.config/ainet/settings.json` that ainetd and the
//! console read: `daemon`, `runtimes`, `permissions`, `messaging`, `queue`,
//! `rigs`, `discovery`, `hooks`, `retention` and `cost`. The desktop app's
//! own keys (`theme`, `terminal`, …) sit in the same file; each reader
//! ignores what it does not own and keeps it when writing.
//!
//! Every field is optional and its default is what ainet did before it
//! could be configured, so a missing file changes nothing. A file that
//! does not parse, or says something out of range, is reported and the
//! last good settings stay ([`Follower`]). An environment variable that
//! already meant something wins over the file (`AINET_SOCKET`,
//! `AINET_LOG`): environment, then file, then default.
//!
//! Settings that would make agents more permissive are opt-in, say so in
//! their descriptions, and only take values from a closed list, so a typo
//! is an error rather than a looser agent.

mod edit;
mod effective;
mod load;
mod policy;
mod report;
mod schema_walk;

use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use edit::{check_key, set, set_in_file, unset, unset_in_file, value_from_text, write_whole};
pub use effective::{
    Applies, Effective, Source, applies, args_loosen, effective, env_overrides, less_safe, unknown_keys,
};
pub use load::{Follower, Loaded, load, load_section, parse, parse_section, schema};
pub use policy::{GlobalRule, SPEC_POLICIES, spec_policy};
pub use report::SettingsReport;
pub use schema_walk::{description, takes_text};

/// The top-level keys this module owns; the rest of the file is the
/// desktop app's.
pub const SECTIONS: &[&str] = &[
    "daemon",
    "runtimes",
    "permissions",
    "messaging",
    "queue",
    "rigs",
    "discovery",
    "hooks",
    "retention",
    "cost",
];

/// What ainetd and the console read from settings.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Settings {
    /// ainetd itself: starting it, its log, its socket.
    pub daemon: DaemonSettings,
    /// How Claude Code, Codex and shells are started in seats.
    pub runtimes: RuntimeSettings,
    /// Permission requests held for you, and `/always` rules.
    pub permissions: PermissionSettings,
    /// Messages typed into agents' terminals.
    pub messaging: MessagingSettings,
    /// Queue items and watchdogs.
    pub queue: QueueSettings,
    /// Rigs, their specs and snapshots.
    pub rigs: RigSettings,
    /// Agents ainet did not start.
    pub discovery: DiscoverySettings,
    /// ainet's hooks in Claude Code's settings.
    pub hooks: HookSettings,
    /// What ainetd keeps, and for how long.
    pub retention: RetentionSettings,
    /// Token prices and context windows.
    pub cost: CostSettings,
}

// ---- daemon -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DaemonSettings {
    /// Start ainetd in the background when the console or the desktop app
    /// needs it and it is not running. Off, they say so and you start it
    /// yourself (`ainet daemon`).
    pub autostart: bool,
    /// The daemon's socket. `$AINET_SOCKET` wins; unset, it is
    /// `$XDG_RUNTIME_DIR/ainet.sock`. After a daemon restart.
    pub socket: Option<PathBuf>,
    /// Start again the seats of rigs that were running when ainetd last
    /// stopped. Off, they come back stopped and `ainet up <rig>` restores
    /// them.
    pub restore_rigs: bool,
    /// ainetd's log. After a daemon restart.
    pub log: LogSettings,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        DaemonSettings {
            autostart: true,
            socket: None,
            restore_rigs: false,
            log: LogSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LogSettings {
    /// What to log: `error`, `warn`, `info`, `debug`, `trace`, or a
    /// tracing filter (`info,ainet_daemon=debug`). `$AINET_LOG` wins.
    pub level: String,
    /// Where an autostarted ainetd writes its log. Unset, it is
    /// `ainetd.log` in ainet's state directory.
    pub file: Option<PathBuf>,
    /// Add to the log instead of starting it afresh on each autostart.
    pub append: bool,
    /// Once the log is this many megabytes, it is moved aside
    /// (`ainetd.log.1`, …) when ainetd next starts. 0 never rotates.
    #[schemars(range(max = 10_000))]
    pub max_size_mb: u64,
    /// How many rotated logs to keep.
    #[schemars(range(min = 1, max = 20))]
    pub keep: u32,
}

impl LogSettings {
    /// Where an autostarted ainetd writes its log: `file`, else
    /// `ainetd.log` in the state directory.
    pub fn path(&self) -> PathBuf {
        self.file
            .as_deref()
            .map(crate::paths::expand)
            .unwrap_or_else(crate::paths::daemon_log)
    }
}

impl Default for LogSettings {
    fn default() -> Self {
        LogSettings {
            level: "info".into(),
            file: None,
            append: false,
            max_size_mb: 0,
            keep: 1,
        }
    }
}

// ---- runtimes ---------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RuntimeSettings {
    /// Claude Code seats.
    pub claude: AgentRuntime,
    /// Codex seats.
    pub codex: AgentRuntime,
    /// Shell seats (OpenRig's `terminal` nodes).
    pub shell: ShellRuntime,
    /// When a new seat counts as ready for its first message.
    pub readiness: Readiness,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AgentRuntime {
    /// The program: a name on `PATH` or a path. Unset, `claude` or
    /// `codex`.
    pub binary: Option<String>,
    /// The model of seats whose spec names none.
    pub model: Option<String>,
    /// Reasoning effort of every new seat (Claude Code's `--effort`,
    /// Codex's `model_reasoning_effort`). Unset, the agent's own default.
    pub effort: Option<Effort>,
    /// Pass the effort a rig spec or a seat names on to the agent, over
    /// `effort`. Off, a spec's effort is read and dropped, as before.
    pub spec_effort: bool,
    /// Claude Code only: its permission mode for new seats. Unset, Claude
    /// Code's own default. `acceptEdits`, `auto`, `dontAsk` and
    /// `bypassPermissions` let agents act without asking you;
    /// `bypassPermissions` skips every permission check.
    pub permission_mode: Option<PermissionMode>,
    /// Further arguments for every seat of this runtime, after ainet's.
    pub args: Vec<String>,
    /// Further environment variables for every seat of this runtime.
    /// ainet's own (`AINET_*`, `OPENRIG_*`) cannot be set here.
    pub env: BTreeMap<String, String>,
    /// Claude Code only: tools allowed without asking, beyond ainet's own
    /// (`mcp__ainet`, always allowed). Less safe: the agent no longer asks
    /// before using them.
    pub allowed_tools: Vec<String>,
    /// Claude Code only: tools the agent may not use.
    pub disallowed_tools: Vec<String>,
    /// MCP servers given to every seat besides ainet's, by name, as
    /// Claude Code's `mcpServers` entries (`{"command": …, "args": […],
    /// "env": {…}}` or `{"type": "http", "url": …}`).
    pub mcp_servers: BTreeMap<String, Value>,
    /// Claude Code only: only ainet's MCP server and `mcp_servers`, none
    /// of your own.
    pub strict_mcp_config: bool,
}

/// Claude Code's `--effort` levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 5] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];

    /// The name Claude Code takes.
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }

    /// A spec's `effort:`, by name; anything else is `None`.
    pub fn parse(s: &str) -> Option<Effort> {
        let s = s.trim().to_ascii_lowercase();
        Effort::ALL.into_iter().find(|e| e.as_str() == s)
    }
}

/// Claude Code's `--permission-mode` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PermissionMode {
    /// Ask before anything risky (Claude Code's default).
    #[serde(rename = "default")]
    Default,
    /// Plan only; change nothing.
    #[serde(rename = "plan")]
    Plan,
    /// Less safe: file edits without asking.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// Less safe: Claude Code decides what needs asking.
    #[serde(rename = "auto")]
    Auto,
    /// Less safe: never ask; what would need asking is refused.
    #[serde(rename = "dontAsk")]
    DontAsk,
    /// Unsafe: no permission checks at all.
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
}

impl PermissionMode {
    pub const ALL: [PermissionMode; 6] = [
        PermissionMode::Default,
        PermissionMode::Plan,
        PermissionMode::AcceptEdits,
        PermissionMode::Auto,
        PermissionMode::DontAsk,
        PermissionMode::BypassPermissions,
    ];

    /// The name Claude Code takes (`--permission-mode`).
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::Plan => "plan",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Auto => "auto",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::BypassPermissions => "bypassPermissions",
        }
    }

    /// Whether agents in this mode do things without asking you that
    /// Claude Code's default would ask about.
    pub fn loosens(self) -> bool {
        !matches!(self, PermissionMode::Default | PermissionMode::Plan)
    }
}

impl std::str::FromStr for PermissionMode {
    type Err = String;

    /// Exactly Claude Code's names: a typo is an error, never a mode.
    fn from_str(s: &str) -> Result<Self, String> {
        PermissionMode::ALL
            .into_iter()
            .find(|m| m.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown permission mode {s:?}: use default, plan, acceptEdits, auto, dontAsk or bypassPermissions"
                )
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ShellRuntime {
    /// The shell. Unset, `$SHELL`, else `/bin/sh`.
    pub binary: Option<String>,
    /// Further arguments for every shell seat.
    pub args: Vec<String>,
    /// Further environment variables for every shell seat.
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Readiness {
    /// How long to wait for a new seat to be ready before typing its
    /// startup texts anyway, in seconds.
    #[schemars(range(min = 1, max = 3600))]
    pub timeout_secs: u64,
    /// The same once Claude Code asks whether to trust the folder (you
    /// answer that), in seconds.
    #[schemars(range(min = 1, max = 86_400))]
    pub trust_timeout_secs: u64,
    /// How often the screen is checked, in milliseconds.
    #[schemars(range(min = 50, max = 10_000))]
    pub poll_ms: u64,
    /// Quiet time after the last output before a seat counts as ready,
    /// in milliseconds, per runtime.
    pub quiet_ms: QuietMs,
    /// Pause between a seat's startup texts, in milliseconds.
    #[schemars(range(max = 60_000))]
    pub startup_text_gap_ms: u64,
}

impl Default for Readiness {
    fn default() -> Self {
        Readiness {
            timeout_secs: 60,
            trust_timeout_secs: 600,
            poll_ms: 300,
            quiet_ms: QuietMs::default(),
            startup_text_gap_ms: 1200,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct QuietMs {
    /// Claude Code seats, in milliseconds.
    #[schemars(range(max = 60_000))]
    pub claude: u64,
    /// Codex seats, in milliseconds.
    #[schemars(range(max = 60_000))]
    pub codex: u64,
    /// Shell seats, in milliseconds.
    #[schemars(range(max = 60_000))]
    pub shell: u64,
}

impl Default for QuietMs {
    fn default() -> Self {
        QuietMs {
            claude: 800,
            codex: 1500,
            shell: 400,
        }
    }
}

// ---- permissions ------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PermissionSettings {
    /// How long a permission request waits for you in the Inbox before
    /// the agent's own terminal dialog takes over, in seconds. Seats
    /// started after a change get a hook timeout to match; hooks you
    /// installed yourself take it when installed again.
    #[schemars(range(min = 10, max = 3600))]
    pub hold_secs: u64,
    /// Keep `/always` rules after a session ends and across daemon
    /// restarts. Less safe: an answer given once keeps allowing. Turning
    /// it off forgets the rules kept so far.
    pub persist_always: bool,
    /// Paths `/always` never covers, on top of ainet's own list (`.ssh`,
    /// `.git`, `.env`, …): a name (`secrets.toml`, matched anywhere) or a
    /// path (`~/work/keys`, `/srv/private`) and everything under it.
    /// ainet's own list always applies.
    pub sensitive_paths: Vec<String>,
    /// Apply a rig spec's `permission_policy` to its Claude Code seats:
    /// `builtin:locked` plans only, `builtin:default` and
    /// `builtin:standard` are Claude Code's default, and `builtin:yolo`
    /// skips every permission check (unsafe). Off, a policy is reported
    /// and ignored, as before; unknown policies always are.
    pub spec_policies: bool,
    /// Rules that allow a request from any agent without asking, as an
    /// `/always` answer would: `{"command": "cargo test"}`, `{"tool":
    /// "WebSearch"}`, `{"fetch_host": "docs.rs"}` or `{"edits_under":
    /// {"tool": "Edit", "dir": "/work/app"}}`. Less safe: nobody is asked.
    pub global_rules: Vec<GlobalRule>,
}

impl Default for PermissionSettings {
    fn default() -> Self {
        PermissionSettings {
            hold_secs: 580,
            persist_always: false,
            sensitive_paths: Vec::new(),
            spec_policies: false,
            global_rules: Vec::new(),
        }
    }
}

impl PermissionSettings {
    /// The `PermissionRequest` hook's timeout in Claude Code's settings:
    /// longer than the hold, so ainet lets go before Claude Code ends the
    /// hook (600 s by default, as before).
    pub fn hook_timeout_secs(&self) -> u64 {
        self.hold_secs + 20
    }

    /// How long `ainet hook` waits for ainetd's answer: the hold and a
    /// little more, inside the hook's timeout.
    pub fn client_timeout_secs(&self) -> u64 {
        self.hold_secs + 10
    }
}

// ---- messaging --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MessagingSettings {
    /// Between pasting a message into a terminal and pressing Enter, in
    /// milliseconds: the agent's input box needs a moment to take it.
    #[schemars(range(max = 5000))]
    pub paste_settle_ms: u64,
    /// Between held messages delivered one after another, in
    /// milliseconds.
    #[schemars(range(max = 60_000))]
    pub flush_gap_ms: u64,
    /// Hold messages while you are at an agent's terminal, and deliver
    /// them when you leave it.
    pub hold_while_interacting: bool,
    /// New seats start with their typing guard on: messages are held
    /// until it is turned off.
    pub typing_guard: bool,
    /// The last line typed with a message, telling the agent how to
    /// answer. `{id}` is the message's number.
    pub message_note: String,
    /// The last line typed with a chatroom mention.
    pub mention_note: String,
}

impl Default for MessagingSettings {
    fn default() -> Self {
        MessagingSettings {
            paste_settle_ms: 60,
            flush_gap_ms: 400,
            hold_while_interacting: true,
            typing_guard: false,
            message_note: crate::conversation::MESSAGE_NOTE.into(),
            mention_note: crate::conversation::MENTION_NOTE.into(),
        }
    }
}

// ---- queue ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct QueueSettings {
    /// Tell a queue item's destination about it right away, unless the
    /// request says otherwise (`--no-nudge`).
    pub nudge: bool,
    /// Watchdogs: reminders typed into a seat every so often.
    pub watchdog: WatchdogSettings,
}

impl Default for QueueSettings {
    fn default() -> Self {
        QueueSettings {
            nudge: true,
            watchdog: WatchdogSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WatchdogSettings {
    /// The interval of a watchdog registered without one, in seconds.
    #[schemars(range(min = 1, max = 2_592_000))]
    pub default_interval_secs: u64,
    /// The shortest interval allowed, in seconds.
    #[schemars(range(min = 1, max = 86_400))]
    pub min_interval_secs: u64,
    /// What a watchdog registered without a message types.
    pub message: String,
    /// How often due watchdogs and blocked items' wakes are checked, in
    /// seconds.
    #[schemars(range(min = 1, max = 60))]
    pub tick_secs: u64,
}

/// The longest default watchdog interval: 30 days.
pub const MAX_WATCHDOG_INTERVAL_SECS: u64 = 2_592_000;

/// What a watchdog types when it was given no message.
pub const WATCHDOG_MESSAGE: &str = "[ainet] watchdog wake: check your queue (queue_list mine=true / ainet queue list --mine) and carry on with what you own.";

impl Default for WatchdogSettings {
    fn default() -> Self {
        WatchdogSettings {
            default_interval_secs: 600,
            min_interval_secs: 30,
            message: WATCHDOG_MESSAGE.into(),
            tick_secs: 5,
        }
    }
}

// ---- rigs -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RigSettings {
    /// More directories of your own specs, searched after
    /// `~/.config/ainet/specs` (the first one with a name wins).
    pub spec_dirs: Vec<PathBuf>,
    /// Where a rig created from one seat, or a loose agent, runs when no
    /// directory is given. Unset, the console's current directory, else
    /// your home.
    pub default_cwd: Option<PathBuf>,
    /// `ainet down` takes a snapshot first unless told `--no-snapshot`.
    pub snapshot_on_down: bool,
    /// Keep only this many snapshots per rig, the newest; older ones are
    /// deleted. Unset, every snapshot is kept.
    #[schemars(range(min = 1))]
    pub keep_snapshots: Option<u32>,
}

impl Default for RigSettings {
    fn default() -> Self {
        RigSettings {
            spec_dirs: Vec::new(),
            default_cwd: None,
            snapshot_on_down: true,
            keep_snapshots: None,
        }
    }
}

// ---- discovery --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DiscoverySettings {
    /// Show every Claude Code session on this machine in the Fleet, from
    /// `~/.claude/sessions`. Off, only agents ainet started or adopted,
    /// and sessions that report through ainet's hooks.
    pub claude_sessions: bool,
    /// Look for agents in tmux panes when asked to discover.
    pub tmux: bool,
    /// How often Claude Code's sessions are checked, in milliseconds.
    #[schemars(range(min = 500, max = 600_000))]
    pub interval_ms: u64,
    /// How often context use, model and last tool are read from
    /// transcripts, in milliseconds.
    #[schemars(range(min = 500, max = 600_000))]
    pub transcripts_ms: u64,
}

impl Default for DiscoverySettings {
    fn default() -> Self {
        DiscoverySettings {
            claude_sessions: true,
            tmux: true,
            interval_ms: 3000,
            transcripts_ms: 6000,
        }
    }
}

// ---- hooks ------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HookSettings {
    /// Claude Code's settings file that `ainet install-hooks --write`
    /// and `--remove` edit (a backup is kept). Unset, `settings.json` in
    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`. Nothing is written there
    /// without you asking; ainet's seats get their hooks without it.
    pub claude_settings: Option<PathBuf>,
}

// ---- retention --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RetentionSettings {
    /// Messages ainetd keeps in memory (all stay on disk).
    #[schemars(range(min = 100, max = 1_000_000))]
    pub messages_in_memory: usize,
    /// Chatroom posts ainetd keeps in memory (all stay on disk).
    #[schemars(range(min = 100, max = 1_000_000))]
    pub chat_in_memory: usize,
    /// Inbox items kept; answered ones go first, waiting ones never.
    #[schemars(range(min = 10, max = 100_000))]
    pub inbox_items: usize,
    /// Rows of scrollback each new terminal keeps.
    #[schemars(range(max = 1_000_000))]
    pub scrollback_lines: usize,
    /// Delete logged events older than this many days. Unset, kept.
    #[schemars(range(min = 1, max = 36_500))]
    pub events_max_age_days: Option<u32>,
    /// Keep at most this many logged events, the newest. Unset, all.
    #[schemars(range(min = 1000, max = 1_000_000_000_000_u64))]
    pub events_max_count: Option<u64>,
    /// Drop answered inbox items this many minutes old. Unset, they
    /// stay until the inbox is full.
    #[schemars(range(min = 1, max = 5_256_000))]
    pub inbox_expiry_mins: Option<u64>,
}

impl Default for RetentionSettings {
    fn default() -> Self {
        RetentionSettings {
            messages_in_memory: 5000,
            chat_in_memory: 5000,
            inbox_items: 200,
            scrollback_lines: 5000,
            events_max_age_days: None,
            events_max_count: None,
            inbox_expiry_mins: None,
        }
    }
}

// ---- cost -------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CostSettings {
    /// Prices by model-id prefix (`claude-opus-5`), over the built-in
    /// table; the longest matching prefix wins. Not used yet: no view
    /// shows cost.
    pub prices: BTreeMap<String, Price>,
    /// Context windows in tokens by model-id prefix, over ainet's guess
    /// (1M for a `[1m]` model, else 200k).
    pub context_windows: BTreeMap<String, u64>,
}

/// Dollars per million tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// Unset, 1.25 times `input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    /// Unset, 0.1 times `input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
}

impl CostSettings {
    /// The context window configured for `model`, by its longest
    /// matching prefix.
    pub fn context_window(&self, model: &str) -> Option<u64> {
        self.context_windows
            .iter()
            .filter(|(k, _)| !k.is_empty() && model.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, v)| *v)
    }
}

// ---- checks -----------------------------------------------------------------

impl Settings {
    /// What is out of range or makes no sense, one line each. Settings
    /// with any are not used.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut range = |key: &str, v: u64, min: u64, max: u64| {
            if v < min || v > max {
                out.push(format!("{key} is {v}; it must be {min} to {max}"));
            }
        };
        let log = &self.daemon.log;
        range("daemon.log.max_size_mb", log.max_size_mb, 0, 10_000);
        range("daemon.log.keep", log.keep.into(), 1, 20);
        let r = &self.runtimes.readiness;
        range("runtimes.readiness.timeout_secs", r.timeout_secs, 1, 3600);
        range(
            "runtimes.readiness.trust_timeout_secs",
            r.trust_timeout_secs,
            1,
            86_400,
        );
        range("runtimes.readiness.poll_ms", r.poll_ms, 50, 10_000);
        range("runtimes.readiness.quiet_ms.claude", r.quiet_ms.claude, 0, 60_000);
        range("runtimes.readiness.quiet_ms.codex", r.quiet_ms.codex, 0, 60_000);
        range("runtimes.readiness.quiet_ms.shell", r.quiet_ms.shell, 0, 60_000);
        range(
            "runtimes.readiness.startup_text_gap_ms",
            r.startup_text_gap_ms,
            0,
            60_000,
        );
        range("permissions.hold_secs", self.permissions.hold_secs, 10, 3600);
        let m = &self.messaging;
        range("messaging.paste_settle_ms", m.paste_settle_ms, 0, 5000);
        range("messaging.flush_gap_ms", m.flush_gap_ms, 0, 60_000);
        let w = &self.queue.watchdog;
        range("queue.watchdog.min_interval_secs", w.min_interval_secs, 1, 86_400);
        range(
            "queue.watchdog.default_interval_secs",
            w.default_interval_secs,
            w.min_interval_secs,
            MAX_WATCHDOG_INTERVAL_SECS,
        );
        range("queue.watchdog.tick_secs", w.tick_secs, 1, 60);
        if let Some(n) = self.rigs.keep_snapshots {
            range("rigs.keep_snapshots", n.into(), 1, u64::MAX);
        }
        let d = &self.discovery;
        range("discovery.interval_ms", d.interval_ms, 500, 600_000);
        range("discovery.transcripts_ms", d.transcripts_ms, 500, 600_000);
        let k = &self.retention;
        range(
            "retention.messages_in_memory",
            k.messages_in_memory as u64,
            100,
            1_000_000,
        );
        range(
            "retention.chat_in_memory",
            k.chat_in_memory as u64,
            100,
            1_000_000,
        );
        range("retention.inbox_items", k.inbox_items as u64, 10, 100_000);
        range(
            "retention.scrollback_lines",
            k.scrollback_lines as u64,
            0,
            1_000_000,
        );
        if let Some(n) = k.events_max_age_days {
            range("retention.events_max_age_days", n.into(), 1, 36_500);
        }
        if let Some(n) = k.events_max_count {
            range("retention.events_max_count", n, 1000, 1_000_000_000_000);
        }
        if let Some(n) = k.inbox_expiry_mins {
            // Ten years: anything more is a typo, and would overflow the
            // time arithmetic.
            range("retention.inbox_expiry_mins", n, 1, 5_256_000);
        }
        if !valid_log_filter(&log.level) {
            out.push(format!(
                "daemon.log.level is {:?}; it must be error, warn, info, debug, trace, off or a tracing filter",
                log.level
            ));
        }
        if w.message.trim().is_empty() {
            out.push("queue.watchdog.message is empty".into());
        }
        for (key, rt) in [("claude", &self.runtimes.claude), ("codex", &self.runtimes.codex)] {
            if rt.binary.as_deref().is_some_and(|b| b.trim().is_empty()) {
                out.push(format!("runtimes.{key}.binary is empty"));
            }
            env_problems(&format!("runtimes.{key}.env"), &rt.env, &mut out);
            for (list, tools) in [
                ("allowed_tools", &rt.allowed_tools),
                ("disallowed_tools", &rt.disallowed_tools),
            ] {
                for t in tools {
                    if t.trim().is_empty() || t.contains(',') {
                        out.push(format!(
                            "runtimes.{key}.{list} has {t:?}; each entry is one tool or pattern, without commas"
                        ));
                    }
                }
            }
            for (name, server) in &rt.mcp_servers {
                if name == "ainet" {
                    out.push(format!(
                        "runtimes.{key}.mcp_servers.ainet is ainet's own server; give yours another name"
                    ));
                } else if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    out.push(format!(
                        "runtimes.{key}.mcp_servers: {name:?} may use only letters, digits, - and _"
                    ));
                }
                if !server.is_object() {
                    out.push(format!(
                        "runtimes.{key}.mcp_servers.{name} must be an object ({{\"command\": …}})"
                    ));
                }
            }
        }
        // Codex has none of these; a setting that did nothing would leave
        // you thinking a seat was more restricted than it is.
        let codex = &self.runtimes.codex;
        for (set, name) in [
            (codex.permission_mode.is_some(), "permission_mode"),
            (!codex.allowed_tools.is_empty(), "allowed_tools"),
            (!codex.disallowed_tools.is_empty(), "disallowed_tools"),
            (codex.strict_mcp_config, "strict_mcp_config"),
        ] {
            if set {
                out.push(format!("runtimes.codex.{name} applies to Claude Code only"));
            }
        }
        env_problems("runtimes.shell.env", &self.runtimes.shell.env, &mut out);
        for p in &self.permissions.sensitive_paths {
            if p.trim().is_empty() || p.split('/').any(|part| part == "..") {
                out.push(format!(
                    "permissions.sensitive_paths has {p:?}; give a name or a path without `..`"
                ));
            }
        }
        for (i, rule) in self.permissions.global_rules.iter().enumerate() {
            if let Some(why) = rule.problem() {
                out.push(format!("permissions.global_rules[{i}]: {why}"));
            }
        }
        if self
            .runtimes
            .shell
            .binary
            .as_deref()
            .is_some_and(|b| b.trim().is_empty())
        {
            out.push("runtimes.shell.binary is empty".into());
        }
        for (model, p) in &self.cost.prices {
            let all = [Some(p.input), Some(p.output), p.cache_write, p.cache_read];
            if all.into_iter().flatten().any(|x| !x.is_finite() || x < 0.0) {
                out.push(format!("cost.prices.{model}: prices must be 0 or more"));
            }
        }
        for (model, w) in &self.cost.context_windows {
            if *w == 0 {
                out.push(format!("cost.context_windows.{model} is 0"));
            }
        }
        out
    }
}

/// Environment variables a setting may not set: ainet's own, which tell
/// an agent who it is and where ainetd lives.
pub fn reserved_env(name: &str) -> bool {
    name.starts_with("AINET_") || name.starts_with("OPENRIG_")
}

fn env_problems(key: &str, env: &BTreeMap<String, String>, out: &mut Vec<String>) {
    for name in env.keys() {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            out.push(format!("{key} has a bad variable name {name:?}"));
        } else if reserved_env(name) {
            out.push(format!("{key}.{name}: ainet sets {name} itself"));
        }
    }
}

/// A level, or a tracing filter: levels and `target=level` directives,
/// comma-separated. A bare target is refused: it is more often a
/// misspelt level.
fn valid_log_filter(s: &str) -> bool {
    const LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace", "off"];
    let s = s.trim();
    !s.is_empty()
        && s.split(',').all(|part| {
            let part = part.trim();
            match part.rsplit_once('=') {
                Some((target, level)) => {
                    !target.is_empty() && LEVELS.contains(&level.to_lowercase().as_str())
                }
                None => LEVELS.contains(&part.to_lowercase().as_str()),
            }
        })
}

#[cfg(test)]
mod tests;
