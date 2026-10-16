//! OpenRig's topology view, without drawing it: the explorer tree (this
//! host, its rigs with their status, each rig's pods with their seat
//! counts, the agents in no rig), a rig's seats as table rows grouped by
//! pod with the summary line under them and the filter over them, the
//! rig's queue changes oldest first (Recent), its overview, and the health
//! checks that can honestly be made from what ainetd reports.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ainet_core::{Agent, AgentState, QItem, QState, RigStatus, Runtime, Seat, SeatStatus};

use crate::store::Store;

/// What the right side of the view is about: a rig, one pod of it, or the
/// agents in no rig.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Rig(String),
    Pod { rig: String, pod: String },
    Loose,
}

impl Scope {
    /// The rig it is in, if any.
    pub fn rig(&self) -> Option<&str> {
        match self {
            Scope::Rig(rig) | Scope::Pod { rig, .. } => Some(rig),
            Scope::Loose => None,
        }
    }

    /// How the view names it: `rig docs`, `pod docs/dev`, `loose agents`.
    pub fn title(&self) -> String {
        match self {
            Scope::Rig(rig) => format!("rig {rig}"),
            Scope::Pod { rig, pod } => format!("pod {rig}/{pod}"),
            Scope::Loose => "loose agents".into(),
        }
    }

    /// Whether `agent` belongs here: in the rig (or its pod), or in no rig
    /// the store knows.
    pub fn admits(&self, store: &Store, agent: &Agent) -> bool {
        match self {
            Scope::Rig(rig) => agent.rig.as_deref() == Some(rig),
            Scope::Pod { rig, pod } => {
                agent.rig.as_deref() == Some(rig) && store.seat_of(agent).is_some_and(|s| &s.pod == pod)
            }
            Scope::Loose => is_loose(store, agent),
        }
    }

    /// Whether it still exists in `store`.
    pub fn exists(&self, store: &Store) -> bool {
        match self {
            Scope::Rig(rig) => store.rigs.contains_key(rig),
            Scope::Pod { rig, pod } => store
                .rigs
                .get(rig)
                .is_some_and(|r| r.seats.iter().any(|s| &s.pod == pod)),
            Scope::Loose => store.agents.values().any(|a| is_loose(store, a)),
        }
    }
}

/// The scope a view opens on: the selected agent's rig, else the first
/// rig, else the loose agents (`None` with nothing at all).
pub fn default_scope(store: &Store) -> Option<Scope> {
    store
        .selected_agent()
        .and_then(|a| a.rig.clone())
        .filter(|r| store.rigs.contains_key(r))
        .or_else(|| store.rigs.keys().next().cloned())
        .map(Scope::Rig)
        .or_else(|| Scope::Loose.exists(store).then_some(Scope::Loose))
}

/// The next rig (or the loose agents) after `current`, round again: what
/// `ctrl-r` goes to.
pub fn next_scope(store: &Store, current: Option<&Scope>) -> Option<Scope> {
    let mut all: Vec<Scope> = store.rigs.keys().cloned().map(Scope::Rig).collect();
    if Scope::Loose.exists(store) {
        all.push(Scope::Loose);
    }
    let at = current.and_then(|c| {
        let rig = match c {
            Scope::Pod { rig, .. } => Scope::Rig(rig.clone()),
            other => other.clone(),
        };
        all.iter().position(|s| *s == rig)
    });
    match at {
        Some(at) => all.get((at + 1) % all.len()).cloned(),
        None => all.into_iter().next(),
    }
}

fn is_loose(store: &Store, agent: &Agent) -> bool {
    agent.rig.as_ref().is_none_or(|r| !store.rigs.contains_key(r))
}

// ---- the explorer tree -----------------------------------------------------

/// A node of the explorer tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// The tree's root.
    Topology,
    /// The machine ainetd runs on.
    Host(String),
    Rig {
        name: String,
        status: RigStatus,
        /// Seats whose agent is running.
        running: usize,
        seats: usize,
    },
    Pod {
        rig: String,
        pod: String,
        running: usize,
        seats: usize,
    },
    /// The agents in no rig.
    Loose { live: usize, agents: usize },
}

impl Node {
    /// What choosing it shows on the right, if anything.
    pub fn scope(&self) -> Option<Scope> {
        match self {
            Node::Rig { name, .. } => Some(Scope::Rig(name.clone())),
            Node::Pod { rig, pod, .. } => Some(Scope::Pod {
                rig: rig.clone(),
                pod: pod.clone(),
            }),
            Node::Loose { .. } => Some(Scope::Loose),
            Node::Topology | Node::Host(_) => None,
        }
    }
}

