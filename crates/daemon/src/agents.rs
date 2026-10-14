//! Agents and their terminals: starting panes, typing into them, reading
//! them back, and the inbox of things that need you.

use std::path::Path;

use ainet_core::protocol::{Capture, SpawnSpec};
use ainet_core::{Agent, AgentState, Event, InboxItem, Response, Runtime, now_ms};
use ainet_pty::{Launch, Pane};
use anyhow::{Result, bail};

use crate::{ACTIVITY_BUCKET_MS, ACTIVITY_BUCKETS, Daemon, Shared, err, lock};

/// Default size of a new pane; the TUI resizes it to its tile.
pub(crate) const ROWS: u16 = 40;
pub(crate) const COLS: u16 = 120;

pub(crate) fn git_branch(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A fresh agent record.
pub(crate) fn new_agent(id: &str, runtime: Runtime, cwd: &Path) -> Agent {
    let now = now_ms();
    Agent {
        id: id.to_string(),
        name: ainet_critter::name(id),
        runtime,
        role: None,
        task: None,
        model: None,
        branch: git_branch(cwd),
        cwd: cwd.to_path_buf(),
        state: AgentState::Unknown,
        state_since: now,
        started_at: now,
        context: None,
        parent: None,
        pid: None,
        last_tool: None,
        activity: vec![0; ACTIVITY_BUCKETS],
        seat: None,
        rig: None,
        transcript: None,
        unread: 0,
        writable: false,
        terminal: false,
    }
}

/// What a parent Claude Code session put in our environment. A child that
/// inherits it stops registering itself and writing its transcript, so
/// every agent starts without it (Den's `spawn` does the same).
fn inherited_session_vars() -> Vec<String> {
    std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| ainet_claude::launch::is_inherited_var(k))
        .collect()
}

