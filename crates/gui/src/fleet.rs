//! The fleet as a GPUI entity: `ainet-model`'s [`Store`], kept current by a
//! [`Link`] to ainetd (or the demo), plus the notices a front end shows as
//! toasts (each one is also emitted as an event). Views read it and
//! re-render when it notifies; requests go out through it. It also keeps
//! the animation clock: a [`Tick`] every [`TICK`], which moves the demo on
//! and the critters of views that are showing some.
//!
//! Terminals on show register here: their screens are streamed from ainetd
//! (`Link::attach`: the screen, then what the agent prints, fed into the
//! store's copy), or, from a daemon that cannot stream and in the demo,
//! fetched when their agent prints something, at most every
//! `terminal::FETCH_EVERY` each (the TUI's pump); ainetd is told when you
//! are at one (`Interact`). A screen that changed is announced as
//! [`ScreenChanged`] for the terminals showing it to redraw: the rest of
//! the app does not re-render for terminal output.
//!
//! It also keeps the events log (what happened, from `ainet_model::journal`)
//! and routes the replies to requests a view asked for itself
//! ([`FleetModel::ask`]); requests whose reply is long text to read (a
//! transcript, a spec) are handed to the workspace as [`ReportWanted`], to
//! open as a tab rather than a toast.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use ainet_core::Request;
use ainet_core::client::Client;
use ainet_model::journal::{self, Happening};
use ainet_model::{Backend, Change, Command, Link, Listing, Msg, Pump, Store, Tally, daemon};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{App, AppContext as _, AsyncApp, Context, EntityId, EventEmitter, Subscription, Task, WeakEntity};

use crate::sink::{self, ChannelSink};

/// How often the demo fleet and the critters move on, as in the TUI.
pub const TICK: Duration = Duration::from_millis(120);

/// How long the keyboard must stay before ainetd hears which terminals you
/// are at: moving between two views of one agent, or through a pane on the
/// way to its tab, tells it nothing.
pub const INTERACT_SETTLE: Duration = Duration::from_millis(50);

/// How many notices are kept for later toasts and a log.
const KEEP_NOTICES: usize = 64;

/// How many lines the events log keeps; older ones make way.
pub const KEEP_EVENTS: usize = 500;

/// What hears the reply to a request asked with [`FleetModel::ask`]: its
/// text, or the error.
pub type OnReply = Box<dyn FnOnce(Result<String, String>, &mut App)>;

/// What hears the items of a request asked with [`FleetModel::list`].
pub type OnListing = Box<dyn FnOnce(Result<Listing, String>, &mut App)>;

/// A request whose reply is long text to read whole (a transcript, a
/// spec, a queue item): the workspace opens it as a report tab and asks
/// for it there.
#[derive(Clone, Debug)]
pub struct ReportWanted {
    pub request: Request,
    /// The tab's title: what the command says once done.
    pub title: String,
}

impl EventEmitter<ReportWanted> for FleetModel {}

/// Whether `req`'s reply is long text, to read in a tab of its own rather
/// than a toast.
pub fn is_report(req: &Request) -> bool {
    matches!(
        req,
        Request::Transcript { .. }
            | Request::SpecShow { .. }
            | Request::Export { .. }
            | Request::QueueShow { .. }
    )
}

/// Where the connection to the fleet stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    /// The pretend fleet; nothing is real.
    Demo,
    /// Looking for the console binary and starting ainetd.
    Starting,
    Connected,
    /// Following the socket, but ainetd is not answering; it retries.
    Offline,
    /// ainetd could not be started; why. The socket is still followed, so a
    /// daemon started by hand is picked up.
    Failed(String),
    /// ainetd is running but does not speak our protocol; why. The console
    /// refuses such a daemon, so this stays until it answers `Hello`.
    Incompatible(String),
}

/// Why ainetd is not usable, as found when starting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartProblem {
    /// It could not be started; a daemon that turns up later is fine.
    NotStarted(String),
    /// One is running, but it refused our `Hello`.
    Incompatible(String),
}

/// Something the user should hear about: a daemon flash, a report, a change
/// in the connection, a request that waits on you.
#[derive(Clone, Debug, Default)]
pub struct Notice {
    /// For reports (`ainet ps`-like replies): their title.
    pub title: Option<String>,
    pub text: String,
    pub error: bool,
    /// For a new inbox item that asks for you: which.
    pub attention: Option<Attention>,
}

/// An inbox item asking for you, as a toast points at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attention {
    /// The agent asking (its id).
    pub agent: String,
    /// The inbox item's id.
    pub item: u64,
}

impl EventEmitter<Notice> for FleetModel {}

/// The animation clock moved on ([`FleetModel::tick`] says where to).
/// Emitted without re-rendering anything; views with critters redraw if
/// they show some.
#[derive(Clone, Copy, Debug)]
pub struct Tick;

impl EventEmitter<Tick> for FleetModel {}

/// An agent's screen (or its scrollback) changed: the terminals showing
/// it redraw. The fleet itself does not notify for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenChanged(pub String);

impl EventEmitter<ScreenChanged> for FleetModel {}