/// A row of the explorer tree: a node and how deep it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub node: Node,
}

/// The explorer tree: TOPOLOGY, this host (`host`), each rig in name order
/// with its pods in the order the rig lists their seats, then the agents
/// in no rig, if there are any.
pub fn tree(store: &Store, host: &str) -> Vec<TreeRow> {
    let row = |depth, node| TreeRow { depth, node };
    let mut rows = vec![row(0, Node::Topology), row(1, Node::Host(host.to_string()))];
    for (name, rig) in &store.rigs {
        let counts = store.rig_counts(name);
        rows.push(row(
            2,
            Node::Rig {
                name: name.clone(),
                status: rig.status,
                running: counts.running,
                seats: counts.seats,
            },
        ));
        for (pod, seats) in pods(&rig.seats) {
            rows.push(row(
                3,
                Node::Pod {
                    rig: name.clone(),
                    pod: pod.to_string(),
                    running: seats.iter().filter(|s| s.status == SeatStatus::Running).count(),
                    seats: seats.len(),
                },
            ));
        }
    }
    let loose: Vec<&Agent> = store.agents.values().filter(|a| is_loose(store, a)).collect();
    if !loose.is_empty() {
        rows.push(row(
            2,
            Node::Loose {
                live: loose.iter().filter(|a| a.state.is_live()).count(),
                agents: loose.len(),
            },
        ));
    }
    rows
}

/// A rig's seats by pod, the pods in the order their first seat is listed.
fn pods(seats: &[Seat]) -> Vec<(&str, Vec<&Seat>)> {
    let mut out: Vec<(&str, Vec<&Seat>)> = Vec::new();
    for s in seats {
        match out.iter_mut().find(|(pod, _)| *pod == s.pod) {
            Some((_, list)) => list.push(s),
            None => out.push((&s.pod, vec![s])),
        }
    }
    out
}

// ---- the table -------------------------------------------------------------

/// What a row's STATE says: its agent's state, or, with no agent running
/// in the seat, the seat's own status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Agent(AgentState),
    Seat(SeatStatus),
}

impl RowState {
    pub fn label(self) -> &'static str {
        match self {
            RowState::Agent(s) => s.label(),
            RowState::Seat(s) => s.label(),
        }
    }
}

/// One seat (or, among the loose agents, one agent) of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatRow {
    /// Its pod; empty for an agent in no rig.
    pub pod: String,
    /// Whether it is the first row of its pod, which carries the pod's
    /// name and a line above it.
    pub first_in_pod: bool,
    /// The SEAT column: the seat's member, or a loose agent's name.
    pub member: String,
    /// The seat's address, for a seat.
    pub address: Option<String>,
    /// The agent running in it, by id.
    pub agent: Option<String>,
    pub runtime: Runtime,
    pub model: Option<String>,
    /// Share of the context window in use, 0.0..=1.0.
    pub context: Option<f32>,
    pub state: RowState,
    /// Open queue items owned by it or addressed to it, latest first.
    pub items: Vec<String>,
    /// Its queue item in progress: id and summary.
    pub work: Option<(String, String)>,
    /// What it is doing now: its task, else its last tool.
    pub now: Option<String>,
    /// Requests of its agent waiting on you.
    pub waiting: usize,
}

/// How long a NOW cell is at most, in characters.
const NOW_MAX: usize = 120;

impl SeatRow {
    /// What tells rows apart: the seat's address, else the agent's id.
    pub fn key(&self) -> &str {
        self.address
            .as_deref()
            .or(self.agent.as_deref())
            .unwrap_or(&self.member)
    }

    /// The RT column: `cl`, `cx` or `sh`.
    pub fn rt(&self) -> &'static str {
        runtime_short(self.runtime)
    }

    /// The CTX column: `38%`, or `—` when unknown.
    pub fn ctx(&self) -> String {
        match self.context {
            Some(c) => format!("{:.0}%", c * 100.0),
            None => "—".into(),
        }
    }

    /// Whether it needs you: waiting on you, failed, or with a request of
    /// yours to answer.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self.state,
            RowState::Agent(AgentState::NeedsYou | AgentState::Failed) | RowState::Seat(SeatStatus::Failed)
        ) || self.waiting > 0
    }

    /// Whether every word of `query` is in its seat, pod, address, model,
    /// runtime or state, ignoring case. An empty query matches all.
    pub fn matches(&self, query: &str) -> bool {
        let hay = [
            self.member.as_str(),
            self.pod.as_str(),
            self.address.as_deref().unwrap_or(""),
            self.model.as_deref().unwrap_or(""),
            self.rt(),
            self.runtime.label(),
            self.state.label(),
        ]
        .join(" ")
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| hay.contains(&word.to_lowercase()))
    }
}

