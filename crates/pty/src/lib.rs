//! Panes: an agent process on a pseudo-terminal the daemon owns, with its
//! screen parsed by `vt100`. This is the part of tmux ainet needs and no
//! more: spawn, write, resize, read the screen, notice the exit. Writing goes
//! straight to the PTY, so a message never races a `tmux paste-buffer`.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc::UnboundedSender;

/// Rows of scrollback each pane keeps.
const SCROLLBACK: usize = 5_000;

/// How long to wait between a paste and the Enter that submits it.
pub const PASTE_SETTLE: Duration = Duration::from_millis(60);

/// What a pane reports back to its owner.
#[derive(Debug, Clone)]
pub enum PaneEvent {
    /// New bytes were parsed into the screen.
    Output { pane: String },
    /// The process ended (the PTY reached EOF).
    Exited { pane: String },
}

/// What to run in a new pane.
pub struct Launch<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    /// Variables to remove from the inherited environment.
    pub remove: &'a [String],
    pub rows: u16,
    pub cols: u16,
}

pub struct Pane {
    id: String,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
}

impl Pane {
    /// Starts `launch.program` on a fresh PTY. A reader thread parses its
    /// output and reports through `events`.
    pub fn spawn(
        id: impl Into<String>,
        launch: Launch<'_>,
        events: UnboundedSender<PaneEvent>,
    ) -> Result<Self> {
        let id = id.into();
        let size = PtySize {
            rows: launch.rows,
            cols: launch.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system().openpty(size).context("open pty")?;

        let mut cmd = CommandBuilder::new(launch.program);
        cmd.args(launch.args);
        cmd.cwd(launch.cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for k in launch.remove {
            cmd.env_remove(k);
        }
        for (k, v) in launch.env {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {}", launch.program))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer = pair.master.take_writer().context("pty writer")?;
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            launch.rows,
            launch.cols,
            SCROLLBACK,
        )));

        let parser = Arc::clone(&screen);
        let pane_id = id.clone();
        thread::Builder::new().name(format!("pane-{id}")).spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        lock(&parser).process(&buf[..n]);
                        let _ = events.send(PaneEvent::Output {
                            pane: pane_id.clone(),
                        });
                    }
                }
            }
            let _ = events.send(PaneEvent::Exited { pane: pane_id });
        })?;

        Ok(Pane {
            id,
            child,
            master: pair.master,
            writer,
            screen,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Writes raw bytes, as if typed.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Pastes a message: bracketed when the program asked for it, so newlines
    /// stay inside the message and nothing is taken as a key binding. Follow
    /// with [`Pane::enter`] after [`PASTE_SETTLE`], once the agent's input box
    /// has taken the paste.
    pub fn paste(&mut self, text: &str) -> Result<()> {
        let bracketed = lock(&self.screen).screen().bracketed_paste();
        if bracketed {
            let mut bytes = Vec::with_capacity(text.len() + 12);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.write(&bytes)
        } else {
            self.write(text.replace('\n', "\r").as_bytes())
        }
    }

    pub fn enter(&mut self) -> Result<()> {
        self.write(b"\r")
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        lock(&self.screen).screen_mut().set_size(rows, cols);
        Ok(())
    }

    /// The visible screen as plain text rows, trailing blank rows dropped.
    pub fn rows(&self) -> Vec<String> {
        let parser = lock(&self.screen);
        let screen = parser.screen();
        let (_, cols) = screen.size();
        let mut rows: Vec<String> = screen.rows(0, cols).collect();
        while rows.last().is_some_and(|r| r.trim().is_empty()) {
            rows.pop();
        }
        rows
    }

    /// The screen with colours and attributes, for a live preview.
    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> R {
        f(lock(&self.screen).screen())
    }

    /// The whole screen state as escape sequences, and its size: feed it to
    /// a fresh `vt100::Parser` of that size to get the same screen.
    pub fn formatted(&self) -> (String, (u16, u16)) {
        let parser = lock(&self.screen);
        let screen = parser.screen();
        (
            String::from_utf8_lossy(&screen.state_formatted()).into_owned(),
            screen.size(),
        )
    }

    /// Up to `history` scrollback rows followed by the visible screen, as
    /// text. Trailing blank rows are dropped.
    pub fn history(&self, history: usize) -> Vec<String> {
        let mut parser = lock(&self.screen);
        let (rows, cols) = parser.screen().size();
        let rows = rows as usize;
        // Find out how much scrollback there is, then read it a screen at a time.
        parser.screen_mut().set_scrollback(usize::MAX);
        let available = parser.screen().scrollback().min(history);
        let mut out = Vec::with_capacity(available + rows);
        let mut offset = available;
        while offset > 0 {
            parser.screen_mut().set_scrollback(offset);
            let take = offset.min(rows);
            out.extend(parser.screen().rows(0, cols).take(take));
            offset -= take;
        }
        parser.screen_mut().set_scrollback(0);
        out.extend(parser.screen().rows(0, cols));
        while out.last().is_some_and(|r| r.trim().is_empty()) {
            out.pop();
        }
        out
    }

    pub fn size(&self) -> (u16, u16) {
        lock(&self.screen).screen().size()
    }

    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub fn kill(&mut self) -> Result<()> {
        self.child.kill()?;
        Ok(())
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if !self.has_exited() {
            let _ = self.child.kill();
        }
    }
}

/// A poisoned lock only means a reader thread panicked mid-parse; the screen
/// is still the best we have.
fn lock(m: &Mutex<vt100::Parser>) -> MutexGuard<'_, vt100::Parser> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_lands_on_the_screen() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let args = vec!["-c".to_string(), "printf 'hello from a pane'".to_string()];
        let pane = Pane::spawn(
            "t",
            Launch {
                program: "sh",
                args: &args,
                cwd: Path::new("/"),
                env: &[],
                remove: &[],
                rows: 10,
                cols: 40,
            },
            tx,
        )
        .unwrap();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(PaneEvent::Exited { .. })) | Ok(None) => break,
                Ok(Some(_)) => {}
                Err(_) => panic!("pane never exited"),
            }
        }
        assert!(
            pane.rows().iter().any(|r| r.contains("hello from a pane")),
            "{:?}",
            pane.rows()
        );
    }

    #[tokio::test]
    async fn history_reaches_above_the_screen() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let args = vec![
            "-c".to_string(),
            "for i in $(seq 1 40); do echo line-$i; done".to_string(),
        ];
        let pane = Pane::spawn(
            "h",
            Launch {
                program: "sh",
                args: &args,
                cwd: Path::new("/"),
                env: &[],
                remove: &[],
                rows: 10,
                cols: 40,
            },
            tx,
        )
        .unwrap();
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            if matches!(ev, PaneEvent::Exited { .. }) {
                break;
            }
        }
        let all = pane.history(100);
        assert_eq!(all.first().map(String::as_str), Some("line-1"), "{all:?}");
        assert!(all.iter().any(|r| r == "line-40"));
        let (ansi, size) = pane.formatted();
        assert_eq!(size, (10, 40));
        let mut copy = vt100::Parser::new(size.0, size.1, 0);
        copy.process(ansi.as_bytes());
        assert_eq!(copy.screen().contents(), pane.with_screen(|s| s.contents()));
    }
}
