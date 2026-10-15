//! The operator console's language: `/commands`, `@Name message` lines,
//! replies, answers. Each becomes a [`Request`] (plus what to say once it is
//! done) or something only the front end can do, so every front end means
//! the same thing by the same words.

use std::path::PathBuf;

use ainet_core::protocol::{QSpec, SeatSpec, SpawnSpec, UpSpec};
use ainet_core::{Agent, Decision, InboxItem, QState, Request, Runtime};

use crate::store::{Entry, Store};

/// What a command needs to know about the front end's state.
pub struct Context<'a> {
    pub store: &'a Store,
    /// The rig the front end is showing (the Terminals view filtered to one
    /// rig), which rig commands act on when none is named.
    pub view_rig: Option<&'a str>,
    /// The inbox item under the cursor.
    pub inbox_item: Option<&'a InboxItem>,
}

/// A parsed `/command`.
#[derive(Debug)]
pub enum Command {
    /// Send this, and say the text once it is done (or title its report).
    Request(Box<Request>, String),
    /// Answer a permission request: the front end picks which one (and may
    /// first show it to you).
    Answer(Decision),
    Help,
    Quit,
    /// Cycle the critters' size.
    Critters,
    /// Open a view: `terminals`, `rig:<name>`, `pod:<rig>/<pod>`, `comms`, …
    View(String),
    /// Columns of the Terminals grid; `None` picks them from the shape.
    Cols(Option<u16>),
    /// Wrong or missing arguments: the usage, or what is missing.
    Error(String),
    /// Nothing to do (nothing selected to act on).
    Nothing,
}

impl Command {
    fn error(text: &str) -> Command {
        Command::Error(text.into())
    }
}

