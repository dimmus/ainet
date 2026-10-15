//! Panes: an agent process on a pseudo-terminal the daemon owns, with its
//! screen parsed by `vt100`. This is the part of tmux ainet needs and no
//! more: spawn, write, resize, read the screen, notice the exit. Writing goes
//! straight to the PTY, so a message never races a `tmux paste-buffer`.
//!
//! A pane can also be attached ([`Pane::attach`]): the attachment hears
//! every byte the program prints after a copy of the screen, so a client
//! can keep its own copy of the screen without fetching it whole.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc::{self, UnboundedSender, error::TrySendError};

/// Rows of scrollback each pane keeps.
const SCROLLBACK: usize = 5_000;

/// Pieces of output an attachment may fall behind by before it loses
/// them (and is sent the whole screen instead, see [`Attachment::lagged`]).
const TAP_QUEUE: usize = 256;

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

/// What an attachment hears, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tap {
    /// Bytes the program printed, already in the screen.
    Bytes(Vec<u8>),
    /// The pane took a new size: the screen is laid out anew.
    Resized,
}

/// The screen and who listens to it, under one lock, so a copy of the
/// screen and the bytes after it never overlap or leave a gap.
struct Term {
    parser: vt100::Parser,
    taps: Vec<Listener>,
    /// Set once the program's output has ended: nobody would ever tell a
    /// listener added after that, so none is added.
    done: bool,
}

struct Listener {
    tx: mpsc::Sender<Tap>,
    /// Set when output had to be dropped because the listener fell behind.
    lagged: Arc<AtomicBool>,
}

impl Term {
    /// Tells every listener; forgets those that went away.
    fn tell(&mut self, tap: Tap) {
        self.taps.retain(|l| match l.tx.try_send(tap.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                l.lagged.store(true, Ordering::Release);
                true
            }
            Err(TrySendError::Closed(_)) => false,
        });
    }
}

/// The screen of a pane as a client starts from: escape sequences that
/// draw it on a fresh `vt100::Parser` of `size`, and its rows as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub ansi: String,
    pub size: (u16, u16),
    pub rows: Vec<String>,
}

impl Snapshot {
    fn of(parser: &vt100::Parser) -> Self {
        let screen = parser.screen();
        Snapshot {
            ansi: String::from_utf8_lossy(&screen.state_formatted()).into_owned(),
            size: screen.size(),
            rows: plain_rows(screen),
        }
    }
}

/// A screen's rows as text, trailing blank rows dropped.
fn plain_rows(screen: &vt100::Screen) -> Vec<String> {
    let (_, cols) = screen.size();
    let mut rows: Vec<String> = screen.rows(0, cols).collect();
    while rows.last().is_some_and(|r| r.trim().is_empty()) {
        rows.pop();
    }
    rows
}

/// One visible row of `screen`, its first `cols` cells, as text with SGR
/// sequences: it starts from the default attributes and ends reset, so it
/// draws the same wherever it is printed. Trailing cells that show
/// nothing are dropped. How ainetd sends scrollback with its colours.
pub fn styled_row(screen: &vt100::Screen, row: u16, cols: u16) -> String {
    let cells: Vec<&vt100::Cell> = (0..cols.min(screen.size().1))
        .filter_map(|col| screen.cell(row, col))
        .collect();
    let shows = |c: &vt100::Cell| {
        c.has_contents() || c.bgcolor() != vt100::Color::Default || c.inverse() || c.underline()
    };
    let end = cells.iter().rposition(|c| shows(c)).map_or(0, |i| i + 1);
    let mut out = String::new();
    let mut current = Style::default();
    for cell in &cells[..end] {
        if cell.is_wide_continuation() {
            continue;
        }
        let style = Style::of(cell);
        if style != current {
            style.write(&mut out);
            current = style;
        }
        match cell.contents() {
            "" => out.push(' '),
            text => out.push_str(text),
        }
    }
    if current != Style::default() {
        out.push_str("\x1b[m");
    }
    out
}

/// The attributes of a cell that [`styled_row`] keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Style {
    fg: vt100::Color,
    bg: vt100::Color,
    /// Bold, dim, italic, underline, inverse.
    flags: [bool; 5],
}

impl Default for Style {
    fn default() -> Self {
        Style {
            fg: vt100::Color::Default,
            bg: vt100::Color::Default,
            flags: [false; 5],
        }
    }
}

impl Style {
    fn of(cell: &vt100::Cell) -> Self {
        Style {
            fg: cell.fgcolor(),
            bg: cell.bgcolor(),
            flags: [
                cell.bold(),
                cell.dim(),
                cell.italic(),
                cell.underline(),
                cell.inverse(),
            ],
        }
    }

