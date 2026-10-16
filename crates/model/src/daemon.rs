//! Starting ainetd when a front end needs it. The daemon always runs as
//! `ainet daemon` from the console binary: hooks, MCP configs and agents'
//! shells call that binary by path, so a front end that is not `ainet`
//! itself (the GUI) must find it rather than start itself. Whether it is
//! started at all, and where its log goes, is `daemon` in settings.json.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ainet_core::client::Client;
use ainet_core::settings::{DaemonSettings, LogSettings};
use ainet_core::{Request, Response, paths, protocol};
use anyhow::{Context, Result, bail};

/// Starts ainetd in the background with `ainet daemon` unless it is already
/// running, and checks it speaks our protocol. settings.json may turn the
/// starting off (`daemon.autostart`); one that does not read is as none.
pub fn ensure_daemon(socket: &Path, ainet: &Path) -> Result<()> {
    let settings = ainet_core::settings::load(&paths::settings_file())
        .ok()
        .flatten()
        .map(|l| l.settings.daemon)
        .unwrap_or_default();
    ensure_daemon_with(socket, ainet, &settings)
}

/// [`ensure_daemon`] with these daemon settings.
pub fn ensure_daemon_with(socket: &Path, ainet: &Path, settings: &DaemonSettings) -> Result<()> {
    if Client::connect(socket).is_ok() {
        return check_version(socket);
    }
    if !settings.autostart {
        bail!(
            "ainetd is not running ({}), and settings.json does not start it (daemon.autostart is off): start it with `ainet daemon`",
            socket.display()
        );
    }
    let log_path = log_file(&settings.log);
    let log = open_log(&log_path, &settings.log)?;
    let mut cmd = Command::new(ainet);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // A socket other than the default (`ainet-gui --socket`) must reach the
    // daemon too; the console always passes the default and is unaffected.
    if socket != paths::socket() {
        cmd.env("AINET_SOCKET", socket);
    }
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
    bail!("ainetd did not start; see {}", log_path.display())
}

/// Where an autostarted daemon logs: `daemon.log.file`, else `ainetd.log`
/// in the state directory.
pub fn log_file(log: &LogSettings) -> PathBuf {
    log.path()
}

/// Opens the daemon's log: moved aside first once it reached
/// `daemon.log.max_size_mb` (`ainetd.log.1`, the older ones shifted, at
/// most `keep`), then added to (`append`) or started afresh, as ainet
/// always did.
pub fn open_log(path: &Path, log: &LogSettings) -> Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if log.max_size_mb > 0 && size >= log.max_size_mb * 1024 * 1024 {
        let keep = log.keep.max(1);
        let numbered = |n: u32| {
            let mut p = path.as_os_str().to_owned();
            p.push(format!(".{n}"));
            PathBuf::from(p)
        };
        let _ = std::fs::remove_file(numbered(keep));
        for n in (1..keep).rev() {
            let _ = std::fs::rename(numbered(n), numbered(n + 1));
        }
        std::fs::rename(path, numbered(1)).with_context(|| format!("rotate {}", path.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(log.append)
        .truncate(!log.append)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    Ok(file)
}

/// Says `Hello` to the daemon at `socket`: fails unless it is running and
/// speaks our protocol version.
pub fn check_version(socket: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ainet-model-daemon-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_log_starts_afresh_by_default() {
        let d = dir("fresh");
        let path = d.join("ainetd.log");
        std::fs::write(&path, "old run\n").unwrap();
        let log = LogSettings::default();
        assert_eq!(log_file(&log), paths::daemon_log());
        use std::io::Write as _;
        writeln!(open_log(&path, &log).unwrap(), "new run").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new run\n");
        assert!(!d.join("ainetd.log.1").exists(), "never rotated");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_log_appends_and_rotates_when_asked() {
        use std::io::Write as _;
        let d = dir("rotate");
        let path = d.join("ainetd.log");
        let log = LogSettings {
            append: true,
            max_size_mb: 1,
            keep: 2,
            file: Some(path.clone()),
            ..LogSettings::default()
        };
        assert_eq!(log_file(&log), path);
        std::fs::write(&path, "one\n").unwrap();
        writeln!(open_log(&path, &log).unwrap(), "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n", "appended");
        let big = "x".repeat(1024 * 1024);
        for run in ["a", "b", "c"] {
            std::fs::write(&path, format!("{run}{big}")).unwrap();
            open_log(&path, &log).unwrap();
        }
        let first = |p: &Path| std::fs::read_to_string(p).unwrap().chars().next();
        assert_eq!(first(&d.join("ainetd.log.1")), Some('c'));
        assert_eq!(first(&d.join("ainetd.log.2")), Some('b'));
        assert!(!d.join("ainetd.log.3").exists(), "keep says two");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "", "a new one");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn autostart_off_says_so_instead_of_starting() {
        let d = dir("off");
        let socket = d.join("none.sock");
        let off = DaemonSettings {
            autostart: false,
            ..DaemonSettings::default()
        };
        // Not run: the program does not exist, so starting would fail
        // differently.
        let e = ensure_daemon_with(&socket, Path::new("/nonexistent/ainet"), &off).unwrap_err();
        assert!(e.to_string().contains("daemon.autostart is off"), "{e}");
        let on = ensure_daemon_with(
            &socket,
            Path::new("/nonexistent/ainet"),
            &DaemonSettings {
                log: LogSettings {
                    file: Some(d.join("ainetd.log")),
                    ..LogSettings::default()
                },
                ..DaemonSettings::default()
            },
        )
        .unwrap_err();
        assert!(on.to_string().contains("start ainetd"), "tried to start it: {on}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
