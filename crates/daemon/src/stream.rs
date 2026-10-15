//! Streamed terminals (`Request::Attach`): a connection that carries one
//! agent's screen and then every byte its program prints, so a client
//! keeps its own copy of the screen up to date for the cost of the output
//! rather than of the whole screen. The whole screen is sent again when
//! the pane is resized, when the client fell behind, and once the output
//! settles (which mends anything the client's parser took differently).

use std::time::{Duration, Instant};

use ainet_core::protocol;
use ainet_core::{Event, Response};
use ainet_pty::{Attachment, Snapshot, Tap};
use tokio::io::{AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::{Shared, lock};

/// How long the output must pause before the whole screen is sent again.
const SETTLE: Duration = Duration::from_millis(300);

/// How long output may flow without a pause before the whole screen is
/// sent anyway.
const RESYNC_EVERY: Duration = Duration::from_secs(5);

/// Starts listening to `key`'s pane: its id, the screen now and what
/// follows it, or why not (no such agent, or a terminal ainetd does not
/// own).
pub(crate) fn attach(d: &Shared, key: &str) -> Result<(String, Snapshot, Attachment), String> {
    let d = lock(d);
    let Some(id) = d.find_agent(key, None) else {
        return Err(format!("no agent {key}"));
    };
    match d.panes.get(&id) {
        Some(pane) => {
            let (snapshot, attachment) = pane.attach();
            Ok((id, snapshot, attachment))
        }
        None => Err(format!(
            "{} has no pane of ainetd's to attach to; fetch its screen instead",
            d.label(&id)
        )),
    }
}

/// Serves an attached connection until the client closes it or the
/// program ends; dropping the attachment then detaches it.
pub(crate) async fn follow(
    agent: String,
    snapshot: Snapshot,
    mut attachment: Attachment,
    lines: &mut Lines<BufReader<OwnedReadHalf>>,
    write: &mut OwnedWriteHalf,
) {
    if send(write, &screen(snapshot)).await.is_err() {
        return;
    }
    let mut text = Utf8::default();
    // Bytes sent since the last whole screen, and when that was.
    let mut unsettled = false;
    let mut last_screen = Instant::now();
    loop {
        let resync = tokio::select! {
            tap = attachment.recv() => match tap {
                // The program ended, and everything it printed was sent.
                None => return,
                // It fell behind: what it missed is in the screen.
                Some(_) if attachment.lagged() => true,
                Some(Tap::Resized) => true,
                Some(Tap::Bytes(bytes)) => {
                    let data = text.push(&bytes);
                    if !data.is_empty() {
                        let event = Event::Bytes { agent: agent.clone(), data };
                        if send(write, &Response::Event { event }).await.is_err() {
                            return;
                        }
                    }
                    unsettled = true;
                    last_screen.elapsed() > RESYNC_EVERY
                }
            },
            _ = tokio::time::sleep(SETTLE), if unsettled => true,
            // The client sends nothing more; a closed connection detaches.
            line = lines.next_line() => match line {
                Ok(Some(_)) => false,
                _ => return,
            },
        };
        if resync {
            // Bytes held back for the rest of a character are in it too.
            text = Utf8::default();
            unsettled = false;
            last_screen = Instant::now();
            if send(write, &screen(attachment.resync())).await.is_err() {
                return;
            }
        }
    }
}

fn screen(snapshot: Snapshot) -> Response {
    Response::Screen {
        rows: snapshot.rows,
        ansi: Some(snapshot.ansi),
        size: snapshot.size,
        styled_rows: None,
    }
}

async fn send(write: &mut OwnedWriteHalf, resp: &Response) -> std::io::Result<()> {
    write.write_all(&protocol::encode(resp)).await
}

/// Output turned into text piece by piece: a character split between two
/// reads waits for its end, so it is never sent as two U+FFFDs.
#[derive(Default)]
struct Utf8 {
    rest: Vec<u8>,
}

impl Utf8 {
    fn push(&mut self, bytes: &[u8]) -> String {
        let mut all = std::mem::take(&mut self.rest);
        all.extend_from_slice(bytes);
        let keep = incomplete_tail(&all);
        self.rest = all.split_off(all.len() - keep);
        String::from_utf8_lossy(&all).into_owned()
    }
}

/// How many bytes at the end of `bytes` begin a character that is not
/// complete yet.
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let b = bytes[bytes.len() - back];
        if b & 0xC0 != 0x80 {
            let len = match b {
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                _ => 1,
            };
            return if len > back { back } else { 0 };
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_split_between_reads_arrive_whole() {
        let text = "a→b😀c";
        let bytes = text.as_bytes();
        // Every way of cutting it in two, and byte by byte.
        for cut in 0..=bytes.len() {
            let mut utf8 = Utf8::default();
            let joined = utf8.push(&bytes[..cut]) + &utf8.push(&bytes[cut..]);
            assert_eq!(joined, text, "cut at {cut}");
        }
        let mut utf8 = Utf8::default();
        let joined: String = bytes.iter().map(|b| utf8.push(&[*b])).collect();
        assert_eq!(joined, text);
        // Bytes that are not UTF-8 at all still go through, replaced.
        assert_eq!(Utf8::default().push(b"x\xffy"), "x\u{fffd}y");
        assert_eq!(incomplete_tail(b"ok"), 0);
        assert_eq!(incomplete_tail(b"\xe2\x86"), 2);
    }
}