/// A runtime as the RT column writes it, as OpenRig's table does.
pub fn runtime_short(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::ClaudeCode => "cl",
        Runtime::Codex => "cx",
        Runtime::Shell => "sh",
    }
}

/// The rows of `scope`: a rig's seats grouped by pod (pods in the order
/// the rig lists them, seats in that order within their pod), or the
/// agents in no rig, the oldest first.
pub fn rows(store: &Store, scope: &Scope) -> Vec<SeatRow> {
    let mut rows: Vec<SeatRow> = match scope {
        Scope::Rig(rig) | Scope::Pod { rig, .. } => {
            let Some(r) = store.rigs.get(rig) else {
                return Vec::new();
            };
            let only = match scope {
                Scope::Pod { pod, .. } => Some(pod.as_str()),
                _ => None,
            };
            pods(&r.seats)
                .into_iter()
                .filter(|(pod, _)| only.is_none_or(|o| o == *pod))
                .flat_map(|(_, seats)| seats)
                .map(|seat| seat_row(store, seat))
                .collect()
        }
        Scope::Loose => {
            let mut loose: Vec<&Agent> = store.agents.values().filter(|a| is_loose(store, a)).collect();
            loose.sort_by_key(|a| a.started_at);
            loose.into_iter().map(|a| agent_row(store, a)).collect()
        }
    };
    mark_pods(&mut rows);
    rows
}

/// The rows `query` matches, each pod's first one shown carrying its
/// name.
pub fn filtered(rows: &[SeatRow], query: &str) -> Vec<SeatRow> {
    let mut out: Vec<SeatRow> = rows.iter().filter(|r| r.matches(query)).cloned().collect();
    mark_pods(&mut out);
    out
}

fn mark_pods(rows: &mut [SeatRow]) {
    let mut last: Option<String> = None;
    for row in rows {
        row.first_in_pod = last.as_ref() != Some(&row.pod);
        last = Some(row.pod.clone());
    }
}

fn seat_row(store: &Store, seat: &Seat) -> SeatRow {
    // The seat's agent, while it runs there.
    let agent = seat
        .agent
        .as_ref()
        .and_then(|id| store.agents.get(id))
        .filter(|a| a.state.is_live() && a.seat.as_deref() == Some(seat.address.as_str()));
    let items = open_items(store, |who| {
        who == seat.address || agent.is_some_and(|a| who == a.id)
    });
    SeatRow {
        pod: seat.pod.clone(),
        first_in_pod: false,
        member: seat.member.clone(),
        address: Some(seat.address.clone()),
        agent: agent.map(|a| a.id.clone()),
        runtime: agent.map_or(seat.runtime, |a| a.runtime),
        model: agent.and_then(|a| a.model.clone()).or_else(|| seat.model.clone()),
        context: agent.and_then(|a| a.context),
        state: match agent {
            Some(a) => RowState::Agent(a.state),
            None => RowState::Seat(seat.status),
        },
        work: work(&items),
        items: items.iter().map(|q| q.id.clone()).collect(),
        now: agent.and_then(now),
        waiting: agent.map_or(0, |a| store.waiting_for(&a.id)),
    }
}

fn agent_row(store: &Store, a: &Agent) -> SeatRow {
    let items = open_items(store, |who| who == a.id || who == a.name);
    SeatRow {
        pod: String::new(),
        first_in_pod: false,
        member: a.name.clone(),
        address: None,
        agent: Some(a.id.clone()),
        runtime: a.runtime,
        model: a.model.clone(),
        context: a.context,
        state: RowState::Agent(a.state),
        work: work(&items),
        items: items.iter().map(|q| q.id.clone()).collect(),
        now: now(a),
        waiting: store.waiting_for(&a.id),
    }
}

/// Open queue items whose owner or destination is `me`, latest first, as
/// the daemon's `mine` reads.
fn open_items(store: &Store, me: impl Fn(&str) -> bool) -> Vec<&QItem> {
    let mut v: Vec<&QItem> = store
        .queue
        .values()
        .filter(|q| q.state.is_open() && (me(&q.destination) || q.owner.as_deref().is_some_and(&me)))
        .collect();
    v.sort_by_key(|q| std::cmp::Reverse(q.updated));
    v
}

fn work(items: &[&QItem]) -> Option<(String, String)> {
    items
        .iter()
        .find(|q| q.state == QState::InProgress)
        .map(|q| (q.id.clone(), first_line(&q.summary, NOW_MAX)))
}

fn now(a: &Agent) -> Option<String> {
    a.task
        .as_deref()
        .or(a.last_tool.as_deref())
        .map(|t| first_line(t, NOW_MAX))
        .filter(|t| !t.is_empty())
}

