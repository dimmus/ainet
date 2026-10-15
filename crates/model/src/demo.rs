//! A pretend fleet for `ainet --demo`: a docs rig (a lead directing three
//! developers, with a reviewer watching), a small infra rig, and a loose
//! agent. States drift, agents message and answer each other, post in the
//! chatroom, pick up queue items, and the odd permission request arrives.
//! It speaks the same `Event`s as the daemon, and draws believable
//! terminals for the Terminals view.

use std::path::PathBuf;

use ainet_core::protocol::SpecEntry;
use ainet_core::{
    Agent, AgentState, ChatMessage, Decision, Edge, EdgeKind, Event, InboxItem, InboxKind, Message, QBlock,
    QItem, QNote, QState, Request, Response, Rig, RigStatus, Runtime, Seat, SeatStatus, SnapshotInfo,
    Transport, now_ms,
};

use crate::link::Listing;

const MIN: i64 = 60_000;

/// What the demo says to a request it does not carry out.
const CANNOT: &str = "the demo cannot do that; run ainet without --demo";

struct Seed {
    id: &'static str,
    rig: Option<&'static str>,
    pod: &'static str,
    member: &'static str,
    role: &'static str,
    task: &'static str,
    model: &'static str,
    runtime: Runtime,
    state: AgentState,
    age_min: i64,
    ctx: f32,
}

const LEAD: &str = "b8a49ab2-7c1e-4d0a-9f3e-1d2c3b4a5e60";
const UI: &str = "c13f0e77-2b9d-4c5a-8e61-0a9b8c7d6e51";
const APP: &str = "d2e4a6c8-3f5b-4e7d-9a1c-2b4d6f8a0c42";
const WRITER: &str = "e5f7a9b1-4c6d-4f8e-8b2d-3c5e7f9b1d33";
const REVIEW: &str = "f6a8b0c2-5d7e-4a9f-9c3e-4d6f8a0c2e24";
const DATA: &str = "0a1b2c3d-6e8f-4b0a-8d4f-5e7a9b1d3f15";
const SRE: &str = "1b2c3d4e-7f9a-4c1b-9e5a-6f8b0c2e4a06";
const GONE: &str = "2c3d4e5f-8a0b-4d2c-8f6b-7a9c1d3f5b97";

const SEEDS: [Seed; 8] = [
    Seed {
        id: LEAD,
        rig: Some("docs"),
        pod: "orch",
        member: "lead",
        role: "TECH LEAD",
        task: "ship the 1.0 docs",
        model: "Opus 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::Working,
        age_min: 11,
        ctx: 0.38,
    },
    Seed {
        id: UI,
        rig: Some("docs"),
        pod: "dev",
        member: "ui",
        role: "FRONTEND DEVELOPER",
        task: "polish the docs site navigation",
        model: "Sonnet 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::Working,
        age_min: 7,
        ctx: 0.22,
    },
    Seed {
        id: APP,
        rig: Some("docs"),
        pod: "dev",
        member: "app",
        role: "BACKEND DEVELOPER",
        task: "make the API tests pass",
        model: "Sonnet 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::NeedsYou,
        age_min: 7,
        ctx: 0.31,
    },
    Seed {
        id: WRITER,
        rig: Some("docs"),
        pod: "dev",
        member: "writer",
        role: "TECHNICAL WRITER",
        task: "write the getting-started guide",
        model: "Sonnet 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::Working,
        age_min: 6,
        ctx: 0.17,
    },
    Seed {
        id: REVIEW,
        rig: Some("docs"),
        pod: "rev",
        member: "review",
        role: "REVIEWER",
        task: "review the navigation change",
        model: "Haiku 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::Idle,
        age_min: 6,
        ctx: 0.09,
    },
    Seed {
        id: DATA,
        rig: Some("infra"),
        pod: "ops",
        member: "data",
        role: "DATA ENGINEER",
        task: "backfill last week's events",
        model: "gpt-6",
        runtime: Runtime::Codex,
        state: AgentState::Working,
        age_min: 5,
        ctx: 0.12,
    },
    Seed {
        id: SRE,
        rig: Some("infra"),
        pod: "ops",
        member: "sre",
        role: "SRE",
        task: "plan the staging cluster upgrade",
        model: "Sonnet 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::Idle,
        age_min: 32,
        ctx: 0.44,
    },
    Seed {
        id: GONE,
        rig: None,
        pod: "",
        member: "",
        role: "RESEARCHER",
        task: "compare vt100 parsers",
        model: "Opus 5.5",
        runtime: Runtime::ClaudeCode,
        state: AgentState::Gone,
        age_min: 54,
        ctx: 0.71,
    },
];

const TOOLS: [&str; 8] = [
    "Bash  npm run build",
    "Edit  src/nav/Sidebar.tsx",
    "Read  docs/guide/install.md",
    "Bash  cargo test -p api",
    "Grep  TODO\\(docs\\)",
    "WebFetch  ratatui.rs",
    "Write  docs/getting-started.md",
    "Bash  git diff --stat",
];

