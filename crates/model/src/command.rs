//! The operator console's language: `/commands`, `@Name message` lines,
//! replies, answers. Each becomes a [`Request`] (plus what to say once it is
//! done) or something only the front end can do, so every front end means
//! the same thing by the same words.

use std::path::PathBuf;

use ainet_core::protocol::{QSpec, SeatSpec, SpawnSpec, UpSpec};
use ainet_core::{Decision, InboxItem, Request, Runtime};

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
                Some(a) => req(
                    Request::Close { agent: a.id.clone() },
                    format!("closing {}", a.label()),
                ),
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
            Some(seat) => req(
                Request::RemoveSeat { seat: seat.into() },
                format!("removed {seat}"),
            ),
            None => Command::error("/remove <seat>"),
        },
        "launch" | "relaunch" => match arg(1)
            .map(str::to_string)
            .or_else(|| store.selected_agent().and_then(|a| a.seat.clone()))
        {
            Some(seat) => req(
                Request::Launch {
                    seat: seat.clone(),
                    fresh: arg(0) == Some("relaunch"),
                },
                format!("launching {seat}"),
            ),
            None => Command::error("/launch <seat>"),
        },
        "continue" => match arg(1)
            .map(str::to_string)
            .or_else(|| store.selected_agent().and_then(|a| a.seat.clone()))
        {
            Some(seat) => req(
                Request::SeatContinue { seat: seat.clone() },
                format!("nudged {seat}"),
            ),
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
            Some(rig) => req(
                Request::ChatSend {
                    rig: rig.clone(),
                    topic: None,
                    sender: None,
                    body: rest(1),
                },
                format!("posted in {rig}"),
            ),
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
            Some(t) => req(
                Request::Capture {
                    targets: vec![t],
                    rig: None,
                    pod: None,
                    lines: 80,
                },
                "terminal".into(),
            ),
            None => Command::error("/capture <agent>"),
        },
        "whoami" => match store.selected.clone() {
            Some(id) => req(Request::Whoami { agent: id }, "who".into()),
            None => Command::error("select an agent"),
        },
        other => Command::Error(format!("no command /{other}; /help lists them")),
    }
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
        Some(a) if !text.is_empty() => Command::Request(
            Box::new(Request::Message {
                from: None,
                to: vec![a.id.clone()],
                rig: None,
                pod: None,
                body: text,
                reply_to: None,
                force: true,
            }),
            format!("sent to {}", a.label()),
        ),
        Some(_) => Command::error("nothing to send"),
        None => Command::error("no such agent"),
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

    use super::{Command, Context, complete, message, parse};
    use crate::demo::Demo;
    use crate::link::{Backend, Link, Msg};
    use crate::store::Store;

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
