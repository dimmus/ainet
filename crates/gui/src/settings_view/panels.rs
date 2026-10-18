//! The parts of ainet's pages that are not one setting each: the
//! `/always` rules in force (Permissions), ainet's hooks in Claude Code
//! (Hooks), the spec library (Rigs & Specs) and Diagnostics. Each asks
//! ainetd for what it shows; without ainetd it says so, and Diagnostics
//! shows what this app can find by itself.

use std::path::{Path, PathBuf};

use ainet_core::Request;
use ainet_core::protocol::{self, AlwaysRule, HookReport, LogTail, PathsReport, SpecEntry};
use ainet_model::config::OLD_DAEMON;
use ainet_model::{ConfigReply, Listing};
use gpui::{ClipboardItem, PromptLevel};
use ui::{Tooltip, prelude::*};

use super::SettingsItem;
use super::field::Panel;
use super::pages::Page;
use crate::fleet::Connection;
use crate::settings;

/// How many lines of ainetd's log Diagnostics shows.
pub const LOG_LINES: usize = 40;

/// What the panels were last told, by ainetd or the file system.
#[derive(Default)]
pub(super) struct Panels {
    pub always: Option<Result<Vec<AlwaysRule>, String>>,
    pub hooks: Option<Result<HookReport, String>>,
    /// A hook install or removal is on its way.
    pub hooks_busy: bool,
    pub specs: Option<Result<Vec<SpecEntry>, String>>,
    pub paths: Option<Result<PathsReport, String>>,
    /// The paths are this app's own finding, not ainetd's answer.
    pub paths_local: bool,
    pub log: Option<Result<LogTail, String>>,
    /// Diagnostics were just copied.
    pub copied: bool,
}

/// A hook change, asked for with a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookChange {
    Install,
    Repair,
    Uninstall,
}

impl HookChange {
    fn word(self) -> &'static str {
        match self {
            HookChange::Install => "Install",
            HookChange::Repair => "Repair",
            HookChange::Uninstall => "Uninstall",
        }
    }
}