const ASKS: [(&str, &str); 4] = [
    ("Bash  go test ./...", "go test ./... -run TestAPI -count=1"),
    (
        "Bash  rm -rf node_modules/.cache",
        "rm -rf node_modules/.cache && npm ci",
    ),
    (
        "Edit  .github/workflows/docs.yml",
        "--- a/.github/workflows/docs.yml\n+++ b/.github/workflows/docs.yml\n@@ -12,3 +12,4 @@\n   - run: npm run build\n+  - run: npm run linkcheck",
    ),
    ("WebFetch  docs.rs", "https://docs.rs/ratatui/latest/ratatui/"),
];

/// Scripted conversations: (from, to, question, answer).
const TALK: [(&str, &str, &str, &str); 5] = [
    (
        UI,
        APP,
        "Does /api/guides return versions sorted, or should the sidebar sort them?",
        "Sorted, newest first, since this morning. The sidebar can render them as they come.",
    ),
    (
        WRITER,
        UI,
        "What's the final label for the version picker: \"Version\" or \"Docs for\"?",
        "\"Version\". It's in src/nav/VersionPicker.tsx if you want to quote it.",
    ),
    (
        LEAD,
        REVIEW,
        "Can you look at the nav change before it merges? Mostly the keyboard focus order.",
        "Yes, I'll review it when dev.ui says it's up. Focus order is on my list.",
    ),
    (
        APP,
        LEAD,
        "The flaky test is the websocket reconnect: OK to raise its timeout to 5s?",
        "OK for now; file a queue item so we find the real cause after 1.0.",
    ),
    (
        SRE,
        DATA,
        "Will the backfill hold the events table lock for long?",
        "No, it batches 10k rows per transaction; locks are short.",
    ),
];

pub struct Demo {
    agents: Vec<Agent>,
    rigs: Vec<Rig>,
    inbox: Vec<InboxItem>,
    messages: Vec<Message>,
    chat: Vec<ChatMessage>,
    queue: Vec<QItem>,
    next_item: u64,
    next_msg: u64,
    talk: usize,
    rng: u64,
    /// Whether it also answers what only the desktop app asks of it
    /// (transcripts, specs, plans, queue requests, listings as popups,
    /// chat topics); the console's demo keeps its replies as they were.
    full: bool,
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

fn address(s: &Seed) -> Option<String> {
    s.rig.map(|r| format!("{}-{}@{r}", s.pod, s.member))
}

impl Demo {
    /// The demo as the desktop app uses it: it also carries out queue
    /// requests, keeps chat topics, and shows transcripts, specs, plans
    /// and listings, which the console's demo says it cannot do.
    pub fn full() -> Self {
        Demo {
            full: true,
            ..Demo::new()
        }
    }

    /// Whether only the full demo answers `req` (the console's says it
    /// cannot do that).
    fn full_only(req: &Request) -> bool {
        match req {
            Request::Up(spec) => spec.plan,
            _ => matches!(
                req,
                Request::QueueClaim { .. }
                    | Request::QueueUnclaim { .. }
                    | Request::QueueUpdate { .. }
                    | Request::QueueHandoff { .. }
                    | Request::QueueBlock { .. }
                    | Request::QueueShow { .. }
                    | Request::Transcript { .. }
                    | Request::SpecShow { .. }
                    | Request::Export { .. }
                    | Request::Specs
                    | Request::SnapshotList { .. }
            ),
        }
    }

