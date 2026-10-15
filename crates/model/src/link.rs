//! A front end's connection to the fleet: ainetd over its socket (a thread
//! that follows the event stream, worker threads for requests, a screen
//! fetcher), or the pretend fleet of the demo. Whatever comes back arrives
//! as a [`Msg`] through a [`Sink`], to be applied to a `Store` on the UI
//! thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ainet_core::client::Client;
use ainet_core::protocol::SpecEntry;
use ainet_core::{Agent, Event, Request, Response, SnapshotInfo};

use crate::demo::Demo;

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
    /// Scrollback: up to the asked number of rows above the screen, then
    /// the screen, as plain text.
    History {
        id: String,
        rows: Vec<String>,
    },
    Flash(String, bool),
    Popup(String, String),
    /// The reply to a request sent with [`Link::ask`]: its token, and the
    /// reply as text (empty when it says no more than "done"), or the
    /// error.
    Reply {
        token: u64,
        reply: Result<String, String>,
    },
    /// The reply to a request sent with [`Link::list`]: its token, and
    /// the items listed, or the error.
    Listing {
        token: u64,
        listing: Result<Listing, String>,
    },
    Connected(bool),
}

/// What a listing request ([`Link::list`]) brings back, item by item, for
/// a front end to pick from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// The spec library (`Request::Specs`).
    Specs(Vec<SpecEntry>),
    /// A rig's snapshots, oldest first (`Request::SnapshotList`).
    Snapshots(Vec<SnapshotInfo>),
}

impl Listing {
    /// The listing in a daemon reply, or what went wrong.
    pub fn from_response(resp: Response) -> Result<Listing, String> {
        match resp {
            Response::Specs { specs } => Ok(Listing::Specs(specs)),
            Response::Snapshots { snapshots } => Ok(Listing::Snapshots(snapshots)),
            Response::Error { message } => Err(message),
            _ => Err("ainetd answered with something else".into()),
        }
    }
}

/// Where messages for the UI go: a channel the UI drains. `send` returns
/// false once the UI is gone, so threads can stop.
pub trait Sink: Clone + Send + 'static {
    fn send(&self, msg: Msg) -> bool;
}

impl Sink for mpsc::Sender<Msg> {
    fn send(&self, msg: Msg) -> bool {
        mpsc::Sender::send(self, msg).is_ok()
    }
}

pub enum Backend {
    Demo(Box<Demo>),
    Daemon(PathBuf),
}

/// The data source, and the threads that talk to it.
pub struct Link<S: Sink> {
    backend: Backend,
    sink: S,
    fetch: Option<mpsc::Sender<Want>>,
}

/// A screen the fetcher is asked for: an agent's id, and how many rows of
/// scrollback (none: the formatted screen).
type Want = (String, usize);