impl SettingsItem {
    /// Asks for what the panels of `page` show, or of every page.
    pub(super) fn fetch_panels(&mut self, page: Option<Page>, cx: &mut Context<Self>) {
        let want = |p: Page| page.is_none_or(|page| page == p);
        let connected = self.connected(cx);
        if want(Page::Permissions) {
            if connected {
                self.ask(Request::AlwaysList { agent: None }, cx, |this, reply, _| {
                    this.panels.always = Some(match reply {
                        Ok(ConfigReply::Always(rules)) => Ok(rules),
                        Ok(_) => Err("ainetd answered with something else".into()),
                        Err(e) => Err(e),
                    });
                });
            } else {
                self.panels.always = None;
            }
        }
        if want(Page::Hooks) {
            if connected {
                self.ask(Request::HookStatus, cx, |this, reply, _| this.hooks_reply(reply));
            } else {
                self.panels.hooks = None;
            }
        }
        if want(Page::Rigs) {
            if connected {
                let this = cx.weak_entity();
                self.fleet.update(cx, |fleet, cx| {
                    fleet.list(
                        Request::Specs,
                        move |listing, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.panels.specs = Some(match listing {
                                    Ok(Listing::Specs(specs)) => Ok(specs),
                                    Ok(_) => Err("ainetd answered with something else".into()),
                                    Err(e) => Err(e),
                                });
                                cx.notify();
                            });
                        },
                        cx,
                    )
                });
            } else {
                self.panels.specs = None;
            }
        }
        if want(Page::Diagnostics) {
            self.fetch_diagnostics(cx);
        }
    }

    /// Asks ainetd about itself; `then` hears the reply on this view.
    fn ask(
        &mut self,
        req: Request,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<ConfigReply, String>, &mut Context<Self>) + 'static,
    ) {
        let this = cx.weak_entity();
        self.fleet.update(cx, |fleet, cx| {
            fleet.config(
                req,
                move |reply, cx| {
                    // Later: the fleet is busy handing this over, and
                    // `then` may ask it again.
                    cx.defer(move |cx| {
                        let _ = this.update(cx, |this, cx| {
                            then(this, reply, cx);
                            cx.notify();
                        });
                    });
                },
                cx,
            )
        });
    }

    fn hooks_reply(&mut self, reply: Result<ConfigReply, String>) {
        self.panels.hooks_busy = false;
        self.panels.hooks = Some(match reply {
            Ok(ConfigReply::Hooks(report)) => Ok(*report),
            Ok(_) => Err("ainetd answered with something else".into()),
            Err(e) => Err(e),
        });
    }

    /// The paths and the log: ainetd's, else as this app finds them.
    fn fetch_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.panels.copied = false;
        if !self.connected(cx) {
            self.local_paths(cx);
            self.local_log(cx);
            return;
        }
        self.ask(Request::Paths, cx, |this, reply, cx| match reply {
            Ok(ConfigReply::Paths(paths)) => {
                this.panels.paths = Some(Ok(*paths));
                this.panels.paths_local = false;
            }
            // An older ainetd: what this app can find.
            Err(e) if e == OLD_DAEMON => this.local_paths(cx),
            Ok(_) => this.panels.paths = Some(Err("ainetd answered with something else".into())),
            Err(e) => this.panels.paths = Some(Err(e)),
        });
        self.ask(
            Request::LogTail { lines: LOG_LINES },
            cx,
            |this, reply, cx| match reply {
                Ok(ConfigReply::Log(log)) => this.panels.log = Some(Ok(*log)),
                Err(e) if e == OLD_DAEMON => this.local_log(cx),
                Ok(_) => this.panels.log = Some(Err("ainetd answered with something else".into())),
                Err(e) => this.panels.log = Some(Err(e)),
            },
        );
    }

    fn local_paths(&mut self, cx: &App) {
        self.panels.paths = Some(Ok(local_paths(settings::file(cx).as_deref())));
        self.panels.paths_local = true;
    }

    /// Reads the log file this app's settings name, off the UI thread.
    fn local_log(&mut self, cx: &mut Context<Self>) {
        let path = local_settings(settings::file(cx).as_deref()).daemon.log.path();
        let read = cx.background_spawn(async move { LogTail::read(&path, LOG_LINES) });
        cx.spawn(async move |this, cx| {
            let log = read.await.map_err(|e| format!("the log: {e}"));
            let _ = this.update(cx, |this, cx| {
                this.panels.log = Some(log);
                cx.notify();
            });
        })
        .detach();
    }

    /// Takes back agent `agent`'s `/always` rule `key`.
    pub(super) fn revoke(&mut self, agent: String, key: String, cx: &mut Context<Self>) {
        self.errors.remove("panel.always");
        let req = Request::AlwaysRevoke {
            agent,
            rule: Some(key),
        };
        self.ask(req, cx, |this, reply, cx| match reply {
            Ok(_) => this.fetch_panels(Some(Page::Permissions), cx),
            Err(why) => {
                this.errors.insert("panel.always", why);
            }
        });
    }

    /// Installs, repairs or removes ainet's hooks once you say so, naming
    /// the file and the backup kept.
    pub(super) fn change_hooks(&mut self, change: HookChange, window: &mut Window, cx: &mut Context<Self>) {
        let file = match &self.panels.hooks {
            Some(Ok(report)) => report.settings_path.display().to_string(),
            _ => "Claude Code's settings.json".into(),
        };
        let (title, what) = match change {
            HookChange::Install => (
                "Install ainet's hooks?",
                "ainet's hooks are added to it; your own settings and hooks stay as they are.",
            ),
            HookChange::Repair => (
                "Repair ainet's hooks?",
                "ainet's hooks in it are written again for this ainet and today's timeouts; your own stay as they are.",
            ),
            HookChange::Uninstall => (
                "Uninstall ainet's hooks?",
                "ainet's hooks are taken out of it; your own settings and hooks stay as they are.",
            ),
        };
        let detail = format!(
            "This changes {file}. {what} A copy of the file as it is now is kept beside it, as {}.bak.ainet.<time>.",
            Path::new(&file)
                .file_name()
                .map_or("settings.json".into(), |n| n.to_string_lossy().to_string())
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            title,
            Some(&detail),
            &[change.word(), "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.panels.hooks_busy = true;
                let req = match change {
                    HookChange::Uninstall => Request::HookUninstall,
                    _ => Request::HookInstall,
                };
                this.ask(req, cx, |this, reply, _| this.hooks_reply(reply));
                cx.notify();
            });
        })
        .detach();
    }

    /// Diagnostics as text, to paste where help is asked for. Values of
    /// environment variables and MCP servers are left out: they may hold
    /// keys.
    pub fn diagnostics_text(&self, cx: &App) -> String {
        let mut out = String::from("ainet diagnostics\n");
        let mut line = |name: &str, value: String| out.push_str(&format!("{name:<18} {value}\n"));
        line("connection", connection_words(&self.fleet.read(cx).connection()));
        line(
            "desktop app",
            format!(
                "ainet-gui {} (protocol {})",
                env!("CARGO_PKG_VERSION"),
                protocol::VERSION
            ),
        );
        match &self.panels.paths {
            Some(Ok(p)) => {
                if p.pid != 0 {
                    line("ainetd", daemon_words(p));
                }
                line("socket", p.socket.display().to_string());
                line(
                    "settings",
                    p.settings
                        .as_ref()
                        .map_or_else(|| "(none)".into(), |f| f.display().to_string()),
                );
                line("state", p.state_dir.display().to_string());
                line("database", p.database.display().to_string());
                line("log", p.log.display().to_string());
                for (i, d) in p.spec_dirs.iter().enumerate() {
                    line(if i == 0 { "specs" } else { "" }, d.display().to_string());
                }
                line("claude config", p.claude_config_dir.display().to_string());
                line("ainet binary", p.exe.display().to_string());
            }
            Some(Err(e)) => line("paths", e.clone()),
            None => {}
        }
        if let Some(report) = &self.report {
            if let Some(e) = &report.error {
                line("settings error", e.clone());
            }
            if !report.unknown.is_empty() {
                line("unknown keys", report.unknown.join(", "));
            }
            let changed: Vec<String> = report
                .modified()
                .map(|e| {
                    let secret = e.key.ends_with(".env") || e.key.ends_with(".mcp_servers");
                    match secret {
                        true => format!("{} = (set)", e.key),
                        false => format!("{} = {}", e.key, e.value),
                    }
                })
                .collect();
            line(
                "changed settings",
                match changed.len() {
                    0 => "none".into(),
                    n => n.to_string(),
                },
            );
            for c in changed {
                out.push_str(&format!("  {c}\n"));
            }
        }
        match &self.panels.log {
            Some(Ok(log)) => {
                out.push_str(&format!("\nlast lines of {}:\n", log.path.display()));
                out.push_str(&ainet_model::popup::log_text(log));
            }
            Some(Err(e)) => out.push_str(&format!("\nthe log: {e}\n")),
            None => {}
        }
        out
    }

    fn copy_diagnostics(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.diagnostics_text(cx)));
        self.panels.copied = true;
        cx.notify();
    }

    /// A panel's own content, under its label and description.
    pub(super) fn render_panel_body(&self, panel: Panel, cx: &mut Context<Self>) -> AnyElement {
        let needs_daemon = |what: &str| {
            note(
                format!("ainetd is not running: {what}"),
                Color::Muted,
                "PANEL-NOT-RUNNING",
            )
        };
        let connected = self.connected(cx);
        match panel {
            Panel::Always if !connected => needs_daemon(
                "the /always rules live in it, and show here once it runs. Global rules below are kept in settings.json.",
            ),
            Panel::Always => self.render_always(cx),
            Panel::Hooks if !connected => needs_daemon(
                "the hooks can be checked and installed without it, from a terminal: ainet hooks status, ainet hooks install.",
            ),
            Panel::Hooks => self.render_hooks(cx),
            Panel::Specs if !connected => {
                needs_daemon("the library shows here once it runs; `ainet specs` lists it without the app.")
            }
            Panel::Specs => self.render_specs(),
            Panel::Diagnostics => self.render_diagnostics(cx),
        }
    }

    fn render_always(&self, cx: &mut Context<Self>) -> AnyElement {
        let rules = match &self.panels.always {
            None => return note("Asking ainetd…".into(), Color::Muted, "ALWAYS-WAITING"),
            Some(Err(e)) => return note(e.clone(), Color::Warning, "ALWAYS-ERROR"),
            Some(Ok(rules)) => rules,
        };
        if rules.is_empty() {
            return note(
                "No rules: every request that needs you is asked.".into(),
                Color::Muted,
                "ALWAYS-NONE",
            );
        }
        let store = &self.fleet.read(cx).store;
        let now = ainet_core::now_ms();
        let header = h_flex()
            .gap_2()
            .child(column("Agent", rems(10.), Color::Muted))
            .child(column("Allows", rems(18.), Color::Muted))
            .child(column("Made", rems(7.), Color::Muted));
        let rows = rules.iter().enumerate().map(|(ix, rule)| {
            let who = match &rule.agent {
                Some(id) => store.agents.get(id).map_or_else(|| id.clone(), |a| a.label()),
                None => "every agent".into(),
            };
            let made = match rule.created {
                0 => "settings.json".into(),
                at => format!("{} ago", ainet_model::popup::uptime(at, now)),
            };
            let action = match &rule.agent {
                Some(agent) => {
                    let (agent, key) = (agent.clone(), rule.key.clone());
                    div()
                        .debug_selector(move || format!("ALWAYS-REVOKE-{ix}"))
                        .child(
                            Button::new(SharedString::from(format!("always-revoke-{ix}")), "Revoke")
                                .label_size(LabelSize::Small)
                                .style(ButtonStyle::Subtle)
                                .tooltip(Tooltip::text("Take it back: the agent asks again next time"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.revoke(agent.clone(), key.clone(), cx)
                                })),
                        )
                        .into_any_element()
                }
                None => Label::new("a Global Rule")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .into_any_element(),
            };
            h_flex()
                .debug_selector(move || format!("ALWAYS-ROW-{ix}"))
                .gap_2()
                .py_0p5()
                .child(column(&who, rems(10.), Color::Default))
                .child(
                    v_flex()
                        .w(rems(18.))
                        .child(Label::new(rule.scope.clone()).size(LabelSize::Small))
                        .child(
                            Label::new(rule.key.clone())
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        ),
                )
                .child(column(
                    &if rule.kept { format!("{made}, kept") } else { made },
                    rems(7.),
                    Color::Muted,
                ))
                .child(action)
        });
        v_flex().gap_1().child(header).children(rows).into_any_element()
    }

    fn render_hooks(&self, cx: &mut Context<Self>) -> AnyElement {
        let report = match &self.panels.hooks {
            None => return note("Asking ainetd…".into(), Color::Muted, "HOOKS-WAITING"),
            Some(Err(e)) => return note(e.clone(), Color::Warning, "HOOKS-ERROR"),
            Some(Ok(report)) => report,
        };
        let (icon, color) = match (report.installed, report.current, &report.error) {
            (_, _, Some(_)) => (IconName::XCircle, Color::Error),
            (true, true, _) => (IconName::Check, Color::Success),
            (true, false, _) => (IconName::Warning, Color::Warning),
            (false, _, _) => (IconName::Circle, Color::Muted),
        };
        let mut body = v_flex().gap_2().child(
            h_flex()
                .debug_selector(|| "HOOKS-SUMMARY".into())
                .gap_2()
                .child(Icon::new(icon).size(IconSize::Small).color(color))
                .child(Label::new(report.summary.clone()).size(LabelSize::Small)),
        );
        if let Some(e) = &report.error {
            body = body.child(Label::new(e.clone()).size(LabelSize::Small).color(Color::Error));
        }
        // Each event, in place or not; an older ainetd lists only those
        // missing.
        let events: Vec<(String, bool)> = match report.events.is_empty() {
            false => report
                .events
                .iter()
                .map(|e| (e.clone(), !report.missing.contains(e)))
                .collect(),
            true => report.missing.iter().map(|e| (e.clone(), false)).collect(),
        };
        body = body.child(
            h_flex()
                .flex_wrap()
                .gap_x_4()
                .gap_y_1()
                .children(events.into_iter().map(|(event, ok)| {
                    let selector = format!("HOOKS-EVENT-{event}");
                    h_flex()
                        .debug_selector(move || selector.clone())
                        .gap_1()
                        .w(rems(13.))
                        .child(
                            Icon::new(if ok { IconName::Check } else { IconName::Dash })
                                .size(IconSize::XSmall)
                                .color(if ok { Color::Success } else { Color::Muted }),
                        )
                        .child(Label::new(event).size(LabelSize::Small).buffer_font(cx))
                        .child(
                            Label::new(if ok { "in place" } else { "missing" })
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                })),
        );
        if report.stale > 0 {
            body = body.child(
                Label::new(format!(
                    "{} of ainet's entries run another ainet or another timeout: Repair writes them again.",
                    report.stale
                ))
                .size(LabelSize::Small)
                .color(Color::Warning),
            );
        }
        if let Some(done) = &report.done {
            let backup = report
                .backup
                .as_ref()
                .map(|b| format!(" The file as it was is kept as {}.", b.display()))
                .unwrap_or_default();
            body = body.child(
                div().debug_selector(|| "HOOKS-DONE".into()).child(
                    Label::new(format!("{done}.{backup}"))
                        .size(LabelSize::Small)
                        .color(Color::Success),
                ),
            );
        }
        let busy = self.panels.hooks_busy;
        let button = |change: HookChange, cx: &mut Context<Self>| {
            let word = change.word();
            div()
                .debug_selector(move || format!("HOOKS-{}", word.to_uppercase()))
                .child(
                    Button::new(SharedString::from(format!("hooks-{word}")), word)
                        .label_size(LabelSize::Small)
                        .style(if change == HookChange::Uninstall {
                            ButtonStyle::Subtle
                        } else {
                            ButtonStyle::Filled
                        })
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.change_hooks(change, window, cx)),
                        ),
                )
        };
        let mut buttons = h_flex().gap_2();
        if !report.installed {
            buttons = buttons.child(button(HookChange::Install, cx));
        } else if !report.current {
            buttons = buttons.child(button(HookChange::Repair, cx));
        }
        if report.installed {
            buttons = buttons.child(button(HookChange::Uninstall, cx));
        }
        buttons = buttons.child(
            IconButton::new("hooks-refresh", IconName::RotateCw)
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
                .tooltip(Tooltip::text("Check Again"))
                .on_click(cx.listener(|this, _, _, cx| this.fetch_panels(Some(Page::Hooks), cx))),
        );
        body.child(buttons).into_any_element()
    }

    fn render_specs(&self) -> AnyElement {
        let specs = match &self.panels.specs {
            None => return note("Asking ainetd…".into(), Color::Muted, "SPECS-WAITING"),
            Some(Err(e)) => return note(e.clone(), Color::Warning, "SPECS-ERROR"),
            Some(Ok(specs)) => specs,
        };
        if specs.is_empty() {
            return note("The library is empty.".into(), Color::Muted, "SPECS-NONE");
        }
        v_flex()
            .gap_1()
            .children(specs.iter().map(|spec| {
                let name = spec.name.clone();
                let from = match spec.source.as_str() {
                    "builtin" => "built in".to_string(),
                    other => other.to_string(),
                };
                h_flex()
                    .debug_selector(move || format!("SPEC-{name}"))
                    .gap_2()
                    .child(
                        h_flex()
                            .w(rems(12.))
                            .gap_1()
                            .child(Label::new(spec.name.clone()).size(LabelSize::Small))
                            .child(
                                Label::new(format!(
                                    "{} seat{}",
                                    spec.seats,
                                    if spec.seats == 1 { "" } else { "s" }
                                ))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            ),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .when_some(spec.summary.clone(), |el, summary| {
                                el.child(Label::new(summary).size(LabelSize::Small).color(Color::Muted))
                            })
                            .child(Label::new(from).size(LabelSize::XSmall).color(Color::Placeholder)),
                    )
            }))
            .into_any_element()
    }

    fn render_diagnostics(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let connection = self.fleet.read(cx).connection();
        let mut rows: Vec<(&'static str, String)> = vec![
            ("Connection", connection_words(&connection)),
            (
                "Desktop App",
                format!(
                    "ainet-gui {} (protocol {})",
                    env!("CARGO_PKG_VERSION"),
                    protocol::VERSION
                ),
            ),
        ];
        let mut notes = Vec::new();
        match &self.panels.paths {
            Some(Ok(p)) => {
                if p.pid != 0 {
                    rows.push(("ainetd", daemon_words(p)));
                }
                rows.push(("Socket", p.socket.display().to_string()));
                rows.push((
                    "Settings",
                    p.settings
                        .as_ref()
                        .map_or_else(|| "(none)".into(), |f| f.display().to_string()),
                ));
                rows.push(("State", p.state_dir.display().to_string()));
                rows.push(("Database", p.database.display().to_string()));
                rows.push(("Log", p.log.display().to_string()));
                for (i, d) in p.spec_dirs.iter().enumerate() {
                    rows.push((if i == 0 { "Specs" } else { "" }, d.display().to_string()));
                }
                rows.push(("Claude Code", p.claude_config_dir.display().to_string()));
                rows.push(("ainet Binary", p.exe.display().to_string()));
                if self.panels.paths_local {
                    notes.push(match connection {
                        Connection::Connected => {
                            "This ainetd is older and does not say its paths: these are this app's own."
                        }
                        _ => "ainetd is not running: these are the paths this app would give it.",
                    });
                }
            }
            Some(Err(e)) => rows.push(("Paths", e.clone())),
            None => rows.push(("Paths", "asking ainetd…".into())),
        }
        let table = v_flex().gap_0p5().children(rows.into_iter().map(|(name, value)| {
            let selector = format!("DIAGNOSTICS-{}", name.to_uppercase().replace(' ', "-"));
            h_flex()
                .debug_selector(move || selector.clone())
                .gap_2()
                .items_start()
                .child(column(name, rems(8.), Color::Muted))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(Label::new(value).size(LabelSize::Small).buffer_font(cx)),
                )
        }));
        let log: AnyElement = match &self.panels.log {
            None => Label::new("Reading the log…")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
            Some(Err(e)) => Label::new(e.clone())
                .size(LabelSize::Small)
                .color(Color::Warning)
                .into_any_element(),
            Some(Ok(log)) if !log.exists || log.lines.is_empty() => {
                Label::new(ainet_model::popup::log_text(log).trim_end().to_string())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .into_any_element()
            }
            Some(Ok(log)) => v_flex()
                .id("diagnostics-log")
                .max_h(rems(24.))
                .overflow_y_scroll()
                .p_2()
                .rounded_sm()
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.panel_background)
                .children(log.lines.iter().map(|l| {
                    Label::new(l.clone())
                        .size(LabelSize::XSmall)
                        .buffer_font(cx)
                        .color(Color::Muted)
                }))
                .into_any_element(),
        };
        let log_title = match &self.panels.log {
            Some(Ok(log)) => format!("Last lines of {}", log.path.display()),
            _ => "ainetd's Log".into(),
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().debug_selector(|| "DIAGNOSTICS-COPY".into()).child(
                            Button::new("diagnostics-copy", "Copy Diagnostics")
                                .label_size(LabelSize::Small)
                                .style(ButtonStyle::Filled)
                                .start_icon(Icon::new(IconName::Copy).size(IconSize::Small))
                                .on_click(cx.listener(|this, _, _, cx| this.copy_diagnostics(cx))),
                        ),
                    )
                    .child(
                        div().debug_selector(|| "DIAGNOSTICS-REFRESH".into()).child(
                            Button::new("diagnostics-refresh", "Refresh")
                                .label_size(LabelSize::Small)
                                .style(ButtonStyle::Subtle)
                                .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.fetch_panels(Some(Page::Diagnostics), cx);
                                    cx.notify();
                                })),
                        ),
                    )
                    .when(self.panels.copied, |el| {
                        el.child(
                            div().debug_selector(|| "DIAGNOSTICS-COPIED".into()).child(
                                Label::new(
                                    "Copied: secrets in environment variables and MCP servers are left out.",
                                )
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                            ),
                        )
                    }),
            )
            .children(
                notes
                    .into_iter()
                    .map(|n| Label::new(n).size(LabelSize::Small).color(Color::Warning)),
            )
            .child(table)
            .child(
                v_flex()
                    .debug_selector(|| "DIAGNOSTICS-LOG".into())
                    .gap_1()
                    .child(Label::new(log_title).size(LabelSize::Small).color(Color::Muted))
                    .child(log),
            )
            .into_any_element()
    }
}

