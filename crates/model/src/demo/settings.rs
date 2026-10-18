//! The desktop app's demo answering requests about ainet itself: a
//! pretend settings.json kept in memory (set and unset edit its text as
//! ainetd edits the real one, with the same checks), a few `/always`
//! rules, hooks that install and uninstall, and believable paths. Nothing
//! is read or written on disk.

use std::path::PathBuf;

use ainet_core::protocol::{AlwaysRule, HookReport, LogTail, PathsReport};
use ainet_core::settings::{self, SettingsReport};
use ainet_core::{Request, Response};

/// The pretend file: a theme of the desktop app's, and a few of ainet's
/// settings away from their defaults.
const SETTINGS: &str = r#"// ainet's settings (the demo's own; nothing here is on disk)
{
  "theme": "One Dark",
  "daemon": {
    "log": { "level": "info,ainet_daemon=debug" },
  },
  "runtimes": {
    "claude": { "effort": "high" },
  },
  "rigs": { "keep_snapshots": 10 },
  "retention": { "events_max_age_days": 30 },
}
"#;

/// The hook events ainet installs, as `ainet_claude::hooks::SPECS` lists
/// them.
const HOOK_EVENTS: [&str; 10] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Notification",
    "Stop",
    "PostToolUse",
    "PreToolUse",
    "PermissionRequest",
    "StopFailure",
    "PostToolUseFailure",
];

/// The pretend daemon's log, as tracing writes it.
const LOG: &[&str] = &[
    "INFO ainet_daemon: listening on /run/user/1000/ainet.sock",
    "INFO ainet_daemon::config: settings from ~/.config/ainet/settings.json: daemon.log.level, runtimes.claude.effort, rigs.keep_snapshots, retention.events_max_age_days",
    "INFO ainet_daemon::rigs: rig docs: 3 seats",
    "DEBUG ainet_daemon::comms: message #12 to dev-ui@docs delivered",
    "INFO ainet_daemon::hooks: permission request from qa@docs held for you",
    "DEBUG ainet_daemon::discover: 2 Claude Code sessions, 0 new",
];

pub(super) struct DemoSettings {
    home: PathBuf,
    /// When the pretend ainetd started.
    started: i64,
    text: String,
    rules: Vec<AlwaysRule>,
    hooks: bool,
}

impl DemoSettings {
    /// `agents` (id, working directory) get an `/always` rule each, the
    /// first a command and the second edits under its directory.
    pub(super) fn new(home: &str, agents: &[(String, PathBuf)]) -> DemoSettings {
        let now = ainet_core::now_ms();
        let mut rules = Vec::new();
        if let Some((id, _)) = agents.first() {
            rules.push(AlwaysRule {
                agent: Some(id.clone()),
                key: "Bash:cargo test".into(),
                scope: "this exact command: cargo test".into(),
                created: now - 42 * 60_000,
                kept: false,
            });
        }
        if let Some((id, cwd)) = agents.get(1) {
            rules.push(AlwaysRule {
                agent: Some(id.clone()),
                key: format!("Edit:under:{}", cwd.display()),
                scope: format!("Edit on files under {}", cwd.display()),
                created: now - 17 * 60_000,
                kept: false,
            });
        }
        DemoSettings {
            home: PathBuf::from(home),
            started: now - (3 * 60 + 12) * 60_000,
            text: SETTINGS.into(),
            rules,
            hooks: false,
        }
    }

    fn file(&self) -> PathBuf {
        self.home.join(".config/ainet/settings.json")
    }

    fn report(&self, changed: Option<String>) -> Result<SettingsReport, String> {
        let loaded = settings::parse(&self.text)?;
        let mut report = SettingsReport::new(&loaded, None, &[], Some(self.file()));
        report.changed = changed;
        Ok(report)
    }

    fn hook_report(&self, done: Option<String>) -> HookReport {
        let path = self.home.join(".claude/settings.json");
        let summary = match self.hooks {
            true => format!("installed in {}", path.display()),
            false => format!("not installed in {}", path.display()),
        };
        HookReport {
            exists: true,
            installed: self.hooks,
            current: self.hooks,
            missing: match self.hooks {
                true => Vec::new(),
                false => HOOK_EVENTS.map(String::from).to_vec(),
            },
            events: HOOK_EVENTS.map(String::from).to_vec(),
            summary,
            backup: done
                .as_ref()
                .map(|_| self.home.join(".claude/settings.json.bak.ainet.demo")),
            done,
            settings_path: path,
            ..HookReport::default()
        }
    }