pub struct FleetModel {
    pub store: Store,
    link: Option<Link<ChannelSink>>,
    demo: bool,
    /// The socket followed, when not the demo.
    socket: Option<PathBuf>,
    /// Why ainetd is not usable: a failed start until it connects anyway,
    /// an incompatible daemon until it answers `Hello`.
    problem: Option<StartProblem>,
    /// Whether a `Hello` is on its way to an incompatible daemon.
    rechecking: bool,
    tick: u64,
    /// How many snapshots were applied: everything anew each time.
    snapshots: u64,
    notices: VecDeque<Notice>,
    /// Inbox items that asked for you so far; the waiting badge glows
    /// anew for each.
    alerts: usize,
    /// The terminals on show: the agent each item (by entity) shows.
    shown: HashMap<EntityId, String>,
    /// Which shown screens are stale, and the sizes asked of their panes.
    pump: Pump,
    /// The shown terminals streamed from ainetd, with their streams'
    /// numbers; the pump leaves them alone.
    streams: HashMap<String, u64>,
    /// The next round of fetches, while some screen waits for its turn.
    pump_task: Option<Task<()>>,
    /// The items that have the keyboard, per agent, and the agents ainetd
    /// was told you are at; told after the focus settles.
    at: HashMap<String, HashSet<EntityId>>,
    told: HashSet<String>,
    telling: Option<Task<()>>,
    /// Who hears the replies on their way, by token.
    asked: HashMap<u64, OnReply>,
    /// Who hears the listings on their way, by token.
    listed: HashMap<u64, OnListing>,
    next_token: u64,
    /// What happened, the latest last, at most [`KEEP_EVENTS`].
    events: VecDeque<Rc<Happening>>,
    /// How many events were ever logged, so a view can tell how many
    /// made way.
    events_logged: usize,
    /// Every request sent, quiet ones and screen fetches with scrollback
    /// included, for tests.
    #[cfg(test)]
    pub sent: Vec<(Request, String)>,
    /// Every screen fetched, for tests.
    #[cfg(test)]
    pub fetched: Vec<(String, (u16, u16))>,
    /// Pretend ainetd streams terminals, for tests: attaching gives these
    /// numbers, and every attach and detach is recorded.
    #[cfg(test)]
    pub streamable: bool,
    #[cfg(test)]
    pub attached: Vec<String>,
    #[cfg(test)]
    pub detached: Vec<String>,
    _tasks: Vec<Task<()>>,
    _quit: Option<Subscription>,
}

impl FleetModel {
    /// The pretend fleet, moving on every [`TICK`].
    pub fn demo(cx: &mut Context<Self>) -> Self {
        let (sink, rx) = sink::channel();
        // The full demo: the app asks it for more than the console does.
        let (link, snapshot) = Link::new(Backend::Demo(Box::new(ainet_model::Demo::full())), sink);
        let mut model = FleetModel::empty(true);
        model.link = Some(link);
        if let Some(snapshot) = snapshot {
            model.store.connected = true;
            model.store.apply(Msg::Snapshot(Box::new(snapshot)));
            model.snapshots = 1;
        }
        model._tasks.push(drain(rx, cx));
        model
            ._tasks
            .push(cx.spawn(async move |this, cx| ticker(this, cx).await));
        model
    }

    /// The animation clock: ticks so far.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// How many snapshots were applied (ainetd's arrives after
    /// connecting, and again on each reconnect), so views can tell what
    /// a snapshot brought from what happened since.
    pub fn snapshots(&self) -> u64 {
        self.snapshots
    }

    /// ainetd at `socket`: starts it through the console binary (off the UI
    /// thread), then follows it. If it cannot be started, the reason shows
    /// in the status bar and on stderr, and the socket is followed anyway.
    pub fn daemon(socket: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut model = FleetModel::empty(false);
        model
            ._tasks
            .push(cx.spawn(async move |this, cx| ticker(this, cx).await));
        let start = cx.background_spawn({
            let socket = socket.clone();
            async move { start_daemon(&socket) }
        });
        model._tasks.push(cx.spawn(async move |this, cx| {
            let outcome = start.await;
            let _ = this.update(cx, |model, cx| model.follow(socket, outcome, cx));
        }));
        model._quit = Some(cx.on_app_quit(Self::leave_terminals));
        model
    }

    /// Not connected to anything, for tests.
    #[cfg(test)]
    pub fn offline() -> Self {
        FleetModel::empty(false)
    }

    fn empty(demo: bool) -> Self {
        FleetModel {
            store: Store::new(),
            link: None,
            demo,
            socket: None,
            problem: None,
            rechecking: false,
            tick: 0,
            snapshots: 0,
            notices: VecDeque::new(),
            alerts: 0,
            shown: HashMap::new(),
            pump: Pump::new(),
            streams: HashMap::new(),
            pump_task: None,
            at: HashMap::new(),
            told: HashSet::new(),
            telling: None,
            asked: HashMap::new(),
            listed: HashMap::new(),
            next_token: 0,
            events: VecDeque::new(),
            events_logged: 0,
            #[cfg(test)]
            sent: Vec::new(),
            #[cfg(test)]
            fetched: Vec::new(),
            #[cfg(test)]
            streamable: false,
            #[cfg(test)]
            attached: Vec::new(),
            #[cfg(test)]
            detached: Vec::new(),
            _tasks: Vec::new(),
            _quit: None,
        }
    }