/// The first line of `s` with text in it, cut to `max` characters.
fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    } else {
        line.to_string()
    }
}

/// The line under the table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub seats: usize,
    pub working: usize,
    /// Rows that need you ([`SeatRow::needs_attention`]).
    pub attention: usize,
    /// Open queue items of the rows' seats, each once.
    pub open: usize,
}

/// The summary of `rows`.
pub fn summary(rows: &[SeatRow]) -> Summary {
    let open: BTreeSet<&str> = rows
        .iter()
        .flat_map(|r| r.items.iter().map(String::as_str))
        .collect();
    Summary {
        seats: rows.len(),
        working: rows
            .iter()
            .filter(|r| r.state == RowState::Agent(AgentState::Working))
            .count(),
        attention: rows.iter().filter(|r| r.needs_attention()).count(),
        open: open.len(),
    }
}

impl std::fmt::Display for Summary {
    /// OpenRig's words: `7 seats · 4 working · 0 need attention · 6 open rows`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = |n: usize| if n == 1 { "" } else { "s" };
        write!(
            f,
            "{} seat{} · {} working · {} need attention · {} open row{}",
            self.seats,
            s(self.seats),
            self.working,
            self.attention,
            self.open,
            s(self.open)
        )
    }
}

// ---- Recent: the queue's changes -------------------------------------------

/// One recorded change of a queue item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueChange {
    /// Its number in the list, from 1, oldest first.
    pub n: usize,
    /// When (Unix ms).
    pub at: i64,
    /// The item's id.
    pub item: String,
    /// Who made it: a seat address, an agent, or `operator`.
    pub by: String,
    /// The item's state after it.
    pub state: QState,
    /// The note made with it (`claimed`, `handed off to …`), or `created`.
    pub note: String,
    /// The item's summary, its first line.
    pub summary: String,
}

/// The recorded changes of the queue items of `scope`, oldest first: each
/// item's creation, then each note on it (ainetd notes every change of
/// state). An item whose state moved on without a note gets one change
/// more, at its last update, so the list ends where the item is.
pub fn changes(store: &Store, scope: &Scope) -> Vec<QueueChange> {
    let in_scope = |q: &QItem| match scope {
        Scope::Rig(rig) => q.rig.as_deref() == Some(rig),
        Scope::Pod { rig, pod } => {
            let seats: Vec<&str> = store
                .rigs
                .get(rig)
                .map(|r| {
                    r.seats
                        .iter()
                        .filter(|s| &s.pod == pod)
                        .map(|s| s.address.as_str())
                        .collect()
                })
                .unwrap_or_default();
            q.rig.as_deref() == Some(rig)
                && [
                    Some(q.source.as_str()),
                    Some(q.destination.as_str()),
                    q.owner.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|who| seats.contains(&who))
        }
        Scope::Loose => q.rig.as_ref().is_none_or(|r| !store.rigs.contains_key(r)),
    };
    let mut out = Vec::new();
    for q in store.queue.values().filter(|q| in_scope(q)) {
        let summary = first_line(&q.summary, NOW_MAX);
        let change = |at, by: &str, state, note: &str| QueueChange {
            n: 0,
            at,
            item: q.id.clone(),
            by: by.to_string(),
            state,
            note: note.to_string(),
            summary: summary.clone(),
        };
        // ainetd does not note an item's creation; the demo does.
        let noted = q
            .notes
            .first()
            .is_some_and(|n| n.at == q.created && n.state == Some(QState::Pending));
        let mut state = QState::Pending;
        if !noted {
            out.push(change(q.created, &q.source, state, "created"));
        }
        for note in &q.notes {
            state = note.state.unwrap_or(state);
            out.push(change(note.at, &note.by, state, &note.text));
        }
        if state != q.state {
            let by = q.owner.as_deref().unwrap_or(&q.destination);
            out.push(change(q.updated, by, q.state, ""));
        }
    }
    // Oldest first; an item's own changes stay in their order.
    out.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then_with(|| item_number(&a.item).cmp(&item_number(&b.item)))
    });
    for (i, c) in out.iter_mut().enumerate() {
        c.n = i + 1;
    }
    out
}

/// `q-12` as 12, so `q-10` sorts after `q-9`.
fn item_number(id: &str) -> (u64, &str) {
    let n = id
        .rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX);
    (n, id)
}

// ---- Overview --------------------------------------------------------------

