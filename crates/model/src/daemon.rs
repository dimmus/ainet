//! Starting ainetd when a front end needs it. The daemon always runs as
//! `ainet daemon` from the console binary: hooks, MCP configs and agents'
//! shells call that binary by path, so a front end that is not `ainet`
//! itself (the GUI) must find it rather than start itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ainet_core::client::Client;
use ainet_core::{Request, Response, paths, protocol};
use anyhow::{Context, Result, bail};

/// Starts ainetd in the background with `ainet daemon` unless it is already
/// running, and checks it speaks our protocol.
pub fn ensure_daemon(socket: &Path, ainet: &Path) -> Result<()> {
    if Client::connect(socket).is_ok() {
        return check_version(socket);
    }
    let log_dir = paths::state_dir();
    std::fs::create_dir_all(&log_dir)?;
    let log = std::fs::File::create(log_dir.join("ainetd.log"))?;
    let mut cmd = Command::new(ainet);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // Its own process group, so Ctrl+C in the front end does not reach it.
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

fn check_version(socket: &Path) -> Result<()> {
    let mut client =
        Client::connect(socket).with_context(|| format!("ainetd is not running ({})", socket.display()))?;
    client.set_timeout(Some(Duration::from_secs(10)))?;
    match client.call(&Request::Hello {
        version: protocol::VERSION,
    })? {
        Response::Hello { .. } => Ok(()),
        Response::Error { message } => bail!(message),
        other => bail!("unexpected reply from ainetd: {other:?}"),
    }
}

/// The console binary: `$AINET_BIN`, else `ainet` next to the running
/// executable, else `ainet` on `PATH`.
pub fn console_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AINET_BIN").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("ainet")))
        .filter(|p| p.is_file());
    sibling.or_else(|| {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("ainet"))
                .find(|p| p.is_file())
        })
    })
}