    fn follow(&mut self, socket: PathBuf, started: Result<(), StartProblem>, cx: &mut Context<Self>) {
        if let Err(problem) = started {
            let (StartProblem::NotStarted(text) | StartProblem::Incompatible(text)) = &problem;
            eprintln!("ainet-gui: {text}");
            self.problem = Some(problem);
        }
        self.socket = Some(socket.clone());
        let (sink, rx) = sink::channel();
        let (link, _) = Link::new(Backend::Daemon(socket), sink);
        self.link = Some(link);
        self._tasks.push(drain(rx, cx));
        cx.notify();
    }

    pub fn connection(&self) -> Connection {
        if self.demo {
            Connection::Demo
        } else if let Some(StartProblem::Incompatible(problem)) = &self.problem {
            Connection::Incompatible(problem.clone())
        } else if self.store.connected {
            Connection::Connected
        } else if let Some(StartProblem::NotStarted(problem)) = &self.problem {
            Connection::Failed(problem.clone())
        } else if self.link.is_none() {
            Connection::Starting
        } else {
            Connection::Offline
        }
    }

    pub fn tally(&self) -> Tally {
        self.store.tally()
    }

    /// How many inbox items have asked for you, for the waiting badge's
    /// glow.
    pub fn alerts(&self) -> usize {
        self.alerts
    }

