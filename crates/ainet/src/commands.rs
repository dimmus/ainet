//! What each command does: build a request, call ainetd, print the reply.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ainet_core::protocol::{QSpec, SeatSpec, SpawnSpec, UpSpec};
use ainet_core::{QState, Request, Response, RigStatus};
use anyhow::{Context, Result, bail};

use crate::cli::{
    ChatCmd, Cli, Cmd, MsgCmd, QueueCmd, RigCmd, SeatCmd, SnapshotCmd, SpecsCmd, TerminalCmd, WatchdogCmd,
};
use crate::conn::{Conn, RIG_TIMEOUT, check, me, unexpected};
use crate::output;

/// Ends the process with this code, after the message (if any) is printed.
#[derive(Debug)]
pub struct Exit(pub u8, pub String);

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

impl std::error::Error for Exit {}

/// How to print a reply.
struct Out {
    json: bool,
}

impl Out {
    fn show(&self, resp: &Response) {
        if self.json {
            println!("{}", output::json(resp));
        } else {
            print!("{}", output::human(resp));
        }
    }

    /// For commands whose reply is only an acknowledgement.
    fn done(&self, resp: &Response, what: &str) {
        match resp {
            Response::Ok if !self.json => println!("{what}"),
            other => self.show(other),
        }
    }

    fn value(&self, v: &serde_json::Value) {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }
}

fn text(words: &[String]) -> String {
    words.join(" ")
}

/// A path the daemon can resolve, which runs elsewhere: made absolute.
fn absolute(p: &Path) -> Result<PathBuf> {
    p.canonicalize().with_context(|| format!("{}", p.display()))
}

fn opt_absolute(p: Option<PathBuf>) -> Result<Option<PathBuf>> {
    p.as_deref().map(absolute).transpose()
}

/// `-` reads stdin.
fn read_body(body: Option<String>, file: Option<PathBuf>) -> Result<String> {
    let stdin = || -> Result<String> {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    };
    match (body, file) {
        (Some(b), _) if b == "-" => stdin(),
        (Some(b), _) => Ok(b),
        (None, Some(f)) if f.as_os_str() == "-" => stdin(),
        (None, Some(f)) => std::fs::read_to_string(&f).with_context(|| format!("{}", f.display())),
        (None, None) => Ok(String::new()),
    }
}

/// `--rig`, else the caller's own rig when run by an agent.
fn resolve_rig(conn: &Conn, rig: Option<String>) -> Result<String> {
    if let Some(r) = rig {
        return Ok(r);
    }
    let Some(me) = me() else {
        bail!("--rig is required (AINET_AGENT_ID is not set, so there is no rig of your own)")
    };
    match conn.ok(&Request::Whoami { agent: me })? {
        Response::Whoami { whoami } => match whoami.seat {
            Some(seat) => Ok(seat.rig),
            None => bail!("you are not in a rig; pass --rig"),
        },
        other => Err(unexpected(other)),
    }
}

fn snapshot(conn: &Conn) -> Result<Response> {
    conn.ok(&Request::Snapshot)
}

