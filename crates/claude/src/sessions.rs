//! The Claude Code sessions running on this machine.
//!
//! Ported from Den's `internal/discover`, `internal/procs` and the process
//! identity checks of `internal/closer` (MIT).
//!
//! Undocumented behaviour (Claude Code 2.1.289+, as Den relies on it):
//! Claude Code writes one JSON file per live session to
//! `$CLAUDE_CONFIG_DIR/sessions/<pid>.json` (default `~/.claude/sessions`):
//!
//! ```json
//! {"pid":4242,"sessionId":"…","cwd":"/work/proj","startedAt":1791453605000,
//!  "version":"2.1.289","kind":"…","entrypoint":"cli","name":"peer-name",
//!  "nameSource":"…","status":"busy","updatedAt":…,"statusUpdatedAt":…,
//!  "messagingSocketPath":"/…/inbox.sock","peerFeatures":["…"]}
//! ```
//!
//! Times are Unix milliseconds. `status` is `"busy"` or `"idle"`;
//! `statusUpdatedAt` is when it last changed (when the last turn started or
//! ended), which Den uses as the agent's last activity. The file is not
//! removed when Claude Code crashes, so a session counts as alive only while
//! its pid exists (`kill(pid, 0)` succeeds or fails with `EPERM`). The pid
//! is only a hint: before signalling, [`check_identity`] verifies the
//! process is Claude Code and no younger than the session.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::util;

/// One Claude Code process as its sessions file reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct LiveSession {
    pub pid: i32,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub cwd: String,
    /// Unix milliseconds.
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    pub version: String,
    pub kind: String,
    pub entrypoint: String,
    /// The session's name (`claude --name`), its peer name for messages.
    pub name: String,
    #[serde(rename = "nameSource")]
    pub name_source: String,
    /// `"busy"` or `"idle"` as Claude Code reports it; see [`LiveSession::state`].
    pub status: String,
    /// Unix milliseconds.
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    /// When the busy/idle status last changed, Unix milliseconds.
    #[serde(rename = "statusUpdatedAt")]
    pub status_updated_at: i64,
    /// The session's inbox socket (Claude Code 2.1.224+ binds one).
    #[serde(rename = "messagingSocketPath")]
    pub messaging_socket_path: String,
    #[serde(rename = "peerFeatures")]
    pub peer_features: Vec<String>,
    /// False when the pid in the file no longer exists: the file is stale and
    /// the agent is gone. Not part of the file.
    #[serde(skip)]
    pub alive: bool,
}

/// What a session is doing, as Den reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Working,
    Idle,
    /// Alive, with no busy/idle status in the file: never shown as idle.
    Unknown,
    Gone,
}

impl LiveSession {
    /// Working, idle, unknown or gone, as Den's fleet derives it.
    pub fn state(&self) -> SessionState {
        match (self.alive, self.status.as_str()) {
            (false, _) => SessionState::Gone,
            (true, "busy") => SessionState::Working,
            (true, "idle") => SessionState::Idle,
            _ => SessionState::Unknown,
        }
    }

    /// The agent's last activity (Unix ms): the last busy/idle change, else
    /// the session's start; `None` when neither is known.
    pub fn last_active_ms(&self) -> Option<i64> {
        if self.status_updated_at > 0 {
            Some(self.status_updated_at)
        } else if self.started_at > 0 {
            Some(self.started_at)
        } else {
            None
        }
    }

    /// The working directory relative to `$HOME` ("~/code/x").
    pub fn short_cwd(&self) -> String {
        short_path(&self.cwd, util::home_dir().as_deref())
    }

    /// The inbox socket, `None` when this session cannot take messages.
    pub fn socket(&self) -> Option<&Path> {
        (!self.messaging_socket_path.is_empty()).then(|| Path::new(&self.messaging_socket_path))
    }

    /// Checks that the pid is still this session's Claude Code: see
    /// [`check_identity`].
    pub fn check_identity(&self) -> Result<(), IdentityError> {
        check_identity(self.pid, self.started_at)
    }
}

