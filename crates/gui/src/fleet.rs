//! The fleet as a GPUI entity: `ainet-model`'s [`Store`], kept current by a
//! [`Link`] to ainetd (or the demo), plus the notices a front end shows as
//! toasts (each one is also emitted as an event). Views read it and
//! re-render when it notifies; requests go out through it. It also keeps
//! the animation clock: a [`Tick`] every [`TICK`], which moves the demo on
//! and the critters of views that are showing some.
//!
//! Terminals on show register here: their screens are fetched when their
//! agent prints something, at most every `terminal::FETCH_EVERY` each (the
//! TUI's pump), and ainetd is told when you are at one (`Interact`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ainet_core::Request;
use ainet_core::client::Client;
use ainet_model::{Backend, Change, Command, Link, Msg, Pump, Store, Tally, daemon};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{AppContext as _, AsyncApp, Context, EntityId, EventEmitter, Subscription, Task, WeakEntity};

use crate::sink::{self, ChannelSink};

/// How often the demo fleet and the critters move on, as in the TUI.
pub const TICK: Duration = Duration::from_millis(120);

/// How long the keyboard must stay before ainetd hears which terminals you
/// are at: moving between two views of one agent, or through a pane on the
/// way to its tab, tells it nothing.
pub const INTERACT_SETTLE: Duration = Duration::from_millis(50);

/// How many notices are kept for later toasts and a log.
const KEEP_NOTICES: usize = 64;

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
/// in the connection.
#[derive(Clone, Debug)]
pub struct Notice {
    /// For reports (`ainet ps`-like replies): their title.
    pub title: Option<String>,
    pub text: String,
    pub error: bool,
}

impl EventEmitter<Notice> for FleetModel {}

/// The animation clock moved on ([`FleetModel::tick`] says where to).
/// Emitted without re-rendering anything; views with critters redraw if
/// they show some.
#[derive(Clone, Copy, Debug)]
pub struct Tick;

impl EventEmitter<Tick> for FleetModel {}

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
    notices: VecDeque<Notice>,
    /// The terminals on show: the agent each item (by entity) shows.
    shown: HashMap<EntityId, String>,
    /// Which shown screens are stale, and the sizes asked of their panes.
    pump: Pump,
    /// The next round of fetches, while some screen waits for its turn.
    pump_task: Option<Task<()>>,
    /// The items that have the keyboard, per agent, and the agents ainetd
    /// was told you are at; told after the focus settles.
    at: HashMap<String, HashSet<EntityId>>,
    told: HashSet<String>,
    telling: Option<Task<()>>,
    /// Every request sent, quiet ones and screen fetches with scrollback
    /// included, for tests.
    #[cfg(test)]
    pub sent: Vec<(Request, String)>,
    /// Every screen fetched, for tests.
    #[cfg(test)]
    pub fetched: Vec<(String, (u16, u16))>,
    _tasks: Vec<Task<()>>,
    _quit: Option<Subscription>,
}

impl FleetModel {
    /// The pretend fleet, moving on every [`TICK`].
    pub fn demo(cx: &mut Context<Self>) -> Self {
        let (sink, rx) = sink::channel();
        let (link, snapshot) = Link::new(Backend::Demo(Box::default()), sink);
        let mut model = FleetModel::empty(true);
        model.link = Some(link);
        if let Some(snapshot) = snapshot {
            model.store.connected = true;
            model.store.apply(Msg::Snapshot(Box::new(snapshot)));
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
            notices: VecDeque::new(),
            shown: HashMap::new(),
            pump: Pump::new(),
            pump_task: None,
            at: HashMap::new(),
            told: HashSet::new(),
            telling: None,
            #[cfg(test)]
            sent: Vec::new(),
            #[cfg(test)]
            fetched: Vec::new(),
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

    /// The notices so far, the latest last.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn notices(&self) -> impl Iterator<Item = &Notice> {
        self.notices.iter()
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
            Command::Request(req, ok) => self.request(*req, ok, cx),
            Command::Error(why) => self.show(why, true, cx),
            _ => {}
        }
    }

    /// A request only ainetd acts on (keystrokes, resizes), sent quietly.
    pub fn quiet(&mut self, req: Request) {
        #[cfg(test)]
        self.sent.push((req.clone(), String::new()));
        if let Some(link) = &self.link {
            link.quiet(req);
        }
    }

    /// Item `item` shows agent `agent`'s terminal: its screen is fetched
    /// now and whenever the agent prints something.
    pub fn show_terminal(&mut self, item: EntityId, agent: &str, cx: &mut Context<Self>) {
        self.shown.insert(item, agent.to_string());
        self.refresh_terminal(agent, cx);
    }

    /// Item `item` no longer shows a terminal (hidden, closed or gone).
    pub fn hide_terminal(&mut self, item: EntityId) {
        self.shown.remove(&item);
    }

    /// Whether some item shows `agent`'s terminal.
    pub fn is_shown(&self, agent: &str) -> bool {
        self.shown.values().any(|a| a == agent)
    }

    /// `agent`'s screen is stale: fetch it as soon as its turn comes.
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

    /// Applies messages from the link, then re-renders once.
    fn apply(&mut self, msgs: Vec<Msg>, cx: &mut Context<Self>) {
        for msg in msgs {
            match self.store.apply(msg) {
                Change::Flash(text, error) => self.notice(None, text, error, cx),
                Change::Popup(title, text) => self.notice(Some(title), text, false, cx),
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
                }
                Change::Output(agent) if self.is_shown(&agent) => self.pump.mark(&agent),
                _ => {}
            }
        }
        self.pump(cx);
        cx.notify();
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
        if self.notices.len() == KEEP_NOTICES {
            self.notices.pop_front();
        }
        let notice = Notice { title, text, error };
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