pub fn run(cli: Cli, socket: PathBuf) -> Result<ExitCode> {
    let out = Out { json: cli.json };
    let conn = Conn::new(socket);
    let Some(cmd) = cli.cmd else {
        if !cli.demo {
            conn.ensure_daemon()?;
        }
        return tui(&conn, cli.demo, !cli.no_mouse);
    };
    // Commands that run without (or before) the daemon.
    match &cmd {
        Cmd::Daemon => return daemon().map(|_| ExitCode::SUCCESS),
        Cmd::Hook => return crate::hook::run(&conn.socket).map(|_| ExitCode::SUCCESS),
        Cmd::Mcp => return crate::mcp::run(conn).map(|_| ExitCode::SUCCESS),
        Cmd::InstallHooks => {
            crate::hook::print_hooks();
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Critter { ids, octant } => {
            critter(ids, *octant);
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Shutdown => {
            out.done(&conn.ok(&Request::Shutdown)?, "ainetd stopped");
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Terminal(TerminalCmd::Status) => return terminal_status(&conn, &out),
        _ => {}
    }
    conn.ensure_daemon()?;
    match cmd {
        Cmd::Daemon | Cmd::Hook | Cmd::Mcp | Cmd::InstallHooks | Cmd::Critter { .. } | Cmd::Shutdown => {
            unreachable!("handled above")
        }
        Cmd::Terminal(TerminalCmd::Status) => unreachable!("handled above"),
        Cmd::Terminal(TerminalCmd::Open { view }) => return tui_on(&view),

        // ---- rigs --------------------------------------------------------
        Cmd::Up {
            source,
            cwd,
            name,
            fresh,
            plan,
        } => {
            // A path is resolved here; a library or rig name goes as is.
            let p = Path::new(&source);
            let source = if p.exists() {
                absolute(p)?.display().to_string()
            } else {
                source
            };
            let req = Request::Up(UpSpec {
                source,
                cwd: opt_absolute(cwd)?,
                fresh,
                name,
                plan,
            });
            out.show(&conn.ok_for(&req, RIG_TIMEOUT)?);
        }
        Cmd::Down {
            rig,
            no_snapshot,
            delete,
        } => {
            let req = Request::Down {
                rig: rig.clone(),
                snapshot: !no_snapshot,
                delete,
            };
            out.done(&conn.ok_for(&req, RIG_TIMEOUT)?, &format!("{rig} is down"));
        }
        Cmd::Ps { nodes, rig } => ps(&conn, &out, nodes, rig)?,
        Cmd::Status { rig } => status(&conn, &out, rig)?,
        Cmd::Snapshot(SnapshotCmd::Create { rig, label }) => {
            out.show(&conn.ok_for(&Request::SnapshotCreate { rig, label }, RIG_TIMEOUT)?)
        }
        Cmd::Snapshot(SnapshotCmd::List { rig }) => out.show(&conn.ok(&Request::SnapshotList { rig })?),
        Cmd::Restore { rig, snapshot, fresh } => {
            let req = Request::Restore {
                rig: rig.clone(),
                snapshot,
                fresh,
            };
            out.done(&conn.ok_for(&req, RIG_TIMEOUT)?, &format!("{rig} restored"));
        }
        Cmd::Grow(g) => {
            let seat = SeatSpec {
                pod: g.pod,
                member: g.member,
                runtime: g.runtime,
                role: g.role,
                label: g.label,
                model: g.model,
                cwd: opt_absolute(g.cwd)?,
                agent_ref: g.agent_ref,
                ..SeatSpec::default()
            };
            let req = Request::Grow {
                rig: g.rig,
                seats: vec![seat],
                edges: vec![],
            };
            out.done(&conn.ok_for(&req, RIG_TIMEOUT)?, "seat added");
        }
        Cmd::Expand {
            rig,
            pod,
            members,
            runtime,
            role,
            model,
            cwd,
        } => {
            let cwd = opt_absolute(cwd)?;
            let mut seats = Vec::new();
            for m in &members {
                let (member, rt) = crate::cli::parse_member(m).map_err(anyhow::Error::msg)?;
                seats.push(SeatSpec {
                    pod: pod.clone(),
                    member,
                    runtime: rt.or(runtime),
                    role: role.clone(),
                    model: model.clone(),
                    cwd: cwd.clone(),
                    ..SeatSpec::default()
                });
            }
            let n = seats.len();
            let req = Request::Grow {
                rig,
                seats,
                edges: vec![],
            };
            out.done(&conn.ok_for(&req, RIG_TIMEOUT)?, &format!("{n} seats added"));
        }
        Cmd::Shrink { rig, pod } => out.done(
            &conn.ok_for(
                &Request::Shrink {
                    rig,
                    pod: pod.clone(),
                },
                RIG_TIMEOUT,
            )?,
            &format!("pod {pod} removed"),
        ),
        Cmd::Remove { seat } => out.done(
            &conn.ok_for(&Request::RemoveSeat { seat: seat.clone() }, RIG_TIMEOUT)?,
            &format!("{seat} removed"),
        ),
        Cmd::Launch { seat, fresh } => out.done(
            &conn.ok_for(
                &Request::Launch {
                    seat: seat.clone(),
                    fresh,
                },
                RIG_TIMEOUT,
            )?,
            &format!("{seat} launched"),
        ),
        Cmd::Create {
            rig,
            runtime,
            cwd,
            role,
            model,
            pod,
            member,
        } => {
            let cwd = match cwd {
                Some(p) => absolute(&p)?,
                None => std::env::current_dir()?,
            };
            let req = Request::Create {
                rig,
                seat: SeatSpec {
                    pod,
                    member,
                    runtime,
                    role,
                    model,
                    cwd: Some(cwd),
                    ..SeatSpec::default()
                },
            };
            out.show(&conn.ok_for(&req, RIG_TIMEOUT)?);
        }
        Cmd::Export { rig, output } => match (conn.ok(&Request::Export { rig })?, output) {
            (Response::Text { text }, Some(path)) if !out.json => {
                std::fs::write(&path, text).with_context(|| format!("{}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            (resp, _) => out.show(&resp),
        },
        Cmd::Import { path } => {
            let path = absolute(&path)?;
            out.done(&conn.ok(&Request::Import { path })?, "imported")
        }
        Cmd::Specs {
            cmd: None | Some(SpecsCmd::Ls),
        } => out.show(&conn.ok(&Request::Specs)?),
        Cmd::Specs {
            cmd: Some(SpecsCmd::Show { name }),
        } => out.show(&conn.ok(&Request::SpecShow { name })?),
        Cmd::Validate { path }
        | Cmd::Rig {
            cmd: RigCmd::Validate { path },
        } => {
            let path = absolute(&path)?;
            out.done(&conn.ok(&Request::Validate { path })?, "valid");
        }
        Cmd::Discover => out.show(&conn.ok(&Request::Discover)?),
        Cmd::Adopt {
            id,
            rig,
            pod,
            member,
            runtime,
            role,
            model,
        } => {
            let req = Request::Adopt {
                id,
                rig,
                seat: SeatSpec {
                    pod,
                    member,
                    runtime,
                    role,
                    model,
                    ..SeatSpec::default()
                },
            };
            out.done(&conn.ok(&req)?, "adopted");
        }
        Cmd::Release { seat } => out.done(
            &conn.ok(&Request::Release { seat: seat.clone() })?,
            &format!("{seat} released"),
        ),
        Cmd::Start => start(&conn, &out)?,

        // ---- talking -----------------------------------------------------
        Cmd::Send {
            text: words,
            mut to,
            pod,
            rig,
            from,
            force,
            raw,
        } => {
            let mut words = words;
            // `send <agent> <text…>`, as before and as in OpenRig.
            if to.is_empty() && pod.is_none() && rig.is_none() {
                if words.len() < 2 {
                    bail!("who to? use `send <agent> <text…>` or --to / --pod / --rig");
                }
                to = vec![words.remove(0)];
            }
            if raw {
                if to.len() != 1 || pod.is_some() || rig.is_some() {
                    bail!("--raw types into one agent's terminal: give exactly one recipient");
                }
                let req = Request::Send {
                    agent: to.remove(0),
                    text: text(&words),
                };
                out.done(&conn.ok(&req)?, "sent");
            } else {
                let req = Request::Message {
                    from: from.or_else(me),
                    to,
                    rig,
                    pod,
                    body: text(&words),
                    reply_to: None,
                    force,
                };
                out.show(&conn.ok(&req)?);
            }
        }
        Cmd::Broadcast {
            text: words,
            rig,
            pod,
            from,
            force,
        } => {
            let rig = resolve_rig(&conn, rig)?;
            let req = Request::Message {
                from: from.or_else(me),
                to: vec![],
                rig: Some(rig),
                pod,
                body: text(&words),
                reply_to: None,
                force,
            };
            out.show(&conn.ok(&req)?);
        }
        Cmd::Msg(m) => return msg(&conn, &out, m),
        Cmd::Chatroom(c) => return chat(&conn, &out, c),
        Cmd::Queue(q) => queue(&conn, &out, q)?,

        // ---- observing ---------------------------------------------------
        Cmd::Whoami { agent } => {
            let Some(agent) = agent.or_else(me) else {
                bail!("not running as an ainet agent (AINET_AGENT_ID is not set); pass --agent")
            };
            out.show(&conn.ok(&Request::Whoami { agent })?);
        }
        Cmd::Capture {
            targets,
            rig,
            pod,
            lines,
        } => {
            if targets.is_empty() && rig.is_none() && pod.is_none() {
                bail!("capture what? name agents or seats, or pass --rig / --pod");
            }
            out.show(&conn.ok(&Request::Capture {
                targets,
                rig,
                pod,
                lines,
            })?);
        }
        Cmd::Transcript { agent, tail, grep } => {
            out.show(&conn.ok(&Request::Transcript { agent, tail, grep })?)
        }
        Cmd::Screen { agent, history } => out.show(&conn.ok(&Request::Screen {
            agent,
            formatted: false,
            history,
        })?),
        Cmd::List => match snapshot(&conn)? {
            Response::Snapshot { agents, inbox, .. } if out.json => {
                out.value(&serde_json::json!({ "agents": agents, "inbox": inbox }))
            }
            resp => out.show(&resp),
        },
        Cmd::Seat(SeatCmd::SetTypingGuard { seat, enabled, .. }) => {
            let what = format!("typing guard {} for {seat}", if enabled { "on" } else { "off" });
            out.done(&conn.ok(&Request::TypingGuard { seat, on: enabled })?, &what);
        }
        Cmd::Seat(SeatCmd::HeldMessages { seat }) => out.show(&conn.ok(&Request::Held { seat })?),
        Cmd::Seat(SeatCmd::Continue { seat }) => out.done(
            &conn.ok(&Request::SeatContinue { seat: seat.clone() })?,
            &format!("nudged {seat}"),
        ),
        Cmd::Watchdog(w) => watchdog(&conn, &out, w)?,

        // ---- agents ------------------------------------------------------
        Cmd::Spawn {
            runtime,
            cwd,
            role,
            task,
            args,
        } => {
            let cwd = match cwd {
                Some(p) => absolute(&p)?,
                None => std::env::current_dir()?,
            };
            let req = Request::Spawn(SpawnSpec {
                runtime: runtime.into(),
                cwd,
                role,
                task,
                parent: None,
                args,
            });
            out.show(&conn.ok(&req)?);
        }
        Cmd::Close { agent } => out.done(
            &conn.ok(&Request::Close { agent: agent.clone() })?,
            &format!("{agent} closed"),
        ),
    }
    Ok(ExitCode::SUCCESS)
}

fn tui(conn: &Conn, demo: bool, mouse: bool) -> Result<ExitCode> {
    ainet_tui::run(ainet_tui::Options {
        demo,
        socket: conn.socket.clone(),
        mouse,
    })?;
    Ok(ExitCode::SUCCESS)
}

/// Opens the TUI on a view (`rig`, `rig:NAME`, `pod:RIG/POD`). The TUI reads
/// it from `AINET_VIEW`; setting our own environment is `unsafe` in Rust
/// 2024, so the TUI runs as a child that inherits the terminal.
fn tui_on(view: &str) -> Result<ExitCode> {
    let status = std::process::Command::new(std::env::current_exe()?)
        .env("AINET_VIEW", view)
        .status()
        .context("start the TUI")?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(c) => ExitCode::from(c.clamp(1, 255) as u8),
        None => ExitCode::FAILURE,
    })
}

fn daemon() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("AINET_LOG").unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(ainet_daemon::run(ainet_daemon::Options::default()))
}

fn critter(ids: &[String], octant: bool) {
    let ids = if ids.is_empty() {
        vec![uuid_ish()]
    } else {
        ids.to_vec()
    };
    for id in ids {
        let seed = ainet_critter::hash(&id);
        let px = ainet_critter::pixels(
            seed,
            &ainet_critter::Colors::default(),
            ainet_critter::Pose::default(),
        );
        if octant {
            print!("{}", ainet_critter::ansi(&ainet_critter::octants(&px)));
        } else {
            print!("{}", ainet_critter::ansi(&ainet_critter::half_blocks(&px)));
        }
        println!("{}  ({id})\n", ainet_critter::name(&id));
    }
}

fn uuid_ish() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}")
}

fn terminal_status(conn: &Conn, out: &Out) -> Result<ExitCode> {
    match conn.call(&Request::Hello {
        version: ainet_core::protocol::VERSION,
    }) {
        Ok(resp @ Response::Hello { .. }) => {
            out.show(&resp);
            if let Ok(Response::Snapshot { agents, rigs, .. }) = conn.call(&Request::Snapshot)
                && !out.json
            {
                println!("{} rigs, {} agents", rigs.len(), agents.len());
            }
            Ok(ExitCode::SUCCESS)
        }
        Ok(other) => Err(unexpected(other)),
        Err(_) => {
            if out.json {
                out.value(&serde_json::json!({ "running": false }));
            } else {
                println!("ainetd is not running ({})", conn.socket.display());
            }
            Ok(ExitCode::from(1))
        }
    }
}

fn ps(conn: &Conn, out: &Out, nodes: bool, rig: Option<String>) -> Result<()> {
    let rigs = match rig {
        Some(rig) => match conn.ok(&Request::Status { rig })? {
            Response::Rig { rig } => vec![rig],
            other => return Err(unexpected(other)),
        },
        None => match conn.ok(&Request::Ps)? {
            Response::Rigs { rigs } => rigs,
            other => return Err(unexpected(other)),
        },
    };
    let agents = match snapshot(conn)? {
        Response::Snapshot { agents, .. } => agents,
        _ => vec![],
    };
    if nodes {
        if out.json {
            let seats: Vec<_> = rigs.iter().flat_map(|r| r.seats.iter()).collect();
            out.value(&serde_json::json!({ "seats": seats }));
        } else {
            print!("{}", output::seats_table(&rigs, &agents, true));
        }
        return Ok(());
    }
    let loose: Vec<_> = agents
        .into_iter()
        .filter(|a| a.rig.is_none() && a.state.is_live())
        .collect();
    if out.json {
        out.value(&serde_json::json!({ "rigs": rigs, "agents": loose }));
        return Ok(());
    }
    print!("{}", output::rigs_table(&rigs));
    if !loose.is_empty() {
        println!("\nAgents outside rigs:");
        print!("{}", output::agents_table(&loose));
    }
    Ok(())
}

fn status(conn: &Conn, out: &Out, rig: String) -> Result<()> {
    let resp = conn.ok(&Request::Status { rig })?;
    match (&resp, out.json) {
        (Response::Rig { rig }, false) => {
            let agents = match conn.call(&Request::Snapshot) {
                Ok(Response::Snapshot { agents, .. }) => agents,
                _ => vec![],
            };
            print!("{}", output::rig_detail(rig, &agents));
        }
        _ => out.show(&resp),
    }
    Ok(())
}

/// Restores every rig that is stopped or partly running.
fn start(conn: &Conn, out: &Out) -> Result<()> {
    let Response::Rigs { rigs } = conn.ok(&Request::Ps)? else {
        bail!("unexpected reply to ps")
    };
    let mut results = Vec::new();
    for rig in rigs
        .iter()
        .filter(|r| matches!(r.status, RigStatus::Stopped | RigStatus::Partial))
    {
        let req = Request::Restore {
            rig: rig.name.clone(),
            snapshot: None,
            fresh: vec![],
        };
        let result = conn.call_for(&req, RIG_TIMEOUT).and_then(check);
        if !out.json {
            match &result {
                Ok(_) => println!("restored {}", rig.name),
                Err(e) => eprintln!("could not restore {}: {e:#}", rig.name),
            }
        }
        results.push(serde_json::json!({ "rig": rig.name, "ok": result.is_ok(), "error": result.err().map(|e| format!("{e:#}")) }));
    }
    if out.json {
        out.value(&serde_json::json!({ "restored": results }));
    } else if results.is_empty() {
        println!("ainetd is running; no rigs to restore");
    }
    Ok(())
}

fn msg(conn: &Conn, out: &Out, cmd: MsgCmd) -> Result<ExitCode> {
    let split = |s: &str| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    match cmd {
        MsgCmd::Send {
            to,
            text: words,
            force,
        } => out.show(&conn.ok(&Request::Message {
            from: me(),
            to: split(&to),
            rig: None,
            pod: None,
            body: text(&words),
            reply_to: None,
            force,
        })?),
        MsgCmd::Inbox {
            all,
            limit,
            keep_unread,
            agent,
        } => out.show(&conn.ok(&Request::Inbox {
            agent: agent.or_else(me).unwrap_or_else(|| "operator".into()),
            all,
            limit,
            mark_read: !keep_unread,
        })?),
        MsgCmd::Reply { id, text: words } => out.show(&conn.ok(&Request::Message {
            from: me(),
            to: vec![],
            rig: None,
            pod: None,
            body: text(&words),
            reply_to: Some(id),
            force: false,
        })?),
        MsgCmd::Thread { id } => out.show(&conn.ok(&Request::Thread { id })?),
        MsgCmd::Ask {
            to,
            text: words,
            timeout,
        } => {
            let sent = conn.ok(&Request::Message {
                from: me(),
                to: split(&to),
                rig: None,
                pod: None,
                body: text(&words),
                reply_to: None,
                force: false,
            })?;
            let id = match sent {
                Response::Messages { messages } if !messages.is_empty() => messages[0].id,
                Response::Messages { .. } => bail!("no agent matched {to:?}"),
                other => return Err(unexpected(other)),
            };
            let wait = Request::WaitReply {
                id,
                timeout_secs: timeout,
            };
            let resp = conn.call_for(&wait, Duration::from_secs(timeout + 10));
            let reply = match resp {
                Ok(Response::Messages { messages }) => messages
                    .iter()
                    .find(|m| m.reply_to == Some(id))
                    .or(messages.last())
                    .cloned(),
                Ok(Response::Error { message }) if !message.to_ascii_lowercase().contains("time") => {
                    bail!(message)
                }
                Ok(Response::Error { .. }) => None,
                Ok(other) => return Err(unexpected(other)),
                Err(e) if format!("{e}").contains("did not answer") => None,
                Err(e) => return Err(e),
            };
            match reply {
                Some(m) if out.json => out.value(&serde_json::to_value(&m)?),
                Some(m) => println!("{}", m.body),
                None => {
                    return Err(Exit(
                        2,
                        format!(
                            "no reply within {timeout}s; your message is #{id} (see `msg thread {id}` later)"
                        ),
                    )
                    .into());
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn chat(conn: &Conn, out: &Out, cmd: ChatCmd) -> Result<ExitCode> {
    match cmd {
        ChatCmd::Send {
            text: words,
            rig,
            topic,
            sender,
        } => {
            let rig = resolve_rig(conn, rig)?;
            let req = Request::ChatSend {
                rig,
                topic,
                sender: sender.or_else(me),
                body: text(&words),
            };
            out.done(&conn.ok(&req)?, "posted");
        }
        ChatCmd::History {
            rig,
            topic,
            after,
            limit,
        } => {
            let rig = resolve_rig(conn, rig)?;
            out.show(&conn.ok(&Request::ChatHistory {
                rig,
                topic,
                after,
                limit,
            })?);
        }
        ChatCmd::Wait {
            rig,
            topic,
            after,
            timeout,
        } => {
            let rig = resolve_rig(conn, rig)?;
            let req = Request::ChatWait {
                rig,
                topic,
                after,
                timeout_secs: timeout,
            };
            match conn.ok_for(&req, Duration::from_secs(timeout + 10))? {
                Response::Chat { messages } if messages.is_empty() => {
                    return Err(Exit(2, format!("no chat message within {timeout}s")).into());
                }
                resp => out.show(&resp),
            }
        }
        ChatCmd::Watch { rig, topic } => {
            let rig = resolve_rig(conn, rig)?;
            let mut after = None;
            if let Response::Chat { messages } = conn.ok(&Request::ChatHistory {
                rig: rig.clone(),
                topic: topic.clone(),
                after: None,
                limit: 20,
            })? {
                print_chat(out, &messages);
                after = messages.last().map(|m| m.id);
            }
            loop {
                let req = Request::ChatWait {
                    rig: rig.clone(),
                    topic: topic.clone(),
                    after,
                    timeout_secs: 60,
                };
                match conn.ok_for(&req, Duration::from_secs(70)) {
                    Ok(Response::Chat { messages }) => {
                        print_chat(out, &messages);
                        if let Some(last) = messages.last() {
                            after = Some(last.id);
                        }
                    }
                    Ok(other) => return Err(unexpected(other)),
                    Err(e) if format!("{e}").contains("did not answer") => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_chat(out: &Out, messages: &[ainet_core::ChatMessage]) {
    for m in messages {
        if out.json {
            println!("{}", serde_json::to_string(m).unwrap_or_default());
        } else {
            print!("{}", output::chat_line(m));
        }
    }
}

fn queue(conn: &Conn, out: &Out, cmd: QueueCmd) -> Result<()> {
    let resp = match cmd {
        QueueCmd::Create {
            to,
            summary,
            body,
            body_file,
            priority,
            tags,
            reply_to,
            no_nudge,
            rig,
            source,
        } => {
            let rig = match rig {
                Some(r) => Some(r),
                None if me().is_some() => resolve_rig(conn, None).ok(),
                None => None,
            };
            conn.ok(&Request::QueueCreate(QSpec {
                rig,
                source: source.or_else(me),
                destination: to,
                summary,
                body: read_body(body, body_file)?,
                priority,
                tags,
                reply_to,
                nudge: !no_nudge,
            }))?
        }
        QueueCmd::List {
            rig,
            mine,
            who,
            state,
            all,
        } => {
            let who = if mine {
                Some(me().unwrap_or_else(|| "operator".into()))
            } else {
                who
            };
            conn.ok(&Request::QueueList { rig, who, state, all })?
        }
        QueueCmd::Show { id } => conn.ok(&Request::QueueShow { id })?,
        QueueCmd::Claim { id } => conn.ok(&Request::QueueClaim { id, by: me() })?,
        QueueCmd::Unclaim { id, reason } => conn.ok(&Request::QueueUnclaim { id, by: me(), reason })?,
        QueueCmd::Update { id, note, state } => {
            if note.is_none() && state.is_none() {
                bail!("give --note, --state, or both");
            }
            conn.ok(&Request::QueueUpdate {
                id,
                by: me(),
                note,
                state,
            })?
        }
        QueueCmd::Done { id, note } => conn.ok(&Request::QueueUpdate {
            id,
            by: me(),
            note,
            state: Some(QState::Done),
        })?,
        QueueCmd::Handoff {
            id,
            to,
            body,
            body_file,
            summary,
        } => conn.ok(&Request::QueueHandoff {
            id,
            by: me(),
            to,
            body: read_body(body, body_file)?,
            summary,
        })?,
        QueueCmd::Block {
            id,
            on,
            continuation,
            wake_after,
        } => conn.ok(&Request::QueueBlock {
            id,
            by: me(),
            on,
            continuation,
            wake_after_secs: wake_after,
        })?,
    };
    out.done(&resp, "ok");
    Ok(())
}

fn watchdog(conn: &Conn, out: &Out, cmd: WatchdogCmd) -> Result<()> {
    match cmd {
        WatchdogCmd::Register {
            target,
            interval,
            message,
            registered_by,
            policy,
        } => {
            if let Some(p) = policy.filter(|p| p != "periodic-reminder") {
                eprintln!("note: policy {p:?} is not supported; using periodic-reminder");
            }
            let req = Request::WatchdogRegister {
                target,
                interval_secs: interval,
                message,
                registered_by: registered_by.or_else(me),
            };
            out.done(&conn.ok(&req)?, "watchdog registered");
        }
        WatchdogCmd::List => out.show(&conn.ok(&Request::WatchdogList)?),
        WatchdogCmd::Stop { id, reason } => out.done(
            &conn.ok(&Request::WatchdogStop {
                id: id.clone(),
                reason,
            })?,
            &format!("watchdog {id} stopped"),
        ),
    }
    Ok(())
}
