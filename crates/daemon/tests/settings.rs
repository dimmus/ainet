//! What settings.json changes in a running ainetd, and that without one
//! (or with only the desktop app's keys) it does what it always did.

mod common;

use std::time::{Duration, Instant};

use ainet_core::protocol::{QSpec, SeatSpec};
use ainet_core::{Message, Request, Response, Rig, Runtime, SeatStatus};
use common::Daemon;

/// Other Claude Code sessions on this machine stay out of the fleet.
const QUIET: &str = r#""discovery": { "claude_sessions": false }"#;

fn message(d: &Daemon, to: &str, body: &str) -> Message {
    match d.call(&Request::Message {
        from: None,
        to: vec![to.into()],
        rig: None,
        pod: None,
        body: body.into(),
        reply_to: None,
        force: false,
    }) {
        Response::Messages { mut messages } => messages.remove(0),
        other => panic!("not sent: {other:?}"),
    }
}

fn interact(d: &Daemon, agent: &str, on: bool) {
    assert!(matches!(
        d.call(&Request::Interact {
            agent: agent.into(),
            on
        }),
        Response::Ok
    ));
}

/// A one-seat shell rig, once its seat runs.
fn shell_rig(d: &Daemon, name: &str, cwd: Option<std::path::PathBuf>) -> Rig {
    let resp = d.call(&Request::Create {
        rig: name.into(),
        seat: SeatSpec {
            runtime: Some(Runtime::Shell),
            cwd,
            ..SeatSpec::default()
        },
    });
    assert!(matches!(resp, Response::Rig { .. }), "{resp:?}");
    let mut rig = None;
    d.until("the seat runs", |d| {
        match d.call(&Request::Status { rig: name.into() }) {
            Response::Rig { rig: r } if r.seats[0].status == SeatStatus::Running => {
                rig = Some(r);
                true
            }
            _ => false,
        }
    });
    rig.unwrap()
}