    /// The answer to a request about ainet itself; `None` for others.
    /// `agent` finds an agent's id by any name it goes by.
    pub(super) fn respond(
        &mut self,
        req: &Request,
        agent: impl Fn(&str) -> Option<String>,
    ) -> Option<Response> {
        let settings = |r: Result<SettingsReport, String>| match r {
            Ok(report) => Response::Settings {
                settings: Box::new(report),
            },
            Err(message) => Response::Error { message },
        };
        Some(match req {
            Request::SettingsGet { key } => settings(
                self.report(None)
                    .and_then(|r| r.only(key.as_deref().unwrap_or_default())),
            ),
            Request::SettingsReload => settings(self.report(None)),
            Request::SettingsSet { key, value } => match settings::set(&self.text, key, value) {
                Ok(text) => {
                    self.text = text;
                    settings(self.report(Some(key.clone())))
                }
                Err(message) => Response::Error { message },
            },
            Request::SettingsUnset { key } => match settings::unset(&self.text, key) {
                Ok(text) => {
                    self.text = text;
                    settings(self.report(Some(key.clone())))
                }
                Err(message) => Response::Error { message },
            },
            Request::AlwaysList { agent: who } => {
                let id = match who.as_deref() {
                    Some(name) => match agent(name) {
                        Some(id) => Some(id),
                        None => {
                            return Some(Response::Error {
                                message: format!("no agent {name}"),
                            });
                        }
                    },
                    None => None,
                };
                Response::AlwaysRules {
                    rules: self
                        .rules
                        .iter()
                        .filter(|r| id.is_none() || r.agent == id)
                        .cloned()
                        .collect(),
                }
            }
            Request::AlwaysRevoke { agent: who, rule } => {
                let id = agent(who).unwrap_or_else(|| who.clone());
                let before = self.rules.len();
                self.rules.retain(|r| {
                    r.agent.as_deref() != Some(&id) || rule.as_ref().is_some_and(|k| *k != r.key)
                });
                match before - self.rules.len() {
                    0 => Response::Error {
                        message: format!("{who} has no such /always rule"),
                    },
                    1 => Response::Text {
                        text: "revoked 1 /always rule".into(),
                    },
                    n => Response::Text {
                        text: format!("revoked {n} /always rules"),
                    },
                }
            }
            Request::HookStatus => self.hooks_reply(None),
            Request::HookInstall => {
                self.hooks = true;
                self.hooks_reply(Some(
                    "installed ainet's hooks (pretend: the demo writes nothing)".into(),
                ))
            }
            Request::HookUninstall => {
                self.hooks = false;
                self.hooks_reply(Some(
                    "removed ainet's hooks (pretend: the demo writes nothing)".into(),
                ))
            }
            Request::Paths => {
                let state = self.home.join(".local/state/ainet");
                Response::Paths {
                    paths: Box::new(PathsReport {
                        socket: PathBuf::from("/run/user/1000/ainet.sock"),
                        database: state.join("ainet.sqlite"),
                        log: state.join("ainetd.log"),
                        state_dir: state,
                        settings: Some(self.file()),
                        spec_dirs: vec![self.home.join(".config/ainet/specs")],
                        claude_config_dir: self.home.join(".claude"),
                        exe: self.home.join(".cargo/bin/ainet"),
                        version: env!("CARGO_PKG_VERSION").into(),
                        protocol: ainet_core::protocol::VERSION,
                        pid: 4_040_001,
                        started: self.started,
                    }),
                }
            }
            Request::LogTail { lines } => {
                let skip = LOG.len().saturating_sub(*lines);
                Response::Log {
                    log: Box::new(LogTail {
                        path: self.home.join(".local/state/ainet/ainetd.log"),
                        exists: true,
                        lines: LOG[skip..].iter().map(|l| l.to_string()).collect(),
                    }),
                }
            }
            _ => return None,
        })
    }

    fn hooks_reply(&self, done: Option<String>) -> Response {
        Response::Hooks {
            hooks: Box::new(self.hook_report(done)),
        }
    }
}