impl<S: Sink> Link<S> {
    /// Starts following the daemon (or sets up the demo). With the demo, the
    /// returned snapshot is the whole fleet, to apply right away; with the
    /// daemon it arrives later through the sink.
    pub fn new(backend: Backend, sink: S) -> (Self, Option<Response>) {
        let mut link = Link {
            backend,
            sink,
            fetch: None,
        };
        let snapshot = match &link.backend {
            Backend::Demo(demo) => Some(demo.snapshot()),
            Backend::Daemon(socket) => {
                listen(socket.clone(), link.sink.clone());
                link.fetch = Some(fetcher(socket.clone(), link.sink.clone()));
                None
            }
        };
        (link, snapshot)
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.backend, Backend::Demo(_))
    }

    /// One animation tick: the demo fleet moves on.
    pub fn tick(&mut self, tick: u64) {
        if let Backend::Demo(demo) = &mut self.backend {
            for ev in demo.step(tick) {
                self.sink.send(Msg::Event(Box::new(ev)));
            }
        }
    }

    /// Sends a request; the outcome comes back as a flash (`ok`, or the
    /// error) or, for replies that are reports, a popup titled `ok`.
    pub fn request(&mut self, req: Request, ok: String) {
        match &mut self.backend {
            Backend::Demo(demo) => {
                let (events, outcome) = demo.handle(&req);
                for ev in events {
                    self.sink.send(Msg::Event(Box::new(ev)));
                }
                match outcome {
                    Ok(Some(text)) => {
                        self.sink.send(Msg::Popup(ok, text));
                    }
                    Ok(None) if !ok.is_empty() => {
                        self.sink.send(Msg::Flash(ok, false));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.sink.send(Msg::Flash(e, true));
                    }
                }
            }
            Backend::Daemon(socket) => {
                let long = matches!(
                    req,
                    Request::Up(_) | Request::Restore { .. } | Request::Down { .. } | Request::Discover
                );
                let timeout = Duration::from_secs(if long { 60 } else { 10 });
                request(socket, req, &self.sink, ok, timeout);
            }
        }
    }

    /// Sends a request whose reply is wanted whole by whoever asked (a
    /// transcript, a spec, to show in a view of its own): it comes back as
    /// [`Msg::Reply`] with `token`, never as a flash or popup.
    pub fn ask(&mut self, req: Request, token: u64) {
        match &mut self.backend {
            Backend::Demo(demo) => {
                let (events, outcome) = demo.handle(&req);
                for ev in events {
                    self.sink.send(Msg::Event(Box::new(ev)));
                }
                self.sink.send(Msg::Reply {
                    token,
                    reply: outcome.map(Option::unwrap_or_default),
                });
            }
            Backend::Daemon(socket) => {
                let (socket, tx) = (socket.clone(), self.sink.clone());
                thread::spawn(move || {
                    let reply = match call(&socket, &req, Duration::from_secs(10)) {
                        Ok(resp) => crate::popup::reply(&resp),
                        Err(e) => Err(format!("ainetd: {e}")),
                    };
                    tx.send(Msg::Reply { token, reply });
                });
            }
        }
    }

    /// Sends a request that lists things to pick from (the spec library, a
    /// rig's snapshots): the items come back as [`Msg::Listing`] with
    /// `token`, never as a flash or popup.
    pub fn list(&mut self, req: Request, token: u64) {
        match &mut self.backend {
            Backend::Demo(demo) => {
                self.sink.send(Msg::Listing {
                    token,
                    listing: demo.list(&req),
                });
            }
            Backend::Daemon(socket) => {
                let (socket, tx) = (socket.clone(), self.sink.clone());
                thread::spawn(move || {
                    let listing = match call(&socket, &req, Duration::from_secs(10)) {
                        Ok(resp) => Listing::from_response(resp),
                        Err(e) => Err(format!("ainetd: {e}")),
                    };
                    tx.send(Msg::Listing { token, listing });
                });
            }
        }
    }

    /// A request only the daemon acts on (keystrokes, resizes), quietly and
    /// with a short timeout; the demo ignores it.
    pub fn quiet(&self, req: Request) {
        if let Backend::Daemon(socket) = &self.backend {
            request(socket, req, &self.sink, String::new(), Duration::from_secs(3));
        }
    }

    /// Asks for a fresh screen of `agent`'s terminal, at `size` (rows,
    /// cols) for the demo, which draws one; it comes back as `Msg::Term`.
    pub fn fetch(&self, agent: &Agent, tick: u64, size: (u16, u16)) {
        match &self.backend {
            Backend::Demo(demo) => {
                let (rows, cols) = size;
                self.sink.send(Msg::Term {
                    id: agent.id.clone(),
                    ansi: demo.terminal(agent, tick, rows, cols),
                    size,
                    rows: demo.screen(agent, tick),
                });
            }
            Backend::Daemon(_) => {
                if let Some(f) = &self.fetch {
                    let _ = f.send((agent.id.clone(), 0));
                }
            }
        }
    }

    /// Asks for up to `lines` rows of `agent`'s scrollback above its
    /// screen, as plain text; it comes back as `Msg::History`.
    pub fn history(&self, agent: &Agent, tick: u64, lines: usize) {
        match &self.backend {
            Backend::Demo(demo) => {
                self.sink.send(Msg::History {
                    id: agent.id.clone(),
                    rows: demo.history(agent, tick, lines),
                });
            }
            Backend::Daemon(_) => {
                if let Some(f) = &self.fetch {
                    let _ = f.send((agent.id.clone(), lines.max(1)));
                }
            }
        }
    }
}

