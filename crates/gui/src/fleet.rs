//! The fleet as a GPUI entity: `ainet-model`'s [`Store`], kept current by a
//! [`Link`] to ainetd (or the demo), plus the notices a front end shows as
//! toasts (each one is also emitted as an event). Views read it and
//! re-render when it notifies.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ainet_core::client::Client;
use ainet_model::{Backend, Change, Link, Msg, Store, Tally, daemon};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{AppContext as _, AsyncApp, Context, EventEmitter, Task, WeakEntity};

use crate::sink::{self, ChannelSink};

/// How often the demo fleet moves on, as in the TUI.
pub const TICK: Duration = Duration::from_millis(120);

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
    _tasks: Vec<Task<()>>,
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

    /// ainetd at `socket`: starts it through the console binary (off the UI
    /// thread), then follows it. If it cannot be started, the reason shows
    /// in the status bar and on stderr, and the socket is followed anyway.
    pub fn daemon(socket: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut model = FleetModel::empty(false);
        let start = cx.background_spawn({
            let socket = socket.clone();
            async move { start_daemon(&socket) }
        });
        model._tasks.push(cx.spawn(async move |this, cx| {
            let outcome = start.await;
            let _ = this.update(cx, |model, cx| model.follow(socket, outcome, cx));
        }));
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
            _tasks: Vec::new(),
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

    /// Moves the Fleet selection `delta` agents down (up if negative).
    pub fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.store.move_selection(delta).is_some() {
            cx.notify();
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
                _ => {}
            }
        }
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

    fn on_tick(&mut self) {
        self.tick += 1;
        if let Some(link) = &mut self.link {
            link.tick(self.tick);
        }
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

/// Moves the demo fleet on; stops when the model is dropped.
async fn ticker(this: WeakEntity<FleetModel>, cx: &mut AsyncApp) {
    loop {
        cx.background_executor().timer(TICK).await;
        if this.update(cx, |model, _| model.on_tick()).is_err() {
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
