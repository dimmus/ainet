//! Delivers a message into a running Claude Code session through the
//! session's inbox socket, the channel other Claude sessions use.
//!
//! Ported from Den's `internal/peer` (MIT).
//!
//! Undocumented behaviour (Claude Code 2.1.224+ binds the socket; checked by
//! Den against 2.1.289): the socket path is `messagingSocketPath` in the
//! session's `~/.claude/sessions/<pid>.json`. The wire format is one JSON
//! line, `{"message":{"content":"…","role":"user"},"type":"user"}\n`. The
//! session sends no acknowledgement: a successful write is the receipt. The
//! receiving Claude sees the text framed as a message from another session,
//! so the text says who really wrote it (the header).
//!
//! The path comes from a session file, which only vouches for itself, so it
//! is not trusted: it must be a socket itself (a symlink is refused, whatever
//! it points at, so the text cannot be steered to the ssh agent or another
//! service), owned by the caller, and the process that answers must run as
//! the caller too (`SO_PEERCRED`). Errors never carry the path.

use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use crate::sessions::LiveSession;
use crate::util;

/// The header ainet puts above every message it relays.
pub const DEFAULT_HEADER: &str = "Operator message via ainet (typed by your human in another terminal):";

/// The header Den uses, for reference.
pub const DEN_HEADER: &str = "Operator message via Den (typed by your human in another terminal):";

/// The text as it is delivered: the header, a blank line, the trimmed text.
pub fn compose(header: &str, text: &str) -> String {
    format!("{header}\n\n{}", text.trim())
}

/// The JSON line written to the socket, newline included. Byte-identical to
/// what Den writes.
pub fn wire_line(header: &str, text: &str) -> String {
    let content = util::go_string(&compose(header, text));
    format!("{{\"message\":{{\"content\":{content},\"role\":\"user\"}},\"type\":\"user\"}}\n")
}

/// Sends `text` to a session with [`DEFAULT_HEADER`].
pub fn send(session: &LiveSession, text: &str) -> Result<()> {
    send_with_header(session, DEFAULT_HEADER, text)
}

/// Sends `text` to a session under `header`. A session that is not alive is
/// refused.
pub fn send_with_header(session: &LiveSession, header: &str, text: &str) -> Result<()> {
    if !session.alive {
        bail!("this session is gone");
    }
    send_to_socket(Path::new(&session.messaging_socket_path), header, text)
}

/// Posts `text` to the socket at `path`. Errors when the session has no
/// socket, the socket is gone or is not a plain socket of this user, the
/// peer runs as another user, or the write fails.
pub fn send_to_socket(path: &Path, header: &str, text: &str) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("this session has no inbox socket (Claude Code 2.1.224+ binds one)");
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| anyhow!("inbox socket is gone"))?;
    if !meta.file_type().is_socket() {
        bail!("inbox socket is not a plain socket (a link or another kind of file); refusing");
    }
    let me = rustix::process::getuid().as_raw();
    if meta.uid() != me {
        bail!("inbox socket is not owned by you; refusing");
    }
    let mut conn = connect(path)?;
    match rustix::net::sockopt::socket_peercred(&conn) {
        Ok(cred) if cred.uid.as_raw() == me => {}
        _ => bail!("inbox socket is served by another user; refusing"),
    }
    conn.set_write_timeout(Some(Duration::from_secs(2)))?;
    conn.write_all(wire_line(header, text).as_bytes())
        .map_err(|e| anyhow!("write: {}", e.kind()))?;
    Ok(())
}

/// Connects without blocking: a Unix socket connect either completes at
/// once or fails (EAGAIN when the listener's backlog is full), so this never
/// hangs on a wedged session. Den dials with a one-second timeout.
fn connect(path: &Path) -> Result<UnixStream> {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let fd = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|e| anyhow!("connect: {e}"))?;
    let addr = SocketAddrUnix::new(path).map_err(|e| anyhow!("connect: {e}"))?;
    rustix::net::connect(&fd, &addr).map_err(|e| anyhow!("connect: {e}"))?;
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    fn listen(path: &Path) -> mpsc::Receiver<String> {
        let ln = UnixListener::bind(path).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for c in ln.incoming() {
                let Ok(c) = c else { return };
                let mut line = String::new();
                BufReader::new(c).read_line(&mut line).ok();
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        rx
    }

    #[test]
    fn writes_one_json_line() {
        let tmp = TempDir::new();
        let path = tmp.join("cc.sock");
        let rx = listen(&path);
        send_to_socket(&path, DEFAULT_HEADER, "  rebase on main first  ").unwrap();
        let line = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        let content = v["message"]["content"].as_str().unwrap();
        assert!(
            content.starts_with(DEFAULT_HEADER) && content.ends_with("rebase on main first"),
            "{content}"
        );
        assert_eq!(
            line,
            "{\"message\":{\"content\":\"Operator message via ainet (typed by your human in another terminal):\\n\\nrebase on main first\",\"role\":\"user\"},\"type\":\"user\"}\n"
        );
        assert!(send_to_socket(&tmp.join("nope.sock"), DEFAULT_HEADER, "x").is_err());
        assert!(send_to_socket(Path::new(""), DEFAULT_HEADER, "x").is_err());
    }

    #[test]
    fn wire_line_escapes_like_go() {
        assert_eq!(
            wire_line("H:", "a <b> & c"),
            "{\"message\":{\"content\":\"H:\\n\\na \\u003cb\\u003e \\u0026 c\",\"role\":\"user\"},\"type\":\"user\"}\n"
        );
    }

    /// A path that is a symlink is refused, even to a socket of ours.
    /// Errors do not name the path.
    #[test]
    fn refuses_links_and_other_files() {
        let tmp = TempDir::new();
        let real = tmp.join("SECRET-real.sock");
        let rx = listen(&real);
        let link = tmp.join("SECRET-link.sock");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let file = tmp.join("SECRET-file");
        std::fs::write(&file, "x").unwrap();
        for p in [&link, &file, &tmp.join("SECRET-none")] {
            let err = send_to_socket(p, DEFAULT_HEADER, "hi").unwrap_err();
            assert!(!err.to_string().contains("SECRET"), "{err}");
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a refused send reached the socket"
        );
        send_to_socket(&real, DEFAULT_HEADER, "hi").unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_gone_session_is_refused() {
        let s = LiveSession {
            messaging_socket_path: "/nonexistent.sock".into(),
            ..Default::default()
        };
        assert!(send(&s, "x").unwrap_err().to_string().contains("gone"));
    }
}