    pub fn new() -> Self {
        let now = now_ms();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/you".into());
        let agents: Vec<Agent> = SEEDS
            .iter()
            .enumerate()
            .map(|(i, s)| Agent {
                id: s.id.into(),
                name: ainet_critter::name(s.id),
                runtime: s.runtime,
                role: Some(s.role.into()),
                task: Some(s.task.into()),
                model: Some(s.model.into()),
                cwd: PathBuf::from(format!(
                    "{home}/projects/{}",
                    if s.rig == Some("infra") {
                        "infra"
                    } else {
                        "web-app"
                    }
                )),
                branch: Some("main".into()),
                state: s.state,
                state_since: now - s.age_min * MIN,
                started_at: now - (s.age_min + 20) * MIN,
                context: Some(s.ctx),
                parent: (s.rig == Some("docs") && s.pod == "dev").then(|| LEAD.to_string()),
                pid: Some(4_040_127 + i as u32 * 7),
                last_tool: Some(TOOLS[i % TOOLS.len()].into()),
                activity: (0..20).map(|k| ((k * 7 + i * 3) % 9) as u64).collect(),
                seat: address(s),
                rig: s.rig.map(Into::into),
                transcript: None,
                unread: 0,
                writable: s.state != AgentState::Gone,
                terminal: s.state != AgentState::Gone,
            })
            .collect();
        let rigs = ["docs", "infra"]
            .iter()
            .map(|name| {
                let seats = SEEDS
                    .iter()
                    .filter(|s| s.rig == Some(name))
                    .map(|s| Seat {
                        address: address(s).unwrap_or_default(),
                        logical: format!("{}.{}", s.pod, s.member),
                        rig: name.to_string(),
                        pod: s.pod.into(),
                        member: s.member.into(),
                        label: None,
                        role: Some(s.role.into()),
                        runtime: s.runtime,
                        model: Some(s.model.into()),
                        cwd: PathBuf::from(&home),
                        agent: Some(s.id.into()),
                        status: SeatStatus::Running,
                        transport: Transport::Pane,
                        restore_policy: "resume_if_possible".into(),
                        typing_guard: false,
                        held: 0,
                    })
                    .collect();
                let edges = if *name == "docs" {
                    let e = |kind, from: &str, to: &str| Edge {
                        kind,
                        from: from.into(),
                        to: to.into(),
                    };
                    vec![
                        e(EdgeKind::DelegatesTo, "orch.lead", "dev.ui"),
                        e(EdgeKind::DelegatesTo, "orch.lead", "dev.app"),
                        e(EdgeKind::DelegatesTo, "orch.lead", "dev.writer"),
                        e(EdgeKind::CanObserve, "rev.review", "dev.ui"),
                    ]
                } else {
                    vec![]
                };
                Rig {
                    name: name.to_string(),
                    summary: Some(
                        if *name == "docs" {
                            "Ship the 1.0 documentation."
                        } else {
                            "Keep the data platform healthy."
                        }
                        .into(),
                    ),
                    spec_path: None,
                    root: PathBuf::from(&home),
                    status: RigStatus::Running,
                    seats,
                    edges,
                    created: now - 60 * MIN,
                }
            })
            .collect();
        let mut demo = Demo {
            agents,
            rigs,
            inbox: Vec::new(),
            messages: Vec::new(),
            chat: Vec::new(),
            queue: Vec::new(),
            next_item: 0,
            next_msg: 0,
            talk: 1,
            rng: 0x2545_f491_4f6c_dd1d,
            full: false,
        };
        demo.push_item(
            4,
            InboxKind::Finished,
            "The sidebar now groups the guides by version.",
            "The sidebar now groups the guides by version and the build passes.",
            false,
        );
        demo.push_item(
            3,
            InboxKind::Question,
            "message #4: Should the guide cover WSL2?",
            "Should the getting-started guide cover Windows through WSL2, or link out?",
            false,
        );
        demo.push_item(2, InboxKind::Permission, ASKS[0].0, ASKS[0].1, true);
        let n = demo.inbox.len() as i64;
        for (i, item) in demo.inbox.iter_mut().enumerate() {
            item.created -= (n - i as i64) * 3 * MIN;
        }
        // A conversation already under way.
        demo.message(
            LEAD,
            UI,
            "Polish the docs nav: group the guides by version. Ask dev.app if you need the API shape.",
            None,
            -9,
        );
        let q = demo.message(UI, APP, TALK[0].2, None, -6);
        demo.message(APP, UI, TALK[0].3, Some(q), -5);
        demo.message(
            WRITER,
            "operator",
            "Should the getting-started guide cover Windows through WSL2, or link out?",
            None,
            -4,
        );
        demo.chat_post("docs", LEAD, "Standup: nav polish (dev.ui), API tests (dev.app), guide draft (dev.writer). @rev.review take the nav PR when it lands.", -8);
        demo.chat_post("docs", REVIEW, "On it once it's up.", -7);
        demo.chat_post(
            "infra",
            SRE,
            "Staging upgrade plan is in docs/ops/upgrade.md; reviews welcome.",
            -20,
        );
        demo.qitem(
            "q-1",
            "orch-lead@docs",
            "dev-app@docs",
            "feature-001: make the API tests pass",
            QState::InProgress,
            -10,
        );
        demo.qitem(
            "q-2",
            "orch-lead@docs",
            "dev-ui@docs",
            "screen-001: theme editor screen",
            QState::Pending,
            -9,
        );
        demo.qitem(
            "q-3",
            "orch-lead@docs",
            "dev-writer@docs",
            "guide-001: getting-started guide",
            QState::InProgress,
            -9,
        );
        demo.qitem(
            "q-4",
            "dev-ui@docs",
            "rev-review@docs",
            "review the nav change (PR #42)",
            QState::Blocked,
            -3,
        );
        demo
    }

    fn label(&self, id: &str) -> String {
        if id == "operator" {
            return "you (operator)".into();
        }
        self.agents
            .iter()
            .find(|a| a.id == id)
            .map(Agent::label)
            .unwrap_or_else(|| id.into())
    }

    fn message(&mut self, from: &str, to: &str, body: &str, reply_to: Option<u64>, minutes: i64) -> u64 {
        self.next_msg += 1;
        let id = self.next_msg;
        let thread = reply_to
            .and_then(|r| self.messages.iter().find(|m| m.id == r).map(|m| m.thread))
            .unwrap_or(id);
        let created = now_ms() + minutes * MIN;
        self.messages.push(Message {
            id,
            from: from.into(),
            from_label: self.label(from),
            to: to.into(),
            to_label: self.label(to),
            body: body.into(),
            reply_to,
            thread,
            created,
            delivered: Some(created),
            read: (minutes < -1).then_some(created + 30_000),
            held: false,
        });
        id
    }