/// What to run in a pane, fully worked out.
pub(crate) struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Daemon {
    /// Starts `cmd` on a new pane for `agent` and registers both.
    pub(crate) fn start_pane(&mut self, mut agent: Agent, cmd: &Command) -> Result<Agent> {
        if !agent.cwd.is_dir() {
            bail!("no such directory: {}", agent.cwd.display());
        }
        let pane = Pane::spawn(
            agent.id.clone(),
            Launch {
                program: &cmd.program,
                args: &cmd.args,
                cwd: &agent.cwd,
                env: &cmd.env,
                remove: &inherited_session_vars(),
                rows: ROWS,
                cols: COLS,
            },
            self.pane_tx.clone(),
        )?;
        agent.pid = pane.pid();
        agent.writable = true;
        agent.terminal = true;
        agent.state = AgentState::Unknown;
        agent.state_since = now_ms();
        self.store.log(
            now_ms(),
            Some(&agent.id),
            "spawn",
            &serde_json::json!({ "program": cmd.program, "seat": agent.seat }),
        );
        self.panes.insert(agent.id.clone(), pane);
        self.agents.insert(agent.id.clone(), agent.clone());
        self.emit(Event::Agent { agent: agent.clone() });
        Ok(agent)
    }

    /// The environment every agent gets: who it is and where ainetd lives.
    pub(crate) fn agent_env(&self, id: &str, seat: Option<&ainet_core::Seat>) -> Vec<(String, String)> {
        let mut env = vec![
            ("AINET_AGENT_ID".to_string(), id.to_string()),
            ("AINET_SOCKET".to_string(), self.socket.display().to_string()),
        ];
        if let Some(s) = seat {
            env.push(("AINET_SEAT".into(), s.address.clone()));
            env.push(("AINET_RIG".into(), s.rig.clone()));
            // OpenRig's own names, for specs and skills written against them.
            env.push(("OPENRIG_SESSION_NAME".into(), s.address.clone()));
            env.push(("OPENRIG_RIG".into(), s.rig.clone()));
        }
        env
    }

    pub(crate) fn pane_exited(&mut self, id: &str) {
        self.panes.remove(id);
        if let Some(a) = self.agents.get_mut(id) {
            a.terminal = false;
            a.writable = false;
        }
        self.interactive.remove(id);
        self.release_held(id);
        self.set_state(id, AgentState::Gone);
        self.store
            .log(now_ms(), Some(id), "exit", &serde_json::Value::Null);
        self.publish(id);
        self.seat_exited(id);
    }

    pub(crate) fn touch(&mut self, id: &str) {
        let now = now_ms();
        let q = self.activity.entry(id.to_string()).or_default();
        q.push_back(now);
        let horizon = now - ACTIVITY_BUCKET_MS * ACTIVITY_BUCKETS as i64;
        while q.front().is_some_and(|t| *t < horizon) {
            q.pop_front();
        }
        let mut buckets = vec![0u64; ACTIVITY_BUCKETS];
        for t in q.iter() {
            let age = ((now - t) / ACTIVITY_BUCKET_MS) as usize;
            if age < ACTIVITY_BUCKETS {
                buckets[ACTIVITY_BUCKETS - 1 - age] += 1;
            }
        }
        if let Some(a) = self.agents.get_mut(id) {
            a.activity = buckets;
        }
    }

    pub(crate) fn set_state(&mut self, id: &str, state: AgentState) {
        if let Some(a) = self.agents.get_mut(id)
            && a.state != state
        {
            a.state = state;
            a.state_since = now_ms();
        }
    }

    pub(crate) fn publish(&self, id: &str) {
        if let Some(a) = self.agents.get(id) {
            self.emit(Event::Agent { agent: a.clone() });
        }
    }

    pub(crate) fn add_item(&mut self, mut item: InboxItem) -> u64 {
        self.next_item += 1;
        item.id = self.next_item;
        self.emit(Event::Inbox { item: item.clone() });
        self.inbox.push(item);
        // Keep the inbox bounded: finished turns age out, held items never do.
        let mut excess = self.inbox.len().saturating_sub(200);
        self.inbox.retain(|i| {
            if excess > 0 && !i.waiting {
                excess -= 1;
                false
            } else {
                true
            }
        });
        self.next_item
    }

    /// Releases every held request of an agent: it was answered in the
    /// agent's own terminal, or the agent moved on.
    pub(crate) fn release_held(&mut self, agent: &str) {
        let ids: Vec<u64> = self
            .inbox
            .iter()
            .filter(|i| i.waiting && i.agent == agent)
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.resolve(id);
        }
    }

    /// Marks an inbox item answered. A held hook still waiting for it is
    /// dropped, so it prints nothing and the terminal dialog decides.
    pub(crate) fn resolve(&mut self, id: u64) {
        if let Some(item) = self.inbox.iter_mut().find(|i| i.id == id) {
            item.waiting = false;
        }
        self.forget_held(id);
        self.emit(Event::InboxResolved { id });
    }

    /// Writes raw bytes to an agent's terminal, wherever it lives.
    pub(crate) fn write_raw(&mut self, id: &str, data: &[u8]) -> Result<()> {
        if let Some(pane) = self.panes.get_mut(id) {
            return pane.write(data);
        }
        if let Some(target) = self.tmux.get(id) {
            return crate::discover::tmux_keys(target, &String::from_utf8_lossy(data));
        }
        bail!(
            "ainet cannot type into {}: it does not own its terminal",
            self.label(id)
        )
    }
}

/// `ainet spawn`: one agent outside any rig.
pub(crate) fn spawn_loose(d: &Shared, spec: SpawnSpec) -> Result<Agent> {
    let mut d = lock(d);
    let id = uuid::Uuid::new_v4().to_string();
    let mut agent = new_agent(&id, spec.runtime, &spec.cwd);
    agent.role = spec.role;
    agent.task = spec.task;
    agent.parent = spec.parent;
    let cmd = crate::launch::loose_command(&d, &agent, &spec.args);
    d.start_pane(agent, &cmd)
}

pub(crate) fn input(d: &Shared, key: &str, data: &str) -> Response {
    let mut d = lock(d);
    let Some(id) = d.find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    match d.write_raw(&id, data.as_bytes()) {
        Ok(()) => Response::Ok,
        Err(e) => err(e),
    }
}

pub(crate) fn resize(d: &Shared, key: &str, rows: u16, cols: u16) -> Response {
    let mut d = lock(d);
    let Some(id) = d.find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    let (rows, cols) = (rows.clamp(2, 500), cols.clamp(10, 1000));
    match d.panes.get_mut(&id) {
        Some(p) if p.size() == (rows, cols) => Response::Ok,
        Some(p) => match p.resize(rows, cols) {
            Ok(()) => Response::Ok,
            Err(e) => err(e),
        },
        // Adopted tmux panes keep the size their own window gives them.
        None => Response::Ok,
    }
}