/// Parses a slash command, without its `/`.
pub fn parse(cx: &Context, cmd: &str) -> Command {
    let store = cx.store;
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let arg = |i: usize| words.get(i).copied();
    let rest = |i: usize| words.get(i..).map(|w| w.join(" ")).unwrap_or_default();
    let rig_arg = |given: Option<&str>| store.rig_for(given, cx.view_rig);
    let req = |r: Request, ok: String| Command::Request(Box::new(r), ok);
    match arg(0).unwrap_or("") {
        "help" | "h" | "?" => Command::Help,
        "quit" | "q" => Command::Quit,
        "crit" | "critters" => Command::Critters,
        "allow" | "y" => Command::Answer(Decision::Allow),
        "always" | "a" => Command::Answer(Decision::Always),
        "deny" | "n" => Command::Answer(Decision::Deny),
        "answer" => match cx.inbox_item {
            Some(item) => answer_question(item, rest(1)),
            None => Command::Nothing,
        },
        "view" | "terminal" | "terminals" => Command::View(arg(1).unwrap_or("terminals").into()),
        "cols" => Command::Cols(arg(1).and_then(|c| c.parse().ok()).filter(|c: &u16| *c > 0)),
        "spawn" | "new" => {
            let runtime = arg(1)
                .and_then(|r| r.parse::<Runtime>().ok())
                .unwrap_or(Runtime::ClaudeCode);
            let cwd = arg(2)
                .map(expand)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let role = rest(3);
            let spec = SpawnSpec {
                runtime,
                cwd,
                role: (!role.is_empty()).then(|| role.to_uppercase()),
                task: None,
                parent: None,
                args: vec![],
            };
            req(Request::Spawn(spec), "spawned".into())
        }
        "close" | "kill" => {
            let target = arg(1)
                .and_then(|k| store.find_agent(k))
                .or(store.selected_agent());
            match target {
                Some(a) => close(a),
                None => Command::Nothing,
            }
        }
        // Rigs.
        "up" | "plan" => {
            let Some(source) = arg(1) else {
                return Command::error("/up <spec file | library name | rig>");
            };
            let cwd = arg(2).map(expand).or_else(|| std::env::current_dir().ok());
            let plan = arg(0) == Some("plan");
            let looks_like_path = std::path::Path::new(source).exists();
            let spec = UpSpec {
                source: if looks_like_path {
                    expand(source).display().to_string()
                } else {
                    source.to_string()
                },
                cwd: if looks_like_path { arg(2).map(expand) } else { cwd },
                fresh: vec![],
                name: None,
                plan,
            };
            req(
                Request::Up(spec),
                if plan {
                    format!("plan for {source}")
                } else {
                    format!("launching {source}")
                },
            )
        }
        "down" => match rig_arg(arg(1)) {
            Some(rig) => req(
                Request::Down {
                    rig: rig.clone(),
                    snapshot: true,
                    delete: false,
                },
                format!("{rig} is down (snapshot taken)"),
            ),
            None => Command::error("/down <rig>"),
        },
        "restore" => match rig_arg(arg(1)) {
            Some(rig) => req(
                Request::Restore {
                    rig: rig.clone(),
                    snapshot: None,
                    fresh: vec![],
                },
                format!("restoring {rig}"),
            ),
            None => Command::error("/restore <rig>"),
        },
        "snapshot" => match rig_arg(arg(1)) {
            Some(rig) => req(
                Request::SnapshotCreate {
                    rig: rig.clone(),
                    label: None,
                },
                format!("snapshot of {rig}"),
            ),
            None => Command::error("/snapshot <rig>"),
        },
        "snapshots" => match rig_arg(arg(1)) {
            Some(rig) => req(
                Request::SnapshotList { rig: rig.clone() },
                format!("snapshots of {rig}"),
            ),
            None => Command::error("/snapshots <rig>"),
        },
        "ps" | "rigs" => req(Request::Ps, "rigs".into()),
        "specs" | "library" => req(Request::Specs, "spec library".into()),
        "spec" | "yaml" => match arg(1) {
            Some(n) => req(Request::SpecShow { name: n.into() }, format!("spec {n}")),
            None => Command::error("/spec <name>"),
        },
        "export" => match rig_arg(arg(1)) {
            Some(rig) => req(Request::Export { rig: rig.clone() }, format!("{rig} as RigSpec")),
            None => Command::error("/export <rig>"),
        },
        "grow" | "add" => {
            // /grow <pod> <member> [runtime] [role…]   (in the selected rig)
            let (Some(pod), Some(member)) = (arg(1), arg(2)) else {
                return Command::error("/grow <pod> <member> [runtime] [ROLE]");
            };
            let Some(rig) = rig_arg(None) else {
                return Command::error("select an agent in the rig first");
            };
            let runtime = arg(3).and_then(|r| r.parse::<Runtime>().ok());
            let role = rest(if runtime.is_some() { 4 } else { 3 });
            let seat = SeatSpec {
                pod: pod.into(),
                member: member.into(),
                runtime,
                role: (!role.is_empty()).then_some(role),
                ..Default::default()
            };
            req(
                Request::Grow {
                    rig: rig.clone(),
                    seats: vec![seat],
                    edges: vec![],
                },
                format!("growing {rig}"),
            )
        }
        "shrink" => match (rig_arg(None), arg(1)) {
            (Some(rig), Some(pod)) => req(
                Request::Shrink { rig, pod: pod.into() },
                format!("removed pod {pod}"),
            ),
            _ => Command::error("/shrink <pod>"),
        },
        "remove" => match arg(1) {
            Some(seat) => remove_seat(seat),
            None => Command::error("/remove <seat>"),
        },
        "launch" | "relaunch" => match arg(1)
            .map(str::to_string)
            .or_else(|| store.selected_agent().and_then(|a| a.seat.clone()))
        {
            Some(seat) => launch(&seat, arg(0) == Some("relaunch")),
            None => Command::error("/launch <seat>"),
        },
        "continue" => match arg(1)
            .map(str::to_string)
            .or_else(|| store.selected_agent().and_then(|a| a.seat.clone()))
        {
            Some(seat) => nudge(&seat),
            None => Command::error("/continue <seat>"),
        },
        "guard" => match (arg(1), arg(2)) {
            (Some(seat), Some(on)) => req(
                Request::TypingGuard {
                    seat: seat.into(),
                    on: on == "on",
                },
                format!("typing guard {on} for {seat}"),
            ),
            _ => Command::error("/guard <seat> on|off"),
        },
        "discover" => req(Request::Discover, "agents outside ainet".into()),
        "adopt" => match (arg(1), arg(2), arg(3), arg(4)) {
            (Some(id), Some(rig), Some(pod), Some(member)) => {
                let seat = SeatSpec {
                    pod: pod.into(),
                    member: member.into(),
                    ..Default::default()
                };
                req(
                    Request::Adopt {
                        id: id.into(),
                        rig: rig.into(),
                        seat,
                    },
                    format!("adopted {id}"),
                )
            }
            _ => Command::error("/adopt <id> <rig> <pod> <member>"),
        },
        // Talking.
        "broadcast" => match rig_arg(None) {
            Some(rig) => req(
                Request::Message {
                    from: None,
                    to: vec![],
                    rig: Some(rig.clone()),
                    pod: None,
                    body: rest(1),
                    reply_to: None,
                    force: false,
                },
                format!("broadcast to {rig}"),
            ),
            None => Command::error("select an agent in the rig first"),
        },
        "chat" => match rig_arg(None) {
            Some(rig) => chat(&rig, None, rest(1)),
            None => Command::error("select an agent in the rig first"),
        },
        "queue" | "task" => {
            // /queue <destination> <summary…>
            let Some(dest) = arg(1) else {
                return Command::error("/queue <seat> <summary>");
            };
            let spec = QSpec {
                destination: dest.into(),
                summary: rest(2),
                nudge: true,
                ..Default::default()
            };
            req(Request::QueueCreate(spec), format!("queued for {dest}"))
        }
        "watchdog" => match (arg(1), arg(2).and_then(|m| m.parse::<u64>().ok())) {
            (Some(seat), Some(min)) => req(
                Request::WatchdogRegister {
                    target: seat.into(),
                    interval_secs: min * 60,
                    message: None,
                    registered_by: None,
                },
                format!("waking {seat} every {min} min"),
            ),
            _ => Command::error("/watchdog <seat> <minutes>"),
        },
        "capture" => match arg(1).map(str::to_string).or_else(|| store.selected.clone()) {
            Some(t) => capture(t),
            None => Command::error("/capture <agent>"),
        },
        "whoami" => match store.selected.clone() {
            Some(id) => req(Request::Whoami { agent: id }, "who".into()),
            None => Command::error("select an agent"),
        },
        other => Command::Error(format!("no command /{other}; /help lists them")),
    }
}