fn short_path(p: &str, home: Option<&Path>) -> String {
    if let Some(h) = home.and_then(|h| h.to_str()).filter(|h| !h.is_empty())
        && let Some(rest) = p.strip_prefix(h)
    {
        return format!("~{rest}");
    }
    p.to_string()
}

/// Where Claude Code writes session files: `<config dir>/sessions`.
pub fn sessions_dir() -> PathBuf {
    util::config_dir().join("sessions")
}

/// Every session file, live ones first, then by start time. A file that does
/// not parse, or has no session id, is skipped. A missing directory is no
/// sessions.
pub fn scan() -> Vec<LiveSession> {
    scan_dir(&sessions_dir())
}

/// [`scan`] of an explicit sessions directory.
pub fn scan_dir(dir: &Path) -> Vec<LiveSession> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<LiveSession> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| !t.is_dir()).unwrap_or(false))
        .filter_map(|e| read_file(&e.path()))
        .collect();
    out.sort_by(|a, b| b.alive.cmp(&a.alive).then(a.started_at.cmp(&b.started_at)));
    out
}

/// The live sessions only.
pub fn live_sessions() -> Vec<LiveSession> {
    live_sessions_in(&sessions_dir())
}

/// [`live_sessions`] of an explicit sessions directory.
pub fn live_sessions_in(dir: &Path) -> Vec<LiveSession> {
    scan_dir(dir).into_iter().filter(|s| s.alive).collect()
}

/// The session whose file is `<pid>.json`, alive or not; `None` when there
/// is no such file or it does not parse.
pub fn session_for_pid(pid: i32) -> Option<LiveSession> {
    session_for_pid_in(&sessions_dir(), pid)
}

/// [`session_for_pid`] in an explicit sessions directory. A file named for
/// another pid is also found by its `pid` field.
pub fn session_for_pid_in(dir: &Path, pid: i32) -> Option<LiveSession> {
    if let Some(s) = read_file(&dir.join(format!("{pid}.json")))
        && s.pid == pid
    {
        return Some(s);
    }
    scan_dir(dir).into_iter().find(|s| s.pid == pid)
}

/// The newest session with this id, live ones first.
pub fn session_by_id(session_id: &str) -> Option<LiveSession> {
    session_by_id_in(&sessions_dir(), session_id)
}

/// [`session_by_id`] in an explicit sessions directory.
pub fn session_by_id_in(dir: &Path, session_id: &str) -> Option<LiveSession> {
    scan_dir(dir).into_iter().find(|s| s.session_id == session_id)
}

/// Finds a session the way `den send` and `den wait` do: a session-id
/// prefix or an exact pid first, then the session name (case-insensitive
/// prefix). A leading "@" is ignored. Live sessions are preferred.
pub fn lookup<'a>(sessions: &'a [LiveSession], query: &str) -> Option<&'a LiveSession> {
    let q = query.trim().trim_start_matches('@').trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let pick = |m: &dyn Fn(&LiveSession) -> bool| -> Option<&'a LiveSession> {
        sessions
            .iter()
            .filter(|s| m(s))
            .max_by_key(|s| (s.alive, s.started_at))
    };
    pick(&|s| s.session_id.starts_with(&q) || (s.pid > 0 && s.pid.to_string() == q))
        .or_else(|| pick(&|s| !s.name.is_empty() && s.name.to_lowercase().starts_with(&q)))
}

fn read_file(path: &Path) -> Option<LiveSession> {
    if path.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
    }
    let raw = fs::read(path).ok()?;
    let mut s: LiveSession = serde_json::from_slice(&raw).ok()?;
    if s.session_id.is_empty() {
        return None;
    }
    if s.pid == 0 {
        s.pid = path.file_stem()?.to_str()?.parse().unwrap_or(0);
    }
    s.alive = alive(s.pid);
    Some(s)
}