    fn chat_post(&mut self, rig: &str, sender: &str, body: &str, minutes: i64) -> ChatMessage {
        let m = ChatMessage {
            id: self.chat.len() as u64 + 1,
            rig: rig.into(),
            topic: "general".into(),
            sender: self.label(sender),
            body: body.into(),
            created: now_ms() + minutes * MIN,
        };
        self.chat.push(m.clone());
        m
    }

    fn qitem(&mut self, id: &str, source: &str, dest: &str, summary: &str, state: QState, minutes: i64) {
        let at = now_ms() + minutes * MIN;
        self.queue.push(QItem {
            id: id.into(),
            rig: Some(dest.rsplit('@').next().unwrap_or("").into()),
            source: source.into(),
            destination: dest.into(),
            owner: (state != QState::Pending).then(|| dest.to_string()),
            summary: summary.into(),
            body: String::new(),
            priority: "normal".into(),
            tags: vec![],
            state,
            reply_to: None,
            block: None,
            successor: None,
            notes: vec![QNote {
                by: source.into(),
                text: "created".into(),
                at,
                state: Some(QState::Pending),
            }],
            created: at,
            updated: at,
        });
    }

    /// The fleet as it stands, for the first frame.
    pub fn snapshot(&self) -> Response {
        Response::Snapshot {
            agents: self.agents.clone(),
            inbox: self.inbox.clone(),
            rigs: self.rigs.clone(),
            messages: self.messages.clone(),
            chat: self.chat.clone(),
            queue: self.queue.clone(),
        }
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn push_item(
        &mut self,
        agent: usize,
        kind: InboxKind,
        summary: &str,
        detail: &str,
        waiting: bool,
    ) -> InboxItem {
        self.next_item += 1;
        let item = InboxItem {
            id: self.next_item,
            agent: self.agents[agent].id.clone(),
            kind,
            summary: summary.into(),
            detail: detail.into(),
            created: now_ms(),
            waiting,
            options: vec![],
            multi_select: false,
        };
        self.inbox.push(item.clone());
        item
    }

    fn set(&mut self, i: usize, state: AgentState) {
        if self.agents[i].state != state {
            self.agents[i].state = state;
            self.agents[i].state_since = now_ms();
        }
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|a| a.id == id)
    }

    /// Advances the simulation one tick (~120 ms).
    pub fn step(&mut self, tick: u64) -> Vec<Event> {
        let mut events = Vec::new();
        // Busy agents fill their sparklines and their context.
        if tick.is_multiple_of(8) {
            for a in self.agents.iter_mut() {
                a.activity.rotate_left(1);
                let busy = a.state == AgentState::Working;
                if let Some(last) = a.activity.last_mut() {
                    *last = if busy {
                        2 + (tick / 8 + a.started_at as u64) % 8
                    } else {
                        0
                    };
                }
                if busy {
                    a.context = a.context.map(|c| (c + 0.004).min(0.97));
                }
            }
            events.extend(self.agents.iter().map(|a| Event::Agent { agent: a.clone() }));
        }
        // A conversation step every ~6 seconds.
        if tick.is_multiple_of(50) && tick > 0 {
            let (from, to, q, a) = TALK[self.talk % TALK.len()];
            let qid = self.message(from, to, q, None, 0);
            events.push(Event::Message {
                message: self.messages.last().cloned().expect("just pushed"),
            });
            self.message(to, from, a, Some(qid), 0);
            events.push(Event::Message {
                message: self.messages.last().cloned().expect("just pushed"),
            });
            if let Some(i) = self.index(to) {
                self.set(i, AgentState::Working);
            }
            self.talk += 1;
        }
        if !tick.is_multiple_of(25) || tick == 0 {
            return events;
        }
        let i = (self.rand() % (self.agents.len() as u64 - 1)) as usize; // the last one stays gone
        let roll = self.rand() % 10;
        let waiting = self
            .inbox
            .iter()
            .any(|it| it.waiting && it.agent == self.agents[i].id);
        match (self.agents[i].state, roll) {
            (AgentState::NeedsYou, _) => {}
            (AgentState::Working, 0..=1) if !waiting => {
                let (summary, detail) = ASKS[(self.rand() % ASKS.len() as u64) as usize];
                self.set(i, AgentState::NeedsYou);
                let item = self.push_item(i, InboxKind::Permission, summary, detail, true);
                events.push(Event::Inbox { item });
            }
            (AgentState::Working, 2..=4) => {
                self.set(i, AgentState::Idle);
                let item = self.push_item(
                    i,
                    InboxKind::Finished,
                    "Done: the change is in and the tests pass.",
                    "Done: the change is in and the tests pass.",
                    false,
                );
                events.push(Event::Inbox { item });
                let seat = self.agents[i].seat.clone();
                if let Some(q) = self
                    .queue
                    .iter_mut()
                    .find(|q| Some(&q.destination) == seat.as_ref() && q.state == QState::InProgress)
                {
                    q.state = QState::Done;
                    q.updated = now_ms();
                    events.push(Event::Queue { item: q.clone() });
                }
            }
            (AgentState::Working, 5) => {
                if let Some(rig) = self.agents[i].rig.clone() {
                    let who = self.agents[i].id.clone();
                    let m = self.chat_post(&rig, &who, "Pushed my part; tests are green on my side.", 0);
                    events.push(Event::Chat { message: m });
                }
            }
            (AgentState::Working, _) => {
                self.agents[i].last_tool = Some(TOOLS[(self.rand() % TOOLS.len() as u64) as usize].into());
            }
            (AgentState::Idle, 0..=5) => self.set(i, AgentState::Working),
            _ => {}
        }
        events.push(Event::Agent {
            agent: self.agents[i].clone(),
        });
        events
    }