/// Closes an agent: its process ends.
pub fn close(a: &Agent) -> Command {
    request(
        Request::Close { agent: a.id.clone() },
        format!("closing {}", a.label()),
    )
}

/// Starts the agent of a seat; `fresh` starts a new session instead of
/// resuming the last one (`/relaunch`).
pub fn launch(seat: &str, fresh: bool) -> Command {
    request(
        Request::Launch {
            seat: seat.into(),
            fresh,
        },
        format!("launching {seat}"),
    )
}

/// Takes a seat out of its rig (`/remove`): its agent stops.
pub fn remove_seat(seat: &str) -> Command {
    request(
        Request::RemoveSeat { seat: seat.into() },
        format!("removed {seat}"),
    )
}

/// Lets a seat go without stopping its process (`ainet release`): the
/// agent runs on outside the rig.
pub fn release(seat: &str) -> Command {
    request(Request::Release { seat: seat.into() }, format!("{seat} released"))
}

/// Nudges a seat's agent to carry on (`/continue`).
pub fn nudge(seat: &str) -> Command {
    request(
        Request::SeatContinue { seat: seat.into() },
        format!("nudged {seat}"),
    )
}

/// Turns a seat's typing guard on or off: messages wait while someone
/// types into its terminal.
pub fn guard(seat: &str, on: bool) -> Command {
    let word = if on { "on" } else { "off" };
    request(
        Request::TypingGuard {
            seat: seat.into(),
            on,
        },
        format!("typing guard {word} for {seat}"),
    )
}

/// The last lines of a terminal, as a report (`/capture`).
pub fn capture(target: String) -> Command {
    request(
        Request::Capture {
            targets: vec![target],
            rig: None,
            pod: None,
            lines: 80,
        },
        "terminal".into(),
    )
}

/// The conversation from an agent's Claude Code transcript, as a report.
pub fn transcript(a: &Agent) -> Command {
    request(
        Request::Transcript {
            agent: a.id.clone(),
            tail: 0,
            grep: None,
        },
        format!("transcript of {}", a.label()),
    )
}

/// What a front end's per-agent menu can ask of an agent. Messages are
/// [`message`]; opening its terminal is the front end's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentCommand {
    Transcript,
    /// A new session in its seat.
    Relaunch,
    /// Nudge it to carry on.
    Continue,
    Close,
    /// Turn its seat's typing guard on or off.
    Guard(bool),
    /// Its terminal's last lines.
    Capture,
    /// Let its seat go; the process runs on.
    Release,
    /// Take its seat out of the rig.
    Remove,
}

/// The request for `cmd` on agent `id`: what the TUI's command for it
/// sends. Seat commands need an agent in a seat.
pub fn for_agent(store: &Store, id: &str, cmd: AgentCommand) -> Command {
    let Some(a) = store.agents.get(id) else {
        return Command::error("no such agent");
    };
    let seat = || {
        a.seat
            .as_deref()
            .ok_or_else(|| Command::Error(format!("{} is not in a rig seat", a.label())))
    };
    let seated = |f: &dyn Fn(&str) -> Command| seat().map_or_else(|e| e, f);
    match cmd {
        AgentCommand::Transcript => transcript(a),
        AgentCommand::Relaunch => seated(&|s| launch(s, true)),
        AgentCommand::Continue => seated(&nudge),
        AgentCommand::Close => close(a),
        AgentCommand::Guard(on) => seated(&|s| guard(s, on)),
        AgentCommand::Capture => capture(a.id.clone()),
        AgentCommand::Release => seated(&release),
        AgentCommand::Remove => seated(&remove_seat),
    }
}

fn request(req: Request, ok: String) -> Command {
    Command::Request(Box::new(req), ok)
}

/// Answers a waiting question with an option's number(s) or your words.
pub fn answer_question(item: &InboxItem, answer: String) -> Command {
    Command::Request(
        Box::new(Request::AnswerQuestion {
            item: item.id,
            answer,
        }),
        "answered".into(),
    )
}

/// Answers a waiting question with the options picked, by their index in
/// `item.options` (from 0): their numbers, as `ainet answer` takes them
/// ("1 3"). Out-of-range picks are dropped; with none left, says why.
pub fn answer_choices(item: &InboxItem, picked: &[usize]) -> Command {
    let mut numbers: Vec<usize> = picked
        .iter()
        .copied()
        .filter(|&i| i < item.options.len())
        .map(|i| i + 1)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        return Command::error("pick an option or type your answer");
    }
    let answer = numbers.iter().map(usize::to_string).collect::<Vec<_>>().join(" ");
    answer_question(item, answer)
}