    /// The SGR sequence that sets exactly this style, from a reset.
    fn write(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (on, code) in self.flags.iter().zip(["1", "2", "3", "4", "7"]) {
            if *on {
                out.push(';');
                out.push_str(code);
            }
        }
        color(out, self.fg, 30);
        color(out, self.bg, 40);
        out.push('m');
    }
}

/// A colour's SGR parameters: `base` is 30 for text, 40 for background.
fn color(out: &mut String, color: vt100::Color, base: u8) {
    match color {
        vt100::Color::Default => {}
        vt100::Color::Idx(i) if i < 8 => out.push_str(&format!(";{}", base + i)),
        vt100::Color::Idx(i) if i < 16 => out.push_str(&format!(";{}", base + 60 + i - 8)),
        vt100::Color::Idx(i) => out.push_str(&format!(";{};5;{i}", base + 8)),
        vt100::Color::Rgb(r, g, b) => out.push_str(&format!(";{};2;{r};{g};{b}", base + 8)),
    }
}

/// A listener on a pane's output ([`Pane::attach`]). Dropping it detaches.
pub struct Attachment {
    term: Arc<Mutex<Term>>,
    rx: mpsc::Receiver<Tap>,
    lagged: Arc<AtomicBool>,
}

impl Attachment {
    /// What happened next; `None` once the program has ended and
    /// everything before was heard.
    pub async fn recv(&mut self) -> Option<Tap> {
        self.rx.recv().await
    }

    /// Whether output was lost because this listener fell behind: only
    /// [`Attachment::resync`] gets it back in step.
    pub fn lagged(&self) -> bool {
        self.lagged.load(Ordering::Acquire)
    }

    /// The whole screen as it is now. Everything still waiting to be heard
    /// is already in it, so it is dropped: what comes next follows on from
    /// this screen.
    pub fn resync(&mut self) -> Snapshot {
        let term = lock(&self.term);
        while self.rx.try_recv().is_ok() {}
        self.lagged.store(false, Ordering::Release);
        Snapshot::of(&term.parser)
    }
}

pub struct Pane {
    id: String,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    screen: Arc<Mutex<Term>>,
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
        let screen = Arc::new(Mutex::new(Term {
            parser: vt100::Parser::new(launch.rows, launch.cols, SCROLLBACK),
            taps: Vec::new(),
            done: false,
        }));

