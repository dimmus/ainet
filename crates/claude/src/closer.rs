//! Closes Claude Code sessions on purpose and safely.
//!
//! Ported from Den's `internal/closer` (MIT): the safety rules, the process
//! identity check and the stop sequence. Den's terminal-window handling
//! (Hyprland window markers) is not ported.
//!
//! - Never close the orchestrator's session, the daemon itself, or any
//!   process the daemon runs inside; a target with no pid, or already gone,
//!   is refused; a working or waiting session only with `force`.
//! - The pid in a sessions file is only a hint: Claude Code may have crashed
//!   and left the file, and the pid may since belong to anything else of the
//!   user's. Before signalling, the process is opened as a pidfd and checked
//!   through `/proc` to be Claude Code and no younger than the session
//!   ([`crate::sessions::check_identity`]); every signal then goes through
//!   the pidfd, so never to whatever takes the pid next. When it cannot be
//!   sure, it refuses rather than guess.
//! - SIGTERM, a wait ([`Waits::stop`], 5 s), and SIGKILL only when forced
//!   ([`Waits::kill`], 2 s). Only the agent's own process is signalled,
//!   never its terminal. A wait cut short (cancelled) is never escalated.
//! - Closes never overlap, and several targets close one at a time with a
//!   pause between (500 ms), so terminals do not tear down all at once.

use std::os::fd::OwnedFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal};

use crate::sessions::{self, IdentityError};

/// One session that may be closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    pub session_id: String,
    /// What to call it in messages.
    pub name: String,
    pub pid: i32,
    /// The session's `startedAt` (Unix ms), for the identity check; 0 looks
    /// it up in the sessions files by `session_id`.
    pub started_at_ms: i64,
    /// "working", "needs you", "idle", "unknown" or "gone".
    pub status: String,
}

/// What must never be closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Protected {
    /// The daemon's own pid.
    pub own_pid: i32,
    /// The orchestrator's session id, if one runs.
    pub orchestrator_session: Option<String>,
    /// Pids above the daemon's process (see [`Protected::current`]).
    pub ancestors: Vec<i32>,
}

impl Protected {
    /// This process and its ancestors.
    pub fn current(orchestrator_session: Option<String>) -> Protected {
        let me = std::process::id() as i32;
        Protected {
            own_pid: me,
            orchestrator_session,
            ancestors: sessions::ancestors(me)
                .into_iter()
                .map(|p| p.pid)
                .filter(|p| *p != me)
                .collect(),
        }
    }
}

/// Why `t` will not be closed, `None` when it may be.
pub fn refusal(t: &Target, force: bool, protect: &Protected) -> Option<String> {
    let busy = t.status == "working" || t.status == "needs you";
    if !t.session_id.is_empty() && protect.orchestrator_session.as_deref() == Some(t.session_id.as_str()) {
        Some("it is the orchestrator".into())
    } else if t.pid != 0 && t.pid == protect.own_pid {
        Some("it is ainet itself".into())
    } else if t.pid != 0 && protect.ancestors.contains(&t.pid) {
        Some("ainet runs inside it".into())
    } else if t.pid == 0 {
        Some("no process to close".into())
    } else if t.status == "gone" {
        Some("already gone".into())
    } else if busy && !force {
        Some(format!("{} · add --force to close it anyway", t.status))
    } else {
        None
    }
}

/// What a confirmation shows and [`run`] carries out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub targets: Vec<Target>,
    /// Refused targets and why.
    pub refused: Vec<(Target, String)>,
    /// Close working ones too, and kill what does not stop.
    pub force: bool,
}

/// Applies the safety rules to the candidates, keeping their order.
pub fn plan(candidates: Vec<Target>, force: bool, protect: &Protected) -> Plan {
    let mut p = Plan {
        force,
        ..Default::default()
    };
    for t in candidates {
        match refusal(&t, force, protect) {
            Some(why) => p.refused.push((t, why)),
            None => p.targets.push(t),
        }
    }
    p
}

/// How long each step waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waits {
    /// After SIGTERM.
    pub stop: Duration,
    /// After SIGKILL.
    pub kill: Duration,
    /// Between two targets.
    pub between: Duration,
    /// How often to look.
    pub poll: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Waits {
            stop: Duration::from_secs(5),
            kill: Duration::from_secs(2),
            between: Duration::from_millis(500),
            poll: Duration::from_millis(100),
        }
    }
}

