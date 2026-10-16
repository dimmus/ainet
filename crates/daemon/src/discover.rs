//! Agents ainet did not start.
//!
//! Every Claude Code session on the machine announces itself in
//! `~/.claude/sessions/<pid>.json` (with its busy/idle status and an inbox
//! socket), so they all show up in the Fleet and can be messaged, as in Den.
//! Agents running in tmux can be adopted into a rig seat (OpenRig's
//! `discover` / `adopt`): ainet then types into them with tmux and reads
//! their screen with `capture-pane`. Both are `discovery` in settings.json:
//! on by default, and each can be turned off.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use ainet_claude::sessions::{self, LiveSession, SessionState};
use ainet_core::protocol::SeatSpec;
use ainet_core::{AgentState, Discovered, Response, Runtime, SeatStatus, Transport};
use anyhow::{Context, Result, bail};

use crate::agents::new_agent;
use crate::launch::SeatPlan;
use crate::{Daemon, Shared, err, lock};

// ---- tmux -------------------------------------------------------------------

fn tmux(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("run tmux")?;
    if !out.status.success() {
        bail!(
            "tmux {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Types literal keys (and control characters) into a tmux pane.
pub(crate) fn tmux_keys(target: &str, data: &str) -> Result<()> {
    // Enter as a key name; everything else literally.
    for (i, part) in data.split('\r').enumerate() {
        if i > 0 {
            tmux(&["send-keys", "-t", target, "Enter"])?;
        }
        if !part.is_empty() {
            tmux(&["send-keys", "-t", target, "-l", part])?;
        }
    }
    Ok(())
}

/// Pastes text into a tmux pane as a bracketed paste, through a buffer of
/// its own so it never touches your clipboard buffers.
pub(crate) fn tmux_paste(target: &str, text: &str) -> Result<()> {
    let buffer = format!("ainet-{}", std::process::id());
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("run tmux")?;
    use std::io::Write;
    child
        .stdin
        .take()
        .context("tmux stdin")?
        .write_all(text.as_bytes())?;
    child.wait()?;
    // -p bracketed, -r keep newlines as typed, -d drop the buffer after.
    tmux(&["paste-buffer", "-p", "-r", "-d", "-b", &buffer, "-t", target])?;
    Ok(())
}

/// A screen as `Response::Screen` carries it: rows, escape sequences, size.
pub(crate) type Captured = (Vec<String>, Option<String>, (u16, u16));

/// A tmux pane's text: rows, and with `formatted` the escape sequences.
pub(crate) fn tmux_capture(target: &str, history: usize, formatted: bool) -> Result<Captured> {
    let start = format!("-{history}");
    let mut args = vec!["capture-pane", "-p", "-J", "-t", target];
    if history > 0 {
        args.extend(["-S", &start]);
    }
    let text = tmux(&args)?;
    let mut rows: Vec<String> = text.lines().map(str::to_string).collect();
    while rows.last().is_some_and(|r| r.trim().is_empty()) {
        rows.pop();
    }
    let size = tmux(&[
        "display-message",
        "-p",
        "-t",
        target,
        "#{pane_height} #{pane_width}",
    ])
    .ok()
    .and_then(|s| {
        let mut it = s.split_whitespace().filter_map(|x| x.parse::<u16>().ok());
        Some((it.next()?, it.next()?))
    })
    .unwrap_or((24, 80));
    let ansi = if formatted {
        // Visible screen with colours, cursor home first so it paints in place.
        let colored = tmux(&["capture-pane", "-p", "-e", "-t", target])?;
        Some(format!("\x1b[H\x1b[2J{}", colored.replace('\n', "\r\n")))
    } else {
        None
    };
    Ok((rows, ansi, size))
}

struct TmuxPane {
    id: String,
    session: String,
    pid: u32,
    cwd: PathBuf,
    title: String,
}

fn tmux_panes() -> Vec<TmuxPane> {
    let Ok(out) = tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id}\t#{session_name}\t#{pane_pid}\t#{pane_current_path}\t#{pane_title}",
    ]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some(TmuxPane {
                id: f.next()?.to_string(),
                session: f.next()?.to_string(),
                pid: f.next()?.parse().ok()?,
                cwd: PathBuf::from(f.next().unwrap_or("")),
                title: f.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// The agent CLI running under a process, if any: Claude Code or Codex.
fn agent_under(pid: u32) -> Option<(Runtime, u32)> {
    let mut stack = vec![pid];
    let mut seen = 0;
    while let Some(p) = stack.pop() {
        seen += 1;
        if seen > 200 {
            break;
        }
        let comm = std::fs::read_to_string(format!("/proc/{p}/comm")).unwrap_or_default();
        let cmdline = std::fs::read(format!("/proc/{p}/cmdline")).unwrap_or_default();
        let argv: Vec<String> = cmdline
            .split(|b| *b == 0)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        let exe = std::fs::read_link(format!("/proc/{p}/exe"))
            .map(|e| e.display().to_string())
            .unwrap_or_default();
        if sessions::is_claude_proc(comm.trim(), &exe, &argv) {
            return Some((Runtime::ClaudeCode, p));
        }
        if comm.trim() == "codex"
            || argv
                .first()
                .is_some_and(|a| a.ends_with("/codex") || a == "codex")
        {
            return Some((Runtime::Codex, p));
        }
        let children = std::fs::read_to_string(format!("/proc/{p}/task/{p}/children")).unwrap_or_default();
        stack.extend(children.split_whitespace().filter_map(|c| c.parse::<u32>().ok()));
    }
    None
}

// ---- Claude Code sessions -------------------------------------------------

/// Sends text into another Claude Code session through its inbox socket.
pub(crate) fn peer_send(id: &str, text: &str) -> Result<()> {
    let Some(session) = sessions::session_by_id(id) else {
        bail!("Claude Code session {id} is not running, or has no inbox socket (Claude Code 2.1.224+)");
    };
    ainet_claude::peer::send_with_header(&session, "Message from ainet:", text)
}

impl Daemon {
    /// Folds the live sessions list into the fleet.
    fn sync_sessions(&mut self, live: &[LiveSession]) {
        for s in live.iter().filter(|s| s.alive && !s.session_id.is_empty()) {
            let id = s.session_id.clone();
            if !self.agents.contains_key(&id) {
                let mut a = new_agent(&id, Runtime::ClaudeCode, std::path::Path::new(&s.cwd));
                a.started_at = s.started_at;
                a.state = AgentState::Idle;
                self.agents.insert(id.clone(), a);
            }
            let owned = self.panes.contains_key(&id) || self.tmux.contains_key(&id);
            let Some(a) = self.agents.get_mut(&id) else {
                continue;
            };
            let mut changed = false;
            let pid = Some(s.pid as u32);
            if a.pid != pid && !owned {
                a.pid = pid;
                changed = true;
            }
            let writable = owned || !s.messaging_socket_path.is_empty();
            if a.writable != writable {
                a.writable = writable;
                changed = true;
            }
            // The file's busy/idle is the floor; hooks say more (needs you, failed).
            let file_state = match s.state() {
                SessionState::Working => Some(AgentState::Working),
                SessionState::Idle => Some(AgentState::Idle),
                _ => None,
            };
            if let Some(st) = file_state
                && matches!(
                    a.state,
                    AgentState::Unknown | AgentState::Working | AgentState::Idle | AgentState::Gone
                )
                && a.state != st
            {
                a.state = st;
                a.state_since = s.status_updated_at.max(s.started_at);
                changed = true;
            }
            if changed {
                self.publish(&id);
            }
        }
        // Sessions we only knew from the file, now gone.
        let gone: Vec<String> = self
            .agents
            .values()
            .filter(|a| a.runtime == Runtime::ClaudeCode && a.state.is_live() && a.seat.is_none())
            .filter(|a| !self.panes.contains_key(&a.id) && !self.tmux.contains_key(&a.id))
            .filter(|a| a.pid.is_some() && !live.iter().any(|s| s.alive && s.session_id == a.id))
            .map(|a| a.id.clone())
            .collect();
        for id in gone {
            self.set_state(&id, AgentState::Gone);
            self.publish(&id);
        }
    }

    /// Context use, model, last tool and cost from each Claude transcript.
    fn refresh_transcripts(&mut self) {
        let ids: Vec<(String, PathBuf)> = self
            .agents
            .values()
            .filter(|a| a.state.is_live() && a.runtime == Runtime::ClaudeCode)
            .filter_map(|a| a.transcript.clone().map(|t| (a.id.clone(), t)))
            .collect();
        let cost = self.settings().cost.clone();
        for (id, path) in ids {
            let summary = crate::hooks::summarize(&path, &cost);
            let Some(s) = summary else { continue };
            if let Some(a) = self.agents.get_mut(&id) {
                let before = (a.context, a.model.clone(), a.last_tool.clone());
                if s.context.is_some() {
                    a.context = s.context;
                }
                if s.model.is_some() {
                    a.model = s.model;
                }
                if s.last_tool.is_some() {
                    a.last_tool = s.last_tool;
                }
                if before != (a.context, a.model.clone(), a.last_tool.clone()) {
                    self.publish(&id);
                }
            }
        }
    }
}

/// Keeps the fleet in step with the machine: Claude Code's sessions every
/// `discovery.interval_ms` (unless `discovery.claude_sessions` is off),
/// transcripts every `discovery.transcripts_ms`.
pub(crate) async fn ticker(d: Shared) {
    let sessions_dir = lock(&d).claude_sessions.clone();
    let mut last_transcripts = std::time::Instant::now();
    loop {
        let (every, sessions_on, transcripts) = {
            let g = lock(&d);
            let s = &g.settings().discovery;
            (
                Duration::from_millis(s.interval_ms),
                s.claude_sessions,
                Duration::from_millis(s.transcripts_ms),
            )
        };
        tokio::time::sleep(every).await;
        if sessions_on {
            let dir = sessions_dir.clone();
            let live = tokio::task::spawn_blocking(move || sessions::live_sessions_in(&dir))
                .await
                .unwrap_or_default();
            lock(&d).sync_sessions(&live);
        }
        if last_transcripts.elapsed() >= transcripts {
            last_transcripts = std::time::Instant::now();
            lock(&d).refresh_transcripts();
        }
    }
}

pub(crate) fn discover(d: &Shared) -> Response {
    let g = lock(d);
    // Asked for: Claude Code's sessions are listed even when they are not
    // tracked on their own; tmux only when `discovery.tmux` is on.
    let tmux_on = g.settings().discovery.tmux;
    let mut found: Vec<Discovered> = Vec::new();
    let adopted = |pane: &str| -> Option<String> {
        g.tmux
            .iter()
            .find(|(_, p)| p.as_str() == pane)
            .and_then(|(a, _)| g.agents.get(a))
            .and_then(|a| a.seat.clone())
    };
    let panes = if tmux_on { tmux_panes() } else { Vec::new() };
    for p in panes {
        if let Some((runtime, pid)) = agent_under(p.pid) {
            found.push(Discovered {
                id: p.id.clone(),
                runtime,
                cwd: p.cwd.clone(),
                tmux_session: Some(p.session.clone()),
                pid: Some(pid),
                title: Some(p.title.clone()).filter(|t| !t.is_empty()),
                seat: adopted(&p.id),
            });
        }
    }
    for s in sessions::live_sessions_in(&g.claude_sessions)
        .into_iter()
        .filter(|s| s.alive)
    {
        let in_tmux = found.iter().any(|f| f.pid == Some(s.pid as u32));
        let owned = g.panes.contains_key(&s.session_id);
        if in_tmux || owned {
            continue;
        }
        found.push(Discovered {
            id: s.session_id.clone(),
            runtime: Runtime::ClaudeCode,
            cwd: PathBuf::from(&s.cwd),
            tmux_session: None,
            pid: Some(s.pid as u32),
            title: Some(s.name.clone()).filter(|n| !n.is_empty()),
            seat: g.agents.get(&s.session_id).and_then(|a| a.seat.clone()),
        });
    }
    Response::Discovered { sessions: found }
}

/// Takes a discovered session into a rig seat (creating the rig if needed).
pub(crate) fn adopt(d: &Shared, id: &str, rig: &str, spec: SeatSpec) -> Response {
    let mut g = lock(d);
    let (agent_id, runtime, cwd, transport) = if id.starts_with('%') {
        let Some(p) = tmux_panes().into_iter().find(|p| p.id == id) else {
            return err(format!("no tmux pane {id}"));
        };
        let Some((runtime, pid)) = agent_under(p.pid) else {
            return err(format!("no agent runs in tmux pane {id}"));
        };
        // A Claude session in that pane reports its session id; use it so
        // hooks and names line up.
        let session = sessions::live_sessions()
            .into_iter()
            .find(|s| s.pid as u32 == pid)
            .map(|s| s.session_id);
        let agent_id = session.unwrap_or_else(|| {
            format!(
                "tmux{}-{}",
                id.trim_start_matches('%'),
                uuid::Uuid::new_v4().simple()
            )
        });
        (agent_id, runtime, p.cwd, Transport::Tmux { pane: id.to_string() })
    } else {
        let Some(s) = sessions::session_by_id(id) else {
            return err(format!("no running session {id}"));
        };
        (
            s.session_id.clone(),
            Runtime::ClaudeCode,
            PathBuf::from(&s.cwd),
            Transport::None,
        )
    };
    if g.agents.get(&agent_id).is_some_and(|a| a.seat.is_some()) {
        return err(format!("{agent_id} is already in a seat"));
    }
    let mut spec = spec;
    spec.runtime = Some(runtime);
    spec.cwd.get_or_insert(cwd.clone());
    let root = g
        .rigs
        .get(rig)
        .map(|s| s.plan.root.clone())
        .unwrap_or_else(|| cwd.clone());
    let resolved = match ainet_spec::seat_from_spec(rig, &root, &spec) {
        Ok(s) => s,
        Err(e) => return err(format!("{e:#}")),
    };
    let plan = SeatPlan::from(&resolved);
    if !g.rigs.contains_key(rig) {
        let rp = crate::launch::RigPlan {
            name: rig.to_string(),
            summary: None,
            root,
            spec_path: None,
            edges: vec![],
            seats: vec![],
            warnings: vec![],
        };
        let rig_rec = ainet_core::Rig {
            name: rig.to_string(),
            summary: None,
            spec_path: None,
            root: rp.root.clone(),
            status: ainet_core::RigStatus::Running,
            seats: vec![],
            edges: vec![],
            created: ainet_core::now_ms(),
        };
        g.rigs.insert(
            rig.to_string(),
            crate::rigs::RigState {
                rig: rig_rec,
                plan: rp,
            },
        );
    }
    let address = plan.address.clone();
    if g.rigs[rig].plan.seat(&address).is_some() {
        return err(format!("{address} exists"));
    }
    if let Transport::Tmux { pane } = &transport {
        g.tmux.insert(agent_id.clone(), pane.clone());
    }
    let mut agent = g
        .agents
        .get(&agent_id)
        .cloned()
        .unwrap_or_else(|| new_agent(&agent_id, runtime, &cwd));
    agent.seat = Some(address.clone());
    agent.rig = Some(rig.to_string());
    agent.role = plan.role.clone().or(plan.label.clone());
    agent.writable = transport != Transport::None || runtime == Runtime::ClaudeCode;
    agent.terminal = matches!(transport, Transport::Tmux { .. });
    if agent.state == AgentState::Unknown || agent.state == AgentState::Gone {
        agent.state = AgentState::Idle;
    }
    g.agents.insert(agent_id.clone(), agent);
    let st = g.rigs.get_mut(rig).expect("inserted above");
    st.plan.seats.push(plan.clone());
    st.rig.seats.push(ainet_core::Seat {
        address: plan.address.clone(),
        logical: plan.logical.clone(),
        rig: rig.to_string(),
        pod: plan.pod.clone(),
        member: plan.member.clone(),
        label: plan.label.clone(),
        role: plan.role.clone(),
        runtime,
        model: plan.model.clone(),
        cwd,
        agent: Some(agent_id.clone()),
        status: SeatStatus::Running,
        transport,
        restore_policy: plan.restore_policy.clone(),
        typing_guard: false,
        held: 0,
    });
    g.relink(rig);
    g.save_rig(rig);
    Response::Rig {
        rig: g.rigs[rig].rig.clone(),
    }
}

/// Lets a seat go: the process keeps running, the seat leaves the rig.
pub(crate) fn release(d: &Shared, key: &str) -> Response {
    let mut g = lock(d);
    let Some(seat) = g.find_seat(key, None).cloned() else {
        return err(format!("no seat {key}"));
    };
    if let Some(a) = &seat.agent {
        g.tmux.remove(a);
        if let Some(agent) = g.agents.get_mut(a) {
            agent.seat = None;
            agent.rig = None;
            agent.parent = None;
        }
        g.publish(a);
    }
    if let Some(st) = g.rigs.get_mut(&seat.rig) {
        st.rig.seats.retain(|s| s.address != seat.address);
        st.plan.seats.retain(|s| s.address != seat.address);
        st.rig
            .edges
            .retain(|e| e.from != seat.logical && e.to != seat.logical);
        st.plan
            .edges
            .retain(|e| e.from != seat.logical && e.to != seat.logical);
    }
    g.save_rig(&seat.rig);
    Response::Ok
}