/// A reply to a Comms entry: a threaded reply to a message, or a post in
/// the same chat topic.
pub fn reply(entry: &Entry, body: String) -> Command {
    match entry {
        Entry::Message(m) => Command::Request(
            Box::new(Request::Message {
                from: None,
                to: vec![],
                rig: None,
                pod: None,
                body,
                reply_to: Some(m.id),
                force: false,
            }),
            format!("replied to #{}", m.id),
        ),
        Entry::Chat(c) => Command::Request(
            Box::new(Request::ChatSend {
                rig: c.rig.clone(),
                topic: Some(c.topic.clone()),
                sender: None,
                body,
            }),
            "posted".into(),
        ),
    }
}

/// A line typed in Comms: `@Name text` to that agent, else a reply to the
/// selected entry (a threaded reply to a message, a post in a chat's
/// topic), else what to do.
pub fn comms(store: &Store, selected: Option<&Entry>, line: &str) -> Command {
    if line.starts_with('@') {
        return message(store, line, None);
    }
    match selected {
        Some(entry) => reply(entry, line.to_string()),
        None => Command::error("select a message to answer, or start with @Name"),
    }
}

/// A message from you: `@Name text` to that agent, or plain text to
/// `default` (the selected agent or focused terminal). Delivered into its
/// terminal (through any typing guard) and kept in its mailbox.
pub fn message(store: &Store, line: &str, default: Option<&str>) -> Command {
    let (target, text) = match line.strip_prefix('@') {
        Some(rest) => {
            let (name, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (store.find_agent(name), text.trim().to_string())
        }
        None => (default.and_then(|i| store.agents.get(i)), line.to_string()),
    };
    match target {
        Some(a) => message_to(a, text),
        None => Command::error("no such agent"),
    }
}

/// `body` for agent `a`, as [`message`] sends it, but to `a` whatever the
/// text says: for a front end where the recipient is already chosen, so a
/// leading `@word` is part of the message.
pub fn message_to(a: &Agent, body: String) -> Command {
    if body.is_empty() {
        return Command::error("nothing to send");
    }
    Command::Request(
        Box::new(Request::Message {
            from: None,
            to: vec![a.id.clone()],
            rig: None,
            pod: None,
            body,
            reply_to: None,
            force: true,
        }),
        format!("sent to {}", a.label()),
    )
}

/// A post in rig `rig`'s chatroom on `topic` (none: ainetd's `general`),
/// from you.
pub fn chat(rig: &str, topic: Option<&str>, body: String) -> Command {
    let ok = match topic {
        Some(topic) => format!("posted in #{topic} {rig}"),
        None => format!("posted in {rig}"),
    };
    request(
        Request::ChatSend {
            rig: rig.into(),
            topic: topic.map(str::to_string),
            sender: None,
            body,
        },
        ok,
    )
}

/// What a front end can do to a queue item, as `ainet queue <verb>` does
/// it when you (not an agent) run it: the daemon records you as the one
/// acting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueCommand {
    /// Take it: you own it and it is in progress.
    Claim,
    /// Give it back, saying why if you like.
    Unclaim { reason: Option<String> },
    /// Move it to `state`, with a note or not.
    State(QState),
    /// A progress note, the state unchanged.
    Note(String),
    /// Close it and open its successor for `to`, carrying `body`.
    Handoff { to: String, body: String },
    /// Block it on another item (`q-N`) or `external:<name>`; then do
    /// `continuation`.
    Block { on: String, continuation: String },
    /// The item in full, as a report.
    Show,
}

/// The request for `cmd` on queue item `id`: what the console's `ainet
/// queue` sends, and what to say once it is done (or the report's
/// title); or what is missing.
pub fn queue(store: &Store, id: &str, cmd: QueueCommand) -> Command {
    if !store.queue.contains_key(id) {
        return Command::error("no such queue item");
    }
    let id = id.to_string();
    let words = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    match cmd {
        QueueCommand::Claim => request(
            Request::QueueClaim {
                id: id.clone(),
                by: None,
            },
            format!("claimed {id}"),
        ),
        QueueCommand::Unclaim { reason } => request(
            Request::QueueUnclaim {
                id: id.clone(),
                by: None,
                reason: reason.as_deref().and_then(words),
            },
            format!("gave {id} back"),
        ),
        QueueCommand::State(state) => request(
            Request::QueueUpdate {
                id: id.clone(),
                by: None,
                note: None,
                state: Some(state),
            },
            format!("{id} is {}", state.label()),
        ),
        QueueCommand::Note(note) => match words(&note) {
            Some(note) => request(
                Request::QueueUpdate {
                    id: id.clone(),
                    by: None,
                    note: Some(note),
                    state: None,
                },
                format!("noted on {id}"),
            ),
            None => Command::error("write a note first"),
        },
        QueueCommand::Handoff { to, body } => match words(&to) {
            Some(to) => request(
                Request::QueueHandoff {
                    id: id.clone(),
                    by: None,
                    body: body.trim().to_string(),
                    summary: None,
                    to: to.clone(),
                },
                format!("handed {id} to {to}"),
            ),
            None => Command::error("hand it off to which seat?"),
        },
        QueueCommand::Block { on, continuation } => match words(&on) {
            Some(on) => request(
                Request::QueueBlock {
                    id: id.clone(),
                    by: None,
                    on: on.clone(),
                    continuation: continuation.trim().to_string(),
                    wake_after_secs: None,
                },
                format!("{id} is blocked on {on}"),
            ),
            None => Command::error("block it on what? q-N or external:<name>"),
        },
        QueueCommand::Show => request(Request::QueueShow { id: id.clone() }, id),
    }
}