        let term = Arc::clone(&screen);
        let pane_id = id.clone();
        thread::Builder::new().name(format!("pane-{id}")).spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut term = lock(&term);
                        term.parser.process(&buf[..n]);
                        if !term.taps.is_empty() {
                            term.tell(Tap::Bytes(buf[..n].to_vec()));
                        }
                        drop(term);
                        let _ = events.send(PaneEvent::Output {
                            pane: pane_id.clone(),
                        });
                    }
                }
            }
            // Attachments hear the end once they have heard the rest.
            let mut term = lock(&term);
            term.taps.clear();
            term.done = true;
            drop(term);
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
        let bracketed = lock(&self.screen).parser.screen().bracketed_paste();
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
        let mut term = lock(&self.screen);
        term.parser.screen_mut().set_size(rows, cols);
        if !term.taps.is_empty() {
            term.tell(Tap::Resized);
        }
        Ok(())
    }

    /// The visible screen as plain text rows, trailing blank rows dropped.
    pub fn rows(&self) -> Vec<String> {
        plain_rows(lock(&self.screen).parser.screen())
    }

    /// The screen with colours and attributes, for a live preview.
    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> R {
        f(lock(&self.screen).parser.screen())
    }

    /// The whole screen state as escape sequences, and its size: feed it to
    /// a fresh `vt100::Parser` of that size to get the same screen.
    pub fn formatted(&self) -> (String, (u16, u16)) {
        let snapshot = Snapshot::of(&lock(&self.screen).parser);
        (snapshot.ansi, snapshot.size)
    }

    /// Starts listening to the program's output: the screen as it is now,
    /// and an [`Attachment`] that hears everything printed after it (and
    /// when the pane is resized), in order. Once the program has ended
    /// the attachment hears only that.
    pub fn attach(&self) -> (Snapshot, Attachment) {
        let (tx, rx) = mpsc::channel(TAP_QUEUE);
        let lagged = Arc::new(AtomicBool::new(false));
        let mut term = lock(&self.screen);
        // Dropping the sender ends the attachment after the snapshot.
        if !term.done {
            term.taps.push(Listener {
                tx,
                lagged: Arc::clone(&lagged),
            });
        }
        let snapshot = Snapshot::of(&term.parser);
        drop(term);
        let attachment = Attachment {
            term: Arc::clone(&self.screen),
            rx,
            lagged,
        };
        (snapshot, attachment)
    }

    /// How many attachments still listen.
    pub fn attached(&self) -> usize {
        lock(&self.screen)
            .taps
            .iter()
            .filter(|l| !l.tx.is_closed())
            .count()
    }

    /// Up to `history` scrollback rows followed by the visible screen, as
    /// text. Trailing blank rows are dropped.
    pub fn history(&self, history: usize) -> Vec<String> {
        self.read_history(history, false).0
    }

    /// [`Pane::history`], and each of its rows with its colours and
    /// attributes ([`styled_row`]).
    pub fn history_styled(&self, history: usize) -> (Vec<String>, Vec<String>) {
        self.read_history(history, true)
    }

    fn read_history(&self, history: usize, styled: bool) -> (Vec<String>, Vec<String>) {
        let mut term = lock(&self.screen);
        let parser = &mut term.parser;
        let (rows, cols) = parser.screen().size();
        let rows = rows as usize;
        let mut out = Vec::new();
        let mut styles = Vec::new();
        let mut read = |screen: &vt100::Screen, take: usize| {
            out.extend(screen.rows(0, cols).take(take));
            if styled {
                styles.extend((0..take as u16).map(|row| styled_row(screen, row, cols)));
            }
        };
        // Find out how much scrollback there is, then read it a screen at a time.
        parser.screen_mut().set_scrollback(usize::MAX);
        let available = parser.screen().scrollback().min(history);
        let mut offset = available;
        while offset > 0 {
            parser.screen_mut().set_scrollback(offset);
            let take = offset.min(rows);
            read(parser.screen(), take);
            offset -= take;
        }
        parser.screen_mut().set_scrollback(0);
        read(parser.screen(), rows);
        while out.last().is_some_and(|r| r.trim().is_empty()) {
            out.pop();
        }
        styles.truncate(out.len());
        (out, styles)
    }

    pub fn size(&self) -> (u16, u16) {
        lock(&self.screen).parser.screen().size()
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
        // Attachments outlive the pane only to hear that it ended.
        let mut term = lock(&self.screen);
        term.taps.clear();
        term.done = true;
    }
}