/// Whether a process with this pid exists: `kill(pid, 0)` succeeds or fails
/// with `EPERM` (another user's process). A zombie counts as alive.
pub fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let Some(p) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    match rustix::process::test_kill_process(p) {
        Ok(()) => true,
        Err(e) => e == rustix::io::Errno::PERM,
    }
}

/// One process in an ancestry chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: i32,
    /// The `Name:` line of `/proc/<pid>/status` (the comm).
    pub comm: String,
}

/// `pid` and its parents up to init, nearest first, from `/proc/<pid>/status`.
pub fn ancestors(pid: i32) -> Vec<Proc> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pid = pid;
    while pid > 1 && seen.insert(pid) {
        let Some((comm, ppid)) = read_status(pid) else {
            break;
        };
        out.push(Proc { pid, comm });
        pid = ppid;
    }
    out
}

/// Whether any ancestor of `pid` (itself included) has this comm, e.g.
/// "herdr" or "tmux: server".
pub fn has_ancestor(pid: i32, comm: &str) -> bool {
    ancestors(pid).iter().any(|p| p.comm == comm)
}

fn read_status(pid: i32) -> Option<(String, i32)> {
    let raw = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let mut comm = String::new();
    let mut ppid = 0;
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("Name:") {
            comm = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("PPid:") {
            ppid = v.trim().parse().unwrap_or(0);
        }
    }
    (!comm.is_empty()).then_some((comm, ppid))
}

/// How much later than the session's recorded start its process may have
/// started: Claude Code writes `startedAt` after it starts, and the boot time
/// behind a process start is kept in whole seconds.
pub const START_SLACK_MS: i64 = 10_000;

/// Why a pid is not (certainly) a session's Claude Code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The process has ended.
    Gone,
    /// The session's start time is not known, so the pid cannot be checked.
    StartUnknown,
    /// pid 0, 1 or negative.
    NotAnAgent(i32),
    /// `/proc` could not be read.
    Unreadable(String),
    /// The process is not Claude Code as far as can be told (its comm).
    NotClaude(String),
    /// The process started this many ms after the session did: a reused pid.
    Younger(i64),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::Gone => write!(f, "its process has already ended"),
            IdentityError::StartUnknown => write!(
                f,
                "its session's start is not known, so its pid cannot be checked"
            ),
            IdentityError::NotAnAgent(pid) => write!(f, "pid {pid} is not an agent's process"),
            IdentityError::Unreadable(e) => write!(f, "cannot read the process: {e}"),
            IdentityError::NotClaude(comm) => write!(
                f,
                "the process is not Claude Code as far as can be told (it is {comm:?}), so it may not be this session's process"
            ),
            IdentityError::Younger(ms) => write!(
                f,
                "the process started {}s after the session did, so it is another process; the session's has ended",
                (ms + 500) / 1000
            ),
        }
    }
}

impl std::error::Error for IdentityError {}

/// Checks through `/proc` that `pid` is Claude Code and started no later
/// than `started_at_ms` (the session's `startedAt`) plus [`START_SLACK_MS`].
/// This is what protects against a reused pid. Den's `closer.checkClaude`.
pub fn check_identity(pid: i32, started_at_ms: i64) -> Result<(), IdentityError> {
    if pid <= 1 {
        return Err(IdentityError::NotAnAgent(pid));
    }
    if started_at_ms <= 0 {
        return Err(IdentityError::StartUnknown);
    }
    let dir = format!("/proc/{pid}");
    let comm = match fs::read_to_string(format!("{dir}/comm")) {
        Ok(c) => c.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(IdentityError::Gone),
        Err(e) => return Err(IdentityError::Unreadable(e.to_string())),
    };
    let exe = fs::read_link(format!("{dir}/exe")).ok();
    let raw = fs::read(format!("{dir}/cmdline")).unwrap_or_default();
    let argv: Vec<String> = String::from_utf8_lossy(&raw)
        .trim_end_matches('\0')
        .split('\0')
        .map(str::to_string)
        .collect();
    let exe = exe.as_deref().and_then(|p| p.to_str()).unwrap_or("");
    if !is_claude_proc(&comm, exe, &argv) {
        return Err(IdentityError::NotClaude(comm));
    }
    let at = proc_start_ms(pid).map_err(IdentityError::Unreadable)?;
    if at > started_at_ms + START_SLACK_MS {
        return Err(IdentityError::Younger(at - started_at_ms));
    }
    Ok(())
}

