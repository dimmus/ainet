//! Streamed terminals through the link, against pretend daemons on a
//! socket: a new one that streams, an old one that knows no `Attach` (the
//! link falls back to fetching screens), and one whose stream ends.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ainet_core::protocol::encode;
use ainet_core::{Agent, Event, Request, Response};
use ainet_model::{Backend, Change, Link, Msg, Store};

/// What a pretend daemon does with a request on one connection: lines to
/// answer with, and whether to hang up after them.
type Answer = fn(&Request) -> (Vec<Response>, bool);

/// A pretend daemon answering with `answer`; it tells `closed` when a
/// client hangs up a connection that had attached.
fn daemon(name: &str, answer: Answer) -> (PathBuf, mpsc::Receiver<()>) {
    let socket = std::env::temp_dir().join(format!("ainet-link-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let (closed_tx, closed) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let closed = closed_tx.clone();
            thread::spawn(move || serve(stream, answer, closed));
        }
    });
    (socket, closed)
}

fn serve(stream: UnixStream, answer: Answer, closed: mpsc::Sender<()>) {
    let mut write = stream.try_clone().unwrap();
    let mut attached = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let req: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            // What every daemon says to a request it does not know.
            Err(e) => {
                let error = Response::Error {
                    message: format!("bad request: {e}"),
                };
                let _ = write.write_all(&encode(&error));
                continue;
            }
        };
        attached |= matches!(req, Request::Attach { .. });
        let (lines, hang_up) = answer(&req);
        for resp in lines {
            if write.write_all(&encode(&resp)).is_err() {
                return;
            }
        }
        if hang_up {
            return;
        }
    }
    if attached {
        let _ = closed.send(());
    }
}

fn screen(text: &str) -> Response {
    Response::Screen {
        rows: vec![text.into()],
        ansi: Some(text.into()),
        size: (3, 20),
        styled_rows: None,
    }
}

fn bytes(data: &str) -> Response {
    Response::Event {
        event: Event::Bytes {
            agent: "a".into(),
            data: data.into(),
        },
    }
}

/// The messages the link sends, applied to a store until `until` holds.
fn apply_until(
    rx: &mpsc::Receiver<Msg>,
    store: &mut Store,
    until: impl Fn(&Store, &Change) -> bool,
) -> Change {
    loop {
        let msg = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the link went quiet");
        let change = store.apply(msg);
        if until(store, &change) {
            return change;
        }
    }
}

fn contents(store: &Store) -> String {
    store
        .terms
        .get("a")
        .map(|p| p.screen().contents())
        .unwrap_or_default()
}

fn agent() -> Agent {
    let Response::Snapshot { agents, .. } = ainet_model::Demo::new().snapshot() else {
        unreachable!()
    };
    Agent {
        id: "a".into(),
        ..agents[0].clone()
    }
}

/// What every pretend daemon says to requests other than `Attach`: a
/// screen fetched the old way, or nothing much.
fn otherwise(req: &Request) -> Vec<Response> {
    match req {
        Request::Screen { .. } => vec![screen("fetched")],
        Request::Snapshot => vec![Response::Error {
            message: "not now".into(),
        }],
        _ => vec![Response::Ok],
    }
}

#[test]
fn attached_terminals_stream_into_the_store_until_detached() {
    let (socket, closed) = daemon("new", |req| match req {
        Request::Attach { .. } => (vec![screen("one"), bytes("\r\ntw"), bytes("o")], false),
        _ => (otherwise(req), false),
    });
    let (tx, rx) = mpsc::channel();
    let (mut link, _) = Link::new(Backend::Daemon(socket), tx);
    let mut store = Store::new();
    link.attach("a").expect("this daemon streams");
    apply_until(&rx, &mut store, |s, _| contents(s) == "one\ntwo");
    // Detaching hangs up, and nothing more comes of the stream.
    link.detach("a");
    closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the stream was never closed");
    while let Ok(msg) = rx.recv_timeout(Duration::from_millis(200)) {
        let change = store.apply(msg);
        assert!(
            !matches!(change, Change::Detached { .. } | Change::Term(_)),
            "{change:?} after detaching"
        );
    }
}

#[test]
fn an_old_daemon_refuses_and_screens_are_fetched_instead() {
    // It knows no `Attach`: it fails to parse it, as ainetd always has.
    let (socket, _) = daemon("old", |req| match req {
        Request::Attach { .. } => {
            let message = "bad request: unknown variant `attach`, expected one of `hello`, `snapshot`";
            (
                vec![Response::Error {
                    message: message.into(),
                }],
                false,
            )
        }
        _ => (otherwise(req), false),
    });
    let (tx, rx) = mpsc::channel();
    let (mut link, _) = Link::new(Backend::Daemon(socket), tx);
    let mut store = Store::new();
    let stream = link.attach("a").expect("worth a try");
    let change = apply_until(&rx, &mut store, |_, c| matches!(c, Change::Detached { .. }));
    assert_eq!(
        change,
        Change::Detached {
            id: "a".into(),
            stream
        }
    );
    link.ended("a", stream);
    // Not tried again with this daemon; fetching works as it always did.
    assert_eq!(link.attach("a"), None);
    link.fetch(&agent(), 0, (3, 20));
    apply_until(&rx, &mut store, |s, _| contents(s) == "fetched");
}

#[test]
fn a_stream_that_ends_says_so() {
    // The program ended: the daemon hangs up after the last of it.
    let (socket, _) = daemon("ends", |req| match req {
        Request::Attach { .. } => (vec![screen("last words")], true),
        _ => (otherwise(req), false),
    });
    let (tx, rx) = mpsc::channel();
    let (mut link, _) = Link::new(Backend::Daemon(socket), tx);
    let mut store = Store::new();
    let first = link.attach("a").unwrap();
    let change = apply_until(&rx, &mut store, |_, c| matches!(c, Change::Detached { .. }));
    assert_eq!(contents(&store), "last words");
    assert_eq!(
        change,
        Change::Detached {
            id: "a".into(),
            stream: first
        }
    );
    // Streaming is still to be had from this daemon, with a new number.
    let second = link.attach("a").unwrap();
    assert_ne!(first, second);
    // The first stream's end does not touch the second.
    link.ended("a", first);
    apply_until(&rx, &mut store, |_, c| {
        *c == Change::Detached {
            id: "a".into(),
            stream: second,
        }
    });
}

#[test]
fn the_demo_does_not_stream() {
    let (tx, _rx) = mpsc::channel();
    let (mut link, _) = Link::new(Backend::Demo(Box::new(ainet_model::Demo::new())), tx);
    assert_eq!(link.attach("a"), None);
}