/// A rig in a few lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Overview {
    pub name: String,
    pub status: RigStatus,
    pub summary: Option<String>,
    /// The spec file it was launched from, if any.
    pub spec: Option<PathBuf>,
    /// Where relative paths in the spec resolve.
    pub root: PathBuf,
    /// When it was made (Unix ms).
    pub created: i64,
    pub pods: usize,
    pub seats: usize,
    /// Seats whose agent is running.
    pub running: usize,
    /// How many rows are in each state, in the order first seen.
    pub states: Vec<(&'static str, usize)>,
    pub edges: usize,
    /// Open queue items of the rig.
    pub open: usize,
}

/// `rig`'s overview, if there is such a rig.
pub fn overview(store: &Store, rig: &str) -> Option<Overview> {
    let r = store.rigs.get(rig)?;
    let counts = store.rig_counts(rig);
    let mut states: Vec<(&'static str, usize)> = Vec::new();
    for row in rows(store, &Scope::Rig(rig.to_string())) {
        let label = row.state.label();
        match states.iter_mut().find(|(l, _)| *l == label) {
            Some((_, n)) => *n += 1,
            None => states.push((label, 1)),
        }
    }
    Some(Overview {
        name: r.name.clone(),
        status: r.status,
        summary: r.summary.clone(),
        spec: r.spec_path.clone(),
        root: r.root.clone(),
        created: r.created,
        pods: pods(&r.seats).len(),
        seats: counts.seats,
        running: counts.running,
        states,
        edges: r.edges.len(),
        open: counts.open,
    })
}

// ---- Health ----------------------------------------------------------------

/// How much a finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Something the health checks found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// The seat (or loose agent) it is about.
    pub seat: String,
    pub agent: Option<String>,
    pub text: String,
}

/// A working agent with no hook event for this many minutes has gone
/// quiet.
pub const QUIET_MINUTES: usize = 5;

/// What [`health`] checks, and nothing more: the view says so, so a clean
/// result never claims more than was looked at.
pub const CHECKS: [&str; 5] = [
    "seats without a running agent",
    "failed agents",
    "requests waiting on you",
    "working agents with no hook event for 5 minutes",
    "typing guards on and messages held",
];

/// The health checks ([`CHECKS`]) over `scope`, the worst first.
pub fn health(store: &Store, scope: &Scope) -> Vec<Finding> {
    let mut out = Vec::new();
    let seats: Vec<&Seat> = match scope {
        Scope::Rig(rig) | Scope::Pod { rig, .. } => store
            .rigs
            .get(rig)
            .map(|r| {
                r.seats
                    .iter()
                    .filter(|s| !matches!(scope, Scope::Pod { pod, .. } if *pod != s.pod))
                    .collect()
            })
            .unwrap_or_default(),
        Scope::Loose => Vec::new(),
    };
    for row in rows(store, scope) {
        let seat = row.address.clone().unwrap_or_else(|| row.member.clone());
        let mut find = |severity, text: String| {
            out.push(Finding {
                severity,
                seat: seat.clone(),
                agent: row.agent.clone(),
                text,
            })
        };
        match row.state {
            RowState::Seat(SeatStatus::Failed) => find(Severity::Error, "the seat failed to start".into()),
            RowState::Seat(SeatStatus::Running) => find(
                Severity::Warning,
                "the seat says running, but no agent of it is known".into(),
            ),
            RowState::Seat(status @ (SeatStatus::Pending | SeatStatus::Launching)) => find(
                Severity::Info,
                format!("no agent yet: the seat is {}", status.label()),
            ),
            RowState::Seat(status) => find(
                Severity::Warning,
                format!("no agent running: the seat is {}", status.label()),
            ),
            RowState::Agent(AgentState::Failed) => {
                find(Severity::Error, "failed: a turn ended on an error".into())
            }
            _ => {}
        }
        if row.waiting > 0 {
            let s = if row.waiting == 1 { "" } else { "s" };
            find(
                Severity::Warning,
                format!("{} request{s} waiting on you", row.waiting),
            );
        } else if row.state == RowState::Agent(AgentState::NeedsYou) {
            find(Severity::Warning, "needs you".into());
        }
        let quiet = row
            .agent
            .as_ref()
            .and_then(|id| store.agents.get(id))
            .is_some_and(gone_quiet);
        if quiet {
            find(
                Severity::Warning,
                format!("working, but no hook event for {QUIET_MINUTES} minutes"),
            );
        }
    }
    for s in seats {
        if s.typing_guard || s.held > 0 {
            let held = match s.held {
                0 => None,
                1 => Some("1 message held".to_string()),
                n => Some(format!("{n} messages held")),
            };
            let text = [s.typing_guard.then(|| "typing guard on".to_string()), held]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Finding {
                severity: Severity::Info,
                seat: s.address.clone(),
                agent: s.agent.clone(),
                text,
            });
        }
    }
    out.sort_by_key(|f| f.severity);
    out
}