/// Whether a process looks like Claude Code: the native binary (comm or
/// executable "claude"), or Node or Bun running its npm package (cli.js
/// under @anthropic-ai/claude-code).
pub fn is_claude_proc(comm: &str, exe: &str, argv: &[String]) -> bool {
    if comm == "claude" || util::base(exe) == "claude" {
        return true;
    }
    let Some(argv0) = argv.first() else {
        return false;
    };
    let prog = util::base(argv0);
    let prog = prog.strip_suffix(".exe").unwrap_or(&prog);
    if matches!(prog, "node" | "nodejs" | "bun") {
        // The script it runs: its first argument that is not a flag.
        if let Some(a) = argv[1..].iter().find(|a| !a.starts_with('-')) {
            return a.contains("/claude-code/") || util::base(a) == "claude";
        }
    }
    false
}

/// When the process started, Unix ms: boot time (`btime` in `/proc/stat`)
/// plus field 22 of `/proc/<pid>/stat` in clock ticks (USER_HZ = 100).
pub fn proc_start_ms(pid: i32) -> Result<i64, String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    // The command name (field 2) is in parentheses and may hold anything.
    let i = stat.rfind(')').ok_or("no command name in stat")?;
    let fields: Vec<&str> = stat[i + 1..].split_whitespace().collect();
    const STARTTIME: usize = 22 - 3; // fields after ")" start at field 3
    let ticks: i64 = fields
        .get(STARTTIME)
        .ok_or("short stat")?
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    Ok(boot_time_s()? * 1000 + ticks * 1000 / 100)
}

fn boot_time_s() -> Result<i64, String> {
    let raw = fs::read_to_string("/proc/stat").map_err(|e| e.to_string())?;
    raw.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .ok_or_else(|| "no btime in /proc/stat".to_string())?
        .trim()
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())
}