    /// What the demo does with a request: events to apply, and a result
    /// (`Some` text for a popup).
    pub fn handle(&mut self, req: &Request) -> (Vec<Event>, Result<Option<String>, String>) {
        if !self.full && Self::full_only(req) {
            return (vec![], Err(CANNOT.into()));
        }
        match req {
            Request::Answer { item, decision } => (self.answer(*item, *decision), Ok(None)),
            Request::AnswerQuestion { item, .. } => (self.answer(*item, Decision::Allow), Ok(None)),
            Request::Message {
                to,
                body,
                reply_to,
                rig,
                ..
            } => {
                let mut events = Vec::new();
                let mut targets: Vec<String> = to.clone();
                if let Some(r) =
                    reply_to.and_then(|r| self.messages.iter().find(|m| m.id == r).map(|m| m.from.clone()))
                {
                    targets.push(r);
                }
                if let Some(rig) = rig {
                    targets.extend(
                        self.agents
                            .iter()
                            .filter(|a| a.rig.as_deref() == Some(rig))
                            .map(|a| a.id.clone()),
                    );
                }
                for t in targets {
                    self.message("operator", &t, body, *reply_to, 0);
                    events.push(Event::Message {
                        message: self.messages.last().cloned().expect("just pushed"),
                    });
                    if let Some(i) = self.index(&t)
                        && self.agents[i].state != AgentState::Gone
                    {
                        self.set(i, AgentState::Working);
                        events.push(Event::Agent {
                            agent: self.agents[i].clone(),
                        });
                    }
                }
                (events, Ok(None))
            }
            Request::ChatSend { rig, topic, body, .. } => {
                let m = ChatMessage {
                    id: self.chat.len() as u64 + 1,
                    rig: rig.clone(),
                    topic: topic
                        .clone()
                        .filter(|_| self.full)
                        .unwrap_or_else(|| "general".into()),
                    sender: "you (operator)".into(),
                    body: body.clone(),
                    created: now_ms(),
                };
                self.chat.push(m.clone());
                (vec![Event::Chat { message: m }], Ok(None))
            }
            Request::QueueCreate(spec) => {
                let id = format!("q-{}", self.queue.len() + 1);
                self.qitem(
                    &id,
                    "operator",
                    &spec.destination,
                    &spec.summary,
                    QState::Pending,
                    0,
                );
                (
                    vec![Event::Queue {
                        item: self.queue.last().cloned().expect("just pushed"),
                    }],
                    Ok(None),
                )
            }
            Request::QueueClaim { id, .. }
            | Request::QueueUnclaim { id, .. }
            | Request::QueueUpdate { id, .. }
            | Request::QueueHandoff { id, .. }
            | Request::QueueBlock { id, .. }
            | Request::QueueShow { id } => match self.queue.iter().position(|q| q.id == *id) {
                Some(at) => self.queue_request(at, req),
                None => (vec![], Err(format!("no queue item {id}"))),
            },
            Request::Transcript { agent, .. } => match self.index(agent) {
                Some(i) => (vec![], Ok(Some(self.transcript(&self.agents[i])))),
                None => (vec![], Err(format!("no agent {agent}"))),
            },
            Request::SpecShow { name: rig } | Request::Export { rig } => {
                match self.rigs.iter().find(|r| r.name == *rig) {
                    Some(r) => (vec![], Ok(Some(rig_yaml(r)))),
                    None => (vec![], Err(format!("no spec named {rig} in the demo"))),
                }
            }
            Request::Ps => (
                vec![],
                Ok(Some(crate::popup::render(&Response::Rigs {
                    rigs: self.rigs.clone(),
                }))),
            ),
            Request::Specs | Request::SnapshotList { .. } => {
                let text = self.list(req).map(|listing| {
                    crate::popup::render(&match listing {
                        Listing::Specs(specs) => Response::Specs { specs },
                        Listing::Snapshots(snapshots) => Response::Snapshots { snapshots },
                    })
                });
                (vec![], text.map(Some))
            }
            // Only a plan: the demo launches nothing.
            Request::Up(spec) if spec.plan => match self.rigs.iter().find(|r| r.name == spec.source) {
                Some(r) => (vec![], Ok(Some(rig_plan(r, spec.cwd.as_deref())))),
                None => (vec![], Err(format!("no spec named {} in the demo", spec.source))),
            },
            Request::Interact { .. }
            | Request::Input { .. }
            | Request::Resize { .. }
            | Request::Send { .. } => (vec![], Ok(None)),
            _ => (vec![], Err(CANNOT.into())),
        }
    }

