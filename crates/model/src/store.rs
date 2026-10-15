//! The fleet as last heard from ainetd: agents, inbox, rigs, messages,
//! chat, queue and the agents' terminals, plus the Fleet tree built from
//! them and the agent selected in it.

use std::collections::{BTreeMap, HashMap, HashSet};

use ainet_core::{
    Agent, AgentState, ChatMessage, Decision, Event, InboxItem, InboxKind, Message, QItem, Request, Response,
    Rig, Seat, SeatStatus,
};

use crate::link::Msg;

/// A row of the Fleet list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Header(String),
    /// A rig: its name.
    Rig(String),
    Agent {
        id: String,
        /// Tree drawing (`├─ `, `│  └─ `) in front of the agent.
        prefix: String,
    },
    /// A seat with nobody in it (stopped or pending).
    Seat {
        rig: String,
        address: String,
        prefix: String,
    },
}

/// One line of the Comms stream: a message or a chat post.
#[derive(Clone, Debug)]
pub enum Entry {
    Message(Message),
    Chat(ChatMessage),
}

impl Entry {
    pub fn created(&self) -> i64 {
        match self {
            Entry::Message(m) => m.created,
            Entry::Chat(c) => c.created,
        }
    }
}

/// What applying a [`Msg`] changed, so a front end can react (animate,
/// refetch a terminal, show a notice) without diffing the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Nothing a front end needs to react to.
    None,
    /// Everything was replaced by a snapshot.
    Snapshot,
    /// Agents or rigs changed; the Fleet tree was rebuilt.
    Fleet,
    /// An item joined the inbox at this index. `alert` when it waits on you.
    InboxAdded { index: usize, alert: bool },
    /// An inbox item was answered.
    InboxResolved,
    /// An agent's terminal has new output.
    Output(String),
    /// A fresh screen for an agent's terminal arrived.
    Term(String),
    /// Scrollback for an agent's terminal arrived.
    History(String),
    /// Messages, chat or the queue changed.
    Comms,
    /// A notice for the user.
    Flash(String, bool),
    /// A result to show in full: a title and text.
    Popup(String, String),
    /// The connection to ainetd came up or went down.
    Connected(bool),
}

/// Headline numbers about the fleet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Agents whose process is still running.
    pub live: usize,
    pub working: usize,
    pub idle: usize,
    /// Agents blocked on a permission, plan or question.
    pub needs_you: usize,
    /// Inbox items waiting on you.
    pub waiting: usize,
}

/// A rig's headline numbers, for its row in the Fleet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RigCounts {
    /// Seats whose agent is running.
    pub running: usize,
    pub seats: usize,
    /// Queue items of the rig not done or canceled.
    pub open: usize,
}

/// How many messages and chat posts are kept.
const KEEP: usize = 2000;

#[derive(Default)]
pub struct Store {
    pub agents: BTreeMap<String, Agent>,
    pub inbox: Vec<InboxItem>,
    pub rigs: BTreeMap<String, Rig>,
    pub messages: Vec<Message>,
    pub chat: Vec<ChatMessage>,
    pub queue: BTreeMap<String, QItem>,
    /// The Fleet tree, rebuilt whenever agents or rigs change.
    pub rows: Vec<Row>,
    /// The agent selected in the Fleet.
    pub selected: Option<String>,
    /// Plain text screens, for a details pane.
    pub screens: HashMap<String, Vec<String>>,
    /// Live terminal state per agent.
    pub terms: HashMap<String, vt100::Parser>,
    /// The scrollback last fetched per agent: rows above the screen, then
    /// the screen, as plain text.
    pub history: HashMap<String, Vec<String>>,
    pub connected: bool,
}

impl Store {
    pub fn new() -> Self {
        Store::default()
    }

    /// Applies one message from the link.
    pub fn apply(&mut self, msg: Msg) -> Change {
        match msg {
            Msg::Snapshot(resp) => {
                let Response::Snapshot {
                    agents,
                    inbox,
                    rigs,
                    messages,
                    chat,
                    queue,
                } = *resp
                else {
                    return Change::None;
                };
                self.agents = agents.into_iter().map(|a| (a.id.clone(), a)).collect();
                self.inbox = inbox;
                self.rigs = rigs.into_iter().map(|r| (r.name.clone(), r)).collect();
                self.messages = messages;
                self.chat = chat;
                self.queue = queue.into_iter().map(|q| (q.id.clone(), q)).collect();
                self.rebuild();
                Change::Snapshot
            }
            Msg::Event(ev) => self.apply_event(*ev),
            Msg::Term { id, ansi, size, rows } => {
                let (r, c) = (size.0.max(1), size.1.max(1));
                let mut parser = vt100::Parser::new(r, c, 0);
                parser.process(ansi.as_bytes());
                self.terms.insert(id.clone(), parser);
                self.screens.insert(id.clone(), rows);
                Change::Term(id)
            }
            Msg::History { id, rows } => {
                self.history.insert(id.clone(), rows);
                Change::History(id)
            }
            Msg::Flash(text, err) => Change::Flash(text, err),
            Msg::Popup(title, text) => Change::Popup(title, text),
            Msg::Connected(up) => {
                let was = std::mem::replace(&mut self.connected, up);
                if was == up {
                    Change::None
                } else {
                    Change::Connected(up)
                }
            }
        }
    }

