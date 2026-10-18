//! `ainet config`, `ainet always` and `ainet hooks`: ainet's own
//! settings, `/always` rules and hooks. Settings work without ainetd:
//! with one running, it reads and writes settings.json and puts a change
//! in force at once; without one (or one older than these requests),
//! the file is read and written here.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ainet_core::client::Client;
use ainet_core::paths;
use ainet_core::protocol::{self, PathsReport, unknown_request};
use ainet_core::settings::{self, Effective, SettingsReport};
use ainet_core::{Request, Response};
use ainet_model::config::{OLD_DAEMON, changed_line, or_local};
use anyhow::{Result, bail};

use crate::cli::{AlwaysCmd, ConfigCmd, HooksCmd};
use crate::conn::{Conn, unexpected};

/// `req` to a running ainetd, without starting one: `None` when none
/// runs.
fn running(conn: &Conn, req: &Request) -> Option<Result<Response>> {
    Client::connect(&conn.socket).ok()?;
    Some(conn.call_for(req, Duration::from_secs(20)))
}

/// The settings report in a reply (an older daemon's "bad request"
/// already answered from the file by [`or_local`]).
fn report_of(resp: Response) -> Result<SettingsReport> {
    match resp {
        Response::Settings { settings } => Ok(*settings),
        other => Err(unexpected(other)),
    }
}

/// A settings request answered by a running ainetd, else by `here` on the
/// file.
fn settings_request(
    conn: &Conn,
    req: &Request,
    here: impl FnOnce(&Path) -> Result<SettingsReport, String>,
) -> Result<SettingsReport> {
    let report = match running(conn, req) {
        Some(answer) => report_of(or_local(req, answer?))?,
        None => here(&paths::settings_file()).map_err(anyhow::Error::msg)?,
    };
    if let Some(n) = &report.notice {
        eprintln!("ainet: {n}");
    }
    Ok(report)
}

/// The value at `key` in a report: the setting's, or an entry of a map.
fn value_at(e: &Effective, key: &str) -> serde_json::Value {
    match key.strip_prefix(&e.key).and_then(|r| r.strip_prefix('.')) {
        Some(rest) => rest
            .split('.')
            .try_fold(&e.value, |v, k| v.get(k))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        None => e.value.clone(),
    }
}

/// A value as `config get` prints it: text bare, anything else as JSON.
fn plain(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn config(conn: &Conn, json: bool, cmd: ConfigCmd) -> Result<ExitCode> {
    match cmd {
        ConfigCmd::Get { key } => {
            let req = Request::SettingsGet {
                key: Some(key.clone()),
            };
            let report = settings_request(conn, &req, |f| SettingsReport::read(f).only(&key))?;
            // The value printed is the one in force; say when the file's
            // is not, as `config list` does.
            if let Some(e) = &report.error {
                eprintln!("ainet: settings.json not in force: {e}");
            }
            match report.get(&key).filter(|_| report.entries.len() == 1) {
                Some(e) if json => {
                    let mut out = serde_json::json!({
                        "key": key,
                        "value": value_at(e, &key),
                        "default": e.default,
                        "source": e.source,
                        "applies": e.applies,
                    });
                    if let Some(err) = &report.error {
                        out["error"] = err.clone().into();
                    }
                    println!("{}", serde_json::to_string_pretty(&out)?)
                }
                Some(e) => println!("{}", plain(&value_at(e, &key))),
                // A group: each of its settings.
                None if json => println!("{}", serde_json::to_string_pretty(&report.entries)?),
                None => print!("{}", lines(report.entries.iter())),
            }
        }
        ConfigCmd::List { key, changed } => {
            let req = Request::SettingsGet { key: key.clone() };
            let mut report = settings_request(conn, &req, |f| {
                SettingsReport::read(f).only(key.as_deref().unwrap_or_default())
            })?;
            if changed {
                report.entries.retain(|e| e.value != e.default);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", ainet_model::popup::settings_text(&report));
            }
        }
        ConfigCmd::Set { key, value } => {
            settings::check_key(&key).map_err(anyhow::Error::msg)?;
            let value = settings::value_from_text(&key, &value.join(" "));
            let req = Request::SettingsSet {
                key: key.clone(),
                value: value.clone(),
            };
            let report = settings_request(conn, &req, |f| {
                settings::set_in_file(f, &key, &value)?;
                Ok(changed(f, &key))
            })?;
            print_change(&report, json)?;
        }
        ConfigCmd::Unset { key } => {
            let req = Request::SettingsUnset { key: key.clone() };
            let report = settings_request(conn, &req, |f| {
                settings::unset_in_file(f, &key)?;
                Ok(changed(f, &key))
            })?;
            print_change(&report, json)?;
        }
        ConfigCmd::Path { all: false } => println!("{}", paths::settings_file().display()),
        ConfigCmd::Path { all: true } => {
            let report = match running(conn, &Request::Paths) {
                Some(answer) => match answer? {
                    Response::Paths { paths } => *paths,
                    Response::Error { message } if unknown_request(&message) => {
                        eprintln!("ainet: {OLD_DAEMON}; these are the paths as this ainet sees them");
                        local_paths(conn)
                    }
                    other => return Err(unexpected(other)),
                },
                None => local_paths(conn),
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", ainet_model::popup::paths_text(&report));
            }
        }
        ConfigCmd::Edit => return edit(conn),
    }
    Ok(ExitCode::SUCCESS)
}

/// The settings with `key` just changed in the file at `path`.
fn changed(path: &Path, key: &str) -> SettingsReport {
    let mut report = SettingsReport::read(path);
    report.changed = Some(key.to_string());
    report
}

fn print_change(report: &SettingsReport, json: bool) -> Result<()> {
    let key = report.changed.as_deref().unwrap_or_default();
    if json {
        let e = report.get(key);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "key": key,
                "value": e.map(|e| value_at(e, key)),
                "applies": e.map(|e| e.applies),
                "file": report.file,
            }))?
        );
    } else if let Some(line) = changed_line(report) {
        println!("{line}");
    }
    Ok(())
}

