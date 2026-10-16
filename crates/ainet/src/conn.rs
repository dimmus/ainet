//! Talking to ainetd: start it when needed, and one call per connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ainet_core::client::Client;
use ainet_core::{Request, Response};
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

/// Starts ainetd in the background unless it is already running: this very
/// binary, as `ainet daemon`.
pub fn ensure_daemon(socket: &Path) -> Result<()> {
    ainet_model::daemon::ensure_daemon(socket, &std::env::current_exe()?)
}

/// The agent this process acts for: `$AINET_AGENT_ID`, if set.
pub fn me() -> Option<String> {
    std::env::var("AINET_AGENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
}