    fn apply_event(&mut self, ev: Event) -> Change {
        match ev {
            Event::Agent { agent } => {
                self.agents.insert(agent.id.clone(), agent);
                self.rebuild();
                Change::Fleet
            }
            Event::AgentRemoved { id } => {
                self.agents.remove(&id);
                self.rebuild();
                Change::Fleet
            }
            Event::Inbox { item } => {
                let alert = item.waiting || item.kind == InboxKind::Question;
                self.inbox.push(item);
                Change::InboxAdded {
                    index: self.inbox.len() - 1,
                    alert,
                }
            }
            Event::InboxResolved { id } => {
                if let Some(it) = self.inbox.iter_mut().find(|i| i.id == id) {
                    it.waiting = false;
                }
                Change::InboxResolved
            }
            Event::Output { agent } => Change::Output(agent),
            Event::Rig { rig } => {
                self.rigs.insert(rig.name.clone(), rig);
                self.rebuild();
                Change::Fleet
            }
            Event::RigRemoved { name } => {
                self.rigs.remove(&name);
                self.rebuild();
                Change::Fleet
            }
            Event::Message { message } => {
                match self.messages.iter_mut().find(|m| m.id == message.id) {
                    Some(m) => *m = message,
                    None => {
                        self.messages.push(message);
                        if self.messages.len() > KEEP {
                            self.messages.remove(0);
                        }
                    }
                }
                Change::Comms
            }
            Event::Chat { message } => {
                self.chat.push(message);
                if self.chat.len() > KEEP {
                    self.chat.remove(0);
                }
                Change::Comms
            }
            Event::Queue { item } => {
                self.queue.insert(item.id.clone(), item);
                Change::Comms
            }
            Event::Watchdog { .. } => Change::None,
        }
    }

