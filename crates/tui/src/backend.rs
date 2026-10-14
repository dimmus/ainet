//! The TUI's connection to ainetd: a thread that follows the event stream,
//! worker threads for requests, and a screen fetcher that pulls formatted
//! terminal state for the tiles on screen.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ainet_core::client::Client;
use ainet_core::{Request, Response};

use crate::app::Msg;

pub(crate) fn call(socket: &Path, req: &Request, timeout: Duration) -> std::io::Result<Response> {
    let mut client = Client::connect(socket)?;
    client.set_timeout(Some(timeout))?;
    client.call(req)
}

/// Follows the daemon: a snapshot, then events, reconnecting when it drops.
pub(crate) fn listen(socket: PathBuf, tx: mpsc::Sender<Msg>) {
    thread::spawn(move || {
        loop {
            match call(&socket, &Request::Snapshot, Duration::from_secs(5)) {
                Ok(snapshot @ Response::Snapshot { .. }) => {
                    let _ = tx.send(Msg::Connected(true));
                    if tx.send(Msg::Snapshot(Box::new(snapshot))).is_err() {
                        return;
                    }
                    if let Ok(events) = Client::connect(&socket).and_then(Client::subscribe) {
                        for event in events {
                            if tx.send(Msg::Event(Box::new(event))).is_err() {
                                return;
                            }
                        }
                    }
                }
                _ => {
                    if tx.send(Msg::Connected(false)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(Msg::Connected(false));
            thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Sends a request on a worker thread; the reply comes back as a message.
pub(crate) fn request(socket: &Path, req: Request, tx: &mpsc::Sender<Msg>, ok: String, timeout: Duration) {
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
        let _ = tx.send(msg);
    });
}

/// A thread that fetches formatted screens for the ids it is sent, one
/// connection, skipping duplicates that queued up meanwhile.
pub(crate) fn fetcher(socket: PathBuf, tx: mpsc::Sender<Msg>) -> mpsc::Sender<String> {
    let (want_tx, want_rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let mut client: Option<Client> = None;
        while let Ok(first) = want_rx.recv() {
            let mut batch: Vec<String> = vec![first];
            batch.extend(want_rx.try_iter());
            let mut seen = HashSet::new();
            batch.retain(|id| seen.insert(id.clone()));
            for id in batch {
                if client.is_none() {
                    client = Client::connect(&socket).ok();
                    if let Some(c) = &client {
                        let _ = c.set_timeout(Some(Duration::from_secs(3)));
                    }
                }
                let Some(c) = client.as_mut() else { break };
                match c.call(&Request::Screen {
                    agent: id.clone(),
                    formatted: true,
                    history: 0,
                }) {
                    Ok(Response::Screen { rows, ansi, size }) => {
                        if tx
                            .send(Msg::Term {
                                id,
                                ansi: ansi.unwrap_or_default(),
                                size,
                                rows,
                            })
                            .is_err()
                        {
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
