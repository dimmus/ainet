//! App state, the data source (daemon or demo) and input handling.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ainet_core::protocol::{QSpec, SeatSpec, SpawnSpec, UpSpec};
use ainet_core::{
    Agent, AgentState, ChatMessage, Decision, Event, InboxItem, InboxKind, Message, QItem, Request, Response,
    Rig, Runtime,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use tachyonfx::fx::{self, RepeatMode};
use tachyonfx::{EffectManager, Interpolation, Motion};

use crate::backend;
use crate::demo::Demo;
use crate::sprite::Size;
use crate::term;
use crate::theme::{Palette, c};

/// One animation tick.
pub const TICK: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Fleet,
    Terminals,
    Comms,
    Inbox,
    Map,
    Review,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Fleet,
        Tab::Terminals,
        Tab::Comms,
        Tab::Inbox,
        Tab::Map,
        Tab::Review,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Fleet => "Fleet",
            Tab::Terminals => "Terminals",
            Tab::Comms => "Comms",
            Tab::Inbox => "Inbox",
            Tab::Map => "Map",
            Tab::Review => "Review",
        }
    }
}

/// What the background threads tell the UI.
pub enum Msg {
    Snapshot(Box<Response>),
    Event(Box<Event>),
    Term {
        id: String,
        ansi: String,
        size: (u16, u16),
        rows: Vec<String>,
    },
    Flash(String, bool),
    Popup(String, String),
    Connected(bool),
}

pub enum Backend {
    Demo(Box<Demo>),
    Daemon(PathBuf),
}