    // ---- the Fleet -------------------------------------------------------

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.selected.as_ref().and_then(|id| self.agents.get(id))
    }

    /// The Fleet row of the selected agent.
    pub fn selected_row(&self) -> Option<usize> {
        self.selected.as_deref().and_then(|id| self.row_of(id))
    }

    pub fn row_of(&self, id: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Agent { id: rid, .. } if rid == id))
    }

    /// Selects the agent `delta` agent rows away from the selected one
    /// (headers and empty seats are skipped). Returns the newly selected
    /// agent, or `None` when the selection did not change.
    pub fn move_selection(&mut self, delta: i32) -> Option<String> {
        self.move_selection_where(delta, |_| true)
    }

    /// [`Store::move_selection`] over the rows `shown` keeps, for a front
    /// end that folds parts of the tree away. From a selected agent that is
    /// not shown, one step goes to the next shown agent after it (or the
    /// one before it, going up).
    pub fn move_selection_where(&mut self, delta: i32, shown: impl Fn(usize) -> bool) -> Option<String> {
        let agents: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(i, r)| matches!(r, Row::Agent { .. }) && shown(*i))
            .map(|(i, _)| i)
            .collect();
        if agents.is_empty() {
            return None;
        }
        let cur = match self.selected_row() {
            Some(s) => match agents.iter().position(|i| *i == s) {
                Some(at) => at as i32,
                // Between two shown agents: `before` of them are above it.
                None => {
                    let before = agents.iter().filter(|i| **i < s).count() as i32;
                    if delta > 0 { before - 1 } else { before }
                }
            },
            None => 0,
        };
        let next = agents[(cur + delta).clamp(0, agents.len() as i32 - 1) as usize];
        match &self.rows[next] {
            Row::Agent { id, .. } if self.selected.as_ref() != Some(id) => {
                self.selected = Some(id.clone());
                self.selected.clone()
            }
            _ => None,
        }
    }

    /// Orders the Fleet: each rig with its seats as a tree of who directs
    /// whom, then loose teams, then loose agents grouped by state. Keeps the
    /// selection on the same agent, else the first one.
    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut placed: HashSet<String> = HashSet::new();
        let children_of = |members: &[&Agent]| {
            let ids: HashSet<&str> = members.iter().map(|a| a.id.as_str()).collect();
            let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let mut sorted: Vec<&&Agent> = members.iter().collect();
            sorted.sort_by_key(|a| (a.seat.clone(), a.started_at));
            for a in sorted {
                if let Some(p) = a.parent.as_ref().filter(|p| ids.contains(p.as_str())) {
                    map.entry(p.clone()).or_default().push(a.id.clone());
                }
            }
            map
        };
        fn walk(
            id: &str,
            prefix: String,
            kids_prefix: String,
            kids: &BTreeMap<String, Vec<String>>,
            rows: &mut Vec<Row>,
            placed: &mut HashSet<String>,
        ) {
            if !placed.insert(id.to_string()) {
                return;
            }
            rows.push(Row::Agent {
                id: id.to_string(),
                prefix,
            });
            if let Some(list) = kids.get(id) {
                for (i, k) in list.iter().enumerate() {
                    let last = i + 1 == list.len();
                    let branch = if last { "└─ " } else { "├─ " };
                    let cont = if last { "   " } else { "│  " };
                    walk(
                        k,
                        format!("{kids_prefix}{branch}"),
                        format!("{kids_prefix}{cont}"),
                        kids,
                        rows,
                        placed,
                    );
                }
            }
        }
        for (name, rig) in &self.rigs {
            rows.push(Row::Rig(name.clone()));
            let members: Vec<&Agent> = self
                .agents
                .values()
                .filter(|a| {
                    a.rig.as_deref() == Some(name.as_str())
                        && rig.seats.iter().any(|s| {
                            s.agent.as_deref() == Some(&a.id) && a.seat.as_deref() == Some(&s.address)
                        })
                })
                .filter(|a| a.state.is_live() || rig.seats.iter().any(|s| s.agent.as_deref() == Some(&a.id)))
                .collect();
            let kids = children_of(&members);
            let ids: HashSet<&str> = members.iter().map(|a| a.id.as_str()).collect();
            let mut roots: Vec<&&Agent> = members
                .iter()
                .filter(|a| a.parent.as_ref().is_none_or(|p| !ids.contains(p.as_str())))
                .collect();
            roots.sort_by_key(|a| a.seat.clone());
            for r in roots {
                walk(&r.id, String::new(), String::new(), &kids, &mut rows, &mut placed);
            }
            for s in &rig.seats {
                let has = s.agent.as_ref().is_some_and(|a| placed.contains(a));
                if !has {
                    rows.push(Row::Seat {
                        rig: name.clone(),
                        address: s.address.clone(),
                        prefix: String::new(),
                    });
                }
            }
        }
        // Loose agents: a team tree first, then by state.
        let loose: Vec<&Agent> = self
            .agents
            .values()
            .filter(|a| a.rig.is_none() && !placed.contains(&a.id))
            .collect();
        let kids = children_of(&loose);
        let ids: HashSet<&str> = loose.iter().map(|a| a.id.as_str()).collect();
        let mut roots: Vec<&&Agent> = loose
            .iter()
            .filter(|a| {
                a.parent.as_ref().is_none_or(|p| !ids.contains(p.as_str())) && kids.contains_key(&a.id)
            })
            .collect();
        roots.sort_by_key(|a| a.started_at);
        if !roots.is_empty() {
            rows.push(Row::Header("TEAM".into()));
            for r in roots {
                walk(&r.id, String::new(), String::new(), &kids, &mut rows, &mut placed);
            }
        }
        for state in [
            AgentState::NeedsYou,
            AgentState::Working,
            AgentState::Idle,
            AgentState::Failed,
            AgentState::Unknown,
            AgentState::Gone,
        ] {
            let mut group: Vec<&Agent> = loose
                .iter()
                .copied()
                .filter(|a| a.state == state && !placed.contains(&a.id))
                .collect();
            if group.is_empty() {
                continue;
            }
            group.sort_by_key(|a| a.started_at);
            rows.push(Row::Header(format!(
                "{} · {}",
                state.label().to_uppercase(),
                group.len()
            )));
            for a in group {
                placed.insert(a.id.clone());
                rows.push(Row::Agent {
                    id: a.id.clone(),
                    prefix: String::new(),
                });
            }
        }
        self.rows = rows;
        let keep = self.selected_row();
        let pick = keep.or_else(|| self.rows.iter().position(|r| matches!(r, Row::Agent { .. })));
        self.selected = pick.and_then(|i| match &self.rows[i] {
            Row::Agent { id, .. } => Some(id.clone()),
            _ => None,
        });
    }

    /// Agents with a live terminal, optionally of one rig, in rig and seat
    /// order: the Terminals grid.
    pub fn terminal_ids(&self, rig: Option<&str>) -> Vec<String> {
        let mut v: Vec<&Agent> = self
            .agents
            .values()
            .filter(|a| a.terminal && a.state.is_live())
            .filter(|a| rig.is_none_or(|r| a.rig.as_deref() == Some(r)))
            .collect();
        v.sort_by_key(|a| (a.rig.is_none(), a.rig.clone(), a.seat.clone(), a.started_at));
        v.into_iter().map(|a| a.id.clone()).collect()
    }

    /// An agent by seat address, logical name or id prefix, live ones only.
    /// Case, spaces, dashes and underscores do not matter; `@` is optional.
    pub fn find_agent(&self, key: &str) -> Option<&Agent> {
        let norm = |s: &str| s.to_lowercase().replace([' ', '-', '_'], "");
        let key = key.trim_start_matches('@');
        let k = norm(key);
        let live = || self.agents.values().filter(|a| a.state.is_live());
        live()
            .find(|a| {
                a.seat
                    .as_deref()
                    .is_some_and(|s| norm(s) == k || s.starts_with(key))
            })
            .or_else(|| live().find(|a| norm(&a.name) == k))
            .or_else(|| live().find(|a| norm(&a.name).starts_with(&k) || a.id.starts_with(key)))
    }

    /// The rig commands act on: the one named, else the one in view, else
    /// the selected agent's, else the only one.
    pub fn rig_for(&self, given: Option<&str>, in_view: Option<&str>) -> Option<String> {
        given
            .or(in_view)
            .map(str::to_string)
            .or_else(|| self.selected_agent().and_then(|a| a.rig.clone()))
            .or_else(|| {
                (self.rigs.len() == 1)
                    .then(|| self.rigs.keys().next().cloned())
                    .flatten()
            })
    }

    /// The fleet in numbers, as front ends show it in their header or
    /// status bar.
    pub fn tally(&self) -> Tally {
        let count = |s: AgentState| self.agents.values().filter(|a| a.state == s).count();
        Tally {
            live: self.agents.values().filter(|a| a.state.is_live()).count(),
            working: count(AgentState::Working),
            idle: count(AgentState::Idle),
            needs_you: count(AgentState::NeedsYou),
            waiting: self.inbox.iter().filter(|i| i.waiting).count(),
        }
    }

    /// How many live agents `id` directs.
    pub fn directs(&self, id: &str) -> usize {
        self.agents
            .values()
            .filter(|k| k.parent.as_deref() == Some(id) && k.state.is_live())
            .count()
    }

    /// Who directs `id`, from the top of the chain of command down to its
    /// direct parent.
    pub fn chain(&self, id: &str) -> Vec<&Agent> {
        let mut up = Vec::new();
        let mut cur = self.agents.get(id).and_then(|a| a.parent.as_deref());
        while let Some(p) = cur.and_then(|p| self.agents.get(p)) {
            // A cycle would loop for ever; the daemon should never make one.
            if up.iter().any(|a: &&Agent| a.id == p.id) || p.id == id {
                break;
            }
            up.push(p);
            cur = p.parent.as_deref();
        }
        up.reverse();
        up
    }

    /// The seat an agent sits in, if it is in a rig.
    pub fn seat_of(&self, agent: &Agent) -> Option<&Seat> {
        let rig = self.rigs.get(agent.rig.as_deref()?)?;
        rig.seat(agent.seat.as_deref()?)
    }

    /// A rig's headline: seats running, seats, and open queue items.
    pub fn rig_counts(&self, name: &str) -> RigCounts {
        let Some(rig) = self.rigs.get(name) else {
            return RigCounts::default();
        };
        RigCounts {
            running: rig
                .seats
                .iter()
                .filter(|s| s.status == SeatStatus::Running)
                .count(),
            seats: rig.seats.len(),
            open: self
                .queue
                .values()
                .filter(|q| q.rig.as_deref() == Some(name) && q.state.is_open())
                .count(),
        }
    }

    /// Inbox items of `id` waiting on you.
    pub fn waiting_for(&self, id: &str) -> usize {
        self.inbox.iter().filter(|i| i.waiting && i.agent == id).count()
    }

    /// Open queue items that are the agent's: claimed by it, or meant for
    /// it (by seat or id), as the daemon's `mine` reads; latest first.
    pub fn queue_for(&self, agent: &Agent) -> Vec<&QItem> {
        let me = |w: &str| w == agent.id || agent.seat.as_deref() == Some(w);
        let mut v: Vec<&QItem> = self
            .queue
            .values()
            .filter(|q| q.state.is_open() && (me(&q.destination) || q.owner.as_deref().is_some_and(me)))
            .collect();
        v.sort_by_key(|q| std::cmp::Reverse(q.updated));
        v
    }

    /// The last `n` messages to or from `id`, oldest first.
    pub fn messages_for(&self, id: &str, n: usize) -> Vec<&Message> {
        let mine: Vec<&Message> = self
            .messages
            .iter()
            .filter(|m| m.from == id || m.to == id)
            .collect();
        mine[mine.len().saturating_sub(n)..].to_vec()
    }

    /// The running operator agent, if any.
    pub fn operator(&self) -> Option<&Agent> {
        self.agents
            .values()
            .find(|a| a.rig.as_deref() == Some("operator") && a.state.is_live())
    }

    // ---- the inbox ---------------------------------------------------------

    /// The oldest request still waiting, else the newest item.
    pub fn waiting_index(&self) -> Option<usize> {
        self.inbox
            .iter()
            .position(|i| i.waiting)
            .or(self.inbox.len().checked_sub(1))
    }

    /// The oldest waiting request of the selected agent, else of anyone.
    pub fn next_waiting(&self) -> Option<usize> {
        self.inbox
            .iter()
            .position(|i| i.waiting && Some(&i.agent) == self.selected.as_ref())
            .or_else(|| self.inbox.iter().position(|i| i.waiting))
    }

    /// The next agent with a request waiting on you after the selected
    /// one, in the inbox's order and round again: where a "next waiting"
    /// key moves the Fleet's selection.
    pub fn next_waiting_agent(&self) -> Option<&str> {
        let mut agents: Vec<&str> = Vec::new();
        for item in self.inbox.iter().filter(|i| i.waiting) {
            if self.agents.contains_key(&item.agent) && !agents.contains(&item.agent.as_str()) {
                agents.push(&item.agent);
            }
        }
        let at = self
            .selected
            .as_deref()
            .and_then(|s| agents.iter().position(|a| *a == s));
        match at {
            Some(at) => agents.get((at + 1) % agents.len()).copied(),
            None => agents.first().copied(),
        }
    }

    /// What to say when inbox item `index` asks for you: its agent's name
    /// and its summary.
    pub fn inbox_alert(&self, index: usize) -> Option<String> {
        let item = self.inbox.get(index)?;
        let name = self
            .agents
            .get(&item.agent)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        Some(format!("{name}: {}", item.summary))
    }

    /// The request that answers `item` with `decision`, and what to say once
    /// it is done; or why it cannot be answered that way.
    pub fn answer(&self, item: &InboxItem, decision: Decision) -> Result<(Request, String), String> {
        if !item.waiting {
            return Err("that one is not waiting on you".into());
        }
        if item.kind == InboxKind::Question && decision != Decision::Deny {
            return Err("type an option's number (or your answer) and Enter".into());
        }
        let name = self
            .agents
            .get(&item.agent)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let verb = match decision {
            Decision::Allow => "allowed",
            Decision::Always => "always allowing",
            Decision::Deny => "denied",
        };
        Ok((
            Request::Answer {
                item: item.id,
                decision,
            },
            format!("{verb} {name}'s {}", item.kind.label()),
        ))
    }

    // ---- comms -------------------------------------------------------------

    /// Messages and chat, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = self
            .messages
            .iter()
            .cloned()
            .map(Entry::Message)
            .chain(self.chat.iter().cloned().map(Entry::Chat))
            .collect();
        v.sort_by_key(Entry::created);
        v
    }
}