pub(crate) fn screen(d: &Shared, key: &str, formatted: bool, history: usize) -> Response {
    let d = lock(d);
    let Some(id) = d.find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    if let Some(p) = d.panes.get(&id) {
        let rows = if history > 0 { p.history(history) } else { p.rows() };
        let (ansi, size) = if formatted {
            let (a, s) = p.formatted();
            (Some(a), s)
        } else {
            (None, p.size())
        };
        return Response::Screen { rows, ansi, size };
    }
    if let Some(target) = d.tmux.get(&id) {
        return match crate::discover::tmux_capture(target, history, formatted) {
            Ok((rows, ansi, size)) => Response::Screen { rows, ansi, size },
            Err(e) => err(e),
        };
    }
    err(format!("{} has no terminal ainet can read", d.label(&id)))
}

pub(crate) fn close(d: &Shared, key: &str) -> Response {
    let mut d = lock(d);
    let Some(id) = d.find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    if let Some(p) = d.panes.get_mut(&id) {
        return match p.kill() {
            Ok(()) => Response::Ok,
            Err(e) => err(e),
        };
    }
    err(format!("{} is not running on a pane ainet owns", d.label(&id)))
}

/// Terminal text of agents picked by name, seat, pod or rig.
pub(crate) fn capture(
    d: &Shared,
    targets: &[String],
    rig: Option<&str>,
    pod: Option<&str>,
    lines: usize,
) -> Response {
    let ids: Vec<String> = {
        let d = lock(d);
        let mut ids: Vec<String> = Vec::new();
        for t in targets {
            match d.find_agent(t, rig) {
                Some(id) => ids.push(id),
                None => return err(format!("no agent or seat {t}")),
            }
        }
        if let Some(r) = rig.filter(|_| targets.is_empty()) {
            let Some(state) = d.rigs.get(r) else {
                return err(format!("no rig {r}"));
            };
            ids.extend(
                state
                    .rig
                    .seats
                    .iter()
                    .filter(|s| pod.is_none_or(|p| s.pod == p))
                    .filter_map(|s| s.agent.clone()),
            );
        }
        ids
    };
    let lines = if lines == 0 { 50 } else { lines };
    let mut captures = Vec::new();
    for id in ids {
        let (label, rows) = {
            let d = lock(d);
            let label = d.label(&id);
            let rows = match d.panes.get(&id) {
                Some(p) => p.history(lines),
                None => match d.tmux.get(&id) {
                    Some(t) => crate::discover::tmux_capture(t, lines, false)
                        .map(|r| r.0)
                        .unwrap_or_default(),
                    None => d
                        .agents
                        .get(&id)
                        .and_then(|a| a.transcript.clone())
                        .map(|p| transcript_lines(&p, lines, None))
                        .unwrap_or_default(),
                },
            };
            (label, rows)
        };
        let skip = rows.len().saturating_sub(lines);
        captures.push(Capture {
            agent: id,
            label,
            lines: rows[skip..].to_vec(),
        });
    }
    Response::Captures { captures }
}

pub(crate) fn transcript(d: &Shared, key: &str, tail: usize, grep: Option<&str>) -> Response {
    let path = {
        let d = lock(d);
        let Some(id) = d.find_agent(key, None) else {
            return err(format!("no agent {key}"));
        };
        match d.agents.get(&id).and_then(|a| a.transcript.clone()) {
            Some(p) => p,
            None => {
                return err(format!(
                    "no transcript known for {} (Claude Code reports it through hooks)",
                    d.label(&id)
                ));
            }
        }
    };
    let tail = if tail == 0 { 40 } else { tail };
    Response::Text {
        text: transcript_lines(&path, tail, grep).join("\n"),
    }
}

/// The conversation in a Claude Code transcript, as "role: text" lines.
fn transcript_lines(path: &Path, tail: usize, grep: Option<&str>) -> Vec<String> {
    let turns = ainet_claude::transcript::conversation(path, tail, grep);
    turns
        .into_iter()
        .map(|(role, text)| format!("{}: {text}", format!("{role:?}").to_lowercase()))
        .collect()
}
