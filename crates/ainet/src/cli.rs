//! The command line, in OpenRig's names so `rig` (a symlink to `ainet`)
//! behaves like the CLI people already know.

use std::path::PathBuf;

use ainet_core::{QState, Runtime};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "ainet", version, about = "A control room for teams of coding agents")]
pub struct Cli {
    /// Show a pretend fleet instead of connecting to ainetd.
    #[arg(long)]
    pub demo: bool,
    /// Do not capture the mouse (keeps your terminal's own text selection).
    #[arg(long)]
    pub no_mouse: bool,
    /// Print the daemon's reply as JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    // ---- rigs ------------------------------------------------------------
    /// Launch a rig from a spec file, a library name, or a known rig's name.
    Up {
        /// Spec path, library name (`starter`) or the name of a known rig.
        source: String,
        /// Working directory for every seat, for this run.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Name the rig differently from its spec.
        #[arg(long)]
        name: Option<String>,
        /// Seats (logical ids) to start fresh instead of resuming.
        #[arg(long, num_args = 1.., value_delimiter = ',')]
        fresh: Vec<String>,
        /// Only describe what would launch; nothing starts.
        #[arg(long)]
        plan: bool,
    },
    /// Stop a rig (taking a snapshot first unless --no-snapshot).
    Down {
        rig: String,
        /// Skip the snapshot.
        #[arg(long)]
        no_snapshot: bool,
        /// Forget the rig after stopping it.
        #[arg(long)]
        delete: bool,
    },
    /// Rigs and the agents outside them.
    Ps {
        /// One row per seat instead of per rig.
        #[arg(long)]
        nodes: bool,
        /// With --nodes: only this rig.
        #[arg(long, requires = "nodes")]
        rig: Option<String>,
    },
    /// The seats of one rig.
    Status { rig: String },
    /// Rig snapshots.
    #[command(subcommand)]
    Snapshot(SnapshotCmd),
    /// Bring a rig back from a snapshot (the latest by default).
    Restore {
        rig: String,
        #[arg(long)]
        snapshot: Option<String>,
        /// Seats (logical ids) to start fresh instead of resuming.
        #[arg(long, num_args = 1.., value_delimiter = ',')]
        fresh: Vec<String>,
    },
    /// Add one seat to a running rig.
    #[command(visible_alias = "add")]
    Grow(GrowArgs),
    /// Add several seats to one pod: `--member name[:runtime]`, repeatable.
    Expand {
        rig: String,
        #[arg(long)]
        pod: String,
        /// `member` or `member:runtime`; repeat or separate with commas.
        #[arg(long = "member", required = true, value_delimiter = ',')]
        members: Vec<String>,
        /// Runtime for members that do not name one.
        #[arg(long)]
        runtime: Option<Runtime>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Remove a whole pod from a rig.
    Shrink { rig: String, pod: String },
    /// Remove one seat (`pod-member@rig`).
    Remove { seat: String },
    /// Launch or relaunch one seat.
    Launch {
        seat: String,
        /// Start a new conversation instead of resuming.
        #[arg(long)]
        fresh: bool,
    },
    /// A one-seat rig without writing a spec.
    Create {
        rig: String,
        #[arg(long)]
        runtime: Option<Runtime>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "main")]
        pod: String,
        #[arg(long, default_value = "agent")]
        member: String,
    },
    /// Print a running rig as RigSpec YAML.
    Export {
        rig: String,
        /// Write to this file instead of stdout.
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    /// Register a rig from a spec without launching it.
    Import { path: PathBuf },
    /// The spec library.
    Specs {
        #[command(subcommand)]
        cmd: Option<SpecsCmd>,
    },
    /// Check a spec without launching anything.
    Validate { path: PathBuf },
    /// `ainet rig validate`, for OpenRig muscle memory.
    #[command(hide = true)]
    Rig {
        #[command(subcommand)]
        cmd: RigCmd,
    },
    /// Agent sessions running outside ainet.
    Discover,
    /// Take a discovered session into a rig seat.
    #[command(visible_alias = "bind")]
    Adopt {
        /// A tmux pane id (`%12`) or session id from `discover`.
        id: String,
        #[arg(long)]
        rig: String,
        #[arg(long)]
        pod: String,
        #[arg(long)]
        member: String,
        #[arg(long)]
        runtime: Option<Runtime>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Let a seat go without stopping its process.
    #[command(visible_alias = "unclaim")]
    Release { seat: String },
    /// Start ainetd and restore every rig that is stopped or partial.
    Start,

    // ---- talking ---------------------------------------------------------
    /// Send a message: `send <agent> <text…>`, or `--to a,b` / `--pod` / `--rig`.
    Send {
        #[arg(required = true)]
        text: Vec<String>,
        /// Recipients: names, seat addresses, logical ids; comma-separated.
        #[arg(long, value_delimiter = ',')]
        to: Vec<String>,
        #[arg(long)]
        pod: Option<String>,
        #[arg(long)]
        rig: Option<String>,
        /// Send as this agent (default: $AINET_AGENT_ID, else you).
        #[arg(long)]
        from: Option<String>,
        /// Deliver even through a typing guard.
        #[arg(long)]
        force: bool,
        /// Type the text into the agent's terminal as is, without a message.
        #[arg(long)]
        raw: bool,
    },
    /// Send a message to every seat in a rig (or one pod of it).
    Broadcast {
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long)]
        rig: Option<String>,
        #[arg(long)]
        pod: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Direct messages with replies.
    #[command(subcommand)]
    Msg(MsgCmd),
    /// A rig's chatroom.
    #[command(subcommand)]
    Chatroom(ChatCmd),
    /// Owned work items.
    #[command(subcommand)]
    Queue(QueueCmd),

    // ---- observing -------------------------------------------------------
    /// Who an agent is, its seat, peers and edges.
    Whoami {
        /// The agent to ask about (default: $AINET_AGENT_ID).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Terminal text of agents: by name or seat, a pod, or a whole rig.
    Capture {
        targets: Vec<String>,
        #[arg(long)]
        rig: Option<String>,
        #[arg(long)]
        pod: Option<String>,
        /// Last N lines of each.
        #[arg(long, default_value_t = 0)]
        lines: usize,
    },
    /// The conversation from an agent's Claude Code transcript.
    Transcript {
        agent: String,
        #[arg(long, default_value_t = 0)]
        tail: usize,
        #[arg(long)]
        grep: Option<String>,
    },
    /// Print an agent's screen.
    Screen {
        agent: String,
        /// Include up to N rows of scrollback.
        #[arg(long, default_value_t = 0)]
        history: usize,
    },
    /// What ainetd knows: every agent.
    List,
    /// Seat settings.
    #[command(subcommand)]
    Seat(SeatCmd),
    /// Scheduled wakes that type a reminder into a seat.
    #[command(subcommand)]
    Watchdog(WatchdogCmd),
    /// Open the TUI on a view.
    #[command(subcommand)]
    Terminal(TerminalCmd),

    // ---- agents and the daemon -------------------------------------------
    /// Start an agent on a pane ainetd owns, outside any rig.
    Spawn {
        #[arg(value_enum, default_value_t = Kind::Claude)]
        runtime: Kind,
        /// Working directory (default: here).
        #[arg(long, short = 'C')]
        cwd: Option<PathBuf>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        task: Option<String>,
        /// Extra arguments for the agent CLI, after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Stop an agent.
    Close { agent: String },
    /// Draw the critter (and name) for one or more ids.
    Critter {
        ids: Vec<String>,
        /// Octant glyphs: full resolution in a quarter of the space.
        #[arg(long)]
        octant: bool,
    },
    /// ainet's hooks in Claude Code's settings (`hooks.claude_settings`,
    /// else ~/.claude/settings.json): prints the block to merge by hand,
    /// or with a flag writes it (a backup is kept), removes it or checks
    /// it. ainet's own seats get their hooks without it.
    InstallHooks {
        /// Install (or repair) them in Claude Code's settings file.
        #[arg(long, conflicts_with_all = ["remove", "status"])]
        write: bool,
        /// Remove every ainet hook from Claude Code's settings file.
        #[arg(long, conflicts_with = "status")]
        remove: bool,
        /// Say whether they are installed and up to date; changes nothing.
        #[arg(long)]
        status: bool,
    },
    /// ainet's settings (settings.json): read them, change one, edit the
    /// file. Works without ainetd; a running one puts a change in force
    /// at once.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// `/always` rules in force: list them, revoke them.
    #[command(subcommand)]
    Always(AlwaysCmd),
    /// ainet's hooks in Claude Code's settings (`hooks.claude_settings`,
    /// else ~/.claude/settings.json): check, install, remove. Changes keep
    /// a backup of the file.
    #[command(subcommand)]
    Hooks(HooksCmd),
    /// Stop ainetd and the agents it owns.
    Shutdown,
    /// Run ainetd in the foreground.
    Daemon,
    /// Open the desktop app (`ainet-gui`, installed separately).
    Gui {
        /// Show a pretend fleet instead of connecting to ainetd.
        #[arg(long)]
        demo: bool,
    },
    /// Called by Claude Code for each hook event; reads the payload on stdin.
    #[command(hide = true)]
    Hook,
    /// The MCP server agents talk through (stdio).
    #[command(hide = true)]
    Mcp,
}

#[derive(Args, Debug)]
pub struct GrowArgs {
    pub rig: String,
    #[arg(long)]
    pub pod: String,
    #[arg(long)]
    pub member: String,
    #[arg(long)]
    pub runtime: Option<Runtime>,
    #[arg(long)]
    pub role: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// `local:` / `path:` agent spec reference.
    #[arg(long)]
    pub agent_ref: Option<String>,
    #[arg(long)]
    pub label: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum SnapshotCmd {
    /// Snapshot a rig now.
    Create {
        rig: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// A rig's snapshots.
    List { rig: String },
}

#[derive(Subcommand, Debug)]
pub enum SpecsCmd {
    /// List the library (the default).
    #[command(visible_alias = "list")]
    Ls,
    /// Print one spec.
    #[command(visible_alias = "preview")]
    Show { name: String },
}

#[derive(Subcommand, Debug)]
pub enum RigCmd {
    /// Check a spec without launching anything.
    Validate { path: PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum MsgCmd {
    /// Send a message to one or more agents (comma-separated).
    Send {
        to: String,
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long)]
        force: bool,
    },
    /// Your unread messages (marks them read unless --keep-unread).
    #[command(visible_alias = "read")]
    Inbox {
        /// Include messages already read.
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long)]
        keep_unread: bool,
        /// Whose inbox (default: $AINET_AGENT_ID, else yours).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Answer a message.
    Reply {
        id: u64,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Send a message and wait for the reply (exit 2 on timeout).
    Ask {
        to: String,
        #[arg(required = true)]
        text: Vec<String>,
        /// Seconds to wait.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Every message in a conversation.
    Thread { id: u64 },
}

#[derive(Subcommand, Debug)]
pub enum ChatCmd {
    /// Post to the chatroom.
    Send {
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long)]
        rig: Option<String>,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        sender: Option<String>,
    },
    /// Recent chat.
    History {
        #[arg(long)]
        rig: Option<String>,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        after: Option<u64>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Follow the chatroom until Ctrl+C.
    Watch {
        #[arg(long)]
        rig: Option<String>,
        #[arg(long)]
        topic: Option<String>,
    },
    /// Wait for the next chat message (exit 2 on timeout).
    Wait {
        #[arg(long)]
        rig: Option<String>,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        after: Option<u64>,
        /// Seconds to wait.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum QueueCmd {
    /// Hand a piece of work to a seat.
    Create {
        /// The seat that should do it.
        #[arg(long, alias = "destination")]
        to: String,
        #[arg(long)]
        summary: String,
        /// Body text; `-` reads stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the body from a file; `-` reads stdin.
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// routine, urgent or critical.
        #[arg(long)]
        priority: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        reply_to: Option<String>,
        /// Do not tell the destination right away.
        #[arg(long)]
        no_nudge: bool,
        #[arg(long)]
        rig: Option<String>,
        /// Who is asking (default: $AINET_AGENT_ID, else you).
        #[arg(long)]
        source: Option<String>,
    },
    /// Open items.
    List {
        #[arg(long)]
        rig: Option<String>,
        /// Items owned by or addressed to you.
        #[arg(long, conflicts_with = "who")]
        mine: bool,
        /// Items owned by or addressed to this agent or seat.
        #[arg(long)]
        who: Option<String>,
        #[arg(long, value_parser = parse_qstate)]
        state: Option<QState>,
        /// Include done and canceled items.
        #[arg(long)]
        all: bool,
    },
    /// One item with its notes.
    Show { id: String },
    /// Take ownership of an item.
    Claim { id: String },
    /// Give an item back.
    Unclaim {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Add a note or change the state.
    Update {
        id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, value_parser = parse_qstate)]
        state: Option<QState>,
    },
    /// Mark an item done.
    Done {
        id: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Close an item and open its successor for another seat.
    Handoff {
        id: String,
        #[arg(long, alias = "destination")]
        to: String,
        /// Body text; `-` reads stdin.
        #[arg(long, conflicts_with = "body_file", required_unless_present = "body_file")]
        body: Option<String>,
        /// Read the body from a file; `-` reads stdin.
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        summary: Option<String>,
    },
    /// Block an owned item on another item or `external:<name>`.
    Block {
        id: String,
        /// A qitem id or `external:<name>`.
        #[arg(long)]
        on: String,
        /// What to do once unblocked.
        #[arg(long)]
        continuation: String,
        /// Wake the owner after this long even if still blocked: 90s, 30m, 2h, 1d.
        #[arg(long, value_parser = parse_duration)]
        wake_after: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum WatchdogCmd {
    /// Every N seconds, type a reminder into a seat.
    Register {
        #[arg(long = "target-session", alias = "target")]
        target: String,
        #[arg(long = "interval-seconds", alias = "interval")]
        interval: u64,
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        registered_by: Option<String>,
        /// Accepted for OpenRig compatibility; only `periodic-reminder` exists.
        #[arg(long)]
        policy: Option<String>,
    },
    /// Registered watchdogs.
    List,
    /// Stop a watchdog.
    Stop {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// One setting's value, or the settings of a group (`queue`).
    Get { key: String },
    /// Change one setting: `ainet config set queue.nudge false`. Text
    /// needs no quotes; lists and maps are JSON.
    Set {
        key: String,
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        value: Vec<String>,
    },
    /// Remove a setting (or a group) from the file: its default applies.
    Unset { key: String },
    /// Every setting: its value, where it comes from, when a change
    /// applies; `*` marks those not at their default.
    List {
        /// Only this setting or group.
        key: Option<String>,
        /// Only the settings not at their default.
        #[arg(long)]
        changed: bool,
    },
    /// Where settings.json is; with --all, every path ainet uses and its
    /// version.
    Path {
        #[arg(long)]
        all: bool,
    },
    /// Open settings.json in $VISUAL or $EDITOR, then check it and have
    /// a running ainetd read it.
    Edit,
}

#[derive(Subcommand, Debug)]
pub enum AlwaysCmd {
    /// The rules in force: one agent's, or every agent's and the global
    /// ones from settings.json.
    List { agent: Option<String> },
    /// Revoke an agent's rule by its key, as `list` shows it, or all of
    /// its rules with --all.
    Revoke {
        agent: String,
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        rule: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum HooksCmd {
    /// Whether they are installed and up to date; changes nothing.
    Status,
    /// Install (or repair) them, keeping a backup of the file.
    Install,
    /// Remove every ainet hook, keeping a backup of the file.
    Uninstall,
}

#[derive(Subcommand, Debug)]
pub enum TerminalCmd {
    /// Open the TUI on a rig (`rig`, `rig:NAME`) or a pod (`pod:RIG/POD`).
    Open { view: String },
    /// Whether ainetd is running, and what it holds.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum SeatCmd {
    /// While on, automatic messages are held instead of typed into the seat.
    SetTypingGuard {
        seat: String,
        #[arg(long, action = ArgAction::Set)]
        enabled: bool,
        /// Accepted for OpenRig compatibility; not recorded.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Messages a typing guard is holding.
    HeldMessages { seat: String },
    /// Nudge a seat stuck at a prompt: press Enter, or relaunch it if stopped.
    Continue { seat: String },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Kind {
    Claude,
    Codex,
    Shell,
}

impl From<Kind> for Runtime {
    fn from(k: Kind) -> Runtime {
        match k {
            Kind::Claude => Runtime::ClaudeCode,
            Kind::Codex => Runtime::Codex,
            Kind::Shell => Runtime::Shell,
        }
    }
}

/// `pending`, `in-progress` (or `in_progress`), `blocked`, `done`, `canceled`.
pub fn parse_qstate(s: &str) -> Result<QState, String> {
    match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "pending" => Ok(QState::Pending),
        "in-progress" | "active" => Ok(QState::InProgress),
        "blocked" => Ok(QState::Blocked),
        "done" => Ok(QState::Done),
        "canceled" | "cancelled" => Ok(QState::Canceled),
        other => Err(format!(
            "unknown state {other:?}: use pending, in-progress, blocked, done or canceled"
        )),
    }
}

/// Parses `90`, `90s`, `30m`, `2h`, `1d` into seconds.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let n: u64 = n
        .parse()
        .map_err(|_| format!("bad duration {s:?}: use 90s, 30m, 2h or 1d"))?;
    let mult = match unit {
        "" | "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" => 3600,
        "d" => 86400,
        _ => return Err(format!("bad duration {s:?}: use 90s, 30m, 2h or 1d")),
    };
    Ok(n * mult)
}

/// Parses `member` or `member:runtime`.
pub fn parse_member(s: &str) -> Result<(String, Option<Runtime>), String> {
    match s.split_once(':') {
        Some((m, r)) if !m.is_empty() => Ok((m.to_string(), Some(r.parse()?))),
        None if !s.is_empty() => Ok((s.to_string(), None)),
        _ => Err(format!("bad member {s:?}: use name or name:runtime")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn bare_runs_the_tui() {
        let cli = parse(&["ainet", "--demo", "--no-mouse"]);
        assert!(cli.cmd.is_none() && cli.demo && cli.no_mouse);
    }

    #[test]
    fn up_takes_fresh_seats() {
        let cli = parse(&[
            "rig",
            "up",
            "starter",
            "--cwd",
            "/tmp",
            "--fresh",
            "dev.impl",
            "dev.review",
            "--json",
        ]);
        assert!(cli.json);
        match cli.cmd {
            Some(Cmd::Up {
                source,
                cwd,
                fresh,
                name,
                plan: false,
            }) => {
                assert_eq!(source, "starter");
                assert_eq!(cwd, Some(PathBuf::from("/tmp")));
                assert_eq!(fresh, ["dev.impl", "dev.review"]);
                assert!(name.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn down_flags() {
        match parse(&["ainet", "down", "r", "--no-snapshot", "--delete"]).cmd {
            Some(Cmd::Down {
                rig,
                no_snapshot: true,
                delete: true,
            }) => assert_eq!(rig, "r"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn grow_and_add_are_the_same() {
        for verb in ["grow", "add"] {
            match parse(&[
                "ainet",
                verb,
                "r",
                "--pod",
                "dev",
                "--member",
                "qa",
                "--runtime",
                "codex",
            ])
            .cmd
            {
                Some(Cmd::Grow(g)) => {
                    assert_eq!(
                        (g.rig.as_str(), g.pod.as_str(), g.member.as_str()),
                        ("r", "dev", "qa")
                    );
                    assert_eq!(g.runtime, Some(Runtime::Codex));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn expand_takes_repeated_members() {
        match parse(&[
            "ainet",
            "expand",
            "r",
            "--pod",
            "dev",
            "--member",
            "a:codex",
            "--member",
            "b,c:terminal",
        ])
        .cmd
        {
            Some(Cmd::Expand { members, .. }) => assert_eq!(members, ["a:codex", "b", "c:terminal"]),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_member("a:codex").unwrap(),
            ("a".into(), Some(Runtime::Codex))
        );
        assert_eq!(parse_member("b").unwrap(), ("b".into(), None));
        assert!(parse_member(":codex").is_err());
        assert!(parse_member("a:nope").is_err());
    }

    #[test]
    fn create_defaults_to_main_agent() {
        match parse(&["ainet", "create", "solo", "--runtime", "claude-code"]).cmd {
            Some(Cmd::Create {
                pod, member, runtime, ..
            }) => {
                assert_eq!((pod.as_str(), member.as_str()), ("main", "agent"));
                assert_eq!(runtime, Some(Runtime::ClaudeCode));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn send_forms() {
        match parse(&["ainet", "send", "--to", "a,b", "hello", "there", "--force"]).cmd {
            Some(Cmd::Send {
                text,
                to,
                force: true,
                raw: false,
                ..
            }) => {
                assert_eq!(to, ["a", "b"]);
                assert_eq!(text, ["hello", "there"]);
            }
            other => panic!("{other:?}"),
        }
        // The old form: the first word is the agent.
        match parse(&["ainet", "send", "nimble-otter", "hi"]).cmd {
            Some(Cmd::Send { text, to, .. }) => {
                assert!(to.is_empty());
                assert_eq!(text, ["nimble-otter", "hi"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["ainet", "send"]).is_err());
    }

    #[test]
    fn msg_commands() {
        assert!(matches!(
            parse(&["ainet", "msg", "read", "--all", "--limit", "5", "--keep-unread"]).cmd,
            Some(Cmd::Msg(MsgCmd::Inbox {
                all: true,
                limit: 5,
                keep_unread: true,
                ..
            }))
        ));
        assert!(matches!(
            parse(&["ainet", "msg", "ask", "dev.review", "ok?", "--timeout", "30"]).cmd,
            Some(Cmd::Msg(MsgCmd::Ask { timeout: 30, .. }))
        ));
        assert!(matches!(
            parse(&["ainet", "msg", "reply", "12", "yes"]).cmd,
            Some(Cmd::Msg(MsgCmd::Reply { id: 12, .. }))
        ));
        assert!(Cli::try_parse_from(["ainet", "msg", "reply", "x", "yes"]).is_err());
    }

    #[test]
    fn queue_commands() {
        match parse(&[
            "ainet",
            "queue",
            "create",
            "--destination",
            "dev.impl",
            "--summary",
            "do it",
            "--tags",
            "a,b",
            "--no-nudge",
        ])
        .cmd
        {
            Some(Cmd::Queue(QueueCmd::Create {
                to,
                tags,
                no_nudge: true,
                ..
            })) => {
                assert_eq!(to, "dev.impl");
                assert_eq!(tags, ["a", "b"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(&["ainet", "queue", "update", "q-1", "--state", "in_progress"]).cmd,
            Some(Cmd::Queue(QueueCmd::Update {
                state: Some(QState::InProgress),
                ..
            }))
        ));
        assert!(Cli::try_parse_from(["ainet", "queue", "update", "q-1", "--state", "nope"]).is_err());
        assert!(Cli::try_parse_from(["ainet", "queue", "list", "--mine", "--who", "x"]).is_err());
    }

    #[test]
    fn aliases() {
        assert!(matches!(
            parse(&["rig", "bind", "%3", "--rig", "r", "--pod", "p", "--member", "m"]).cmd,
            Some(Cmd::Adopt { .. })
        ));
        assert!(matches!(
            parse(&["rig", "unclaim", "dev-impl@r"]).cmd,
            Some(Cmd::Release { .. })
        ));
        assert!(matches!(
            parse(&["rig", "rig", "validate", "x.yaml"]).cmd,
            Some(Cmd::Rig { .. })
        ));
        assert!(matches!(
            parse(&["rig", "specs"]).cmd,
            Some(Cmd::Specs { cmd: None })
        ));
        assert!(matches!(
            parse(&["rig", "specs", "show", "starter"]).cmd,
            Some(Cmd::Specs {
                cmd: Some(SpecsCmd::Show { .. })
            })
        ));
    }

    #[test]
    fn seat_and_chat() {
        assert!(matches!(
            parse(&[
                "ainet",
                "seat",
                "set-typing-guard",
                "dev-impl@r",
                "--enabled",
                "false"
            ])
            .cmd,
            Some(Cmd::Seat(SeatCmd::SetTypingGuard { enabled: false, .. }))
        ));
        assert!(
            Cli::try_parse_from(["ainet", "seat", "set-typing-guard", "s", "--enabled", "maybe"]).is_err()
        );
        assert!(matches!(
            parse(&["ainet", "chatroom", "wait", "--after", "4", "--timeout", "9"]).cmd,
            Some(Cmd::Chatroom(ChatCmd::Wait {
                after: Some(4),
                timeout: 9,
                ..
            }))
        ));
    }

    #[test]
    fn openrig_additions() {
        assert!(matches!(
            parse(&["rig", "up", "starter", "--plan"]).cmd,
            Some(Cmd::Up { plan: true, .. })
        ));
        assert!(matches!(
            parse(&["rig", "ps", "--nodes", "--rig", "r"]).cmd,
            Some(Cmd::Ps {
                nodes: true,
                rig: Some(_)
            })
        ));
        assert!(Cli::try_parse_from(["rig", "ps", "--rig", "r"]).is_err());
        assert!(matches!(
            parse(&["rig", "seat", "continue", "dev-impl@r"]).cmd,
            Some(Cmd::Seat(SeatCmd::Continue { .. }))
        ));
        assert!(matches!(
            parse(&["rig", "specs", "preview", "starter"]).cmd,
            Some(Cmd::Specs {
                cmd: Some(SpecsCmd::Show { .. })
            })
        ));
        assert!(matches!(
            parse(&[
                "rig",
                "queue",
                "handoff",
                "q-1",
                "--to",
                "dev.qa",
                "--body",
                "over to you"
            ])
            .cmd,
            Some(Cmd::Queue(QueueCmd::Handoff { .. }))
        ));
        assert!(Cli::try_parse_from(["rig", "queue", "handoff", "q-1", "--to", "x"]).is_err());
        assert!(matches!(
            parse(&[
                "rig",
                "queue",
                "block",
                "q-1",
                "--on",
                "external:ci",
                "--continuation",
                "rerun",
                "--wake-after",
                "2h"
            ])
            .cmd,
            Some(Cmd::Queue(QueueCmd::Block {
                wake_after: Some(7200),
                ..
            }))
        ));
        assert!(matches!(
            parse(&[
                "rig",
                "watchdog",
                "register",
                "--target-session",
                "dev-impl@r",
                "--interval-seconds",
                "600",
                "--policy",
                "periodic-reminder"
            ])
            .cmd,
            Some(Cmd::Watchdog(WatchdogCmd::Register { interval: 600, .. }))
        ));
        assert!(matches!(
            parse(&["rig", "watchdog", "stop", "w-1", "--reason", "done"]).cmd,
            Some(Cmd::Watchdog(WatchdogCmd::Stop { .. }))
        ));
        assert!(matches!(
            parse(&["rig", "terminal", "open", "pod:r/dev"]).cmd,
            Some(Cmd::Terminal(TerminalCmd::Open { .. }))
        ));
        assert_eq!(parse_duration("30m"), Ok(1800));
        assert_eq!(parse_duration("1d"), Ok(86400));
        assert_eq!(parse_duration("45"), Ok(45));
        assert!(parse_duration("2w").is_err());
        assert!(parse_duration("h").is_err());
    }

    #[test]
    fn config_commands() {
        match parse(&[
            "ainet",
            "config",
            "set",
            "runtimes.claude.args",
            "--verbose",
            "-x",
        ])
        .cmd
        {
            Some(Cmd::Config(ConfigCmd::Set { key, value })) => {
                assert_eq!(key, "runtimes.claude.args");
                assert_eq!(value, ["--verbose", "-x"]);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["ainet", "config", "set", "queue.watchdog.message", "wake", "up"]).cmd {
            Some(Cmd::Config(ConfigCmd::Set { value, .. })) => assert_eq!(value, ["wake", "up"]),
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["ainet", "config", "set", "queue.nudge"]).is_err());
        assert!(matches!(
            parse(&["ainet", "config", "get", "queue.nudge", "--json"]).cmd,
            Some(Cmd::Config(ConfigCmd::Get { .. }))
        ));
        assert!(matches!(
            parse(&["ainet", "config", "unset", "queue"]).cmd,
            Some(Cmd::Config(ConfigCmd::Unset { .. }))
        ));
        assert!(matches!(
            parse(&["ainet", "config", "list", "queue", "--changed"]).cmd,
            Some(Cmd::Config(ConfigCmd::List {
                key: Some(_),
                changed: true
            }))
        ));
        assert!(matches!(
            parse(&["ainet", "config", "list"]).cmd,
            Some(Cmd::Config(ConfigCmd::List {
                key: None,
                changed: false
            }))
        ));
        assert!(matches!(
            parse(&["ainet", "config", "path", "--all"]).cmd,
            Some(Cmd::Config(ConfigCmd::Path { all: true }))
        ));
        assert!(matches!(
            parse(&["ainet", "config", "edit"]).cmd,
            Some(Cmd::Config(ConfigCmd::Edit))
        ));
        assert!(Cli::try_parse_from(["ainet", "config", "frob"]).is_err());
    }

    #[test]
    fn always_and_hooks_commands() {
        assert!(matches!(
            parse(&["ainet", "always", "list"]).cmd,
            Some(Cmd::Always(AlwaysCmd::List { agent: None }))
        ));
        assert!(matches!(
            parse(&["ainet", "always", "list", "dev.impl"]).cmd,
            Some(Cmd::Always(AlwaysCmd::List { agent: Some(_) }))
        ));
        assert!(matches!(
            parse(&["ainet", "always", "revoke", "a1", "Bash:cargo test"]).cmd,
            Some(Cmd::Always(AlwaysCmd::Revoke {
                rule: Some(_),
                all: false,
                ..
            }))
        ));
        assert!(matches!(
            parse(&["ainet", "always", "revoke", "a1", "--all"]).cmd,
            Some(Cmd::Always(AlwaysCmd::Revoke {
                rule: None,
                all: true,
                ..
            }))
        ));
        // Revoking everything is said outright, never by leaving the key out.
        assert!(Cli::try_parse_from(["ainet", "always", "revoke", "a1"]).is_err());
        assert!(Cli::try_parse_from(["ainet", "always", "revoke", "a1", "k", "--all"]).is_err());
        for (verb, want) in [("status", 0), ("install", 1), ("uninstall", 2)] {
            let got = match parse(&["ainet", "hooks", verb]).cmd {
                Some(Cmd::Hooks(HooksCmd::Status)) => 0,
                Some(Cmd::Hooks(HooksCmd::Install)) => 1,
                Some(Cmd::Hooks(HooksCmd::Uninstall)) => 2,
                other => panic!("{other:?}"),
            };
            assert_eq!(got, want);
        }
        assert!(Cli::try_parse_from(["ainet", "hooks"]).is_err());
    }

    #[test]
    fn gui_takes_demo() {
        let cli = parse(&["ainet", "gui", "--demo"]);
        assert!(!cli.demo, "--demo belongs to the subcommand");
        assert!(matches!(cli.cmd, Some(Cmd::Gui { demo: true })));
    }

    #[test]
    fn kept_commands() {
        assert!(matches!(
            parse(&["ainet", "spawn", "codex", "-C", "/tmp", "--", "--x"]).cmd,
            Some(Cmd::Spawn { .. })
        ));
        assert!(matches!(parse(&["ainet", "list", "--json"]).cmd, Some(Cmd::List)));
        assert!(matches!(
            parse(&["ainet", "screen", "a", "--history", "50"]).cmd,
            Some(Cmd::Screen { history: 50, .. })
        ));
        assert!(matches!(parse(&["ainet", "hook"]).cmd, Some(Cmd::Hook)));
        assert!(matches!(parse(&["ainet", "mcp"]).cmd, Some(Cmd::Mcp)));
        assert!(matches!(
            parse(&["ainet", "gui"]).cmd,
            Some(Cmd::Gui { demo: false })
        ));
        assert!(matches!(
            parse(&["ainet", "capture", "a", "b", "--lines", "5"]).cmd,
            Some(Cmd::Capture { lines: 5, .. })
        ));
    }
}