/// Whether `pid` has a live child process, from
/// `/proc/<pid>/task/*/children`; zombies do not count. Anything that cannot
/// be read counts as busy.
pub fn has_live_children(pid: i32) -> bool {
    if pid <= 0 {
        return true;
    }
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return true;
    };
    let mut any = false;
    for t in tasks.flatten() {
        any = true;
        let Ok(raw) = fs::read_to_string(t.path().join("children")) else {
            return true;
        };
        for c in raw.split_whitespace() {
            let Ok(stat) = fs::read_to_string(format!("/proc/{c}/stat")) else {
                continue; // gone since
            };
            if let Some(i) = stat.rfind(')')
                && stat.as_bytes().get(i + 2) == Some(&b'Z')
            {
                continue;
            }
            return true;
        }
    }
    !any
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn scan_reads_session_files() {
        let tmp = TempDir::new();
        let dir = tmp.join("sessions");
        fs::create_dir_all(&dir).unwrap();
        let me = std::process::id() as i32;
        write(
            &dir,
            &format!("{me}.json"),
            &format!(
                r#"{{"pid":{me},"sessionId":"live-1","cwd":"/work/proj","startedAt":200,"version":"2.1.289","kind":"interactive","entrypoint":"cli","name":"mossy","nameSource":"flag","status":"busy","updatedAt":300,"statusUpdatedAt":250,"messagingSocketPath":"/run/x.sock","peerFeatures":["send"],"extra":1}}"#
            ),
        );
        // A stale file: a pid that cannot exist.
        write(
            &dir,
            "999999999.json",
            r#"{"sessionId":"gone-1","startedAt":100,"status":"idle"}"#,
        );
        write(&dir, "bad.json", "not json");
        write(&dir, "noid.json", r#"{"pid":5}"#);
        write(&dir, "notes.txt", r#"{"sessionId":"x"}"#);

        let all = scan_dir(&dir);
        assert_eq!(all.len(), 2, "{all:?}");
        let live = &all[0];
        assert!(live.alive);
        assert_eq!(live.session_id, "live-1");
        assert_eq!(live.pid, me);
        assert_eq!(live.cwd, "/work/proj");
        assert_eq!(live.version, "2.1.289");
        assert_eq!(live.kind, "interactive");
        assert_eq!(live.entrypoint, "cli");
        assert_eq!(live.name, "mossy");
        assert_eq!(live.name_source, "flag");
        assert_eq!(live.status_updated_at, 250);
        assert_eq!(live.socket(), Some(Path::new("/run/x.sock")));
        assert_eq!(live.peer_features, vec!["send".to_string()]);
        assert_eq!(live.state(), SessionState::Working);
        assert_eq!(live.last_active_ms(), Some(250));

        let gone = &all[1];
        assert!(!gone.alive);
        assert_eq!(gone.pid, 999_999_999, "pid from the file name");
        assert_eq!(gone.state(), SessionState::Gone);
        assert_eq!(gone.last_active_ms(), Some(100));

        assert_eq!(live_sessions_in(&dir).len(), 1);
        assert_eq!(session_for_pid_in(&dir, me).unwrap().session_id, "live-1");
        assert!(session_for_pid_in(&dir, 12345678).is_none());
        assert_eq!(session_by_id_in(&dir, "gone-1").unwrap().pid, 999_999_999);
        assert!(scan_dir(&tmp.join("missing")).is_empty());

        assert_eq!(lookup(&all, "live").unwrap().session_id, "live-1");
        assert_eq!(lookup(&all, &me.to_string()).unwrap().session_id, "live-1");
        assert_eq!(lookup(&all, "@Mos").unwrap().session_id, "live-1");
        assert!(lookup(&all, "zzz").is_none());
        assert!(lookup(&all, "").is_none());
    }

    #[test]
    fn short_cwd_is_relative_to_home() {
        assert_eq!(short_path("/home/u/code", Some(Path::new("/home/u"))), "~/code");
        assert_eq!(short_path("/home/u", Some(Path::new("/home/u"))), "~");
        assert_eq!(short_path("/srv/x", Some(Path::new("/home/u"))), "/srv/x");
    }

    #[test]
    fn ancestors_include_self() {
        let me = std::process::id() as i32;
        let a = ancestors(me);
        assert!(!a.is_empty() && a[0].pid == me, "{a:?}");
        assert!(alive(me));
        assert!(!alive(0));
        assert!(!alive(-1));
    }

    #[test]
    fn identity_checks() {
        let me = std::process::id() as i32;
        // This test binary is not Claude Code.
        assert!(matches!(
            check_identity(me, util::now_ms()),
            Err(IdentityError::NotClaude(_))
        ));
        assert_eq!(check_identity(1, 5), Err(IdentityError::NotAnAgent(1)));
        assert_eq!(check_identity(me, 0), Err(IdentityError::StartUnknown));
        assert_eq!(check_identity(999_999_999, 5), Err(IdentityError::Gone));
        let started = proc_start_ms(me).unwrap();
        assert!((started - util::now_ms()).abs() < 3_600_000, "start {started}");
    }

    #[test]
    fn claude_process_shapes() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(is_claude_proc("claude", "", &[]));
        assert!(is_claude_proc("x", "/usr/bin/claude", &[]));
        assert!(is_claude_proc(
            "node",
            "/usr/bin/node",
            &s(&[
                "node",
                "--no-warnings",
                "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ])
        ));
        assert!(is_claude_proc(
            "bun",
            "/x/bun",
            &s(&["bun", "/home/u/.local/bin/claude"])
        ));
        assert!(!is_claude_proc(
            "node",
            "/usr/bin/node",
            &s(&["node", "server.js"])
        ));
        assert!(!is_claude_proc(
            "postgres",
            "/usr/bin/postgres",
            &s(&["postgres"])
        ));
    }
}