/// Why a close did not (certainly) end its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseError {
    /// The pid is not (certainly) the session's Claude Code; nothing was
    /// signalled.
    Identity(IdentityError),
    /// The process could not be held safely; nothing was signalled.
    Hold(String),
    /// A signal failed.
    Signal(String),
    /// The wait was cut short: it was asked to stop, and whether it did is
    /// not known. It was not killed.
    NotKnown,
    /// It did not stop within the wait and was not forced: left running.
    StillRunning,
}

impl std::fmt::Display for CloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloseError::Identity(e) => write!(f, "{e}; nothing was signalled"),
            CloseError::Hold(e) => write!(f, "cannot hold the process safely ({e}); nothing was signalled"),
            CloseError::Signal(e) => write!(f, "signal: {e}"),
            CloseError::NotKnown => {
                write!(
                    f,
                    "the wait was cut short; it was asked to stop, and whether it did is not known"
                )
            }
            CloseError::StillRunning => write!(f, "it did not stop; it was left running"),
        }
    }
}

impl std::error::Error for CloseError {}

/// What happened to one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub target: Target,
    /// The process is gone.
    pub ended: bool,
    /// It needed SIGKILL.
    pub killed: bool,
    pub error: Option<CloseError>,
}

/// A process opened as a pidfd: signals go to it alone, whoever has its pid
/// later.
#[derive(Debug)]
pub struct Held {
    fd: OwnedFd,
}

impl Held {
    /// Opens `pid` as a pidfd **without** any identity check. Use [`hold`]
    /// for a session's process.
    pub fn open(pid: i32) -> Result<Held, CloseError> {
        let p = Pid::from_raw(pid).ok_or(CloseError::Identity(IdentityError::NotAnAgent(pid)))?;
        match rustix::process::pidfd_open(p, PidfdFlags::empty()) {
            Ok(fd) => Ok(Held { fd }),
            Err(rustix::io::Errno::SRCH) => Err(CloseError::Identity(IdentityError::Gone)),
            Err(e) => Err(CloseError::Hold(format!("pidfd: {e}"))),
        }
    }

    pub fn signal(&self, sig: Signal) -> Result<(), rustix::io::Errno> {
        rustix::process::pidfd_send_signal(&self.fd, sig)
    }

    /// Whether the held process is still running: a pidfd turns readable
    /// when its process exits.
    pub fn alive(&self) -> bool {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
        match poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        ) {
            Ok(0) => true,
            Ok(_) => !fds[0].revents().contains(PollFlags::IN),
            Err(_) => true,
        }
    }
}

/// Opens the target's process after checking it is the session's Claude
/// Code (see the module docs). `started_at_ms` 0 is looked up in the
/// sessions files by `session_id`.
pub fn hold(pid: i32, session_id: &str, started_at_ms: i64) -> Result<Held, CloseError> {
    if pid <= 1 {
        return Err(CloseError::Identity(IdentityError::NotAnAgent(pid)));
    }
    let started = if started_at_ms > 0 {
        started_at_ms
    } else {
        sessions::session_by_id(session_id)
            .map(|s| s.started_at)
            .unwrap_or(0)
    };
    if started <= 0 {
        return Err(CloseError::Identity(IdentityError::StartUnknown));
    }
    let h = Held::open(pid)?;
    sessions::check_identity(pid, started).map_err(CloseError::Identity)?;
    if !h.alive() {
        return Err(CloseError::Identity(IdentityError::Gone));
    }
    Ok(h)
}

/// Closes one target: hold and check it, SIGTERM, wait, SIGKILL when
/// forced. `cancel` cuts the waits short (never escalating).
pub fn close(t: &Target, force: bool, waits: &Waits, cancel: Option<&AtomicBool>) -> Outcome {
    let out = |ended, error| Outcome {
        target: t.clone(),
        ended,
        killed: false,
        error,
    };
    match hold(t.pid, &t.session_id, t.started_at_ms) {
        Ok(h) => Outcome {
            target: t.clone(),
            ..close_held(&h, force, waits, cancel)
        },
        Err(CloseError::Identity(IdentityError::Gone)) => out(true, None),
        Err(e) => out(false, Some(e)),
    }
}