    /// The answer to a listing request: its rigs as the spec library, and
    /// two snapshots of each.
    pub fn list(&self, req: &Request) -> Result<Listing, String> {
        match req {
            Request::Specs => Ok(Listing::Specs(
                self.rigs
                    .iter()
                    .map(|r| SpecEntry {
                        name: r.name.clone(),
                        summary: r.summary.clone(),
                        seats: r.seats.len(),
                        source: "demo".into(),
                    })
                    .collect(),
            )),
            Request::SnapshotList { rig } => match self.rigs.iter().find(|r| r.name == *rig) {
                Some(r) => Ok(Listing::Snapshots(
                    [(26 * 60, Some("before the docs push")), (90, None)]
                        .into_iter()
                        .map(|(minutes, label)| {
                            let created = now_ms() - minutes * MIN;
                            SnapshotInfo {
                                id: format!("{}@{created}", r.name),
                                rig: r.name.clone(),
                                label: label.map(Into::into),
                                created,
                                seats: r.seats.len(),
                            }
                        })
                        .collect(),
                )),
                None => Err(format!("no rig {rig}")),
            },
            _ => Err("that is not a listing".into()),
        }
    }

    /// A queue request on item `at`, as ainetd would carry it out, by you.
    fn queue_request(&mut self, at: usize, req: &Request) -> (Vec<Event>, Result<Option<String>, String>) {
        let now = now_ms();
        let note = |text: String, state: Option<QState>| QNote {
            by: "operator".into(),
            text,
            at: now,
            state,
        };
        let mut successor = None;
        let next_id = format!("q-{}", self.queue.len() + 1);
        let q = &mut self.queue[at];
        match req {
            Request::QueueShow { .. } => {
                return (vec![], Ok(Some(crate::popup::qitem_text(q, now))));
            }
            Request::QueueClaim { .. } => {
                q.owner = Some("operator".into());
                q.state = QState::InProgress;
                q.notes.push(note("claimed".into(), Some(QState::InProgress)));
            }
            Request::QueueUnclaim { reason, .. } => {
                q.owner = None;
                q.state = QState::Pending;
                let why = reason.clone().unwrap_or_else(|| "unclaimed".into());
                q.notes.push(note(why, Some(QState::Pending)));
            }
            Request::QueueUpdate {
                note: text, state, ..
            } => {
                if let Some(state) = state {
                    q.state = *state;
                    q.block = None;
                }
                q.notes.push(note(text.clone().unwrap_or_default(), *state));
            }
            Request::QueueBlock { on, continuation, .. } => {
                q.state = QState::Blocked;
                q.block = Some(QBlock {
                    on: on.clone(),
                    continuation: continuation.clone(),
                    wake_at: None,
                });
                q.notes
                    .push(note(format!("blocked on {on}"), Some(QState::Blocked)));
            }
            Request::QueueHandoff {
                to, body, summary, ..
            } => {
                let id = next_id;
                q.state = QState::Done;
                q.successor = Some(id.clone());
                q.notes
                    .push(note(format!("handed off to {to}"), Some(QState::Done)));
                let mut next = q.clone();
                next.id = id;
                next.source = "operator".into();
                next.destination = to.clone();
                next.owner = None;
                next.state = QState::Pending;
                next.summary = summary.clone().unwrap_or_else(|| q.summary.clone());
                next.body = body.clone();
                next.reply_to = Some(q.id.clone());
                next.successor = None;
                next.notes = vec![note("created".into(), Some(QState::Pending))];
                next.created = now;
                successor = Some(next);
            }
            _ => {}
        }
        q.updated = now;
        let mut events = vec![Event::Queue { item: q.clone() }];
        if let Some(next) = successor {
            self.queue.push(next.clone());
            events.push(Event::Queue { item: next });
        }
        (events, Ok(None))
    }

    /// A believable Claude Code transcript of what `agent` has been at.
    fn transcript(&self, agent: &Agent) -> String {
        let task = agent.task.clone().unwrap_or_default();
        let mut out = format!(
            "── {} ({}) ──\n\nyou: {task}\n\n{}: I'll start by looking at what's there for \"{task}\".\n",
            agent.label(),
            agent.runtime.label(),
            agent.name
        );
        if let Some(tool) = &agent.last_tool {
            out.push_str(&format!("  [tool] {tool}\n"));
        }
        for m in self
            .messages
            .iter()
            .filter(|m| m.to == agent.id || m.from == agent.id)
        {
            let who = if m.from == agent.id {
                &agent.name
            } else {
                &m.from_label
            };
            out.push_str(&format!("\n{who}: {}\n", m.body));
        }
        out
    }