#[cfg(test)]
mod tests {
    use ainet_core::{Agent, Decision, Event, InboxItem, InboxKind, Message, Request};

    use super::{Change, Row, Store};
    use crate::demo::Demo;
    use crate::link::Msg;

    fn demo() -> Store {
        let mut store = Store::new();
        let change = store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        assert_eq!(change, Change::Snapshot);
        store
    }

    fn by_seat<'a>(store: &'a Store, seat: &str) -> &'a Agent {
        store
            .agents
            .values()
            .find(|a| a.seat.as_deref() == Some(seat))
            .unwrap_or_else(|| panic!("no agent in {seat}"))
    }

    fn event(store: &mut Store, ev: Event) -> Change {
        store.apply(Msg::Event(Box::new(ev)))
    }

    fn item(id: u64, agent: &str, kind: InboxKind, waiting: bool) -> InboxItem {
        InboxItem {
            id,
            agent: agent.into(),
            kind,
            summary: "Bash: ls".into(),
            detail: String::new(),
            created: 0,
            waiting,
            options: vec![],
            multi_select: false,
        }
    }

    #[test]
    fn tally_counts_live_agents_and_waiting_items() {
        let mut store = demo();
        let t = store.tally();
        let live = store.agents.values().filter(|a| a.state.is_live()).count();
        assert_eq!(t.live, live);
        assert!(t.working + t.idle + t.needs_you <= t.live);
        let waiting = store.inbox.iter().filter(|i| i.waiting).count();
        assert_eq!(t.waiting, waiting);
        let agent = store.agents.keys().next().unwrap().clone();
        event(
            &mut store,
            Event::Inbox {
                item: item(9_999, &agent, InboxKind::Permission, true),
            },
        );
        assert_eq!(store.tally().waiting, waiting + 1);
    }

    /// The seats of a rig's agent rows, with their tree prefixes.
    fn tree(store: &Store, rig: &str) -> Vec<(String, String)> {
        let start = store
            .rows
            .iter()
            .position(|r| *r == Row::Rig(rig.into()))
            .unwrap()
            + 1;
        store.rows[start..]
            .iter()
            .take_while(|r| matches!(r, Row::Agent { .. }))
            .map(|r| match r {
                Row::Agent { id, prefix } => {
                    (store.agents[id].seat.clone().unwrap_or_default(), prefix.clone())
                }
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn the_fleet_is_rigs_as_trees_of_who_directs_whom() {
        let store = demo();
        assert_eq!(store.rows[0], Row::Rig("docs".into()));
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
        };
        assert_eq!(
            tree(&store, "docs"),
            pairs(&[
                ("orch-lead@docs", ""),
                ("dev-app@docs", "├─ "),
                ("dev-ui@docs", "├─ "),
                ("dev-writer@docs", "└─ "),
                ("rev-review@docs", ""),
            ])
        );
        assert_eq!(
            tree(&store, "infra"),
            pairs(&[("ops-data@infra", ""), ("ops-sre@infra", "")])
        );
        // The loose agent that has exited comes last, under its state.
        assert!(matches!(&store.rows[store.rows.len() - 2], Row::Header(h) if h == "GONE · 1"));
    }

    #[test]
    fn the_first_agent_is_selected_and_the_selection_survives_rebuilds() {
        let mut store = demo();
        let lead = by_seat(&store, "orch-lead@docs").id.clone();
        assert_eq!(store.selected.as_deref(), Some(lead.as_str()));
        assert_eq!(store.selected_row(), Some(1));

        let next = store.move_selection(1).expect("moved");
        assert_eq!(store.agents[&next].seat.as_deref(), Some("dev-app@docs"));
        assert_eq!(store.move_selection(-100), Some(lead.clone()));
        assert_eq!(store.move_selection(-1), None, "already at the top");

        store.move_selection(2);
        let kept = store.selected.clone();
        let mut sre = by_seat(&store, "ops-sre@infra").clone();
        sre.task = Some("rotate the keys".into());
        assert_eq!(event(&mut store, Event::Agent { agent: sre }), Change::Fleet);
        assert_eq!(store.selected, kept);

        // When the selected agent goes, the first agent is selected again.
        let gone = kept.unwrap();
        event(&mut store, Event::AgentRemoved { id: gone.clone() });
        assert!(!store.agents.contains_key(&gone));
        assert_eq!(store.selected.as_deref(), Some(lead.as_str()));
    }

    #[test]
    fn folded_rows_are_stepped_over() {
        let mut store = demo();
        let row = |store: &Store, seat: &str| store.row_of(&by_seat(store, seat).id).unwrap();
        // Fold the docs rig's team under its lead: rows 2..=4.
        let (app, writer) = (row(&store, "dev-app@docs"), row(&store, "dev-writer@docs"));
        let folded = |i: usize| !(app..=writer).contains(&i);
        let next = store.move_selection_where(1, folded).unwrap();
        assert_eq!(store.agents[&next].seat.as_deref(), Some("rev-review@docs"));
        store.move_selection_where(-1, folded);
        assert_eq!(store.selected_row(), Some(row(&store, "orch-lead@docs")));

        // From a folded agent, down goes to the next shown one, up to the
        // one before it.
        store.selected = Some(by_seat(&store, "dev-ui@docs").id.clone());
        store.move_selection_where(1, folded);
        assert_eq!(store.selected_row(), Some(row(&store, "rev-review@docs")));
        store.selected = Some(by_seat(&store, "dev-ui@docs").id.clone());
        store.move_selection_where(-1, folded);
        assert_eq!(store.selected_row(), Some(row(&store, "orch-lead@docs")));
        // Nothing shown, nothing selected anew.
        assert_eq!(store.move_selection_where(1, |_| false), None);
    }

    #[test]
    fn agents_know_their_chain_seat_rig_work_and_mail() {
        let store = demo();
        let lead = by_seat(&store, "orch-lead@docs");
        let ui = by_seat(&store, "dev-ui@docs");
        let names = |v: Vec<&Agent>| v.into_iter().map(|a| a.id.clone()).collect::<Vec<_>>();
        assert_eq!(names(store.chain(&ui.id)), std::slice::from_ref(&lead.id));
        assert!(store.chain(&lead.id).is_empty());
        assert_eq!(store.directs(&lead.id), 3);
        assert_eq!(store.directs(&ui.id), 0);
        assert_eq!(store.seat_of(ui).map(|s| s.address.as_str()), Some("dev-ui@docs"));

        let docs = store.rig_counts("docs");
        assert_eq!(docs.seats, store.rigs["docs"].seats.len());
        assert!(docs.running <= docs.seats);
        let open = store
            .queue
            .values()
            .filter(|q| q.rig.as_deref() == Some("docs") && q.state.is_open())
            .count();
        assert_eq!(docs.open, open);
        assert_eq!(store.rig_counts("nope"), super::RigCounts::default());

        for a in store.agents.values() {
            for q in store.queue_for(a) {
                assert!(q.state.is_open());
                let mine = [Some(a.id.as_str()), a.seat.as_deref()];
                assert!(
                    mine.contains(&Some(q.destination.as_str())) || mine.contains(&q.owner.as_deref()),
                    "{} is not {}'s",
                    q.id,
                    a.name
                );
            }
            let mail = store.messages_for(&a.id, 3);
            assert!(mail.len() <= 3);
            assert!(mail.iter().all(|m| m.from == a.id || m.to == a.id));
        }
        let waiting = store.inbox.iter().filter(|i| i.waiting).count();
        let per_agent: usize = store.agents.keys().map(|id| store.waiting_for(id)).sum();
        assert_eq!(per_agent, waiting);
    }

    #[test]
    fn terminals_are_in_rig_and_seat_order() {
        let store = demo();
        let seats = |rig| -> Vec<String> {
            store
                .terminal_ids(rig)
                .iter()
                .map(|id| store.agents[id].seat.clone().unwrap_or_default())
                .collect()
        };
        assert_eq!(seats(Some("infra")), ["ops-data@infra", "ops-sre@infra"]);
        let all = seats(None);
        assert_eq!(all.first().map(String::as_str), Some("dev-app@docs"));
        assert!(all.len() >= 7, "{all:?}");
    }

    #[test]
    fn agents_are_found_by_seat_name_or_id() {
        let store = demo();
        let lead = by_seat(&store, "orch-lead@docs").clone();
        assert_eq!(store.find_agent("orch-lead@docs").map(|a| &a.id), Some(&lead.id));
        assert_eq!(store.find_agent("@orch-lead").map(|a| &a.id), Some(&lead.id));
        let shouted = lead.name.to_uppercase().replace(' ', "_");
        assert_eq!(store.find_agent(&shouted).map(|a| &a.id), Some(&lead.id));
        assert_eq!(store.find_agent(&lead.id[..8]).map(|a| &a.id), Some(&lead.id));
        assert!(store.find_agent("nobody-at-all").is_none());
    }

    #[test]
    fn rig_commands_act_on_the_named_then_viewed_then_selected_rig() {
        let mut store = demo();
        assert_eq!(store.rig_for(Some("x"), Some("infra")).as_deref(), Some("x"));
        assert_eq!(store.rig_for(None, Some("infra")).as_deref(), Some("infra"));
        assert_eq!(store.rig_for(None, None).as_deref(), Some("docs"));
        store.selected = None;
        assert_eq!(store.rig_for(None, None), None, "two rigs, none chosen");
    }

    #[test]
    fn inbox_items_alert_when_they_wait_on_you() {
        let mut store = demo();
        let lead = by_seat(&store, "orch-lead@docs").id.clone();
        let before = store.inbox.len();
        let change = event(
            &mut store,
            Event::Inbox {
                item: item(900, &lead, InboxKind::Permission, true),
            },
        );
        assert_eq!(
            change,
            Change::InboxAdded {
                index: before,
                alert: true
            }
        );
        let name = store.agents[&lead].name.clone();
        assert_eq!(store.inbox_alert(before), Some(format!("{name}: Bash: ls")));
        assert_eq!(store.inbox_alert(before + 1), None);
        let done = event(
            &mut store,
            Event::Inbox {
                item: item(901, &lead, InboxKind::Finished, false),
            },
        );
        assert!(matches!(done, Change::InboxAdded { alert: false, .. }));
        // The selected agent's own request comes first.
        assert_eq!(store.next_waiting().map(|i| store.inbox[i].id), Some(900));

        event(&mut store, Event::InboxResolved { id: 900 });
        assert!(!store.inbox.iter().any(|i| i.id == 900 && i.waiting));
    }

    #[test]
    fn the_next_waiting_agent_goes_round_those_waiting() {
        let mut store = demo();
        for i in store.inbox.iter_mut() {
            i.waiting = false;
        }
        assert_eq!(store.next_waiting_agent(), None);
        let lead = by_seat(&store, "orch-lead@docs").id.clone();
        let ui = by_seat(&store, "dev-ui@docs").id.clone();
        for (id, agent) in [(900, &ui), (901, &lead), (902, &ui)] {
            event(
                &mut store,
                Event::Inbox {
                    item: item(id, agent, InboxKind::Permission, true),
                },
            );
        }
        store.selected = None;
        assert_eq!(store.next_waiting_agent(), Some(ui.as_str()));
        store.selected = Some(ui.clone());
        assert_eq!(store.next_waiting_agent(), Some(lead.as_str()));
        store.selected = Some(lead.clone());
        assert_eq!(store.next_waiting_agent(), Some(ui.as_str()), "round again");
    }

    #[test]
    fn answers_say_who_was_answered() {
        let store = demo();
        let lead = by_seat(&store, "orch-lead@docs");
        let held = item(7, &lead.id, InboxKind::Permission, true);
        let (req, ok) = store.answer(&held, Decision::Always).unwrap();
        assert!(matches!(
            req,
            Request::Answer {
                item: 7,
                decision: Decision::Always
            }
        ));
        assert!(
            ok.starts_with(&format!("always allowing {}'s", lead.name)),
            "{ok}"
        );

        let old = item(8, &lead.id, InboxKind::Permission, false);
        assert!(store.answer(&old, Decision::Allow).is_err());
        let question = item(9, &lead.id, InboxKind::Question, true);
        assert!(store.answer(&question, Decision::Allow).is_err());
        assert!(store.answer(&question, Decision::Deny).is_ok());
    }

    #[test]
    fn connection_changes_are_reported_once() {
        let mut store = Store::new();
        assert_eq!(store.apply(Msg::Connected(false)), Change::None);
        assert_eq!(store.apply(Msg::Connected(true)), Change::Connected(true));
        assert_eq!(store.apply(Msg::Connected(true)), Change::None);
        assert!(store.connected);
    }

    #[test]
    fn messages_update_in_place_and_comms_interleave() {
        let mut store = demo();
        let first = store.messages[0].clone();
        let edited = Message {
            body: "edited".into(),
            ..first.clone()
        };
        let n = store.messages.len();
        assert_eq!(
            event(&mut store, Event::Message { message: edited }),
            Change::Comms
        );
        assert_eq!(store.messages.len(), n);
        assert_eq!(store.messages[0].body, "edited");

        let entries = store.entries();
        assert_eq!(entries.len(), store.messages.len() + store.chat.len());
        assert!(entries.windows(2).all(|w| w[0].created() <= w[1].created()));
    }

    #[test]
    fn screens_become_terminals() {
        let mut store = Store::new();
        let change = store.apply(Msg::Term {
            id: "a".into(),
            ansi: "\x1b[1mhello".into(),
            size: (4, 20),
            rows: vec!["hello".into()],
        });
        assert_eq!(change, Change::Term("a".into()));
        assert_eq!(store.terms["a"].screen().size(), (4, 20));
        assert!(store.terms["a"].screen().contents().starts_with("hello"));
        assert_eq!(store.screens["a"], ["hello"]);

        let change = store.apply(Msg::History {
            id: "a".into(),
            rows: vec!["older".into(), "hello".into()],
        });
        assert_eq!(change, Change::History("a".into()));
        assert_eq!(store.history["a"], ["older", "hello"]);
    }
}