/// Working, by its state, with a full window of activity that is all
/// zero: no hook event for [`QUIET_MINUTES`].
fn gone_quiet(a: &Agent) -> bool {
    a.state == AgentState::Working
        && a.activity.len() >= QUIET_MINUTES
        && a.activity[a.activity.len() - QUIET_MINUTES..]
            .iter()
            .all(|n| *n == 0)
}

#[cfg(test)]
mod tests {
    use ainet_core::{Event, QNote};

    use super::*;
    use crate::demo::Demo;
    use crate::link::Msg;

    fn demo() -> Store {
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        store
    }

    fn docs() -> Scope {
        Scope::Rig("docs".into())
    }

    #[test]
    fn the_tree_is_host_rigs_pods_and_the_loose_agents() {
        let store = demo();
        let tree = tree(&store, "box");
        let shown: Vec<(usize, String)> = tree
            .iter()
            .map(|r| {
                let text = match &r.node {
                    Node::Topology => "TOPOLOGY".to_string(),
                    Node::Host(h) => h.clone(),
                    Node::Rig {
                        name, running, seats, ..
                    } => format!("{name} {running}/{seats}"),
                    Node::Pod { pod, seats, .. } => format!("{pod} {seats}"),
                    Node::Loose { live, agents } => format!("loose {live}/{agents}"),
                };
                (r.depth, text)
            })
            .collect();
        let want = [
            (0, "TOPOLOGY"),
            (1, "box"),
            (2, "docs 5/5"),
            (3, "orch 1"),
            (3, "dev 3"),
            (3, "rev 1"),
            (2, "infra 2/2"),
            (3, "ops 2"),
            (2, "loose 0/1"),
        ];
        assert_eq!(shown, want.map(|(d, t)| (d, t.to_string())));
        assert_eq!(tree[2].node.scope(), Some(docs()));
        assert_eq!(
            tree[4].node.scope(),
            Some(Scope::Pod {
                rig: "docs".into(),
                pod: "dev".into()
            })
        );
        assert_eq!(tree[8].node.scope(), Some(Scope::Loose));
        assert_eq!(tree[1].node.scope(), None);
    }