    fn answer(&mut self, item: u64, decision: Decision) -> Vec<Event> {
        let Some(it) = self.inbox.iter_mut().find(|it| it.id == item && it.waiting) else {
            return vec![];
        };
        it.waiting = false;
        let agent = it.agent.clone();
        let mut events = vec![Event::InboxResolved { id: item }];
        if let Some(i) = self.index(&agent) {
            self.set(
                i,
                if decision == Decision::Deny {
                    AgentState::Idle
                } else {
                    AgentState::Working
                },
            );
            events.push(Event::Agent {
                agent: self.agents[i].clone(),
            });
        }
        events
    }

    /// The conversation lines of a believable agent terminal.
    pub fn screen(&self, agent: &Agent, tick: u64) -> Vec<String> {
        let task = agent.task.clone().unwrap_or_default();
        let mut rows = vec![
            format!("> {task}"),
            String::new(),
            format!("⏺ I'll start by looking at what's there for \"{task}\"."),
            String::new(),
            format!(
                "⏺ {}",
                agent.last_tool.clone().unwrap_or_default().replace("  ", "(") + ")"
            ),
            "  ⎿  done".into(),
        ];
        // The latest message it got, as it arrives in its terminal.
        for m in self.messages.iter().filter(|m| m.to == agent.id).rev().take(1) {
            rows.push(String::new());
            rows.push(format!("> [ainet] message #{} from {}:", m.id, m.from_label));
            rows.push(format!("  {}", m.body));
            rows.push(String::new());
            rows.push(format!("⏺ ainet - reply_message (id: {})", m.id));
            rows.push("  ⎿  delivered".into());
        }
        rows.push(String::new());
        match agent.state {
            AgentState::Working => rows.push(format!(
                "✻ Thinking{} ({}s · esc to interrupt)",
                ".".repeat((tick / 3 % 4) as usize),
                tick / 8 % 60
            )),
            AgentState::NeedsYou => {
                rows.push("Do you want to proceed?".into());
                rows.push(" ❯ 1. Yes".into());
                rows.push("   2. No, and tell Claude what to do differently".into());
            }
            _ => {}
        }
        rows
    }

    /// Up to `lines` rows of a believable scrollback, then the screen's
    /// conversation, as plain text (what ainetd sends for `history`).
    pub fn history(&self, agent: &Agent, tick: u64, lines: usize) -> Vec<String> {
        let task = agent.task.clone().unwrap_or_default();
        let cwd = agent.cwd.display();
        let mut older = Vec::new();
        for step in 1..=12 {
            older.push(format!("> step {step} of \"{task}\""));
            older.push(String::new());
            older.push(format!("⏺ Read({cwd}/src/part{step}.rs)"));
            older.push(format!("  ⎿  Read {} lines", 40 + step * 7));
            older.push(String::new());
            older.push(format!("⏺ Bash(cargo test part{step})"));
            older.push(format!("  ⎿  test result: ok. {} passed; 0 failed", 3 + step));
            older.push(String::new());
        }
        let skip = older.len().saturating_sub(lines);
        let mut rows = older.split_off(skip);
        rows.extend(self.screen(agent, tick));
        rows
    }