/// A line of text in a panel, with a selector for tests.
fn note(text: String, color: Color, selector: &'static str) -> AnyElement {
    div()
        .debug_selector(move || selector.into())
        .child(Label::new(text).size(LabelSize::Small).color(color))
        .into_any_element()
}

/// A table cell `width` wide.
fn column(text: &str, width: Rems, color: Color) -> impl IntoElement {
    div()
        .w(width)
        .flex_none()
        .child(Label::new(text.to_string()).size(LabelSize::Small).color(color))
}

/// The connection, in words.
fn connection_words(c: &Connection) -> String {
    match c {
        Connection::Demo => "the demo fleet: nothing here is real".into(),
        Connection::Starting => "starting ainetd".into(),
        Connection::Connected => "connected to ainetd".into(),
        Connection::Offline => "ainetd is not answering; retrying".into(),
        Connection::Failed(why) => format!("ainetd could not be started: {why}"),
        Connection::Incompatible(why) => format!("ainetd does not speak this app's protocol: {why}"),
    }
}

/// ainetd's version, pid and uptime.
fn daemon_words(p: &PathsReport) -> String {
    let up = match p.started {
        0 => String::new(),
        started => format!(
            ", up {}",
            ainet_model::popup::uptime(started, ainet_core::now_ms())
        ),
    };
    format!("{} (protocol {}), pid {}{up}", p.version, p.protocol, p.pid)
}

/// ainet's settings as the file at `file` says, or the defaults.
fn local_settings(file: Option<&Path>) -> ainet_core::settings::Settings {
    file.and_then(|f| ainet_core::settings::load(f).ok().flatten())
        .map(|loaded| loaded.settings)
        .unwrap_or_default()
}

/// The paths as this app finds them, for when ainetd does not say.
pub fn local_paths(file: Option<&Path>) -> PathsReport {
    use ainet_core::paths;
    let s = local_settings(file);
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
        .unwrap_or_else(|| PathBuf::from("~/.claude"));
    PathsReport {
        socket: paths::socket(),
        state_dir: paths::state_dir(),
        database: paths::database(),
        log: s.daemon.log.path(),
        settings: file.map(Path::to_path_buf),
        spec_dirs: std::iter::once(paths::specs_dir())
            .chain(s.rigs.spec_dirs.iter().map(|d| paths::expand(d)))
            .collect(),
        claude_config_dir: claude,
        exe: ainet_model::daemon::console_binary().unwrap_or_else(|| PathBuf::from("ainet")),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: protocol::VERSION,
        pid: 0,
        started: 0,
    }
}