/// The stop sequence on a held process. The returned target is empty.
pub fn close_held(h: &Held, force: bool, waits: &Waits, cancel: Option<&AtomicBool>) -> Outcome {
    let mut o = Outcome {
        target: Target::default(),
        ended: false,
        killed: false,
        error: None,
    };
    match h.signal(Signal::TERM) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(e) => {
            o.error = Some(CloseError::Signal(e.to_string()));
            return o;
        }
    }
    let (gone, cut) = wait_gone(h, waits.stop, waits.poll, cancel);
    o.ended = gone;
    if gone {
        return o;
    }
    if cut {
        o.error = Some(CloseError::NotKnown);
        return o;
    }
    if !force {
        o.error = Some(CloseError::StillRunning);
        return o;
    }
    match h.signal(Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {
            let (gone, cut) = wait_gone(h, waits.kill, waits.poll, cancel);
            o.ended = gone;
            o.killed = gone;
            if !gone {
                o.error = Some(if cut {
                    CloseError::NotKnown
                } else {
                    CloseError::StillRunning
                });
            }
        }
        Err(e) => o.error = Some(CloseError::Signal(e.to_string())),
    }
    o
}

fn wait_gone(h: &Held, wait: Duration, poll: Duration, cancel: Option<&AtomicBool>) -> (bool, bool) {
    let deadline = Instant::now() + wait;
    loop {
        if !h.alive() {
            return (true, false);
        }
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            return (!h.alive(), true);
        }
        if Instant::now() >= deadline {
            return (false, false);
        }
        std::thread::sleep(poll);
    }
}

static RUN: Mutex<()> = Mutex::new(());

/// Carries a plan out, one target at a time with [`Waits::between`] between
/// them. Runs never overlap. Blocking: run it off the async runtime.
pub fn run(plan: &Plan, waits: &Waits, cancel: Option<&AtomicBool>) -> Vec<Outcome> {
    let _guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for (i, t) in plan.targets.iter().enumerate() {
        if i > 0 && !cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            std::thread::sleep(waits.between);
        }
        out.push(close(t, plan.force, waits, cancel));
    }
    out
}

/// The one-line outcome: "closed A · killed B · still running: C (why)".
pub fn summary(outcomes: &[Outcome]) -> String {
    let (mut ended, mut killed, mut left) = (Vec::new(), Vec::new(), Vec::new());
    for o in outcomes {
        if o.killed {
            killed.push(o.target.name.clone());
        } else if o.ended {
            ended.push(o.target.name.clone());
        } else {
            left.push(match &o.error {
                Some(e) => format!("{} ({e})", o.target.name),
                None => o.target.name.clone(),
            });
        }
    }
    let mut parts = Vec::new();
    if !ended.is_empty() {
        parts.push(format!("closed {}", ended.join(", ")));
    }
    if !killed.is_empty() {
        parts.push(format!("killed {}", killed.join(", ")));
    }
    if !left.is_empty() {
        parts.push(format!("still running: {}", left.join(", ")));
    }
    if parts.is_empty() {
        "nothing closed".into()
    } else {
        parts.join(" · ")
    }
}

