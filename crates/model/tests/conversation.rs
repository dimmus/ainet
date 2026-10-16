//! Conversations through the link: entries from a daemon that sends them,
//! and from one from before `Request::Conversation`, the transcript as
//! plain text with a notice saying why.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ainet_core::conversation::EntryKind;
use ainet_core::protocol::encode;
use ainet_core::{Conversation, ConversationEntry, Request, Response};
use ainet_model::link::{OLD_DAEMON, conversation_reply};
use ainet_model::{Backend, Demo, Link, Msg};

/// A pretend daemon: `old` ones do not know `Conversation`, as daemons
/// from before it answer.
fn daemon(name: &str, old: bool) -> PathBuf {
    let socket = std::env::temp_dir().join(format!("ainet-talk-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            thread::spawn(move || serve(stream, old));
        }
    });
    socket
}

fn serve(stream: UnixStream, old: bool) {
    let mut write = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let req: Request = serde_json::from_str(&line).unwrap();
        let resp = match req {
            Request::Conversation { .. } if old => Response::Error {
                message: "bad request: unknown variant `conversation`, expected one of `hello`, …".into(),
            },
            Request::Conversation { after, tail, .. } => Response::Conversation {
                conversation: Conversation {
                    entries: vec![ConversationEntry::new(
                        after.unwrap_or(0) + 1,
                        EntryKind::Reply,
                        format!("tail {tail}"),
                    )],
                    ..Conversation::default()
                },
            },
            Request::Transcript { tail, .. } => Response::Text {
                text: format!("user: hi\nassistant: hello ({tail})"),
            },
            _ => Response::Ok,
        };
        if write.write_all(&encode(&resp)).is_err() {
            return;
        }
    }
}

fn conversation(rx: &mpsc::Receiver<Msg>) -> (u64, Result<Conversation, String>) {
    loop {
        match rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the link went quiet")
        {
            Msg::Conversation { token, reply } => return (token, reply),
            _ => continue,
        }
    }
}

#[test]
fn a_daemon_that_knows_conversations_sends_entries() {
    let (tx, rx) = mpsc::channel();
    let (mut link, _) = Link::new(Backend::Daemon(daemon("new", false)), tx);
    link.conversation("a", Some(41), 30, 7);
    let (token, reply) = conversation(&rx);
    assert_eq!(token, 7);
    let c = reply.unwrap();
    assert_eq!((c.entries[0].id, c.entries[0].text.as_str()), (42, "tail 30"));
    assert_eq!((c.notice, c.plain), (None, None));
}

#[test]
fn an_old_daemon_gives_the_transcript_as_text() {
    let (tx, rx) = mpsc::channel();
    let (mut link, _) = Link::new(Backend::Daemon(daemon("old", true)), tx);
    link.conversation("a", None, 30, 3);
    let (token, reply) = conversation(&rx);
    assert_eq!(token, 3);
    let c = reply.unwrap();
    assert!(c.entries.is_empty());
    assert_eq!(c.plain.as_deref(), Some("user: hi\nassistant: hello (30)"));
    assert_eq!(c.notice.as_deref(), Some(OLD_DAEMON));

    // The same through the demo pretending to be old, and the errors.
    let (tx, rx) = mpsc::channel();
    let (mut demo, _) = Link::new(Backend::Demo(Box::new(Demo::without_conversations())), tx);
    let Response::Snapshot { agents, .. } = Demo::full().snapshot() else {
        unreachable!()
    };
    demo.conversation(&agents[0].id, None, 0, 1);
    let (_, reply) = conversation(&rx);
    let c = reply.unwrap();
    assert!(c.plain.unwrap().contains(agents[0].task.as_deref().unwrap()));
    assert_eq!(c.notice.as_deref(), Some(OLD_DAEMON));
    let no_agent = Response::Error {
        message: "no agent x".into(),
    };
    assert_eq!(
        conversation_reply(Ok(no_agent), || unreachable!()),
        Err("no agent x".into())
    );
    assert_eq!(
        conversation_reply(Ok(Response::Ok), || unreachable!()),
        Err("ainetd answered with something else".into())
    );
    assert_eq!(
        conversation_reply(Err("ainetd: gone".into()), || unreachable!()),
        Err("ainetd: gone".into())
    );
}