/// A poisoned lock only means a reader thread panicked mid-parse; the screen
/// is still the best we have.
fn lock(m: &Mutex<Term>) -> MutexGuard<'_, Term> {
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

    fn shell(id: &str, script: &str, tx: UnboundedSender<PaneEvent>) -> Pane {
        let args = vec!["-c".to_string(), script.to_string()];
        Pane::spawn(
            id,
            Launch {
                program: "sh",
                args: &args,
                cwd: Path::new("/"),
                env: &[],
                remove: &[],
                rows: 6,
                cols: 30,
            },
            tx,
        )
        .unwrap()
    }

    /// What an attachment hears until the program ends, played onto
    /// `copy`.
    async fn follow(attachment: &mut Attachment, copy: &mut vt100::Parser) {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), attachment.recv()).await {
                Ok(Some(Tap::Bytes(bytes))) => copy.process(&bytes),
                Ok(Some(Tap::Resized)) => {
                    let snapshot = attachment.resync();
                    *copy = vt100::Parser::new(snapshot.size.0, snapshot.size.1, 0);
                    copy.process(snapshot.ansi.as_bytes());
                }
                Ok(None) => return,
                Err(_) => panic!("the pane never ended"),
            }
        }
    }

    #[tokio::test]
    async fn an_attachment_keeps_a_copy_of_the_screen() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let pane = shell(
            "a",
            "printf 'first\\n'; read x; printf '\\033[31mred\\033[0m %s\\n' \"$x\"",
            tx,
        );
        // Whatever was printed before attaching is in the snapshot.
        let mut snapshot = pane.attach().0;
        for _ in 0..50 {
            if snapshot.rows.iter().any(|r| r == "first") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            snapshot = pane.attach().0;
        }
        assert_eq!(snapshot.rows.first().map(String::as_str), Some("first"));
        let (snapshot, mut attachment) = pane.attach();
        let mut copy = vt100::Parser::new(snapshot.size.0, snapshot.size.1, 0);
        copy.process(snapshot.ansi.as_bytes());
        let mut pane = pane;
        pane.write(b"typed\r").unwrap();
        follow(&mut attachment, &mut copy).await;
        assert!(
            copy.screen().contents().contains("red typed"),
            "{}",
            copy.screen().contents()
        );
        assert_eq!(copy.screen().contents(), pane.with_screen(|s| s.contents()));
        assert_eq!(copy.screen().cell(2, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
    }

    #[tokio::test]
    async fn resizes_and_lag_send_the_whole_screen_again() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pane = shell("r", "read x; seq 1 300000; read y", tx);
        let (_, mut attachment) = pane.attach();
        pane.resize(8, 40).unwrap();
        let heard = tokio::time::timeout(Duration::from_secs(5), attachment.recv()).await;
        assert_eq!(heard.unwrap(), Some(Tap::Resized));
        // Far more output than the queue holds, unheard: it lags, and a
        // resync puts it back in step with the screen as it is.
        pane.write(b"go\r").unwrap();
        for _ in 0..500 {
            if attachment.lagged() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(attachment.lagged());
        let mut snapshot = attachment.resync();
        for _ in 0..500 {
            if snapshot.rows.iter().any(|r| r == "300000") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            snapshot = attachment.resync();
        }
        assert!(!attachment.lagged());
        assert_eq!(snapshot.size, (8, 40));
        assert!(snapshot.rows.iter().any(|r| r == "300000"), "{:?}", snapshot.rows);
        // Only what came after the resync is heard next.
        pane.write(b"done\r").unwrap();
        let mut copy = vt100::Parser::new(8, 40, 0);
        copy.process(snapshot.ansi.as_bytes());
        follow(&mut attachment, &mut copy).await;
        assert_eq!(copy.screen().contents(), pane.with_screen(|s| s.contents()));
    }

    #[tokio::test]
    async fn dropping_an_attachment_detaches_it() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pane = shell("d", "while read x; do echo \"$x\"; done", tx);
        let (_, first) = pane.attach();
        let (_, _second) = pane.attach();
        assert_eq!(pane.attached(), 2);
        drop(first);
        assert_eq!(pane.attached(), 1);
        // The next output forgets it for good.
        pane.write(b"hi\r").unwrap();
        for _ in 0..100 {
            if lock(&pane.screen).taps.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(lock(&pane.screen).taps.len(), 1);
    }

    #[tokio::test]
    async fn attaching_after_the_end_hears_the_end() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let pane = shell("e", "printf bye", tx);
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(PaneEvent::Exited { .. })) => break,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => panic!("the pane never ended"),
            }
        }
        let (snapshot, mut attachment) = pane.attach();
        assert_eq!(snapshot.rows.first().map(String::as_str), Some("bye"));
        let heard = tokio::time::timeout(Duration::from_secs(5), attachment.recv()).await;
        assert_eq!(heard.expect("the attachment never ended"), None);
        assert_eq!(pane.attached(), 0);
    }

    #[tokio::test]
    async fn dropping_the_pane_ends_its_attachments() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let pane = shell("p", "read x", tx);
        let (_, mut attachment) = pane.attach();
        drop(pane);
        let heard = tokio::time::timeout(Duration::from_secs(5), attachment.recv()).await;
        assert_eq!(heard.expect("the attachment never ended"), None);
    }

    #[test]
    fn styled_rows_draw_the_same_anywhere() {
        let mut parser = vt100::Parser::new(3, 20, 0);
        parser.process(
            "plain \x1b[1;31mbold red\x1b[0m\r\n\x1b[44m  \x1b[0m\x1b[38;5;200mx\x1b[38;2;1;2;3my\x1b[0m 世\r\n"
                .as_bytes(),
        );
        let screen = parser.screen();
        let rows: Vec<String> = (0..3).map(|r| styled_row(screen, r, 20)).collect();
        assert_eq!(rows[0], "plain \x1b[0;1;31mbold red\x1b[m");
        assert_eq!(rows[2], "", "a blank row is nothing");
        // Printed at another row of another screen, each row looks as it did.
        let mut copy = vt100::Parser::new(5, 20, 0);
        copy.process(format!("\x1b[4;1H{}\x1b[5;1H{}", rows[0], rows[1]).as_bytes());
        for (from, to) in [(0, 3), (1, 4)] {
            for col in 0..20 {
                let (a, b) = (
                    screen.cell(from, col).unwrap(),
                    copy.screen().cell(to, col).unwrap(),
                );
                assert_eq!(a.contents(), b.contents(), "({from}, {col})");
                assert_eq!(
                    (a.fgcolor(), a.bgcolor(), a.bold()),
                    (b.fgcolor(), b.bgcolor(), b.bold())
                );
            }
        }
    }
}