    /// An agent's terminal at `rows`×`cols`, as escape sequences.
    pub fn terminal(&self, agent: &Agent, tick: u64, rows: u16, cols: u16) -> String {
        let (rows, cols) = (rows.max(8) as usize, cols.max(20) as usize);
        let claude = agent.runtime == Runtime::ClaudeCode;
        let accent = if claude {
            "38;2;215;119;87"
        } else {
            "38;2;134;198;232"
        };
        let dim = "38;2;124;124;124";
        let fit = |s: &str, w: usize| -> String {
            let mut out: String = s.chars().take(w).collect();
            let n = out.chars().count();
            out.push_str(&" ".repeat(w.saturating_sub(n)));
            out
        };
        let mut out = String::from("\x1b[H\x1b[2J");
        let mut put = |row: usize, sgr: &str, text: &str| {
            if row < rows {
                out.push_str(&format!("\x1b[{};1H\x1b[{sgr}m{}\x1b[0m", row + 1, text));
            }
        };
        let inner = cols.saturating_sub(2);
        put(0, accent, &format!("╭{}╮", "─".repeat(inner)));
        let title = if claude {
            "✻ Welcome to Claude Code"
        } else {
            ">_ OpenAI Codex"
        };
        put(1, accent, &format!("│{}│", fit(&format!(" {title}"), inner)));
        put(
            2,
            dim,
            &format!("│{}│", fit(&format!("   cwd: {}", agent.cwd.display()), inner)),
        );
        put(3, accent, &format!("╰{}╯", "─".repeat(inner)));
        let body = self.screen(agent, tick);
        let room = rows.saturating_sub(4 + 5);
        let skip = body.len().saturating_sub(room);
        for (i, line) in body[skip..].iter().enumerate() {
            let sgr = if line.starts_with('>') {
                dim
            } else if line.starts_with('⏺') {
                "1;37"
            } else if line.starts_with('✻') {
                accent
            } else if line.contains('❯') {
                "38;2;134;198;232"
            } else {
                "37"
            };
            put(5 + i, sgr, &fit(line, cols));
        }
        let base = rows - 4;
        put(base, dim, &format!("╭{}╮", "─".repeat(inner)));
        put(base + 1, "37", &format!("│{}│", fit(" > ", inner)));
        put(base + 2, dim, &format!("╰{}╯", "─".repeat(inner)));
        let mode = if agent.state == AgentState::NeedsYou {
            "  ⏵⏵ waiting for permission"
        } else {
            "  ? for shortcuts"
        };
        put(base + 3, dim, &fit(mode, cols));
        out.push_str(&format!("\x1b[{};5H", base + 2));
        out
    }
}

/// What `ainet up --plan` says about a rig: its seats in launch order
/// (in `cwd` when given) and its edges.
fn rig_plan(rig: &Rig, cwd: Option<&std::path::Path>) -> String {
    let mut out = format!(
        "rig {}  ({} seats, root {})\n",
        rig.name,
        rig.seats.len(),
        rig.root.display()
    );
    if let Some(s) = &rig.summary {
        out.push_str(&format!("{}\n", s.trim()));
    }
    out.push_str("\nseats, in launch order:\n");
    for s in &rig.seats {
        out.push_str(&format!(
            "  {:<28} {:<12} {:<14} {}\n",
            s.address,
            s.runtime.spec_name(),
            s.model.as_deref().unwrap_or("default model"),
            cwd.unwrap_or(&s.cwd).display(),
        ));
    }
    if !rig.edges.is_empty() {
        out.push_str("\nedges:\n");
        for e in &rig.edges {
            out.push_str(&format!("  {} {} {}\n", e.from, e.kind.label(), e.to));
        }
    }
    out
}

/// A rig as the YAML of a RigSpec, as `ainet export` writes one: its
/// pods, their members and the edges between seats.
fn rig_yaml(rig: &Rig) -> String {
    let mut out = format!("version: \"0.2\"\nname: {}\n", rig.name);
    if let Some(summary) = &rig.summary {
        out.push_str(&format!("summary: >\n  {summary}\n"));
    }
    out.push_str("\npods:\n");
    let mut pods: Vec<&str> = rig.seats.iter().map(|s| s.pod.as_str()).collect();
    pods.dedup();
    for pod in pods {
        out.push_str(&format!("  - id: {pod}\n    members:\n"));
        for seat in rig.seats.iter().filter(|s| s.pod == pod) {
            out.push_str(&format!(
                "      - id: {}\n        runtime: {}\n        cwd: \"{}\"\n",
                seat.member,
                seat.runtime.spec_name(),
                seat.cwd.display()
            ));
            if let Some(role) = &seat.role {
                out.push_str(&format!("        role: {role}\n"));
            }
        }
    }
    out.push_str("\nedges:\n");
    for e in &rig.edges {
        out.push_str(&format!(
            "  - kind: {}\n    from: {}\n    to: {}\n",
            e.kind.label(),
            e.from,
            e.to
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_reaches_above_the_screen() {
        let demo = Demo::new();
        let Response::Snapshot { agents, .. } = demo.snapshot() else {
            panic!("a snapshot");
        };
        let agent = &agents[0];
        let screen = demo.screen(agent, 0);
        let rows = demo.history(agent, 0, 10);
        assert_eq!(rows.len(), 10 + screen.len());
        assert!(rows.ends_with(&screen));
        assert!(demo.history(agent, 0, 10_000).len() > 50);
    }

    /// The console's demo answers as it always has; only the desktop
    /// app's carries out the requests added for it.
    #[test]
    fn only_the_full_demo_answers_the_apps_requests() {
        let asks = [
            Request::Transcript {
                agent: SEEDS[0].id.into(),
                tail: 0,
                grep: None,
            },
            Request::SpecShow { name: "docs".into() },
            Request::Export { rig: "docs".into() },
            Request::Specs,
            Request::SnapshotList { rig: "docs".into() },
            Request::QueueShow { id: "q-1".into() },
        ];
        for req in &asks {
            let (events, reply) = Demo::new().handle(req);
            assert!(events.is_empty() && reply == Err(CANNOT.into()), "{req:?}");
            assert!(Demo::full().handle(req).1.is_ok(), "{req:?}");
        }
        let post = Request::ChatSend {
            rig: "docs".into(),
            topic: Some("release".into()),
            sender: None,
            body: "hi".into(),
        };
        let topic = |mut demo: Demo| match demo.handle(&post).0.as_slice() {
            [Event::Chat { message }] => message.topic.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(topic(Demo::new()), "general");
        assert_eq!(topic(Demo::full()), "release");
    }
}