/// A row of the Fleet list.
#[derive(Clone, Debug)]
pub enum Row {
    Header(String),
    /// A rig: its name.
    Rig(String),
    Agent {
        id: String,
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

pub struct Flash {
    pub text: String,
    pub err: bool,
    pub at: Instant,
}

pub struct Popup {
    pub title: String,
    pub text: String,
    pub scroll: u16,
}

/// The Terminals view.
#[derive(Default)]
pub struct Grid {
    /// Fixed column count; `None` picks one from the window's shape.
    pub cols: Option<u16>,
    pub focus: Option<String>,
    /// The focused terminal fills the view.
    pub zoom: bool,
    /// Keys go to the focused terminal (leave with Ctrl+]).
    pub typing: bool,
    /// Only this rig's terminals.
    pub rig: Option<String>,
}

pub struct App {
    pub pal: Palette,
    pub tick: u64,
    pub tab: Tab,
    pub agents: BTreeMap<String, Agent>,
    pub inbox: Vec<InboxItem>,
    pub rigs: BTreeMap<String, Rig>,
    pub messages: Vec<Message>,
    pub chat: Vec<ChatMessage>,
    pub queue: BTreeMap<String, QItem>,
    pub rows: Vec<Row>,
    pub selected: Option<String>,
    pub fleet_state: ListState,
    pub inbox_state: ListState,
    pub comms_state: ListState,
    pub input: String,
    /// Cursor position in `input`, in chars.
    pub cursor: usize,
    pub flash: Option<Flash>,
    pub help: bool,
    pub popup: Option<Popup>,
    /// Critters in the Fleet list, and at which size.
    pub critter_rows: Option<Size>,
    /// Plain text screens, for the details pane.
    pub screens: HashMap<String, Vec<String>>,
    /// Live terminal state per agent, for the grid and the details pane.
    pub terms: HashMap<String, vt100::Parser>,
    pub grid: Grid,
    pub connected: bool,
    pub fx: EffectManager<&'static str>,
    pub quitting: Option<Instant>,
    /// Smoothed context gauge, so it glides instead of jumping.
    pub gauge: f64,
    /// Where things were drawn last frame, for effects and mouse hits.
    pub areas: Areas,
    backend: Backend,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    fetch: Option<mpsc::Sender<String>>,
    dirty: HashSet<String>,
    last_fetch: HashMap<String, Instant>,
    /// Sizes we asked panes to take, so we ask once per change.
    sizes: HashMap<String, (u16, u16)>,
}

#[derive(Default, Clone)]
pub struct Areas {
    pub body: Rect,
    pub details: Rect,
    pub tabs: Vec<(Rect, Tab)>,
    pub inbox_tab: Rect,
    /// Terminal tiles: outer rect, agent id, inner (rows, cols).
    pub tiles: Vec<(Rect, String, (u16, u16))>,
}

impl App {
    /// `view` picks the first view: `terminals`, `rig:<name>`, `comms`, …
    pub fn new(backend: Backend, view: Option<&str>) -> Self {
        let (tx, rx) = mpsc::channel();
        let pal = Palette::load();
        let mut fx = EffectManager::default();
        // Opening: the screen coalesces out of noise.
        fx.add_effect(fx::coalesce((900, Interpolation::QuadOut)));
        let mut app = App {
            pal,
            tick: 0,
            tab: Tab::Fleet,
            agents: BTreeMap::new(),
            inbox: Vec::new(),
            rigs: BTreeMap::new(),
            messages: Vec::new(),
            chat: Vec::new(),
            queue: BTreeMap::new(),
            rows: Vec::new(),
            selected: None,
            fleet_state: ListState::default(),
            inbox_state: ListState::default(),
            comms_state: ListState::default(),
            input: String::new(),
            cursor: 0,
            flash: None,
            help: false,
            popup: None,
            critter_rows: None,
            screens: HashMap::new(),
            terms: HashMap::new(),
            grid: Grid::default(),
            connected: false,
            fx,
            quitting: None,
            gauge: 0.0,
            areas: Areas::default(),
            backend,
            tx,
            rx,
            fetch: None,
            dirty: HashSet::new(),
            last_fetch: HashMap::new(),
            sizes: HashMap::new(),
        };
        match &app.backend {
            Backend::Demo(demo) => {
                let snap = demo.snapshot();
                app.connected = true;
                app.apply(Msg::Snapshot(Box::new(snap)));
            }
            Backend::Daemon(socket) => {
                backend::listen(socket.clone(), app.tx.clone());
                app.fetch = Some(backend::fetcher(socket.clone(), app.tx.clone()));
            }
        }
        if let Some(v) = view {
            app.open_view(v);
        }
        app
    }

    /// `terminals`, `rig:<name>`, `pod:<rig>/<pod>`, `comms`, `inbox`, `map`.
    pub fn open_view(&mut self, view: &str) {
        let (kind, arg) = view.split_once(':').unwrap_or((view, ""));
        match kind {
            "terminals" | "terminal" | "grid" => self.tab = Tab::Terminals,
            "rig" | "saved" => {
                self.tab = Tab::Terminals;
                self.grid.rig = Some(arg.to_string()).filter(|a| !a.is_empty());
            }
            "pod" => {
                self.tab = Tab::Terminals;
                self.grid.rig = arg.split('/').next().map(str::to_string);
            }
            "comms" | "chat" | "queue" => self.tab = Tab::Comms,
            "inbox" | "attention" | "feed" => self.tab = Tab::Inbox,
            "map" | "graph" => self.tab = Tab::Map,
            _ if self.rigs.contains_key(view) => {
                self.tab = Tab::Terminals;
                self.grid.rig = Some(view.to_string());
            }
            _ => {}
        }
        self.dirty_visible();
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.backend, Backend::Demo(_))
    }

    /// True once the closing effect has played.
    pub fn done(&self) -> bool {
        self.quitting
            .is_some_and(|t| t.elapsed() > Duration::from_millis(380))
    }

    pub fn quit(&mut self) {
        if self.grid.typing {
            self.stop_typing();
        }
        if self.quitting.is_none() {
            self.quitting = Some(Instant::now());
            self.fx.add_effect(fx::dissolve((360, Interpolation::QuadIn)));
        }
    }

    /// Advances animations and the demo by one tick.
    pub fn on_tick(&mut self) {
        self.tick += 1;
        if let Backend::Demo(demo) = &mut self.backend {
            for ev in demo.step(self.tick) {
                let _ = self.tx.send(Msg::Event(Box::new(ev)));
            }
        }
        let target = self.selected_agent().and_then(|a| a.context).unwrap_or(0.0) as f64;
        self.gauge += (target - self.gauge) * 0.25;
        if self
            .flash
            .as_ref()
            .is_some_and(|f| f.at.elapsed() > Duration::from_secs(5))
        {
            self.flash = None;
        }
        if self.is_demo() {
            self.dirty_visible();
        }
    }

    /// Applies everything the background threads sent.
    pub fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.apply(msg);
        }
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Snapshot(resp) => {
                if let Response::Snapshot {
                    agents,
                    inbox,
                    rigs,
                    messages,
                    chat,
                    queue,
                } = *resp
                {
                    self.agents = agents.into_iter().map(|a| (a.id.clone(), a)).collect();
                    self.inbox = inbox;
                    self.rigs = rigs.into_iter().map(|r| (r.name.clone(), r)).collect();
                    self.messages = messages;
                    self.chat = chat;
                    self.queue = queue.into_iter().map(|q| (q.id.clone(), q)).collect();
                    self.select_waiting();
                    self.rebuild();
                    self.dirty_visible();
                }
            }
            Msg::Event(ev) => self.apply_event(*ev),
            Msg::Term { id, ansi, size, rows } => {
                let (r, c) = (size.0.max(1), size.1.max(1));
                let mut parser = vt100::Parser::new(r, c, 0);
                parser.process(ansi.as_bytes());
                self.terms.insert(id.clone(), parser);
                self.screens.insert(id, rows);
            }
            Msg::Flash(text, err) => self.flash(text, err),
            Msg::Popup(title, text) => {
                self.popup = Some(Popup {
                    title,
                    text,
                    scroll: 0,
                })
            }
            Msg::Connected(up) => {
                if up != self.connected {
                    self.flash(
                        if up {
                            "connected to ainetd".into()
                        } else {
                            "ainetd is not answering".into()
                        },
                        !up,
                    );
                }
                self.connected = up;
            }
        }
    }

    fn apply_event(&mut self, ev: Event) {
        match ev {
            Event::Agent { agent } => {
                self.agents.insert(agent.id.clone(), agent);
                self.rebuild();
            }
            Event::AgentRemoved { id } => {
                self.agents.remove(&id);
                self.rebuild();
            }
            Event::Inbox { item } => {
                if item.waiting || item.kind == InboxKind::Question {
                    // Make the Inbox tab glow a few times.
                    let glow = fx::repeat(
                        fx::ping_pong(fx::hsl_shift(
                            Some([0.0, 20.0, 25.0]),
                            Some([0.0, 10.0, 15.0]),
                            (300, Interpolation::SineInOut),
                        )),
                        RepeatMode::Times(3),
                    );
                    self.fx
                        .add_unique_effect("inbox-tab", glow.with_area(self.areas.inbox_tab));
                    let name = self
                        .agents
                        .get(&item.agent)
                        .map(|a| a.name.clone())
                        .unwrap_or_default();
                    self.flash(format!("{name}: {}", item.summary), false);
                }
                self.inbox.push(item);
                if self.inbox_state.selected().is_none() {
                    self.select_waiting();
                }
            }
            Event::InboxResolved { id } => {
                if let Some(it) = self.inbox.iter_mut().find(|i| i.id == id) {
                    it.waiting = false;
                }
            }
            Event::Output { agent } => {
                if self.shows_terminal(&agent) {
                    self.dirty.insert(agent);
                }
            }
            Event::Rig { rig } => {
                self.rigs.insert(rig.name.clone(), rig);
                self.rebuild();
            }
            Event::RigRemoved { name } => {
                self.rigs.remove(&name);
                self.rebuild();
            }
            Event::Message { message } => match self.messages.iter_mut().find(|m| m.id == message.id) {
                Some(m) => *m = message,
                None => {
                    self.messages.push(message);
                    if self.messages.len() > 2000 {
                        self.messages.remove(0);
                    }
                }
            },
            Event::Chat { message } => {
                self.chat.push(message);
                if self.chat.len() > 2000 {
                    self.chat.remove(0);
                }
            }
            Event::Queue { item } => {
                self.queue.insert(item.id.clone(), item);
            }
            Event::Watchdog { .. } => {}
        }
    }

    pub fn flash(&mut self, text: String, err: bool) {
        self.flash = Some(Flash {
            text,
            err,
            at: Instant::now(),
        });
    }

    // ---- the Fleet list ------------------------------------------------

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.selected.as_ref().and_then(|id| self.agents.get(id))
    }

    /// Orders the Fleet: each rig with its seats as a tree of who directs
    /// whom, then loose teams, then loose agents grouped by state.
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
        // Keep the selection on the same agent; fall back to the first one.
        let keep = self.selected.as_ref().and_then(|id| self.row_of(id));
        let pick = keep.or_else(|| self.rows.iter().position(|r| matches!(r, Row::Agent { .. })));
        self.fleet_state.select(pick);
        self.selected = pick.and_then(|i| match &self.rows[i] {
            Row::Agent { id, .. } => Some(id.clone()),
            _ => None,
        });
        if self
            .grid
            .focus
            .as_ref()
            .is_none_or(|f| !self.agents.contains_key(f))
        {
            self.grid.focus = self.grid_ids().into_iter().next();
        }
    }

    fn row_of(&self, id: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Agent { id: rid, .. } if rid == id))
    }

    fn move_selection(&mut self, delta: i32) {
        match self.tab {
            Tab::Inbox => step(&mut self.inbox_state, self.inbox.len(), delta),
            Tab::Comms => {
                let n = self.entries().len();
                step(&mut self.comms_state, n, delta);
            }
            Tab::Terminals => self.move_focus(delta, 0),
            _ => {
                let agents: Vec<usize> = self
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| matches!(r, Row::Agent { .. }))
                    .map(|(i, _)| i)
                    .collect();
                if agents.is_empty() {
                    return;
                }
                let cur = self
                    .fleet_state
                    .selected()
                    .and_then(|s| agents.iter().position(|i| *i == s))
                    .unwrap_or(0) as i32;
                let next = agents[(cur + delta).clamp(0, agents.len() as i32 - 1) as usize];
                if let Row::Agent { id, .. } = &self.rows[next]
                    && self.selected.as_ref() != Some(id)
                {
                    let id = id.clone();
                    self.selected = Some(id.clone());
                    self.fleet_state.select(Some(next));
                    // The details pane fades in for the new agent.
                    let fade = fx::fade_from_fg(c(self.pal.bg), (280, Interpolation::SineOut));
                    self.fx
                        .add_unique_effect("details", fade.with_area(self.areas.details));
                    self.dirty.insert(id);
                }
            }
        }
    }

    fn cycle_critters(&mut self) {
        self.critter_rows = match self.critter_rows {
            None => Some(Size::Small),
            Some(Size::Small) => Some(Size::Octant),
            Some(_) => None,
        };
        let label = match self.critter_rows {
            None => "critters off",
            Some(Size::Small) => "small critters",
            Some(_) => "octant critters (needs a Unicode 16 font)",
        };
        self.flash(label.into(), false);
    }

    /// Puts the Inbox cursor on the oldest request still waiting, else the newest item.
    fn select_waiting(&mut self) {
        let pick = self
            .inbox
            .iter()
            .position(|i| i.waiting)
            .or(self.inbox.len().checked_sub(1));
        self.inbox_state.select(pick);
    }

    fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        if self.grid.typing {
            self.stop_typing();
        }
        if tab == Tab::Inbox && self.inbox_state.selected().is_none() {
            self.select_waiting();
        }
        if tab == Tab::Comms && self.comms_state.selected().is_none() {
            let n = self.entries().len();
            self.comms_state.select(n.checked_sub(1));
        }
        let forward = Tab::ALL.iter().position(|t| *t == tab) > Tab::ALL.iter().position(|t| *t == self.tab);
        self.tab = tab;
        let motion = if forward {
            Motion::LeftToRight
        } else {
            Motion::RightToLeft
        };
        let sweep = fx::sweep_in(motion, 14, 0, c(self.pal.bg), (380, Interpolation::QuadOut));
        self.fx
            .add_unique_effect("body", sweep.with_area(self.areas.body));
        self.dirty_visible();
    }

    fn cycle_tab(&mut self, back: bool) {
        let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let n = Tab::ALL.len();
        self.set_tab(Tab::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }]);
    }

    // ---- terminals -------------------------------------------------------

    /// Agents shown in the Terminals view, in rig and seat order.
    pub fn grid_ids(&self) -> Vec<String> {
        let mut v: Vec<&Agent> = self
            .agents
            .values()
            .filter(|a| a.terminal && a.state.is_live())
            .filter(|a| {
                self.grid
                    .rig
                    .as_deref()
                    .is_none_or(|r| a.rig.as_deref() == Some(r))
            })
            .collect();
        v.sort_by_key(|a| (a.rig.is_none(), a.rig.clone(), a.seat.clone(), a.started_at));
        v.into_iter().map(|a| a.id.clone()).collect()
    }

    fn shows_terminal(&self, id: &str) -> bool {
        match self.tab {
            Tab::Terminals => {
                self.areas.tiles.iter().any(|(_, t, _)| t == id) || self.grid.focus.as_deref() == Some(id)
            }
            Tab::Fleet => self.selected.as_deref() == Some(id),
            _ => false,
        }
    }

    fn dirty_visible(&mut self) {
        match self.tab {
            Tab::Terminals => {
                let ids = self.grid_ids();
                self.dirty.extend(ids);
            }
            Tab::Fleet => {
                if let Some(s) = self.selected.clone() {
                    self.dirty.insert(s);
                }
            }
            _ => {}
        }
    }

    /// Fetches screens that changed, at most ~15 times a second each.
    pub fn pump(&mut self) {
        let due: Vec<String> = self
            .dirty
            .iter()
            .filter(|id| {
                self.last_fetch
                    .get(*id)
                    .is_none_or(|t| t.elapsed() > Duration::from_millis(66))
            })
            .cloned()
            .collect();
        for id in due {
            self.dirty.remove(&id);
            self.last_fetch.insert(id.clone(), Instant::now());
            match &self.backend {
                Backend::Demo(demo) => {
                    let (rows, cols) = self.sizes.get(&id).copied().unwrap_or((24, 90));
                    if let Some(agent) = self.agents.get(&id) {
                        let ansi = demo.terminal(agent, self.tick, rows, cols);
                        let plain = demo.screen(agent, self.tick);
                        let _ = self.tx.send(Msg::Term {
                            id,
                            ansi,
                            size: (rows, cols),
                            rows: plain,
                        });
                    }
                }
                Backend::Daemon(_) => {
                    if let Some(f) = &self.fetch {
                        let _ = f.send(id);
                    }
                }
            }
        }
    }

    /// After a frame: ask each pane to take the size of its tile.
    pub fn sync_sizes(&mut self) {
        if self.tab != Tab::Terminals {
            return;
        }
        let tiles = self.areas.tiles.clone();
        for (_, id, size) in tiles {
            if size.0 < 2 || size.1 < 10 || self.sizes.get(&id) == Some(&size) {
                continue;
            }
            self.sizes.insert(id.clone(), size);
            self.dirty.insert(id.clone());
            if let Backend::Daemon(socket) = &self.backend {
                backend::request(
                    socket,
                    Request::Resize {
                        agent: id,
                        rows: size.0,
                        cols: size.1,
                    },
                    &self.tx,
                    String::new(),
                    Duration::from_secs(3),
                );
            }
        }
    }

    fn move_focus(&mut self, dx: i32, dy: i32) {
        let ids = self.grid_ids();
        if ids.is_empty() {
            return;
        }
        let cols = self.areas.tiles.len().max(1);
        let cols = term::grid_cols(ids.len(), self.areas.body, self.grid.cols).min(cols);
        let cur = self
            .grid
            .focus
            .as_ref()
            .and_then(|f| ids.iter().position(|i| i == f))
            .unwrap_or(0) as i32;
        let next = (cur + dx + dy * cols as i32).clamp(0, ids.len() as i32 - 1) as usize;
        self.grid.focus = Some(ids[next].clone());
    }

    /// Ctrl+O: talk to the operator, OpenRig's kernel agent that picks and
    /// launches teams. Starts it (here) when it is not running.
    fn operator(&mut self) {
        let running = self
            .agents
            .values()
            .find(|a| a.rig.as_deref() == Some("operator") && a.state.is_live())
            .map(|a| a.id.clone());
        match running {
            Some(id) => {
                self.set_tab(Tab::Terminals);
                self.grid.focus = Some(id);
                self.start_typing();
            }
            None => {
                let spec = UpSpec {
                    source: "operator".into(),
                    cwd: std::env::current_dir().ok(),
                    fresh: vec![],
                    name: None,
                    plan: false,
                };
                self.request(
                    Request::Up(spec),
                    "starting the operator; Ctrl+O again to talk to it".into(),
                );
            }
        }
    }

    fn start_typing(&mut self) {
        let Some(id) = self.grid.focus.clone() else { return };
        self.grid.zoom = true;
        self.grid.typing = true;
        self.request(
            Request::Interact {
                agent: id.clone(),
                on: true,
            },
            String::new(),
        );
        let name = self.agents.get(&id).map(|a| a.label()).unwrap_or_default();
        self.flash(format!("typing into {name} · Ctrl+] to leave"), false);
        self.dirty.insert(id);
    }

    fn stop_typing(&mut self) {
        self.grid.typing = false;
        if let Some(id) = self.grid.focus.clone() {
            self.request(Request::Interact { agent: id, on: false }, String::new());
        }
    }

    fn type_into(&mut self, bytes: Vec<u8>) {
        let Some(id) = self.grid.focus.clone() else { return };
        if let Backend::Daemon(socket) = &self.backend {
            let data = String::from_utf8_lossy(&bytes).into_owned();
            backend::request(
                socket,
                Request::Input {
                    agent: id.clone(),
                    data,
                },
                &self.tx,
                String::new(),
                Duration::from_secs(3),
            );
        }
        self.dirty.insert(id);
    }

    pub fn on_paste(&mut self, text: String) {
        if self.grid.typing {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.type_into(bytes);
        } else {
            let at = self.byte_at(self.cursor);
            self.input.insert_str(at, &text.replace(['\r', '\n'], " "));
            self.cursor += text.chars().count();
        }
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

    pub fn selected_entry(&self) -> Option<Entry> {
        self.comms_state
            .selected()
            .and_then(|i| self.entries().get(i).cloned())
    }

    // ---- talking to the backend ----------------------------------------

    /// Sends a request; the outcome comes back as a flash or a popup.
    fn request(&mut self, req: Request, ok: String) {
        match &mut self.backend {
            Backend::Demo(demo) => {
                let (events, outcome) = demo.handle(&req);
                for ev in events {
                    let _ = self.tx.send(Msg::Event(Box::new(ev)));
                }
                match outcome {
                    Ok(Some(text)) => {
                        self.popup = Some(Popup {
                            title: ok,
                            text,
                            scroll: 0,
                        })
                    }
                    Ok(None) if !ok.is_empty() => self.flash(ok, false),
                    Ok(None) => {}
                    Err(e) => self.flash(e, true),
                }
            }
            Backend::Daemon(socket) => {
                let long = matches!(
                    req,
                    Request::Up(_) | Request::Restore { .. } | Request::Down { .. } | Request::Discover
                );
                let timeout = Duration::from_secs(if long { 60 } else { 10 });
                backend::request(socket, req, &self.tx, ok, timeout);
            }
        }
    }

    fn selected_item(&self) -> Option<InboxItem> {
        self.inbox_state
            .selected()
            .and_then(|i| self.inbox.get(i))
            .cloned()
    }

    /// ^Y / ^N / ^A: answers the request on screen. Elsewhere it first opens
    /// the Inbox on the oldest waiting request, so you read before you allow.
    fn answer(&mut self, decision: Decision) {
        if self.tab == Tab::Inbox {
            let Some(item) = self.selected_item() else { return };
            if !item.waiting {
                self.flash("that one is not waiting on you".into(), true);
                return;
            }
            if item.kind == InboxKind::Question && decision != Decision::Deny {
                self.flash("type an option's number (or your answer) and Enter".into(), false);
                return;
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
            self.request(
                Request::Answer {
                    item: item.id,
                    decision,
                },
                format!("{verb} {name}'s {}", item.kind.label()),
            );
            return;
        }
        let pos = self
            .inbox
            .iter()
            .position(|i| i.waiting && Some(&i.agent) == self.selected.as_ref())
            .or_else(|| self.inbox.iter().position(|i| i.waiting));
        match pos {
            Some(p) => {
                self.inbox_state.select(Some(p));
                self.set_tab(Tab::Inbox);
            }
            None => self.flash("nothing is waiting on you".into(), false),
        }
    }

    fn find_agent(&self, key: &str) -> Option<&Agent> {
        let norm = |s: &str| s.to_lowercase().replace([' ', '-', '_'], "");
        let k = norm(key.trim_start_matches('@'));
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

    /// The rig commands act on: the one named, else the selected agent's,
    /// else the only one.
    fn rig_arg(&self, given: Option<&str>) -> Option<String> {
        given
            .map(str::to_string)
            .or_else(|| self.grid.rig.clone().filter(|_| self.tab == Tab::Terminals))
            .or_else(|| self.selected_agent().and_then(|a| a.rig.clone()))
            .or_else(|| {
                (self.rigs.len() == 1)
                    .then(|| self.rigs.keys().next().cloned())
                    .flatten()
            })
    }

    fn submit(&mut self) {
        let line = std::mem::take(&mut self.input);
        self.cursor = 0;
        let line = line.trim().to_string();
        if line.is_empty() {
            if self.tab == Tab::Terminals {
                self.start_typing();
            }
            return;
        }
        if let Some(cmd) = line.strip_prefix('/') {
            self.command(cmd);
            return;
        }
        // A waiting question in the Inbox takes the line as its answer.
        if self.tab == Tab::Inbox
            && let Some(item) = self
                .selected_item()
                .filter(|i| i.waiting && i.kind == InboxKind::Question)
        {
            self.request(
                Request::AnswerQuestion {
                    item: item.id,
                    answer: line,
                },
                "answered".into(),
            );
            return;
        }
        // In Comms, a line answers the selected message.
        if self.tab == Tab::Comms && !line.starts_with('@') {
            match self.selected_entry() {
                Some(Entry::Message(m)) => {
                    self.request(
                        Request::Message {
                            from: None,
                            to: vec![],
                            rig: None,
                            pod: None,
                            body: line,
                            reply_to: Some(m.id),
                            force: false,
                        },
                        format!("replied to #{}", m.id),
                    );
                }
                Some(Entry::Chat(c)) => {
                    self.request(
                        Request::ChatSend {
                            rig: c.rig.clone(),
                            topic: Some(c.topic.clone()),
                            sender: None,
                            body: line,
                        },
                        "posted".into(),
                    );
                }
                None => self.flash("select a message to answer, or start with @Name".into(), true),
            }
            return;
        }
        // @Name message, or a message for the selected agent / focused tile.
        let (target, text) = match line.strip_prefix('@') {
            Some(rest) => {
                let (name, text) = rest.split_once(' ').unwrap_or((rest, ""));
                (self.find_agent(name).cloned(), text.trim().to_string())
            }
            None => {
                let id = if self.tab == Tab::Terminals {
                    self.grid.focus.clone()
                } else {
                    self.selected.clone()
                };
                (id.and_then(|i| self.agents.get(&i).cloned()), line.clone())
            }
        };
        match target {
            Some(a) if !text.is_empty() => {
                // From you, into its terminal and its mailbox.
                self.request(
                    Request::Message {
                        from: None,
                        to: vec![a.id.clone()],
                        rig: None,
                        pod: None,
                        body: text,
                        reply_to: None,
                        force: true,
                    },
                    format!("sent to {}", a.label()),
                );
            }
            Some(_) => self.flash("nothing to send".into(), true),
            None => self.flash("no such agent".into(), true),
        }
    }

    /// Slash commands: the operator console.
    fn command(&mut self, cmd: &str) {
        let words: Vec<&str> = cmd.split_whitespace().collect();
        let arg = |i: usize| words.get(i).copied();
        let rest = |i: usize| words.get(i..).map(|w| w.join(" ")).unwrap_or_default();
        match arg(0).unwrap_or("") {
            "help" | "h" | "?" => self.help = true,
            "quit" | "q" => self.quit(),
            "crit" | "critters" => self.cycle_critters(),
            "allow" | "y" => self.answer(Decision::Allow),
            "always" | "a" => self.answer(Decision::Always),
            "deny" | "n" => self.answer(Decision::Deny),
            "answer" => {
                if let Some(item) = self.selected_item() {
                    self.request(
                        Request::AnswerQuestion {
                            item: item.id,
                            answer: rest(1),
                        },
                        "answered".into(),
                    );
                }
            }
            "view" | "terminal" | "terminals" => {
                self.open_view(arg(1).unwrap_or("terminals"));
            }
            "cols" => self.grid.cols = arg(1).and_then(|c| c.parse().ok()).filter(|c: &u16| *c > 0),
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
                self.request(Request::Spawn(spec), "spawned".into());
            }
            "close" | "kill" => {
                let target = arg(1)
                    .and_then(|k| self.find_agent(k))
                    .or(self.selected_agent())
                    .map(|a| (a.id.clone(), a.label()));
                if let Some((id, name)) = target {
                    self.request(Request::Close { agent: id }, format!("closing {name}"));
                }
            }
            // Rigs.
            "up" | "plan" => {
                let Some(source) = arg(1) else {
                    return self.flash("/up <spec file | library name | rig>".into(), true);
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
                self.request(
                    Request::Up(spec),
                    if plan {
                        format!("plan for {source}")
                    } else {
                        format!("launching {source}")
                    },
                );
            }
            "down" => match self.rig_arg(arg(1)) {
                Some(rig) => self.request(
                    Request::Down {
                        rig: rig.clone(),
                        snapshot: true,
                        delete: false,
                    },
                    format!("{rig} is down (snapshot taken)"),
                ),
                None => self.flash("/down <rig>".into(), true),
            },
            "restore" => match self.rig_arg(arg(1)) {
                Some(rig) => self.request(
                    Request::Restore {
                        rig: rig.clone(),
                        snapshot: None,
                        fresh: vec![],
                    },
                    format!("restoring {rig}"),
                ),
                None => self.flash("/restore <rig>".into(), true),
            },
            "snapshot" => match self.rig_arg(arg(1)) {
                Some(rig) => self.request(
                    Request::SnapshotCreate {
                        rig: rig.clone(),
                        label: None,
                    },
                    format!("snapshot of {rig}"),
                ),
                None => self.flash("/snapshot <rig>".into(), true),
            },
            "snapshots" => match self.rig_arg(arg(1)) {
                Some(rig) => self.request(
                    Request::SnapshotList { rig: rig.clone() },
                    format!("snapshots of {rig}"),
                ),
                None => self.flash("/snapshots <rig>".into(), true),
            },
            "ps" | "rigs" => self.request(Request::Ps, "rigs".into()),
            "specs" | "library" => self.request(Request::Specs, "spec library".into()),
            "spec" | "yaml" => match arg(1) {
                Some(n) => self.request(Request::SpecShow { name: n.into() }, format!("spec {n}")),
                None => self.flash("/spec <name>".into(), true),
            },
            "export" => match self.rig_arg(arg(1)) {
                Some(rig) => self.request(Request::Export { rig: rig.clone() }, format!("{rig} as RigSpec")),
                None => self.flash("/export <rig>".into(), true),
            },
            "grow" | "add" => {
                // /grow <pod> <member> [runtime] [role…]   (in the selected rig)
                let (Some(pod), Some(member)) = (arg(1), arg(2)) else {
                    return self.flash("/grow <pod> <member> [runtime] [ROLE]".into(), true);
                };
                let Some(rig) = self.rig_arg(None) else {
                    return self.flash("select an agent in the rig first".into(), true);
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
                self.request(
                    Request::Grow {
                        rig: rig.clone(),
                        seats: vec![seat],
                        edges: vec![],
                    },
                    format!("growing {rig}"),
                );
            }
            "shrink" => match (self.rig_arg(None), arg(1)) {
                (Some(rig), Some(pod)) => self.request(
                    Request::Shrink { rig, pod: pod.into() },
                    format!("removed pod {pod}"),
                ),
                _ => self.flash("/shrink <pod>".into(), true),
            },
            "remove" => match arg(1) {
                Some(seat) => self.request(
                    Request::RemoveSeat { seat: seat.into() },
                    format!("removed {seat}"),
                ),
                None => self.flash("/remove <seat>".into(), true),
            },
            "launch" | "relaunch" => match arg(1)
                .map(str::to_string)
                .or_else(|| self.selected_agent().and_then(|a| a.seat.clone()))
            {
                Some(seat) => self.request(
                    Request::Launch {
                        seat: seat.clone(),
                        fresh: arg(0) == Some("relaunch"),
                    },
                    format!("launching {seat}"),
                ),
                None => self.flash("/launch <seat>".into(), true),
            },
            "continue" => match arg(1)
                .map(str::to_string)
                .or_else(|| self.selected_agent().and_then(|a| a.seat.clone()))
            {
                Some(seat) => self.request(
                    Request::SeatContinue { seat: seat.clone() },
                    format!("nudged {seat}"),
                ),
                None => self.flash("/continue <seat>".into(), true),
            },
            "guard" => match (arg(1), arg(2)) {
                (Some(seat), Some(on)) => self.request(
                    Request::TypingGuard {
                        seat: seat.into(),
                        on: on == "on",
                    },
                    format!("typing guard {on} for {seat}"),
                ),
                _ => self.flash("/guard <seat> on|off".into(), true),
            },
            "discover" => self.request(Request::Discover, "agents outside ainet".into()),
            "adopt" => match (arg(1), arg(2), arg(3), arg(4)) {
                (Some(id), Some(rig), Some(pod), Some(member)) => {
                    let seat = SeatSpec {
                        pod: pod.into(),
                        member: member.into(),
                        ..Default::default()
                    };
                    self.request(
                        Request::Adopt {
                            id: id.into(),
                            rig: rig.into(),
                            seat,
                        },
                        format!("adopted {id}"),
                    );
                }
                _ => self.flash("/adopt <id> <rig> <pod> <member>".into(), true),
            },
            // Talking.
            "broadcast" => match self.rig_arg(None) {
                Some(rig) => self.request(
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
                None => self.flash("select an agent in the rig first".into(), true),
            },
            "chat" => match self.rig_arg(None) {
                Some(rig) => self.request(
                    Request::ChatSend {
                        rig: rig.clone(),
                        topic: None,
                        sender: None,
                        body: rest(1),
                    },
                    format!("posted in {rig}"),
                ),
                None => self.flash("select an agent in the rig first".into(), true),
            },
            "queue" | "task" => {
                // /queue <destination> <summary…>
                let Some(dest) = arg(1) else {
                    return self.flash("/queue <seat> <summary>".into(), true);
                };
                let spec = QSpec {
                    destination: dest.into(),
                    summary: rest(2),
                    nudge: true,
                    ..Default::default()
                };
                self.request(Request::QueueCreate(spec), format!("queued for {dest}"));
            }
            "watchdog" => match (arg(1), arg(2).and_then(|m| m.parse::<u64>().ok())) {
                (Some(seat), Some(min)) => self.request(
                    Request::WatchdogRegister {
                        target: seat.into(),
                        interval_secs: min * 60,
                        message: None,
                        registered_by: None,
                    },
                    format!("waking {seat} every {min} min"),
                ),
                _ => self.flash("/watchdog <seat> <minutes>".into(), true),
            },
            "capture" => match arg(1).map(str::to_string).or_else(|| self.selected.clone()) {
                Some(t) => self.request(
                    Request::Capture {
                        targets: vec![t],
                        rig: None,
                        pod: None,
                        lines: 80,
                    },
                    "terminal".into(),
                ),
                None => self.flash("/capture <agent>".into(), true),
            },
            "whoami" => match self.selected.clone() {
                Some(id) => self.request(Request::Whoami { agent: id }, "who".into()),
                None => self.flash("select an agent".into(), true),
            },
            other => self.flash(format!("no command /{other}; /help lists them"), true),
        }
    }

    /// Tab after `@Mos` completes the name.
    fn complete(&mut self) -> bool {
        let Some(partial) = self.input.strip_prefix('@').filter(|p| !p.contains(' ')) else {
            return false;
        };
        let Some(a) = self.find_agent(partial) else {
            return false;
        };
        self.input = format!("@{} ", a.seat.clone().unwrap_or_else(|| a.name.replace(' ', "")));
        self.cursor = self.input.chars().count();
        true
    }

    // ---- keys and mouse -------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.grid.typing {
            if term::is_leave_key(&key) {
                self.stop_typing();
                self.flash("left the terminal".into(), false);
            } else if let Some(bytes) = term::encode_key(key) {
                self.type_into(bytes);
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.help {
            self.help = false;
            return;
        }
        if let Some(p) = &mut self.popup {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => p.scroll = p.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => p.scroll = p.scroll.saturating_sub(1),
                KeyCode::PageDown => p.scroll = p.scroll.saturating_add(10),
                KeyCode::PageUp => p.scroll = p.scroll.saturating_sub(10),
                _ => self.popup = None,
            }
            return;
        }
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => self.quit(),
            KeyCode::Char('y') if ctrl => self.answer(Decision::Allow),
            KeyCode::Char('n') if ctrl => self.answer(Decision::Deny),
            KeyCode::Char('a') if ctrl => self.answer(Decision::Always),
            KeyCode::Char('t') if ctrl => self.set_tab(Tab::Map),
            KeyCode::Char('o') if ctrl => self.operator(),
            KeyCode::Char('g') if ctrl => self.set_tab(Tab::Terminals),
            KeyCode::Char('e') if ctrl => self.cycle_critters(),
            KeyCode::Char('z') if ctrl && self.tab == Tab::Terminals => self.grid.zoom = !self.grid.zoom,
            KeyCode::Char('r') if ctrl && self.tab == Tab::Terminals => {
                // Cycle the rig filter: all → each rig → all.
                let names: Vec<String> = self.rigs.keys().cloned().collect();
                self.grid.rig = match &self.grid.rig {
                    None => names.first().cloned(),
                    Some(r) => names.iter().skip_while(|n| *n != r).nth(1).cloned(),
                };
                self.dirty_visible();
            }
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let chars: Vec<char> = self.input.chars().collect();
                let mut i = self.cursor;
                while i > 0 && chars[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && chars[i - 1] != ' ' {
                    i -= 1;
                }
                self.input = chars[..i].iter().chain(&chars[self.cursor..]).collect();
                self.cursor = i;
            }
            KeyCode::Char('?') if self.input.is_empty() => self.help = true,
            KeyCode::Char('+') if self.input.is_empty() && self.tab == Tab::Terminals => {
                let n = self.grid_ids().len().max(1) as u16;
                let cur = self
                    .grid
                    .cols
                    .unwrap_or_else(|| term::grid_cols(n as usize, self.areas.body, None) as u16);
                self.grid.cols = Some((cur + 1).min(n));
            }
            KeyCode::Char('-') if self.input.is_empty() && self.tab == Tab::Terminals => {
                let n = self.grid_ids().len().max(1);
                let cur = self
                    .grid
                    .cols
                    .unwrap_or_else(|| term::grid_cols(n, self.areas.body, None) as u16);
                self.grid.cols = Some(cur.saturating_sub(1).max(1));
            }
            KeyCode::Char('=') if self.input.is_empty() && self.tab == Tab::Terminals => {
                self.grid.cols = None
            }
            KeyCode::Char(ch) if !ctrl => {
                let byte = self.byte_at(self.cursor);
                self.input.insert(byte, ch);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let byte = self.byte_at(self.cursor);
                self.input.remove(byte);
            }
            KeyCode::Delete if self.cursor < self.input.chars().count() => {
                let byte = self.byte_at(self.cursor);
                self.input.remove(byte);
            }
            KeyCode::Left if self.tab == Tab::Terminals && self.input.is_empty() => self.move_focus(-1, 0),
            KeyCode::Right if self.tab == Tab::Terminals && self.input.is_empty() => self.move_focus(1, 0),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Up if self.tab == Tab::Terminals => self.move_focus(0, -1),
            KeyCode::Down if self.tab == Tab::Terminals => self.move_focus(0, 1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-5),
            KeyCode::PageDown => self.move_selection(5),
            KeyCode::Tab => {
                if !self.complete() {
                    self.cycle_tab(false);
                }
            }
            KeyCode::BackTab => self.cycle_tab(true),
            KeyCode::Esc => {
                if self.grid.zoom && self.input.is_empty() {
                    self.grid.zoom = false;
                }
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Enter => self.submit(),
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::Down(_) => {
                let at = (m.column, m.row).into();
                if let Some(tab) = self
                    .areas
                    .tabs
                    .iter()
                    .find(|(r, _)| r.contains(at))
                    .map(|(_, t)| *t)
                {
                    self.set_tab(tab);
                } else if let Some(id) = self
                    .areas
                    .tiles
                    .iter()
                    .find(|(r, _, _)| r.contains(at))
                    .map(|(_, id, _)| id.clone())
                {
                    if self.grid.focus.as_deref() == Some(&id) && !self.grid.typing {
                        self.start_typing();
                    } else {
                        if self.grid.typing {
                            self.stop_typing();
                        }
                        self.grid.focus = Some(id);
                    }
                }
            }
            _ => {}
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.input
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }
}

fn step(state: &mut ListState, n: usize, delta: i32) {
    if n == 0 {
        return;
    }
    let cur = state.selected().unwrap_or(n - 1) as i32;
    state.select(Some((cur + delta).clamp(0, n as i32 - 1) as usize));
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}