    #[test]
    fn a_rigs_rows_are_grouped_by_pod() {
        let store = demo();
        let rows = rows(&store, &docs());
        // Each row as its pod label (on a pod's first row), then its cells.
        let cells: Vec<String> = rows
            .iter()
            .map(|r| {
                let pod = if r.first_in_pod { r.pod.as_str() } else { "" };
                let work = r.work.as_ref().map_or("—", |w| w.0.as_str());
                let model = r.model.as_deref().unwrap_or("—");
                let (seat, rt, ctx, state, q) = (&r.member, r.rt(), r.ctx(), r.state.label(), r.items.len());
                format!("{pod}|{seat}|{rt}|{model}|{ctx}|{state}|{q}|{work}")
            })
            .collect();
        assert_eq!(
            cells,
            [
                "orch|lead|cl|Opus 5.5|38%|working|0|—",
                "dev|ui|cl|Sonnet 5.5|22%|working|1|—",
                "|app|cl|Sonnet 5.5|31%|needs you|1|q-1",
                "|writer|cl|Sonnet 5.5|17%|working|1|q-3",
                "rev|review|cl|Haiku 5.5|9%|idle|1|—",
            ]
        );
        assert_eq!(rows[0].now.as_deref(), Some("ship the 1.0 docs"));
        assert_eq!(rows[2].waiting, 1);
        let infra = super::rows(&store, &Scope::Rig("infra".into()));
        assert_eq!(infra[0].rt(), "cx");
        // One pod alone.
        let dev = super::rows(
            &store,
            &Scope::Pod {
                rig: "docs".into(),
                pod: "dev".into(),
            },
        );
        assert_eq!(
            dev.iter().map(|r| r.member.as_str()).collect::<Vec<_>>(),
            ["ui", "app", "writer"]
        );
        assert!(dev[0].first_in_pod && !dev[1].first_in_pod);
        // The loose agents: the gone researcher.
        let loose = super::rows(&store, &Scope::Loose);
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].state, RowState::Agent(AgentState::Gone));
        assert!(loose[0].first_in_pod);
    }

    #[test]
    fn an_empty_seat_is_a_stopped_row() {
        let mut store = demo();
        let mut rig = store.rigs["docs"].clone();
        rig.seats[4].agent = None;
        rig.seats[4].status = SeatStatus::Stopped;
        store.apply(Msg::Event(Box::new(Event::Rig { rig })));
        let rows = rows(&store, &docs());
        let review = &rows[4];
        assert_eq!(review.state, RowState::Seat(SeatStatus::Stopped));
        assert_eq!(review.state.label(), "stopped");
        assert_eq!((review.agent.as_deref(), review.ctx().as_str()), (None, "—"));
        assert_eq!(review.model.as_deref(), Some("Haiku 5.5"), "the seat's own");
        // It is still owed its queue item.
        assert_eq!(review.items, ["q-4"]);
    }

    #[test]
    fn the_filter_matches_seat_pod_model_and_state() {
        let store = demo();
        let rows = rows(&store, &docs());
        let members = |q: &str| {
            filtered(&rows, q)
                .iter()
                .map(|r| (r.first_in_pod, r.member.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(members("").len(), 5);
        assert_eq!(
            members("writer"),
            [(true, "writer".to_string())],
            "a seat, first of its pod shown"
        );
        assert_eq!(members("DEV").len(), 3, "a pod, any case");
        assert_eq!(members("haiku"), [(true, "review".to_string())], "a model");
        assert_eq!(members("needs"), [(true, "app".to_string())], "a state");
        assert_eq!(members("dev working").len(), 2, "every word");
        assert!(members("nobody").is_empty());
    }

    #[test]
    fn the_summary_counts_seats_working_attention_and_open_items() {
        let store = demo();
        let rows = rows(&store, &docs());
        let sum = summary(&rows);
        assert_eq!(
            sum,
            Summary {
                seats: 5,
                working: 3,
                attention: 1,
                open: 4
            }
        );
        assert_eq!(
            sum.to_string(),
            "5 seats · 3 working · 1 need attention · 4 open rows"
        );
        assert_eq!(
            Summary {
                seats: 1,
                open: 1,
                ..Summary::default()
            }
            .to_string(),
            "1 seat · 0 working · 0 need attention · 1 open row"
        );
    }

    #[test]
    fn recent_lists_queue_changes_oldest_first() {
        let mut store = demo();
        let changes_now = changes(&store, &docs());
        // The demo notes each creation, but not the moves since: those
        // end each item where it is.
        let items: Vec<(&str, &str, QState, &str)> = changes_now
            .iter()
            .map(|c| (c.item.as_str(), c.by.as_str(), c.state, c.note.as_str()))
            .collect();
        assert_eq!(
            items,
            [
                ("q-1", "orch-lead@docs", QState::Pending, "created"),
                ("q-1", "dev-app@docs", QState::InProgress, ""),
                ("q-2", "orch-lead@docs", QState::Pending, "created"),
                ("q-3", "orch-lead@docs", QState::Pending, "created"),
                ("q-3", "dev-writer@docs", QState::InProgress, ""),
                ("q-4", "dev-ui@docs", QState::Pending, "created"),
                ("q-4", "rev-review@docs", QState::Blocked, ""),
            ]
        );
        assert_eq!(
            changes_now.iter().map(|c| c.n).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7]
        );
        assert!(changes_now.windows(2).all(|w| w[0].at <= w[1].at));

        // A claim noted by ainetd, and a move with no note.
        let mut q2 = store.queue["q-2"].clone();
        q2.state = QState::InProgress;
        q2.owner = Some("dev-ui@docs".into());
        q2.notes.push(QNote {
            by: "dev-ui@docs".into(),
            text: "claimed".into(),
            at: q2.created + 60_000,
            state: Some(QState::InProgress),
        });
        q2.updated = q2.created + 60_000;
        let mut q4 = store.queue["q-4"].clone();
        q4.state = QState::Done;
        q4.updated = q4.created + 120_000;
        for item in [q2, q4] {
            store.apply(Msg::Event(Box::new(Event::Queue { item })));
        }
        let all = changes(&store, &docs());
        let last: Vec<(&str, &str, QState, &str)> = all
            .iter()
            .filter(|c| c.note != "created")
            .map(|c| (c.item.as_str(), c.by.as_str(), c.state, c.note.as_str()))
            .collect();
        assert_eq!(
            last,
            [
                ("q-1", "dev-app@docs", QState::InProgress, ""),
                ("q-3", "dev-writer@docs", QState::InProgress, ""),
                ("q-2", "dev-ui@docs", QState::InProgress, "claimed"),
                ("q-4", "rev-review@docs", QState::Done, ""),
            ]
        );
        assert_eq!(all.last().unwrap().n, 8);
        assert!(all.windows(2).all(|w| w[0].at <= w[1].at));

        // An item ainetd made has no creation note: one is added.
        let mut q5 = store.queue["q-1"].clone();
        q5.id = "q-5".into();
        q5.notes.clear();
        q5.state = QState::Pending;
        q5.created += 1;
        store.apply(Msg::Event(Box::new(Event::Queue { item: q5 })));
        let with = changes(&store, &docs());
        let made = with.iter().find(|c| c.item == "q-5").unwrap();
        assert_eq!(
            (made.note.as_str(), made.by.as_str()),
            ("created", "orch-lead@docs")
        );
        assert_eq!(with.iter().filter(|c| c.item == "q-5").count(), 1);

        // The infra rig has no queue; a pod only its own items.
        assert!(changes(&store, &Scope::Rig("infra".into())).is_empty());
        let rev = changes(
            &store,
            &Scope::Pod {
                rig: "docs".into(),
                pod: "rev".into(),
            },
        );
        assert!(rev.iter().all(|c| c.item == "q-4"));
    }

    #[test]
    fn the_overview_sums_the_rig_up() {
        let store = demo();
        let o = overview(&store, "docs").unwrap();
        assert_eq!((o.pods, o.seats, o.running, o.edges, o.open), (3, 5, 5, 4, 4));
        assert_eq!(o.states, [("working", 3), ("needs you", 1), ("idle", 1)]);
        assert_eq!(o.summary.as_deref(), Some("Ship the 1.0 documentation."));
        assert!(overview(&store, "nope").is_none());
    }

    #[test]
    fn health_reports_only_what_it_checked() {
        let mut store = demo();
        // The demo's docs rig: dev.app waits on you.
        let found = health(&store, &docs());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].seat, "dev-app@docs");
        assert_eq!(found[0].text, "1 request waiting on you");
        assert!(
            health(&store, &Scope::Rig("infra".into())).is_empty(),
            "no findings"
        );

        // A stopped seat, a failed agent, a quiet one and a guard.
        let mut rig = store.rigs["docs"].clone();
        rig.seats[4].agent = None;
        rig.seats[4].status = SeatStatus::Stopped;
        rig.seats[0].typing_guard = true;
        rig.seats[0].held = 2;
        store.apply(Msg::Event(Box::new(Event::Rig { rig })));
        let mut ui = store.find_agent("dev-ui@docs").unwrap().clone();
        ui.state = AgentState::Failed;
        let mut writer = store.find_agent("dev-writer@docs").unwrap().clone();
        writer.activity = vec![3, 0, 0, 0, 0, 0];
        for agent in [ui, writer] {
            store.apply(Msg::Event(Box::new(Event::Agent { agent })));
        }
        let found: Vec<(Severity, String, String)> = health(&store, &docs())
            .into_iter()
            .map(|f| (f.severity, f.seat, f.text))
            .collect();
        let want = [
            (Severity::Error, "dev-ui@docs", "failed: a turn ended on an error"),
            (Severity::Warning, "dev-app@docs", "1 request waiting on you"),
            (
                Severity::Warning,
                "dev-writer@docs",
                "working, but no hook event for 5 minutes",
            ),
            (
                Severity::Warning,
                "rev-review@docs",
                "no agent running: the seat is stopped",
            ),
            (
                Severity::Info,
                "orch-lead@docs",
                "typing guard on, 2 messages held",
            ),
        ];
        assert_eq!(found, want.map(|(s, a, t)| (s, a.to_string(), t.to_string())));
        assert_eq!(CHECKS.len(), 5);
    }

    #[test]
    fn scopes_open_on_the_selection_and_go_round() {
        let mut store = demo();
        store.selected = store.find_agent("ops-sre@infra").map(|a| a.id.clone());
        assert_eq!(default_scope(&store), Some(Scope::Rig("infra".into())));
        store.selected = None;
        assert_eq!(default_scope(&store), Some(docs()));
        assert_eq!(
            next_scope(&store, Some(&docs())),
            Some(Scope::Rig("infra".into()))
        );
        assert_eq!(
            next_scope(&store, Some(&Scope::Rig("infra".into()))),
            Some(Scope::Loose)
        );
        assert_eq!(next_scope(&store, Some(&Scope::Loose)), Some(docs()));
        let dev = Scope::Pod {
            rig: "docs".into(),
            pod: "dev".into(),
        };
        assert_eq!(next_scope(&store, Some(&dev)), Some(Scope::Rig("infra".into())));
        assert_eq!(next_scope(&store, None), Some(docs()));
        assert!(dev.exists(&store) && !Scope::Rig("nope".into()).exists(&store));
        let lead = store.find_agent("orch-lead@docs").unwrap();
        assert!(docs().admits(&store, lead) && !dev.admits(&store, lead));
        assert_eq!(Store::new().rigs.len(), 0);
        assert_eq!(default_scope(&Store::new()), None);
    }
}