/// Starts the operator rig in the current directory.
pub fn operator_up() -> Command {
    Command::Request(
        Box::new(Request::Up(UpSpec {
            source: "operator".into(),
            cwd: std::env::current_dir().ok(),
            fresh: vec![],
            name: None,
            plan: false,
        })),
        "starting the operator; Ctrl+O again to talk to it".into(),
    )
}

/// `@Mos` completed to `@seat-address ` (or the name without spaces).
pub fn complete(store: &Store, input: &str) -> Option<String> {
    let partial = input.strip_prefix('@').filter(|p| !p.contains(' '))?;
    let a = store.find_agent(partial)?;
    Some(format!(
        "@{} ",
        a.seat.clone().unwrap_or_else(|| a.name.replace(' ', ""))
    ))
}

/// `~/x` as `$HOME/x`.
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use ainet_core::{Decision, InboxKind, Request, Runtime};

    use ainet_core::QState;

    use super::{
        AgentCommand, Command, Context, QueueCommand, answer_choices, chat, comms, complete, for_agent,
        message, message_to, parse, queue,
    };
    use crate::demo::Demo;
    use crate::link::{Backend, Link, Msg};
    use crate::store::{Entry, Store};

    fn demo() -> Store {
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        store
    }

    /// The request a command sends and what it says when done.
    fn sends(cmd: Command) -> (Request, String) {
        match cmd {
            Command::Request(req, ok) => (*req, ok),
            other => panic!("not a request: {other:?}"),
        }
    }

    fn run(store: &Store, view_rig: Option<&str>, line: &str) -> Command {
        parse(
            &Context {
                store,
                view_rig,
                inbox_item: None,
            },
            line,
        )
    }

    #[test]
    fn picked_options_answer_by_their_numbers() {
        let store = demo();
        let mut question = store.inbox[0].clone();
        question.kind = InboxKind::Question;
        question.options = vec!["yes".into(), "no".into(), "later".into()];
        let (req, ok) = sends(answer_choices(&question, &[2, 0, 2, 7]));
        assert!(
            matches!(&req, Request::AnswerQuestion { item, answer } if *item == question.id && answer == "1 3"),
            "{req:?}"
        );
        assert_eq!(ok, "answered");
        assert!(matches!(answer_choices(&question, &[]), Command::Error(_)));
        assert!(matches!(answer_choices(&question, &[3]), Command::Error(_)));
    }

    #[test]
    fn rig_commands_use_the_rig_in_view_or_of_the_selection() {
        let store = demo();
        match sends(run(&store, Some("infra"), "down")) {
            (
                Request::Down {
                    rig,
                    snapshot,
                    delete,
                },
                ok,
            ) => {
                assert_eq!(rig, "infra");
                assert!(snapshot && !delete);
                assert_eq!(ok, "infra is down (snapshot taken)");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            sends(run(&store, None, "restore")),
            (Request::Restore { rig, .. }, _) if rig == "docs"
        ));
        assert!(matches!(run(&Store::new(), None, "down"), Command::Error(e) if e == "/down <rig>"));
    }

    #[test]
    fn up_takes_library_names_and_plans() {
        let store = demo();
        match sends(run(&store, None, "plan starter")) {
            (Request::Up(spec), ok) => {
                assert_eq!(spec.source, "starter");
                assert!(spec.plan);
                assert_eq!(ok, "plan for starter");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(run(&store, None, "up"), Command::Error(_)));
    }

    #[test]
    fn grow_reads_an_optional_runtime_then_the_role() {
        let store = demo();
        match sends(run(&store, None, "grow dev perf codex PERFORMANCE ENGINEER")) {
            (Request::Grow { rig, seats, .. }, _) => {
                assert_eq!(rig, "docs");
                assert_eq!(seats[0].member, "perf");
                assert_eq!(seats[0].runtime, Some(Runtime::Codex));
                assert_eq!(seats[0].role.as_deref(), Some("PERFORMANCE ENGINEER"));
            }
            other => panic!("{other:?}"),
        }
        match sends(run(&store, None, "grow dev docs WRITER")) {
            (Request::Grow { seats, .. }, _) => {
                assert_eq!(seats[0].runtime, None);
                assert_eq!(seats[0].role.as_deref(), Some("WRITER"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn front_end_commands_are_left_to_the_front_end() {
        let store = demo();
        assert!(matches!(run(&store, None, "y"), Command::Answer(Decision::Allow)));
        assert!(matches!(run(&store, None, "cols 3"), Command::Cols(Some(3))));
        assert!(matches!(run(&store, None, "cols 0"), Command::Cols(None)));
        assert!(matches!(run(&store, None, "view"), Command::View(v) if v == "terminals"));
        assert!(matches!(run(&store, None, "answer 1"), Command::Nothing));
        assert!(matches!(run(&store, None, "frobnicate"), Command::Error(e) if e.contains("/help")));
    }

    #[test]
    fn messages_go_to_the_named_agent_or_the_default() {
        let store = demo();
        let lead = store.find_agent("orch-lead@docs").unwrap().clone();
        match sends(message(&store, "@orch-lead ship it", None)) {
            (Request::Message { to, body, force, .. }, _) => {
                assert_eq!(to, std::slice::from_ref(&lead.id));
                assert_eq!(body, "ship it");
                assert!(force);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            sends(message(&store, "hello", Some(&lead.id))),
            (Request::Message { body, .. }, _) if body == "hello"
        ));
        assert!(matches!(message(&store, "@orch-lead", None), Command::Error(e) if e == "nothing to send"));
        assert!(matches!(message(&store, "@nobody hi", None), Command::Error(e) if e == "no such agent"));
        assert!(matches!(message(&store, "hi", None), Command::Error(e) if e == "no such agent"));

        // With the recipient chosen, a leading `@word` is just text.
        let ui = store.find_agent("dev-ui@docs").unwrap();
        assert_ne!(ui.id, lead.id);
        match sends(message_to(&lead, "@dev-ui can you check this".into())) {
            (Request::Message { to, body, force, .. }, ok) => {
                assert_eq!(to, std::slice::from_ref(&lead.id));
                assert_eq!(body, "@dev-ui can you check this");
                assert!(force);
                assert_eq!(ok, format!("sent to {}", lead.label()));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(message_to(&lead, String::new()), Command::Error(e) if e == "nothing to send"));
    }

    #[test]
    fn comms_lines_reply_or_message() {
        let store = demo();
        let entries = store.entries();
        let message = entries
            .iter()
            .find_map(|e| match e {
                Entry::Message(m) => Some(m.clone()),
                Entry::Chat(_) => None,
            })
            .unwrap();
        let chat = entries
            .iter()
            .find_map(|e| match e {
                Entry::Chat(c) => Some(c.clone()),
                Entry::Message(_) => None,
            })
            .unwrap();

        // With an entry selected, a line answers it.
        match sends(comms(&store, Some(&Entry::Message(message.clone())), "on it")) {
            (
                Request::Message {
                    to,
                    body,
                    reply_to,
                    force,
                    ..
                },
                ok,
            ) => {
                assert!(to.is_empty());
                assert_eq!(
                    (body.as_str(), reply_to, force),
                    ("on it", Some(message.id), false)
                );
                assert_eq!(ok, format!("replied to #{}", message.id));
            }
            other => panic!("{other:?}"),
        }
        match sends(comms(&store, Some(&Entry::Chat(chat.clone())), "same here")) {
            (
                Request::ChatSend {
                    rig,
                    topic,
                    body,
                    sender,
                },
                ok,
            ) => {
                assert_eq!(
                    (rig, topic, body),
                    (chat.rig.clone(), Some(chat.topic.clone()), "same here".into())
                );
                assert_eq!((sender, ok.as_str()), (None, "posted"));
            }
            other => panic!("{other:?}"),
        }
        // `@Name` messages that agent, selection or not.
        let lead = store.find_agent("orch-lead@docs").unwrap();
        for selected in [None, Some(Entry::Chat(chat))] {
            assert_eq!(
                format!(
                    "{:?}",
                    sends(comms(&store, selected.as_ref(), "@orch-lead ship it"))
                ),
                format!("{:?}", sends(message_to(lead, "ship it".into())))
            );
        }
        // Nothing selected: what to do.
        assert!(matches!(
            comms(&store, None, "hello"),
            Command::Error(e) if e == "select a message to answer, or start with @Name"
        ));
    }

    #[test]
    fn agent_commands_send_what_the_typed_commands_send() {
        let mut store = demo();
        let lead = store.find_agent("orch-lead@docs").unwrap().clone();
        let seat = lead.seat.clone().unwrap();
        store.selected = Some(lead.id.clone());
        // The menu and the command line agree.
        for (cmd, line) in [
            (AgentCommand::Relaunch, format!("relaunch {seat}")),
            (AgentCommand::Continue, format!("continue {seat}")),
            (AgentCommand::Close, "close".to_string()),
            (AgentCommand::Capture, "capture".to_string()),
            (AgentCommand::Guard(true), format!("guard {seat} on")),
            (AgentCommand::Guard(false), format!("guard {seat} off")),
            (AgentCommand::Remove, format!("remove {seat}")),
        ] {
            let (menu, menu_ok) = sends(for_agent(&store, &lead.id, cmd));
            let (typed, typed_ok) = sends(run(&store, None, &line));
            assert_eq!(format!("{menu:?}"), format!("{typed:?}"), "{cmd:?}");
            assert_eq!(menu_ok, typed_ok, "{cmd:?}");
        }
        match sends(for_agent(&store, &lead.id, AgentCommand::Relaunch)) {
            (Request::Launch { fresh, .. }, ok) => {
                assert!(fresh);
                assert_eq!(ok, format!("launching {seat}"));
            }
            other => panic!("{other:?}"),
        }
        match sends(for_agent(&store, &lead.id, AgentCommand::Transcript)) {
            (Request::Transcript { agent, tail, grep }, ok) => {
                assert_eq!(agent, lead.id);
                assert_eq!((tail, grep), (0, None));
                assert_eq!(ok, format!("transcript of {seat}"));
            }
            other => panic!("{other:?}"),
        }

        match sends(for_agent(&store, &lead.id, AgentCommand::Release)) {
            (Request::Release { seat: s }, ok) => {
                assert_eq!(s, seat);
                assert_eq!(ok, format!("{seat} released"));
            }
            other => panic!("{other:?}"),
        }

        // Seat commands need a seat.
        let loose = store.agents.values().find(|a| a.seat.is_none()).unwrap().clone();
        assert!(matches!(
            for_agent(&store, &loose.id, AgentCommand::Continue),
            Command::Error(e) if e.contains("not in a rig seat")
        ));
        for cmd in [AgentCommand::Release, AgentCommand::Remove] {
            assert!(matches!(
                for_agent(&store, &loose.id, cmd),
                Command::Error(e) if e.contains("not in a rig seat")
            ));
        }
        assert!(matches!(
            for_agent(&store, &loose.id, AgentCommand::Close),
            Command::Request(..)
        ));
        assert!(matches!(
            for_agent(&store, "nobody", AgentCommand::Close),
            Command::Error(e) if e == "no such agent"
        ));
    }

    #[test]
    fn queue_commands_send_what_ainet_queue_sends() {
        let store = demo();
        let id = "q-2";
        let run = |cmd| {
            let (req, ok) = sends(queue(&store, id, cmd));
            (format!("{req:?}"), ok)
        };
        let id_ = id.to_string();
        // As `ainet queue …` sends them when you run it (no agent: `by` is
        // none).
        let cases = [
            (
                QueueCommand::Claim,
                Request::QueueClaim {
                    id: id_.clone(),
                    by: None,
                },
                "claimed q-2",
            ),
            (
                QueueCommand::Unclaim {
                    reason: Some(" busy ".into()),
                },
                Request::QueueUnclaim {
                    id: id_.clone(),
                    by: None,
                    reason: Some("busy".into()),
                },
                "gave q-2 back",
            ),
            (
                QueueCommand::State(QState::Done),
                Request::QueueUpdate {
                    id: id_.clone(),
                    by: None,
                    note: None,
                    state: Some(QState::Done),
                },
                "q-2 is done",
            ),
            (
                QueueCommand::Note("half way".into()),
                Request::QueueUpdate {
                    id: id_.clone(),
                    by: None,
                    note: Some("half way".into()),
                    state: None,
                },
                "noted on q-2",
            ),
            (
                QueueCommand::Handoff {
                    to: "rev-review@docs".into(),
                    body: "please check\n".into(),
                },
                Request::QueueHandoff {
                    id: id_.clone(),
                    by: None,
                    to: "rev-review@docs".into(),
                    body: "please check".into(),
                    summary: None,
                },
                "handed q-2 to rev-review@docs",
            ),
            (
                QueueCommand::Block {
                    on: "q-1".into(),
                    continuation: "pick up the screen".into(),
                },
                Request::QueueBlock {
                    id: id_.clone(),
                    by: None,
                    on: "q-1".into(),
                    continuation: "pick up the screen".into(),
                    wake_after_secs: None,
                },
                "q-2 is blocked on q-1",
            ),
            (QueueCommand::Show, Request::QueueShow { id: id_.clone() }, "q-2"),
        ];
        for (cmd, want, ok) in cases {
            assert_eq!(run(cmd.clone()), (format!("{want:?}"), ok.to_string()), "{cmd:?}");
        }
        // An empty reason is no reason.
        assert!(matches!(
            sends(queue(
                &store,
                id,
                QueueCommand::Unclaim {
                    reason: Some(" ".into())
                }
            ))
            .0,
            Request::QueueUnclaim { reason: None, .. }
        ));
        // What is missing.
        for (cmd, why) in [
            (QueueCommand::Note("  ".into()), "write a note first"),
            (
                QueueCommand::Handoff {
                    to: String::new(),
                    body: "x".into(),
                },
                "hand it off to which seat?",
            ),
            (
                QueueCommand::Block {
                    on: " ".into(),
                    continuation: String::new(),
                },
                "block it on what? q-N or external:<name>",
            ),
        ] {
            assert!(
                matches!(queue(&store, id, cmd), Command::Error(e) if e == why),
                "{why}"
            );
        }
        assert!(matches!(
            queue(&store, "q-99", QueueCommand::Claim),
            Command::Error(e) if e == "no such queue item"
        ));
    }

    #[test]
    fn chat_posts_go_to_a_rig_and_topic() {
        let store = demo();
        match sends(chat("docs", Some("design"), "hi".into())) {
            (
                Request::ChatSend {
                    rig,
                    topic,
                    sender,
                    body,
                },
                ok,
            ) => {
                assert_eq!(
                    (rig.as_str(), topic.as_deref(), sender, body.as_str()),
                    ("docs", Some("design"), None, "hi")
                );
                assert_eq!(ok, "posted in #design docs");
            }
            other => panic!("{other:?}"),
        }
        // `/chat` is unchanged: no topic, ainetd's general.
        assert_eq!(
            format!("{:?}", sends(run(&store, None, "chat hello all"))),
            format!("{:?}", sends(chat("docs", None, "hello all".into())))
        );
        assert_eq!(sends(run(&store, None, "chat hi")).1, "posted in docs");
    }

    #[test]
    fn asked_replies_come_back_whole_by_their_token() {
        let (tx, rx) = mpsc::channel();
        let (mut link, snapshot) = Link::new(Backend::Demo(Box::default()), tx);
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(snapshot.unwrap())));
        let lead = store.find_agent("orch-lead@docs").unwrap().clone();
        let reply = |rx: &mpsc::Receiver<Msg>| {
            rx.try_iter()
                .find_map(|m| match m {
                    Msg::Reply { token, reply } => Some((token, reply)),
                    _ => None,
                })
                .expect("a reply")
        };

        let (req, _) = sends(for_agent(&store, &lead.id, AgentCommand::Transcript));
        link.ask(req, 7);
        let (token, text) = reply(&rx);
        assert_eq!(token, 7);
        let text = text.unwrap();
        assert!(text.contains(lead.task.as_deref().unwrap()), "{text}");

        link.ask(Request::SpecShow { name: "docs".into() }, 8);
        let (token, yaml) = reply(&rx);
        assert_eq!(token, 8);
        assert!(yaml.unwrap().contains("name: docs"));
        link.ask(Request::Export { rig: "nope".into() }, 9);
        assert_eq!(reply(&rx), (9, Err("no spec named nope in the demo".into())));

        // A request that only does something: its events, then an empty
        // reply; never a flash.
        let (req, _) = sends(queue(&store, "q-2", QueueCommand::Claim));
        link.ask(req, 10);
        let msgs: Vec<Msg> = rx.try_iter().collect();
        assert!(msgs.iter().any(|m| matches!(m, Msg::Event(_))));
        assert!(
            msgs.iter()
                .any(|m| matches!(m, Msg::Reply { token: 10, reply: Ok(t) } if t.is_empty()))
        );
        assert!(!msgs.iter().any(|m| matches!(m, Msg::Flash(..))));
    }

    #[test]
    fn names_complete_to_seat_addresses() {
        let store = demo();
        assert_eq!(complete(&store, "@orch-l").as_deref(), Some("@orch-lead@docs "));
        assert_eq!(complete(&store, "@orch-lead hi"), None);
        assert_eq!(complete(&store, "orch"), None);
    }

    #[test]
    fn the_demo_link_answers_requests_through_the_sink() {
        let (tx, rx) = mpsc::channel();
        let (mut link, snapshot) = Link::new(Backend::Demo(Box::default()), tx);
        assert!(link.is_demo());
        assert!(snapshot.is_some(), "the demo hands over its fleet at once");

        link.request(Request::Ps, "rigs".into());
        let replies: Vec<Msg> = rx.try_iter().collect();
        assert!(
            replies
                .iter()
                .any(|m| matches!(m, Msg::Popup(title, text) if title == "rigs" && text.contains("docs"))),
            "a report comes back as a popup"
        );

        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(snapshot.unwrap())));
        let waiting = store
            .inbox
            .iter()
            .find(|i| i.waiting && i.kind == InboxKind::Permission)
            .cloned()
            .expect("the demo holds a request");
        let (req, ok) = store.answer(&waiting, Decision::Allow).unwrap();
        link.request(req, ok.clone());
        let replies: Vec<Msg> = rx.try_iter().collect();
        assert!(replies.iter().any(|m| matches!(m, Msg::Event(_))));
        assert!(
            replies
                .iter()
                .any(|m| matches!(m, Msg::Flash(t, false) if *t == ok))
        );
    }
}