/// Reads an idle duration such as "30m", "1h", "1h30m", "90s" or "2d".
pub fn parse_idle(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();
    if let Some(d) = s.strip_suffix('d') {
        return d
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(|n| Duration::from_secs(n * 86_400));
    }
    let mut total = 0u64;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        total += n * match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
    }
    (num.is_empty() && total > 0).then(|| Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn t(session: &str, name: &str, pid: i32, status: &str) -> Target {
        Target {
            session_id: session.into(),
            name: name.into(),
            pid,
            started_at_ms: 1,
            status: status.into(),
        }
    }

    fn cands() -> Vec<Target> {
        vec![
            t("a", "Mossy Lemur", 11, "idle"),
            t("b", "Snug Capybara", 12, "working"),
            t("orc", "orchestrator", 13, "idle"),
            t("me", "ainet", 99, "idle"),
            t("up", "Shell", 7, "idle"),
            t("g", "Comet Newt", 14, "gone"),
            t("n", "Plucky", 15, "needs you"),
            t("z", "No Pid", 0, "idle"),
        ]
    }

    fn protect() -> Protected {
        Protected {
            own_pid: 99,
            orchestrator_session: Some("orc".into()),
            ancestors: vec![7],
        }
    }

    #[test]
    fn plan_applies_the_safety_rules() {
        let p = plan(cands(), false, &protect());
        assert_eq!(
            p.targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["Mossy Lemur"]
        );
        let why: std::collections::HashMap<&str, &str> = p
            .refused
            .iter()
            .map(|(t, w)| (t.name.as_str(), w.as_str()))
            .collect();
        for (name, want) in [
            ("Snug Capybara", "working · add --force"),
            ("orchestrator", "it is the orchestrator"),
            ("ainet", "it is ainet itself"),
            ("Shell", "ainet runs inside it"),
            ("Comet Newt", "already gone"),
            ("Plucky", "needs you · add --force"),
            ("No Pid", "no process to close"),
        ] {
            assert!(why[name].starts_with(want), "{name}: {}", why[name]);
        }
        let f = plan(cands(), true, &protect());
        assert_eq!(
            f.targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["Mossy Lemur", "Snug Capybara", "Plucky"]
        );
    }

    #[test]
    fn hold_refuses_what_is_not_claude() {
        let me = std::process::id() as i32;
        assert!(matches!(
            hold(me, "", crate::util::now_ms()),
            Err(CloseError::Identity(IdentityError::NotClaude(_)))
        ));
        assert!(matches!(
            hold(1, "", 5),
            Err(CloseError::Identity(IdentityError::NotAnAgent(1)))
        ));
        assert!(matches!(
            hold(me, "no-such-session-id", 0),
            Err(CloseError::Identity(IdentityError::StartUnknown))
        ));
        assert!(matches!(hold(999_999_999, "", 5), Err(CloseError::Identity(_))));
        // A target that is not Claude Code is never signalled.
        let o = close(&t("x", "Me", me, "idle"), true, &Waits::default(), None);
        assert!(!o.ended && matches!(o.error, Some(CloseError::Identity(_))));
        assert_eq!(summary(&[o]).split(" (").next(), Some("still running: Me"));
    }

    #[test]
    fn stop_sequence_on_a_held_process() {
        let fast = Waits {
            stop: Duration::from_millis(300),
            kill: Duration::from_secs(3),
            between: Duration::ZERO,
            poll: Duration::from_millis(20),
        };

        // Stops on SIGTERM.
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let h = Held::open(child.id() as i32).unwrap();
        assert!(h.alive());
        let o = close_held(&h, false, &fast, None);
        assert!(o.ended && !o.killed && o.error.is_none(), "{o:?}");
        child.wait().unwrap();

        // Ignores SIGTERM: left running unless forced.
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let h = Held::open(child.id() as i32).unwrap();
        let o = close_held(&h, false, &fast, None);
        assert_eq!(
            (o.ended, o.error.clone()),
            (false, Some(CloseError::StillRunning))
        );
        let o = close_held(&h, true, &fast, None);
        assert!(o.ended && o.killed, "{o:?}");
        child.wait().unwrap();

        // A cancelled wait is never escalated.
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let h = Held::open(child.id() as i32).unwrap();
        let cancel = AtomicBool::new(true);
        let o = close_held(&h, true, &fast, Some(&cancel));
        assert_eq!(
            (o.ended, o.killed, o.error),
            (false, false, Some(CloseError::NotKnown))
        );
        assert!(h.alive());
        h.signal(Signal::KILL).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn durations_and_summary() {
        assert_eq!(parse_idle("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_idle("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_idle("2d"), Some(Duration::from_secs(172_800)));
        assert_eq!(parse_idle("x"), None);
        assert_eq!(parse_idle("10"), None);
        let ok = |name: &str, ended, killed| Outcome {
            target: t("", name, 1, ""),
            ended,
            killed,
            error: None,
        };
        assert_eq!(
            summary(&[ok("A", true, false), ok("B", true, true)]),
            "closed A · killed B"
        );
        assert_eq!(summary(&[]), "nothing closed");
    }
}