fn snapshot(d: &Daemon) -> (Vec<Message>, usize) {
    match d.call(&Request::Snapshot) {
        Response::Snapshot { messages, inbox, .. } => (messages, inbox.len()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn messages_end_with_the_configured_note_and_hold_as_told() {
    let d = Daemon::start("note", None);
    let claude = d.fake_claude();
    d.write_settings(&format!(
        r#"{{ {QUIET},
             "runtimes": {{ "claude": {{ "binary": {claude:?} }} }},
             "messaging": {{ "message_note": "Reply with id={{id}}, please.",
                            "hold_while_interacting": false }} }}"#
    ));
    std::thread::sleep(Duration::from_millis(1500));
    let agent = d.spawn(Runtime::ClaudeCode, vec![]);
    d.wait_for(&agent.id, "ready");
    interact(&d, &agent.id, true);
    let m = message(&d, &agent.id, "Ship it.");
    assert!(!m.held, "not held while you are at its terminal");
    d.wait_for(&agent.id, "[ainet] message #1 from you (operator):");
    d.wait_for(&agent.id, &format!("Reply with id={}, please.", m.id));

    // Back to the defaults, live: today's note, and held while you type.
    interact(&d, &agent.id, false);
    d.write_settings(&format!(
        r#"{{ {QUIET}, "runtimes": {{ "claude": {{ "binary": {claude:?} }} }} }}"#
    ));
    std::thread::sleep(Duration::from_millis(2000));
    let m = message(&d, &agent.id, "Again.");
    d.wait_for(
        &agent.id,
        &format!("(Answer with the ainet tool reply_message id={}", m.id),
    );
    interact(&d, &agent.id, true);
    assert!(message(&d, &agent.id, "Later.").held);
}

#[test]
fn being_at_a_terminal_holds_messages_by_default() {
    let d = Daemon::start("hold", None);
    let agent = d.shell("printf 'ready\\n'; sleep 60");
    interact(&d, &agent, true);
    let m = message(&d, &agent, "echo hi");
    assert!(m.held, "held while you are at the terminal");
}

#[test]
fn pauses_follow_settings() {
    let d = Daemon::start(
        "pauses",
        Some(&format!(
            r#"{{ {QUIET}, "messaging": {{ "paste_settle_ms": 1200, "flush_gap_ms": 1000 }} }}"#
        )),
    );
    let agent = d.shell("printf 'ready\\n'; sleep 60");
    let at = Instant::now();
    assert!(matches!(
        d.call(&Request::Send {
            agent: agent.clone(),
            text: "x".into()
        }),
        Response::Ok
    ));
    assert!(at.elapsed() >= Duration::from_millis(1150), "{:?}", at.elapsed());

    interact(&d, &agent, true);
    assert!(message(&d, &agent, "a").held);
    assert!(message(&d, &agent, "b").held);
    let at = Instant::now();
    interact(&d, &agent, false);
    // Two held messages, each pasted (1.2 s) and followed by the gap (1 s).
    assert!(at.elapsed() >= Duration::from_millis(4000), "{:?}", at.elapsed());
}

#[test]
fn default_pauses_are_short() {
    let d = Daemon::start("short", None);
    let agent = d.shell("printf 'ready\\n'; sleep 60");
    let at = Instant::now();
    d.call(&Request::Send {
        agent,
        text: "x".into(),
    });
    assert!(at.elapsed() < Duration::from_millis(1000), "{:?}", at.elapsed());
    assert_eq!(ainet_pty::PASTE_SETTLE, Duration::from_millis(60));
}

#[test]
fn new_seats_take_the_typing_guard_default() {
    let d = Daemon::start("guard-off", None);
    let rig = shell_rig(&d, "plain", Some(d.dir.clone()));
    assert!(!rig.seats[0].typing_guard, "off by default");

    d.write_settings(&format!(
        r#"{{ {QUIET}, "messaging": {{ "typing_guard": true }} }}"#
    ));
    std::thread::sleep(Duration::from_millis(1500));
    let rig = shell_rig(&d, "guarded", Some(d.dir.clone()));
    assert!(rig.seats[0].typing_guard);
    let agent = rig.seats[0].agent.clone().unwrap();
    assert!(message(&d, &agent, "echo hi").held, "the guard holds it");
}

#[test]
fn queue_nudges_and_watchdogs_follow_settings() {
    let d = Daemon::start("queue", None);
    let rig = shell_rig(&d, "q", Some(d.dir.clone()));
    let seat = rig.seats[0].address.clone();
    let create = |d: &Daemon, nudge: Option<bool>| {
        let resp = d.call(&Request::QueueCreate(QSpec {
            destination: seat.clone(),
            summary: "do it".into(),
            nudge,
            ..QSpec::default()
        }));
        assert!(matches!(resp, Response::QItem { .. }), "{resp:?}");
    };
    let register = |d: &Daemon, secs: u64| {
        d.call(&Request::WatchdogRegister {
            target: seat.clone(),
            interval_secs: secs,
            message: None,
            registered_by: None,
        })
    };

    // Today's behaviour: nudged, at least 30 s, the queue reminder.
    create(&d, None);
    assert_eq!(snapshot(&d).0.len(), 1, "the destination was told");
    match register(&d, 29) {
        Response::Error { message } => assert_eq!(message, "a watchdog interval must be at least 30 seconds"),
        other => panic!("{other:?}"),
    }
    match register(&d, 0) {
        Response::Watchdogs { watchdogs } => {
            assert_eq!(watchdogs[0].interval_secs, 600);
            assert_eq!(watchdogs[0].message, ainet_core::settings::WATCHDOG_MESSAGE);
        }
        other => panic!("{other:?}"),
    }

    d.write_settings(&format!(
        r#"{{ {QUIET}, "queue": {{ "nudge": false,
             "watchdog": {{ "min_interval_secs": 5, "default_interval_secs": 7, "message": "wake up" }} }} }}"#
    ));
    d.until("the new minimum", |d| {
        matches!(register(d, 6), Response::Watchdogs { .. })
    });
    create(&d, None);
    assert_eq!(snapshot(&d).0.len(), 1, "not told this time");
    create(&d, Some(true));
    assert_eq!(snapshot(&d).0.len(), 2, "unless the request asks");
    match register(&d, 0) {
        Response::Watchdogs { watchdogs } => {
            assert_eq!(
                (watchdogs[0].interval_secs, watchdogs[0].message.as_str()),
                (7, "wake up")
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(register(&d, 4), Response::Error { message } if message.contains("at least 5 seconds")));

    // A broken file keeps the last good settings.
    d.write_settings(r#"{ "queue": { "watchdog": { "min_interval_secs": "soon" } } }"#);
    std::thread::sleep(Duration::from_millis(2000));
    assert!(matches!(register(&d, 6), Response::Watchdogs { .. }));
}

#[test]
fn rigs_find_specs_directories_and_snapshots_as_told() {
    let d = Daemon::start("rigs", None);
    let specs = d.dir.join("more-specs");
    std::fs::create_dir_all(&specs).unwrap();
    std::fs::write(
        specs.join("extra.yaml"),
        "version: \"0.2\"\nname: extra\nsummary: From settings\npods:\n  - id: dev\n    label: Dev\n    members:\n      - id: solo\n        runtime: terminal\n        cwd: \".\"\n",
    )
    .unwrap();
    let names = |d: &Daemon| match d.call(&Request::Specs) {
        Response::Specs { specs } => specs.into_iter().map(|s| s.name).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert!(!names(&d).contains(&"extra".to_string()));
    let work = d.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let rig = shell_rig(&d, "one", Some(d.dir.clone()));
    for _ in 0..3 {
        d.call(&Request::SnapshotCreate {
            rig: "one".into(),
            label: None,
        });
        std::thread::sleep(Duration::from_millis(5));
    }
    let count = |d: &Daemon| match d.call(&Request::SnapshotList { rig: "one".into() }) {
        Response::Snapshots { snapshots } => snapshots.len(),
        other => panic!("{other:?}"),
    };
    assert_eq!(count(&d), 3, "all kept by default");
    assert_eq!(rig.seats[0].cwd, d.dir);

    d.write_settings(&format!(
        r#"{{ {QUIET}, "rigs": {{ "spec_dirs": [{specs:?}], "default_cwd": {work:?}, "keep_snapshots": 2 }} }}"#
    ));
    d.until("the extra spec", |d| names(d).contains(&"extra".to_string()));
    assert!(matches!(
        d.call(&Request::SpecShow { name: "extra".into() }),
        Response::Text { text } if text.contains("From settings")
    ));
    d.call(&Request::SnapshotCreate {
        rig: "one".into(),
        label: None,
    });
    assert_eq!(count(&d), 2, "the newest two");
    let rig = shell_rig(&d, "two", None);
    assert_eq!(rig.seats[0].cwd, work, "no directory given: rigs.default_cwd");
}

#[test]
fn memory_caps_follow_settings() {
    let d = Daemon::start(
        "caps",
        Some(&format!(
            r#"{{ {QUIET}, "retention": {{ "messages_in_memory": 100, "inbox_items": 10 }} }}"#
        )),
    );
    for i in 0..105 {
        message(&d, "operator", &format!("note {i}"));
    }
    let (messages, inbox) = snapshot(&d);
    assert_eq!((messages.len(), inbox), (100, 10));
    assert_eq!(messages[0].body, "note 5", "the oldest went");

    // Back to the defaults: nothing comes back, but nothing more goes.
    d.write_settings("{}");
    std::thread::sleep(Duration::from_millis(1500));
    for i in 0..20 {
        message(&d, "operator", &format!("more {i}"));
    }
    let (messages, inbox) = snapshot(&d);
    assert_eq!((messages.len(), inbox), (120, 30));
}

#[test]
fn scrollback_follows_settings() {
    let lines = |d: &Daemon| {
        let agent = d.shell("seq 1 300; printf 'ready\\n'; sleep 60");
        match d.call(&Request::Screen {
            agent,
            formatted: false,
            history: 1000,
        }) {
            Response::Screen { rows, .. } => rows.len(),
            other => panic!("{other:?}"),
        }
    };
    let d = Daemon::start("scroll", None);
    assert!(lines(&d) >= 300, "5000 rows by default");
    d.write_settings(r#"{ "retention": { "scrollback_lines": 20 } }"#);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(lines(&d) <= 20 + 40, "20 rows above a 40-row screen");
}

#[test]
fn rigs_that_were_running_come_back_only_when_asked() {
    let d = Daemon::start("restore", None);
    shell_rig(&d, "keep", Some(d.dir.clone()));
    let dir = d.stop();
    let status = |d: &Daemon| match d.call(&Request::Status { rig: "keep".into() }) {
        Response::Rig { rig } => rig.seats[0].status,
        other => panic!("{other:?}"),
    };

    // By default a restart leaves the seat stopped.
    let d = Daemon::on(dir);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(status(&d), SeatStatus::Stopped);
    // Nothing ran this time, so the next start has nothing to restore:
    // bring it up again first.
    assert!(matches!(
        d.call(&Request::Up(ainet_core::protocol::UpSpec {
            source: "keep".into(),
            ..Default::default()
        })),
        Response::Rig { .. }
    ));
    d.until("running again", |d| status(d) == SeatStatus::Running);
    d.write_settings(r#"{ "daemon": { "restore_rigs": true } }"#);
    let dir = d.stop();

    let d = Daemon::on(dir);
    d.until("restored on start", |d| status(d) == SeatStatus::Running);
}

#[test]
fn claude_sessions_join_the_fleet_unless_told_not_to() {
    let in_fleet = |settings: Option<&str>, name: &str| {
        let d = Daemon::start(name, settings);
        let sessions = d.dir.join("claude-sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let pid = std::process::id();
        std::fs::write(
            sessions.join(format!("{pid}.json")),
            format!(
                r#"{{ "pid": {pid}, "sessionId": "sess-{name}", "cwd": "/", "startedAt": 1, "status": "idle" }}"#
            ),
        )
        .unwrap();
        // Asked for, it is listed either way; tmux only when on.
        match d.call(&Request::Discover) {
            Response::Discovered { sessions } => {
                assert!(sessions.iter().any(|s| s.id == format!("sess-{name}")));
                if name == "off" {
                    assert!(sessions.iter().all(|s| s.tmux_session.is_none()));
                }
            }
            other => panic!("{other:?}"),
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(2500) {
            if let Response::Snapshot { agents, .. } = d.call(&Request::Snapshot)
                && agents.iter().any(|a| a.id == format!("sess-{name}"))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    };
    assert!(in_fleet(Some(r#"{ "discovery": { "interval_ms": 500 } }"#), "on"));
    assert!(!in_fleet(
        Some(r#"{ "discovery": { "interval_ms": 500, "claude_sessions": false, "tmux": false } }"#),
        "off"
    ));
}