/// Settings one per line, as `config list` shows them.
fn lines<'a>(entries: impl Iterator<Item = &'a Effective> + Clone) -> String {
    let width = entries.clone().map(|e| e.key.chars().count()).max().unwrap_or(0);
    entries
        .map(|e| format!("{}\n", ainet_model::popup::setting_line(e, width)))
        .collect()
}

/// What a new settings.json starts as when `config edit` makes it.
const NEW_FILE: &str = "// ainet's settings: `ainet config list` shows each one with its default.\n// The desktop app's own keys (theme, terminal, ...) live here too.\n{\n}\n";

/// `ainet config edit`: $VISUAL or $EDITOR on settings.json, then the
/// file checked and a running ainetd told to read it.
fn edit(conn: &Conn) -> Result<ExitCode> {
    let path = paths::settings_file();
    if !path.exists() {
        settings::write_whole(&path, NEW_FILE)?;
    }
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "vi".into());
    // Through the shell, as editors are usually given with arguments
    // (`code -w`).
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&path)
        .status()?;
    if !status.success() {
        bail!("{editor} did not finish cleanly; settings.json is as it left it");
    }
    let problem = settings::load(&path).err();
    match running(conn, &Request::SettingsReload) {
        Some(answer) => match answer? {
            Response::Settings { settings } => match settings.error {
                Some(e) => eprintln!("ainetd keeps the last good settings: {e}"),
                None => println!("ainetd has read {}", path.display()),
            },
            Response::Error { message } if unknown_request(&message) => {
                eprintln!("ainet: {OLD_DAEMON}");
            }
            other => return Err(unexpected(other)),
        },
        None if problem.is_none() => println!("{} reads; ainetd is not running", path.display()),
        None => {}
    }
    match problem {
        Some(e) => Err(crate::commands::Exit(1, e).into()),
        None => Ok(ExitCode::SUCCESS),
    }
}

/// The paths as this process would use them, for when no daemon says.
fn local_paths(conn: &Conn) -> PathsReport {
    let s = crate::settings::get();
    PathsReport {
        socket: conn.socket.clone(),
        state_dir: paths::state_dir(),
        database: paths::database(),
        log: s.daemon.log.path(),
        settings: Some(paths::settings_file()),
        spec_dirs: std::iter::once(paths::specs_dir())
            .chain(s.rigs.spec_dirs.iter().map(|p| paths::expand(p)))
            .collect(),
        claude_config_dir: ainet_claude::config_dir(),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ainet")),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: protocol::VERSION,
        pid: 0,
        started: 0,
    }
}

/// A reply to an `/always` request, with an older daemon's "bad request"
/// said plainly.
fn older(resp: Response) -> Result<Response> {
    match resp {
        Response::Error { message } if unknown_request(&message) => bail!(OLD_DAEMON),
        Response::Error { message } => bail!(message),
        other => Ok(other),
    }
}

pub fn always(conn: &Conn, json: bool, cmd: AlwaysCmd) -> Result<()> {
    let req = match cmd {
        AlwaysCmd::List { agent } => Request::AlwaysList { agent },
        AlwaysCmd::Revoke { agent, rule, .. } => Request::AlwaysRevoke { agent, rule },
    };
    let resp = older(conn.call(&req)?)?;
    if json {
        println!("{}", crate::output::json(&resp));
    } else {
        print!("{}", crate::output::human(&resp));
    }
    Ok(())
}

pub fn hooks(json: bool, cmd: HooksCmd) -> Result<()> {
    let s = crate::settings::get();
    let path = ainet_daemon::claude_settings_file(s);
    let command = std::env::current_exe()
        .map(|p| ainet_claude::hooks::hook_command(&p))
        .unwrap_or_else(|_| "ainet hook".into());
    let action = match cmd {
        HooksCmd::Status => ainet_daemon::HookAction::Status,
        HooksCmd::Install => ainet_daemon::HookAction::Install,
        HooksCmd::Uninstall => ainet_daemon::HookAction::Uninstall,
    };
    let report = ainet_daemon::hook_action(&path, &command, s.permissions.hook_timeout_secs(), action)
        .map_err(anyhow::Error::msg)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", ainet_model::popup::hooks_text(&report));
    }
    Ok(())
}
