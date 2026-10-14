//! Talking to ainetd: start it when needed, and one call per connection.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ainet_core::client::Client;
use ainet_core::protocol;
use ainet_core::{Request, Response, paths};
use anyhow::{Context, Result, bail};

/// How long an ordinary call may take.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Launching or stopping a rig can take a while.
pub const RIG_TIMEOUT: Duration = Duration::from_secs(180);

/// The daemon's socket and a way to call it.
#[derive(Clone, Debug)]
pub struct Conn {
    pub socket: PathBuf,
}

impl Conn {
    pub fn new(socket: PathBuf) -> Conn {
        Conn { socket }
    }

    /// One request, one reply, on a fresh connection.
    pub fn call_for(&self, req: &Request, timeout: Duration) -> Result<Response> {
        let mut client = Client::connect(&self.socket)
            .with_context(|| format!("ainetd is not running ({})", self.socket.display()))?;
        client.set_timeout(Some(timeout))?;
        client.call(req).map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                anyhow::anyhow!("ainetd did not answer within {}s", timeout.as_secs())
            }
            _ => e.into(),
        })
    }

    pub fn call(&self, req: &Request) -> Result<Response> {
        self.call_for(req, DEFAULT_TIMEOUT)
    }

    /// Like `call`, but a `Response::Error` becomes an `Err`.
    pub fn ok(&self, req: &Request) -> Result<Response> {
        check(self.call(req)?)
    }

    pub fn ok_for(&self, req: &Request, timeout: Duration) -> Result<Response> {
        check(self.call_for(req, timeout)?)
    }

    /// Starts ainetd in the background unless it is already running.
    pub fn ensure_daemon(&self) -> Result<()> {
        ensure_daemon(&self.socket)
    }
}

/// `Response::Error` as an `Err`, anything else as is.
pub fn check(resp: Response) -> Result<Response> {
    match resp {
        Response::Error { message } => bail!(message),
        other => Ok(other),
    }
}

pub fn unexpected(resp: Response) -> anyhow::Error {
    match resp {
        Response::Error { message } => anyhow::anyhow!(message),
        other => anyhow::anyhow!("unexpected reply from ainetd: {other:?}"),
    }
}

/// Starts ainetd in the background unless it is already running.
pub fn ensure_daemon(socket: &Path) -> Result<()> {
    let conn = Conn::new(socket.to_path_buf());
    if Client::connect(socket).is_ok() {
        return check_version(&conn);
    }
    let log_dir = paths::state_dir();
    std::fs::create_dir_all(&log_dir)?;
    let log = std::fs::File::create(log_dir.join("ainetd.log"))?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // Its own process group, so Ctrl+C in the TUI does not reach it.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn().context("start ainetd")?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if Client::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "ainetd did not start; see {}",
        log_dir.join("ainetd.log").display()
    )
}

fn check_version(conn: &Conn) -> Result<()> {
    match conn.call(&Request::Hello {
        version: protocol::VERSION,
    })? {
        Response::Hello { .. } => Ok(()),
        other => Err(unexpected(other)),
    }
}

/// The agent this process acts for: `$AINET_AGENT_ID`, if set.
pub fn me() -> Option<String> {
    std::env::var("AINET_AGENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
}
