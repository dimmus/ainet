//! ainet's own settings, `/always` rules, hooks and paths as a front end
//! asks for them ([`crate::Link::config`]): the replies as values, and
//! what to do with a daemon older than these requests. Settings then
//! come from, and go to, settings.json directly, with a notice saying
//! the daemon was not asked; the rest say it is older.

use std::path::Path;

use ainet_core::Request;
use ainet_core::Response;
use ainet_core::protocol::{AlwaysRule, HookReport, PathsReport, unknown_request};
use ainet_core::settings::{self, SettingsReport};

/// Said with settings read or written here because ainetd is older than
/// the requests for them.
pub const OLD_DAEMON_SETTINGS: &str = "This ainetd is older and was not asked: this is settings.json as written. Restart it (ainet shutdown; it starts again when next needed) to be sure it uses them.";

/// Said when an older ainetd does not know a request.
pub const OLD_DAEMON: &str = "this ainetd is older and does not know that; restart it (ainet shutdown; it starts again when next needed)";

/// A reply to a request about ainet itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigReply {
    /// The settings in force (after a set or unset too).
    Settings(Box<SettingsReport>),
    /// `/always` rules.
    Always(Vec<AlwaysRule>),
    /// ainet's hooks in Claude Code's settings.
    Hooks(Box<HookReport>),
    /// The paths in use and versions.
    Paths(Box<PathsReport>),
    /// Done, in words (a revoked rule).
    Done(String),
}

impl ConfigReply {
    /// The reply in a daemon's answer, or what went wrong.
    pub fn from_response(resp: Response) -> Result<ConfigReply, String> {
        match resp {
            Response::Settings { settings } => Ok(ConfigReply::Settings(settings)),
            Response::AlwaysRules { rules } => Ok(ConfigReply::Always(rules)),
            Response::Hooks { hooks } => Ok(ConfigReply::Hooks(hooks)),
            Response::Paths { paths } => Ok(ConfigReply::Paths(paths)),
            Response::Text { text } => Ok(ConfigReply::Done(text)),
            Response::Ok => Ok(ConfigReply::Done(String::new())),
            Response::Error { message } if unknown_request(&message) => Err(OLD_DAEMON.into()),
            Response::Error { message } => Err(message),
            _ => Err("ainetd answered with something else".into()),
        }
    }
}

/// Whether `req` is about ainet itself: settings, `/always` rules,
/// hooks, paths.
pub fn is_config(req: &Request) -> bool {
    matches!(
        req,
        Request::SettingsGet { .. }
            | Request::SettingsSet { .. }
            | Request::SettingsUnset { .. }
            | Request::SettingsReload
            | Request::AlwaysList { .. }
            | Request::AlwaysRevoke { .. }
            | Request::HookStatus
            | Request::HookInstall
            | Request::HookUninstall
            | Request::Paths
    )
}

/// The answer to a settings request carried out on the settings file at
/// `file` by this process, for a daemon that does not know the request:
/// with [`OLD_DAEMON_SETTINGS`] as its notice. `None` for other requests.
pub fn local(req: &Request, file: &Path) -> Option<Response> {
    let changed = match req {
        Request::SettingsGet { .. } | Request::SettingsReload => None,
        Request::SettingsSet { key, value } => Some(settings::set_in_file(file, key, value).map(|_| key)),
        Request::SettingsUnset { key } => Some(settings::unset_in_file(file, key).map(|_| key)),
        _ => return None,
    };
    let changed = match changed.transpose() {
        Ok(key) => key.cloned(),
        Err(message) => return Some(Response::Error { message }),
    };
    let mut report = SettingsReport::read(file);
    if let Request::SettingsGet { key: Some(key) } = req {
        report = match report.only(key) {
            Ok(r) => r,
            Err(message) => return Some(Response::Error { message }),
        };
    }
    report.changed = changed;
    report.notice = Some(OLD_DAEMON_SETTINGS.into());
    Some(Response::Settings {
        settings: Box::new(report),
    })
}

/// `answer` from ainetd to `req`, or, when ainetd is older than a
/// settings request, the request carried out on settings.json here.
pub fn or_local(req: &Request, answer: Response) -> Response {
    match &answer {
        Response::Error { message } if unknown_request(message) => {
            local(req, &ainet_core::paths::settings_file()).unwrap_or(answer)
        }
        _ => answer,
    }
}