fn call(socket: &Path, req: &Request, timeout: Duration) -> std::io::Result<Response> {
    let mut client = Client::connect(socket)?;
    client.set_timeout(Some(timeout))?;
    client.call(req)
}

/// Follows the daemon: a snapshot, then events, reconnecting when it drops.
fn listen<S: Sink>(socket: PathBuf, tx: S) {
    thread::spawn(move || {
        loop {
            match call(&socket, &Request::Snapshot, Duration::from_secs(5)) {
                Ok(snapshot @ Response::Snapshot { .. }) => {
                    tx.send(Msg::Connected(true));
                    if !tx.send(Msg::Snapshot(Box::new(snapshot))) {
                        return;
                    }
                    if let Ok(events) = Client::connect(&socket).and_then(Client::subscribe) {
                        for event in events {
                            if !tx.send(Msg::Event(Box::new(event))) {
                                return;
                            }
                        }
                    }
                }
                _ => {
                    if !tx.send(Msg::Connected(false)) {
                        return;
                    }
                }
            }
            tx.send(Msg::Connected(false));
            thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Sends a request on a worker thread; the reply comes back as a message.
fn request<S: Sink>(socket: &Path, req: Request, tx: &S, ok: String, timeout: Duration) {
    let (socket, tx) = (socket.to_path_buf(), tx.clone());
    thread::spawn(move || {
        let msg = match call(&socket, &req, timeout) {
            Ok(Response::Error { message }) => Msg::Flash(message, true),
            Ok(Response::Spawned { agent }) => Msg::Flash(format!("{} joined the fleet", agent.name), false),
            Ok(Response::Text { text }) => Msg::Popup(ok, text),
            Ok(
                resp @ (Response::Discovered { .. }
                | Response::Rigs { .. }
                | Response::Specs { .. }
                | Response::Snapshots { .. }
                | Response::Watchdogs { .. }
                | Response::Captures { .. }
                | Response::Whoami { .. }),
            ) => Msg::Popup(ok, crate::popup::render(&resp)),
            Ok(Response::Rig { rig }) => {
                Msg::Flash(format!("{ok}: rig {} is {}", rig.name, rig.status.label()), false)
            }
            Ok(_) => Msg::Flash(ok, false),
            Err(e) => Msg::Flash(format!("ainetd: {e}"), true),
        };
        tx.send(msg);
    });
}

/// A thread that fetches the screens it is asked for (formatted, or
/// scrollback as text), one connection, skipping duplicates that queued up
/// meanwhile.
fn fetcher<S: Sink>(socket: PathBuf, tx: S) -> mpsc::Sender<Want> {
    let (want_tx, want_rx) = mpsc::channel::<Want>();
    thread::spawn(move || {
        let mut client: Option<Client> = None;
        while let Ok(first) = want_rx.recv() {
            let mut batch: Vec<Want> = vec![first];
            batch.extend(want_rx.try_iter());
            let mut seen = HashSet::new();
            batch.retain(|want| seen.insert(want.clone()));
            for (id, history) in batch {
                if client.is_none() {
                    client = Client::connect(&socket).ok();
                    if let Some(c) = &client {
                        let _ = c.set_timeout(Some(Duration::from_secs(3)));
                    }
                }
                let Some(c) = client.as_mut() else { break };
                match c.call(&Request::Screen {
                    agent: id.clone(),
                    formatted: history == 0,
                    history,
                }) {
                    Ok(Response::Screen { rows, ansi, size }) => {
                        let msg = if history == 0 {
                            Msg::Term {
                                id,
                                ansi: ansi.unwrap_or_default(),
                                size,
                                rows,
                            }
                        } else {
                            Msg::History { id, rows }
                        };
                        if !tx.send(msg) {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => client = None,
                }
            }
        }
    });
    want_tx
}