    /// The notices so far, the latest last.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn notices(&self) -> impl Iterator<Item = &Notice> {
        self.notices.iter()
    }

    /// The events log, the latest last.
    pub fn events(&self) -> &VecDeque<Rc<Happening>> {
        &self.events
    }

    /// How many events were ever logged (the log keeps the last
    /// [`KEEP_EVENTS`]).
    pub fn events_logged(&self) -> usize {
        self.events_logged
    }

    fn log(&mut self, happening: Happening) {
        if self.events.len() == KEEP_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(Rc::new(happening));
        self.events_logged += 1;
    }

    /// Logs `happening` as if the fleet had said so, for tests.
    #[cfg(test)]
    pub fn log_for_test(&mut self, happening: Happening, cx: &mut Context<Self>) {
        self.log(happening);
        cx.notify();
    }

    /// Selects an agent in the Fleet.
    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.store.agents.contains_key(id) && self.store.selected.as_deref() != Some(id) {
            self.store.selected = Some(id.to_string());
            cx.notify();
        }
    }

    /// Moves the Fleet selection `delta` agents down (up if negative),
    /// over the rows `shown` keeps (the Fleet panel folds rigs away).
    pub fn move_selection_where(
        &mut self,
        delta: i32,
        shown: impl Fn(usize) -> bool,
        cx: &mut Context<Self>,
    ) {
        if self.store.move_selection_where(delta, shown).is_some() {
            cx.notify();
        }
    }

    /// Sends a request; `ok` is said once it is done (or titles its
    /// report). Without a link yet, says that ainetd is not there.
    pub fn request(&mut self, req: Request, ok: String, cx: &mut Context<Self>) {
        #[cfg(test)]
        self.sent.push((req.clone(), ok.clone()));
        match &mut self.link {
            Some(link) => link.request(req, ok),
            None => self.show("ainetd is not running yet".into(), true, cx),
        }
    }

    /// Carries out a command of `ainet-model`'s command layer: sends its
    /// request, or shows why it cannot. The front-end-only commands are
    /// the caller's.
    pub fn run(&mut self, command: Command, cx: &mut Context<Self>) {
        match command {
            // Long text to read: the workspace opens a tab and asks there.
            Command::Request(req, title) if is_report(&req) => cx.emit(ReportWanted { request: *req, title }),
            Command::Request(req, ok) => self.request(*req, ok, cx),
            Command::Error(why) => self.show(why, true, cx),
            _ => {}
        }
    }

    /// Sends a request whose reply `on_reply` hears, whole: the text of a
    /// report, empty for a plain "done", or the error. Nothing else hears
    /// it (no toast).
    pub fn ask(
        &mut self,
        req: Request,
        on_reply: impl FnOnce(Result<String, String>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        self.sent.push((req.clone(), String::new()));
        let Some(link) = &mut self.link else {
            // Later, as a reply would come: the asker may be mid-update.
            return cx.defer(move |cx| on_reply(Err("ainetd is not running yet".into()), cx));
        };
        self.next_token += 1;
        self.asked.insert(self.next_token, Box::new(on_reply));
        link.ask(req, self.next_token);
    }

    /// Sends a request that lists things to pick from (the spec library, a
    /// rig's snapshots); `on_listing` hears the items, or the error.
    /// Nothing else hears it.
    pub fn list(
        &mut self,
        req: Request,
        on_listing: impl FnOnce(Result<Listing, String>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        self.sent.push((req.clone(), String::new()));
        let Some(link) = &mut self.link else {
            return cx.defer(move |cx| on_listing(Err("ainetd is not running yet".into()), cx));
        };
        self.next_token += 1;
        self.listed.insert(self.next_token, Box::new(on_listing));
        link.list(req, self.next_token);
    }

    /// A request only ainetd acts on (keystrokes, resizes), sent quietly.
    pub fn quiet(&mut self, req: Request) {
        #[cfg(test)]
        self.sent.push((req.clone(), String::new()));
        if let Some(link) = &self.link {
            link.quiet(req);
        }
    }

    /// Item `item` shows agent `agent`'s terminal: it is streamed, or its
    /// screen is fetched now and whenever the agent prints something.
    pub fn show_terminal(&mut self, item: EntityId, agent: &str, cx: &mut Context<Self>) {
        self.shown.insert(item, agent.to_string());
        self.stream_or_fetch(agent, cx);
    }

    /// Item `item` no longer shows a terminal (hidden, closed or gone);
    /// one no item shows any more stops streaming.
    pub fn hide_terminal(&mut self, item: EntityId) {
        let Some(agent) = self.shown.remove(&item) else {
            return;
        };
        if !self.is_shown(&agent) && self.streams.remove(&agent).is_some() {
            #[cfg(test)]
            self.detached.push(agent.clone());
            if let Some(link) = &mut self.link {
                link.detach(&agent);
            }
        }
    }

    /// Whether `agent`'s terminal is streamed rather than fetched.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_streamed(&self, agent: &str) -> bool {
        self.streams.contains_key(agent)
    }

    /// Streams `agent`'s terminal if ainetd can and it is not already;
    /// otherwise fetches its screen.
    fn stream_or_fetch(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.streams.contains_key(agent) {
            return;
        }
        match self.attach(agent) {
            Some(stream) => {
                self.streams.insert(agent.to_string(), stream);
            }
            None => self.refresh_terminal(agent, cx),
        }
    }

    fn attach(&mut self, agent: &str) -> Option<u64> {
        #[cfg(test)]
        if self.streamable {
            self.attached.push(agent.to_string());
            return Some(self.attached.len() as u64);
        }
        self.link.as_mut()?.attach(agent)
    }

    /// Whether some item shows `agent`'s terminal.
    pub fn is_shown(&self, agent: &str) -> bool {
        self.shown.values().any(|a| a == agent)
    }

    /// `agent`'s screen is stale: fetch it as soon as its turn comes
    /// (streamed screens never are).
    pub fn refresh_terminal(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.pump.mark(agent);
        self.pump(cx);
    }

    /// `agent`'s terminal was laid out at `size` (rows, cols): asks its
    /// pane to take that size, once per change, and fetches the screen.
    pub fn resize_terminal(&mut self, agent: &str, size: (u16, u16), cx: &mut Context<Self>) {
        if self.pump.resize(agent, size) {
            self.quiet(Request::Resize {
                agent: agent.to_string(),
                rows: size.0,
                cols: size.1,
            });
            self.pump(cx);
        }
    }

    /// The size `agent`'s pane was asked to take.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn terminal_size(&self, agent: &str) -> (u16, u16) {
        self.pump.size(agent)
    }

    /// Types `bytes` into `agent`'s terminal; its screen is fetched after.
    pub fn input(&mut self, agent: &str, bytes: &[u8], cx: &mut Context<Self>) {
        self.quiet(Request::Input {
            agent: agent.to_string(),
            data: String::from_utf8_lossy(bytes).into_owned(),
        });
        self.refresh_terminal(agent, cx);
    }

    /// Asks for up to `lines` rows of `agent`'s scrollback; they land in
    /// the store's `history`.
    pub fn fetch_history(&mut self, agent: &str, lines: usize) {
        #[cfg(test)]
        self.sent.push((
            Request::Screen {
                agent: agent.to_string(),
                formatted: false,
                history: lines,
            },
            String::new(),
        ));
        if let (Some(link), Some(a)) = (&self.link, self.store.agents.get(agent)) {
            link.history(a, self.tick, lines);
        }
    }

    /// Item `item`, showing `agent`'s terminal, got or lost the keyboard.
    /// ainetd holds messages for an agent while you are at its terminal;
    /// it is told once the focus settles ([`INTERACT_SETTLE`]).
    pub fn set_at_terminal(&mut self, item: EntityId, agent: &str, on: bool, cx: &mut Context<Self>) {
        let items = self.at.entry(agent.to_string()).or_default();
        if on {
            items.insert(item);
        } else {
            items.remove(&item);
        }
        if self.telling.is_none() {
            self.telling = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(INTERACT_SETTLE).await;
                let _ = this.update(cx, |fleet, cx| fleet.tell_interact(cx));
            }));
        }
    }

    /// Tells ainetd which terminals you are at, where that changed.
    fn tell_interact(&mut self, cx: &mut Context<Self>) {
        self.telling = None;
        self.at.retain(|_, items| !items.is_empty());
        let mut agents: Vec<String> = self.at.keys().chain(&self.told).cloned().collect();
        agents.sort();
        agents.dedup();
        for agent in agents {
            let on = self.at.contains_key(&agent);
            if on != self.told.contains(&agent) {
                if on {
                    self.told.insert(agent.clone());
                } else {
                    self.told.remove(&agent);
                }
                self.request(Request::Interact { agent, on }, String::new(), cx);
            }
        }
    }

    /// On quitting: ainetd stops holding messages for the terminals you
    /// were at, before the app is gone (the usual request would go out on
    /// a thread the exit cuts short).
    fn leave_terminals(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.at.clear();
        let agents: Vec<String> = self.told.drain().collect();
        let Some(socket) = self.socket.clone().filter(|_| !agents.is_empty()) else {
            return Task::ready(());
        };
        cx.background_spawn(async move {
            for agent in agents {
                let _ = Client::connect(&socket).and_then(|mut client| {
                    client.set_timeout(Some(Duration::from_millis(500)))?;
                    client.call(&Request::Interact { agent, on: false })
                });
            }
        })
    }

    /// Fetches the stale screens whose turn it is, and comes back when the
    /// next one's turn comes.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.pump_task.is_some() {
            return;
        }
        let now = cx.background_executor().now();
        for (id, size) in self.pump.due(now) {
            // A streamed screen comes as it changes (and anew at a new
            // size).
            if self.streams.contains_key(&id) {
                continue;
            }
            #[cfg(test)]
            self.fetched.push((id.clone(), size));
            if let (Some(link), Some(agent)) = (&self.link, self.store.agents.get(&id)) {
                link.fetch(agent, self.tick, size);
            }
        }
        if let Some(wait) = self.pump.wait(now) {
            self.pump_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |fleet, cx| {
                    fleet.pump_task = None;
                    fleet.pump(cx);
                });
            }));
        }
    }

    /// Shows `text` in the status bar for a while.
    pub fn show(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.notice(None, text, error, cx);
        cx.notify();
    }

    /// Applies messages from the link, then re-renders once if anything
    /// changed. Terminal output re-renders nothing: the screens that
    /// changed are announced ([`ScreenChanged`]) for their terminals alone.
    fn apply(&mut self, msgs: Vec<Msg>, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut screens: Vec<String> = Vec::new();
        for msg in msgs {
            // Read before applying: a change says what it was before.
            if let Some(happening) = journal::happened(&self.store, &msg) {
                self.log(happening);
                changed = true;
            }
            let change = self.store.apply(msg);
            changed |= match &change {
                Change::None
                | Change::Output(_)
                | Change::Term(_)
                | Change::History(_)
                | Change::Detached { .. } => false,
                Change::Flash(text, _) => !text.is_empty(),
                _ => true,
            };
            match change {
                // A quiet request (a key, a resize, `Interact`) done: the
                // link says so with no words, which is no news.
                Change::Flash(text, _) if text.is_empty() => {}
                Change::Flash(text, error) => self.notice(None, text, error, cx),
                Change::Popup(title, text) => self.notice(Some(title), text, false, cx),
                Change::Reply { token, reply } => {
                    if let Some(on_reply) = self.asked.remove(&token) {
                        on_reply(reply, cx);
                    }
                }
                Change::Listing { token, listing } => {
                    if let Some(on_listing) = self.listed.remove(&token) {
                        on_listing(listing, cx);
                    }
                }
                Change::Connected(up) => {
                    match self.problem {
                        Some(StartProblem::NotStarted(_)) if up => self.problem = None,
                        // The daemon may have been replaced by one we can
                        // talk to: ask it again.
                        Some(StartProblem::Incompatible(_)) if up => self.recheck(cx),
                        _ => {}
                    }
                    let text = if up {
                        "connected to ainetd"
                    } else {
                        "lost ainetd; retrying"
                    };
                    self.notice(None, text.into(), !up, cx);
                    // A daemon (re)started has forgotten which terminals
                    // you are at: tell it again.
                    if up {
                        self.told.clear();
                        self.tell_interact(cx);
                    }
                }
                // Everything anew (first connect or reconnect): the
                // terminals on show are fetched again, as the TUI's, or
                // streamed from a daemon that can.
                Change::Snapshot => {
                    self.snapshots += 1;
                    let shown: HashSet<String> = self.shown.values().cloned().collect();
                    for agent in shown {
                        if !self.streams.contains_key(&agent) {
                            self.stream_or_fetch(&agent, cx);
                        }
                    }
                }
                Change::Output(agent) if self.is_shown(&agent) && !self.streams.contains_key(&agent) => {
                    self.pump.mark(&agent)
                }
                Change::Term(agent) | Change::History(agent) => {
                    if !screens.contains(&agent) {
                        screens.push(agent);
                    }
                }
                // The stream ended (the daemon went, the program ended):
                // fetched from now on, until a snapshot tries again.
                Change::Detached { id, stream } => {
                    if self.streams.get(&id) == Some(&stream) {
                        self.streams.remove(&id);
                        if let Some(link) = &mut self.link {
                            link.ended(&id, stream);
                        }
                        if self.is_shown(&id) {
                            self.pump.mark(&id);
                        }
                    }
                }
                // A request waiting on you (or a question): a toast and a
                // glowing badge, as the TUI's flash and glowing Inbox tab.
                Change::InboxAdded { index, alert: true } => {
                    self.alerts += 1;
                    if let (Some(text), Some(item)) =
                        (self.store.inbox_alert(index), self.store.inbox.get(index))
                    {
                        let notice = Notice {
                            text,
                            attention: Some(Attention {
                                agent: item.agent.clone(),
                                item: item.id,
                            }),
                            ..Notice::default()
                        };
                        self.push_notice(notice, cx);
                    }
                }
                _ => {}
            }
        }
        self.pump(cx);
        for agent in screens {
            cx.emit(ScreenChanged(agent));
        }
        if changed {
            cx.notify();
        }
    }

    /// Applies `msgs` as if the link had sent them, for tests.
    #[cfg(test)]
    pub fn receive(&mut self, msgs: Vec<Msg>, cx: &mut Context<Self>) {
        self.apply(msgs, cx);
    }

    /// Says `Hello` to the daemon again, off the UI thread, and forgets
    /// that it was incompatible if it answers.
    fn recheck(&mut self, cx: &mut Context<Self>) {
        let Some(socket) = self.socket.clone().filter(|_| !self.rechecking) else {
            return;
        };
        self.rechecking = true;
        let hello = cx.background_spawn(async move { hello(&socket) });
        cx.spawn(async move |this, cx| {
            let answer = hello.await;
            let _ = this.update(cx, |model, cx| {
                model.rechecking = false;
                if answer.is_ok() {
                    model.problem = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn notice(&mut self, title: Option<String>, text: String, error: bool, cx: &mut Context<Self>) {
        let notice = Notice {
            title,
            text,
            error,
            attention: None,
        };
        self.push_notice(notice, cx);
    }

    fn push_notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        if self.notices.len() == KEEP_NOTICES {
            self.notices.pop_front();
        }
        cx.emit(notice.clone());
        self.notices.push_back(notice);
    }

    fn on_tick(&mut self, cx: &mut Context<Self>) {
        self.tick += 1;
        if let Some(link) = &mut self.link {
            link.tick(self.tick);
        }
        // The demo's terminals move with the clock, not on output.
        if self.demo && !self.shown.is_empty() {
            self.pump.mark_all(self.shown.values().cloned());
            self.pump(cx);
        }
        cx.emit(Tick);
    }
}

/// Finds the console binary and starts ainetd with it, unless one is
/// running already; either way checks that it speaks our protocol.
/// Blocking: runs on the background executor.
fn start_daemon(socket: &Path) -> Result<(), StartProblem> {
    if Client::connect(socket).is_ok() {
        return hello(socket);
    }
    let Some(ainet) = daemon::console_binary() else {
        return Err(StartProblem::NotStarted(
            "cannot start ainetd: the console binary `ainet` is not next to ainet-gui, on PATH or in $AINET_BIN"
                .into(),
        ));
    };
    daemon::ensure_daemon(socket, &ainet)
        .map_err(|e| StartProblem::NotStarted(format!("cannot start ainetd: {e:#}")))
}

/// Asks the running daemon whether it speaks our protocol, as the console
/// does before using it.
fn hello(socket: &Path) -> Result<(), StartProblem> {
    daemon::check_version(socket).map_err(|e| {
        StartProblem::Incompatible(format!("ainetd at {} is incompatible: {e:#}", socket.display()))
    })
}

/// Applies whatever the link sends, in batches, until the link is gone.
fn drain(mut rx: UnboundedReceiver<Msg>, cx: &mut Context<FleetModel>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        while let Some(first) = rx.next().await {
            let mut batch = vec![first];
            while let Ok(msg) = rx.try_recv() {
                batch.push(msg);
            }
            if this.update(cx, |model, cx| model.apply(batch, cx)).is_err() {
                return;
            }
        }
    })
}

/// Moves the clock (and the demo fleet) on; stops when the model is
/// dropped.
async fn ticker(this: WeakEntity<FleetModel>, cx: &mut AsyncApp) {
    loop {
        cx.background_executor().timer(TICK).await;
        if this.update(cx, |model, cx| model.on_tick(cx)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    async fn keeps_flashes_reports_and_connection_changes(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        fleet.update(cx, |f, cx| {
            f.apply(
                vec![
                    Msg::Connected(true),
                    Msg::Popup("ps".into(), "2 rigs".into()),
                    Msg::Flash("bad seat".into(), true),
                ],
                cx,
            )
        });
        fleet.read_with(cx, |f, _| {
            let texts: Vec<_> = f.notices.iter().map(|n| n.text.as_str()).collect();
            assert_eq!(texts, ["connected to ainetd", "2 rigs", "bad seat"]);
            assert_eq!(f.notices[1].title.as_deref(), Some("ps"));
            let last = f.notices().last().expect("a notice");
            assert!(last.error && last.text == "bad seat");
            assert_eq!(f.connection(), Connection::Connected);
        });

        // Only so many are kept.
        fleet.update(cx, |f, cx| {
            let many = (0..KEEP_NOTICES + 5)
                .map(|i| Msg::Flash(format!("n{i}"), false))
                .collect();
            f.apply(many, cx)
        });
        fleet.read_with(cx, |f, _| {
            assert_eq!(f.notices.len(), KEEP_NOTICES);
            assert_eq!(f.notices.back().unwrap().text, format!("n{}", KEEP_NOTICES + 4));
        });
    }

    #[gpui::test]
    async fn quiet_requests_done_are_no_news(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        let notified = std::rc::Rc::new(std::cell::Cell::new(0));
        let _watch = cx.update(|cx| {
            let notified = notified.clone();
            cx.observe(&fleet, move |_, _| notified.set(notified.get() + 1))
        });
        // What ainetd says to every key, resize and `Interact`.
        fleet.update(cx, |f, cx| {
            let done = (0..5).map(|_| Msg::Flash(String::new(), false)).collect();
            f.apply(done, cx);
            let output = Msg::Event(Box::new(ainet_core::Event::Output { agent: "a".into() }));
            f.apply(vec![output], cx);
        });
        cx.run_until_parked();
        fleet.read_with(cx, |f, _| assert_eq!(f.notices().count(), 0));
        assert_eq!(notified.get(), 0, "nothing to draw anew");

        fleet.update(cx, |f, cx| f.apply(vec![Msg::Flash("saved".into(), false)], cx));
        cx.run_until_parked();
        fleet.read_with(cx, |f, _| assert_eq!(f.notices().count(), 1));
        assert_eq!(notified.get(), 1);
    }

    #[gpui::test]
    async fn a_new_daemon_refetches_screens_and_hears_where_you_are(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        let snapshot = || Msg::Snapshot(Box::new(ainet_model::Demo::new().snapshot()));
        fleet.update(cx, |f, cx| f.apply(vec![Msg::Connected(true), snapshot()], cx));
        let a = fleet.read_with(cx, |f, _| f.store.agents.keys().next().unwrap().clone());
        let item = fleet.entity_id();
        fleet.update(cx, |f, cx| {
            f.show_terminal(item, &a, cx);
            f.set_at_terminal(item, &a, true, cx);
        });
        cx.executor().advance_clock(INTERACT_SETTLE * 2);
        cx.run_until_parked();
        let interacts = |cx: &mut TestAppContext| {
            fleet.update(cx, |f, _| {
                std::mem::take(&mut f.sent)
                    .into_iter()
                    .filter_map(|(r, _)| match r {
                        Request::Interact { agent, on } => Some((agent, on)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(interacts(cx), [(a.clone(), true)]);

        // ainetd goes and comes back: the screen on show is fetched anew,
        // and the new daemon holds the agent's messages again.
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 2);
        cx.run_until_parked();
        fleet.update(cx, |f, _| f.fetched.clear());
        fleet.update(cx, |f, cx| {
            f.apply(vec![Msg::Connected(false)], cx);
            f.apply(vec![Msg::Connected(true), snapshot()], cx);
        });
        cx.run_until_parked();
        assert_eq!(interacts(cx), [(a.clone(), true)]);
        let fetched = fleet.update(cx, |f, _| std::mem::take(&mut f.fetched));
        assert_eq!(fetched.into_iter().map(|(id, _)| id).collect::<Vec<_>>(), [a]);
    }

    #[gpui::test]
    async fn shown_terminals_are_fetched_on_output_at_most_so_often(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        let (a, b) = ("a".to_string(), "b".to_string());
        let output = |id: &str| Msg::Event(Box::new(ainet_core::Event::Output { agent: id.into() }));
        let fetched = |cx: &mut TestAppContext| {
            fleet.update(cx, |f, _| {
                std::mem::take(&mut f.fetched)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>()
            })
        };
        let item = fleet.entity_id();
        fleet.update(cx, |f, cx| f.show_terminal(item, &a, cx));
        assert_eq!(fetched(cx), [a.as_str()], "fetched when it shows");

        // Output soon after waits its turn, once however much came.
        fleet.update(cx, |f, cx| f.apply(vec![output(&a), output(&a), output(&b)], cx));
        cx.run_until_parked();
        assert!(fetched(cx).is_empty());
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 2);
        cx.run_until_parked();
        assert_eq!(fetched(cx), [a.as_str()], "b is not on show");

        // A new size is asked once, and fetched at it.
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 2);
        fleet.update(cx, |f, cx| {
            f.resize_terminal(&a, (30, 100), cx);
            f.resize_terminal(&a, (30, 100), cx);
        });
        let fetched_at = fleet.update(cx, |f, _| std::mem::take(&mut f.fetched));
        assert_eq!(fetched_at, [(a.clone(), (30, 100))]);
        let resizes = fleet.read_with(cx, |f, _| {
            f.sent
                .iter()
                .filter(|(r, _)| matches!(r, Request::Resize { .. }))
                .count()
        });
        assert_eq!(resizes, 1);

        // Hidden: output no longer fetches.
        fleet.update(cx, |f, cx| {
            f.hide_terminal(item);
            f.apply(vec![output(&a)], cx);
        });
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 2);
        cx.run_until_parked();
        assert!(fetched(cx).is_empty());
    }

    #[gpui::test]
    async fn streamed_terminals_are_not_fetched_until_their_stream_ends(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        fleet.update(cx, |f, _| f.streamable = true);
        let a = "a".to_string();
        let output = || Msg::Event(Box::new(ainet_core::Event::Output { agent: "a".into() }));
        let take = |cx: &mut TestAppContext| {
            fleet.update(cx, |f, _| {
                (
                    std::mem::take(&mut f.attached),
                    std::mem::take(&mut f.fetched)
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect::<Vec<_>>(),
                )
            })
        };
        let (first, second) = (fleet.entity_id(), EntityId::from(u64::MAX));
        fleet.update(cx, |f, cx| {
            f.show_terminal(first, &a, cx);
            // A second view of it shares the stream.
            f.show_terminal(second, &a, cx);
        });
        assert_eq!(take(cx), (vec![a.clone()], vec![]), "attached, not fetched");
        assert!(fleet.read_with(cx, |f, _| f.is_streamed(&a)));

        // Output, keys and new sizes come down the stream.
        fleet.update(cx, |f, cx| {
            f.apply(vec![output()], cx);
            f.input(&a, b"x", cx);
            f.resize_terminal(&a, (30, 100), cx);
        });
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 2);
        cx.run_until_parked();
        assert_eq!(take(cx), (vec![], vec![]));

        // An old stream's end is no news; this one's is: fetched instead.
        let ended = |stream| Msg::Detached {
            id: "a".into(),
            stream,
        };
        fleet.update(cx, |f, cx| f.apply(vec![ended(7)], cx));
        assert!(fleet.read_with(cx, |f, _| f.is_streamed(&a)));
        fleet.update(cx, |f, cx| f.apply(vec![ended(1)], cx));
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 2);
        cx.run_until_parked();
        assert_eq!(take(cx), (vec![], vec![a.clone()]));
        assert!(!fleet.read_with(cx, |f, _| f.is_streamed(&a)));

        // A daemon (re)connected streams it again.
        let snapshot = Msg::Snapshot(Box::new(ainet_model::Demo::new().snapshot()));
        fleet.update(cx, |f, cx| f.apply(vec![snapshot], cx));
        assert_eq!(take(cx).0, std::slice::from_ref(&a));

        // Hidden from one view it still streams; from both, it stops.
        fleet.update(cx, |f, _| f.hide_terminal(first));
        assert!(fleet.read_with(cx, |f, _| f.detached.is_empty()));
        fleet.update(cx, |f, _| f.hide_terminal(second));
        assert_eq!(
            fleet.read_with(cx, |f, _| f.detached.clone()),
            std::slice::from_ref(&a)
        );
        assert!(!fleet.read_with(cx, |f, _| f.is_streamed(&a)));
    }

    #[gpui::test]
    async fn changed_screens_are_announced_not_notified(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        let notified = std::rc::Rc::new(std::cell::Cell::new(0));
        let changed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let _watch = cx.update(|cx| {
            let (notified, changed) = (notified.clone(), changed.clone());
            (
                cx.observe(&fleet, move |_, _| notified.set(notified.get() + 1)),
                cx.subscribe(&fleet, move |_, e: &ScreenChanged, _| {
                    changed.borrow_mut().push(e.0.clone())
                }),
            )
        });
        let term = |id: &str| Msg::Term {
            id: id.into(),
            ansi: "hi".into(),
            size: (2, 10),
            rows: vec!["hi".into()],
        };
        let bytes = |id: &str| {
            Msg::Event(Box::new(ainet_core::Event::Bytes {
                agent: id.into(),
                data: " there".into(),
            }))
        };
        // However many pieces came for a screen, it is announced once.
        fleet.update(cx, |f, cx| {
            f.apply(vec![term("a"), bytes("a"), bytes("a"), term("b")], cx)
        });
        cx.run_until_parked();
        assert_eq!(*changed.borrow(), ["a", "b"]);
        assert_eq!(notified.get(), 0);
        fleet.read_with(cx, |f, _| {
            assert_eq!(f.store.terms["a"].screen().contents(), "hi there there")
        });
    }

    #[gpui::test]
    async fn a_failed_start_shows_until_ainetd_answers(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        fleet.read_with(cx, |f, _| assert_eq!(f.connection(), Connection::Starting));
        fleet.update(cx, |f, cx| {
            f.problem = Some(StartProblem::NotStarted("cannot start ainetd: no ainet".into()));
            cx.notify();
        });
        fleet.read_with(cx, |f, _| {
            assert_eq!(
                f.connection(),
                Connection::Failed("cannot start ainetd: no ainet".into())
            )
        });
        fleet.update(cx, |f, cx| f.apply(vec![Msg::Connected(true)], cx));
        fleet.read_with(cx, |f, _| assert_eq!(f.connection(), Connection::Connected));
    }

    #[gpui::test]
    async fn an_incompatible_daemon_stays_reported_when_it_connects(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::empty(false));
        let why = "ainetd at /tmp/s is incompatible: protocol 1, not 2";
        fleet.update(cx, |f, cx| {
            f.problem = Some(StartProblem::Incompatible(why.into()));
            f.apply(vec![Msg::Connected(true)], cx);
        });
        cx.run_until_parked();
        fleet.read_with(cx, |f, _| {
            assert!(f.store.connected);
            assert_eq!(f.connection(), Connection::Incompatible(why.into()));
        });
    }
}