/// One line on a setting just set or unset, from the report after it:
/// `queue.nudge = false (applies now)`, and a warning when the value lets
/// agents do more without asking.
pub fn changed_line(report: &SettingsReport) -> Option<String> {
    let key = report.changed.as_deref()?;
    let Some(e) = report.get(key) else {
        // A whole group unset.
        let group = format!("{key}.");
        return report
            .entries
            .iter()
            .any(|e| e.key.starts_with(&group))
            .then(|| format!("{key} is unset: its settings are their defaults"));
    };
    let value = if e.key == key {
        e.value.clone()
    } else {
        // An entry of a map: the map's value for it.
        let rest = &key[e.key.len() + 1..];
        rest.split('.')
            .try_fold(&e.value, |v, k| v.get(k))
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    let shown = if value.is_null() {
        format!("{key} is unset (default {})", e.default)
    } else {
        format!("{key} = {value}")
    };
    let mut line = format!("{shown} (applies {})", e.applies.label());
    if settings::less_safe(&e.key, &e.value) {
        line.push_str("; less safe: agents act without asking you");
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ainet-model-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn an_older_daemon_has_the_file_read_and_written_here() {
        let file = temp("old");
        std::fs::write(&file, "{\n  // mine\n  \"theme\": \"One Dark\",\n}\n").unwrap();
        let set = Request::SettingsSet {
            key: "queue.nudge".into(),
            value: serde_json::json!(false),
        };
        let Some(Response::Settings { settings }) = local(&set, &file) else {
            panic!("no report")
        };
        assert_eq!(settings.notice.as_deref(), Some(OLD_DAEMON_SETTINGS));
        assert_eq!(settings.changed.as_deref(), Some("queue.nudge"));
        assert_eq!(settings.get("queue.nudge").unwrap().value, false);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("// mine") && text.contains("One Dark"), "{text}");
        assert_eq!(
            changed_line(&settings).as_deref(),
            Some("queue.nudge = false (applies now)")
        );
        // Bad values and unknown keys are refused, and nothing is written.
        let bad = Request::SettingsSet {
            key: "queue.nudge".into(),
            value: serde_json::json!("maybe"),
        };
        assert!(matches!(local(&bad, &file), Some(Response::Error { .. })));
        let unknown = Request::SettingsSet {
            key: "queue.nudgy".into(),
            value: serde_json::json!(true),
        };
        assert!(
            matches!(local(&unknown, &file), Some(Response::Error { message }) if message.contains("no setting"))
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        let unset = Request::SettingsUnset {
            key: "queue.nudge".into(),
        };
        let Some(Response::Settings { settings }) = local(&unset, &file) else {
            panic!("no report")
        };
        assert_eq!(settings.get("queue.nudge").unwrap().value, true);
        assert!(local(&Request::Paths, &file).is_none());
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn replies_read_as_values_and_old_daemons_say_so() {
        let old = Response::Error {
            message: "bad request: unknown variant `hook_status`".into(),
        };
        assert_eq!(ConfigReply::from_response(old), Err(OLD_DAEMON.into()));
        assert_eq!(
            ConfigReply::from_response(Response::Text {
                text: "revoked 1".into()
            }),
            Ok(ConfigReply::Done("revoked 1".into()))
        );
        assert!(is_config(&Request::Paths) && !is_config(&Request::Ps));
    }

    #[test]
    fn a_less_safe_value_is_said_to_be() {
        let mut report = SettingsReport::read(Path::new("/nonexistent/settings.json"));
        let e = report
            .entries
            .iter_mut()
            .find(|e| e.key == "runtimes.claude.permission_mode")
            .unwrap();
        e.value = serde_json::json!("bypassPermissions");
        report.changed = Some("runtimes.claude.permission_mode".into());
        let line = changed_line(&report).unwrap();
        assert!(
            line.contains("applies to new seats") && line.contains("less safe"),
            "{line}"
        );
    }
}

#[cfg(test)]
mod link_tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::Duration;

    use ainet_core::{Request, Response};

    use super::*;
    use crate::command::{Command, Context, parse};
    use crate::{Backend, Demo, Link, Msg, Store};

    /// A daemon from before these requests: every line is a bad request.
    fn old_daemon(name: &str) -> std::path::PathBuf {
        let socket = std::env::temp_dir().join(format!("ainet-model-old-{}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let op: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let reply = match op["op"].as_str() {
                    Some("snapshot") => ainet_core::protocol::encode(&Demo::new().snapshot()),
                    Some(other) => ainet_core::protocol::encode(&Response::Error {
                        message: format!("bad request: unknown variant `{other}`"),
                    }),
                    None => continue,
                };
                let _ = stream.write_all(&reply);
            }
        });
        socket
    }

    fn next_config(rx: &mpsc::Receiver<Msg>) -> Result<ConfigReply, String> {
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("a reply") {
                Msg::Config { token: 7, reply } => return reply,
                _ => continue,
            }
        }
    }

    #[test]
    fn an_older_daemon_is_said_to_be_older() {
        let socket = old_daemon("config");
        let (tx, rx) = mpsc::channel();
        let (mut link, _) = Link::new(Backend::Daemon(socket.clone()), tx);
        link.config(Request::HookStatus, 7);
        assert_eq!(next_config(&rx), Err(OLD_DAEMON.into()));
        // Settings are read from the file here instead, with a notice.
        link.config(
            Request::SettingsGet {
                key: Some("queue".into()),
            },
            7,
        );
        match next_config(&rx) {
            Ok(ConfigReply::Settings(r)) => {
                assert_eq!(r.notice.as_deref(), Some(OLD_DAEMON_SETTINGS));
                assert!(r.entries.iter().all(|e| e.key.starts_with("queue.")));
            }
            other => panic!("{other:?}"),
        }
        // A plain request says so as a flash.
        link.request(Request::Paths, "paths".into());
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("a flash") {
                Msg::Flash(text, true) => {
                    assert_eq!(text, OLD_DAEMON);
                    break;
                }
                _ => continue,
            }
        }
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn the_full_demo_answers_and_the_console_demo_cannot() {
        let (tx, rx) = mpsc::channel();
        let (mut link, _) = Link::new(Backend::Demo(Box::new(Demo::full())), tx);
        link.config(Request::SettingsGet { key: None }, 7);
        let Ok(ConfigReply::Settings(r)) = next_config(&rx) else {
            panic!("no settings")
        };
        assert!(r.modified().count() >= 3, "a few set in the pretend file");
        assert_eq!(r.get("runtimes.claude.effort").unwrap().value, "high");
        link.config(
            Request::SettingsSet {
                key: "queue.nudge".into(),
                value: serde_json::json!(false),
            },
            7,
        );
        let Ok(ConfigReply::Settings(r)) = next_config(&rx) else {
            panic!("not set")
        };
        assert_eq!(r.get("queue.nudge").unwrap().value, false);
        assert_eq!(r.changed.as_deref(), Some("queue.nudge"));
        link.config(
            Request::SettingsSet {
                key: "queue.nudge".into(),
                value: serde_json::json!("maybe"),
            },
            7,
        );
        assert!(next_config(&rx).is_err(), "checked as ainetd checks");
        link.config(Request::AlwaysList { agent: None }, 7);
        let Ok(ConfigReply::Always(rules)) = next_config(&rx) else {
            panic!("no rules")
        };
        assert_eq!(rules.len(), 2);
        let first = rules[0].clone();
        link.config(
            Request::AlwaysRevoke {
                agent: first.agent.clone().unwrap(),
                rule: Some(first.key.clone()),
            },
            7,
        );
        assert!(matches!(next_config(&rx), Ok(ConfigReply::Done(t)) if t.contains("revoked 1")));
        link.config(Request::HookInstall, 7);
        assert!(matches!(next_config(&rx), Ok(ConfigReply::Hooks(h)) if h.installed && h.done.is_some()));
        link.config(Request::Paths, 7);
        assert!(matches!(next_config(&rx), Ok(ConfigReply::Paths(p)) if p.settings.is_some()));

        let (tx, rx) = mpsc::channel();
        let (mut console, _) = Link::new(Backend::Demo(Box::default()), tx);
        console.config(Request::SettingsGet { key: None }, 7);
        assert!(next_config(&rx).is_err());
    }

    fn run(line: &str) -> Command {
        let store = Store::new();
        parse(
            &Context {
                store: &store,
                view_rig: None,
                inbox_item: None,
            },
            line,
        )
    }

    #[test]
    fn set_unset_and_config_are_commands() {
        match run("set queue.watchdog.message wake up and check") {
            Command::Request(req, _) => match *req {
                Request::SettingsSet { key, value } => {
                    assert_eq!(key, "queue.watchdog.message");
                    assert_eq!(value, "wake up and check");
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            run("set queue.nudge false"),
            Command::Request(req, _) if matches!(&*req, Request::SettingsSet { value, .. } if *value == false)
        ));
        assert!(matches!(run("set queue.nudge"), Command::Error(e) if e == "/set <key> <value>"));
        assert!(matches!(run("set queue.nudgy 1"), Command::Error(e) if e.contains("no setting")));
        assert!(matches!(
            run("unset queue"),
            Command::Request(req, _) if matches!(&*req, Request::SettingsUnset { key } if key == "queue")
        ));
        assert!(matches!(run("unset"), Command::Error(_)));
        assert!(matches!(
            run("config"),
            Command::Request(req, _) if matches!(&*req, Request::SettingsGet { key: None })
        ));
        assert!(matches!(
            run("config runtimes"),
            Command::Request(req, _) if matches!(&*req, Request::SettingsGet { key: Some(k) } if k == "runtimes")
        ));
        assert_eq!(
            crate::command::command_named("/set x")
                .map(|c| c.usage())
                .as_deref(),
            Some("/set <key> <value>")
        );
    }

    #[test]
    fn settings_read_as_text() {
        let mut r = ainet_core::settings::SettingsReport::read(std::path::Path::new("/nonexistent/s.json"));
        r.entries
            .retain(|e| e.key.starts_with("queue.") || e.key == "daemon.socket");
        let text = crate::popup::settings_text(&r);
        assert!(text.contains("queue.nudge") && text.contains("true"), "{text}");
        assert!(text.contains("applies after a daemon restart"), "{text}");
        assert!(text.starts_with("settings.json: /nonexistent/s.json"));
        assert!(matches!(
            crate::popup::reply(&Response::Settings { settings: Box::new(r) }),
            Ok(t) if t.contains("queue.nudge")
        ));
    }
}
