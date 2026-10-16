//! Turns daemon replies into text: for the TUI's result popup, the GUI's
//! report tabs, and (a queue item) the console's `ainet queue show`.

use ainet_core::protocol::{AlwaysRule, HookReport, PathsReport};
use ainet_core::settings::{Applies, Effective, SettingsReport, Source};
use ainet_core::{QItem, Response};

/// A reply wanted whole ([`crate::Link::ask`]): its text, empty when it
/// says no more than "done", or the daemon's error.
pub fn reply(resp: &Response) -> Result<String, String> {
    match resp {
        Response::Error { message } => Err(message.clone()),
        Response::Text { text } => Ok(text.clone()),
        Response::Spawned { agent } => Ok(format!("{} joined the fleet", agent.name)),
        Response::Rig { rig } => Ok(format!("rig {} is {}", rig.name, rig.status.label())),
        Response::Discovered { .. }
        | Response::Rigs { .. }
        | Response::Specs { .. }
        | Response::Snapshots { .. }
        | Response::Watchdogs { .. }
        | Response::Captures { .. }
        | Response::Whoami { .. }
        | Response::QItem { .. }
        | Response::AlwaysRules { .. }
        | Response::Hooks { .. }
        | Response::Paths { .. } => Ok(render(resp)),
        Response::Settings { settings } => Ok(match crate::config::changed_line(settings) {
            Some(line) => format!("{line}\n"),
            None => render(resp),
        }),
        _ => Ok(String::new()),
    }
}

/// A queue item in full, as `ainet queue show` prints it: what, from whom
/// to whom, its state, body and notes, with how long ago as of `now`
/// (Unix ms).
pub fn qitem_text(q: &QItem, now: i64) -> String {
    let mut out = format!("{}  {}  {}\n", q.id, q.state.label(), q.summary);
    out.push_str(&format!("from {} to {}", q.source, q.destination));
    if let Some(o) = &q.owner {
        out.push_str(&format!(", owned by {o}"));
    }
    out.push_str(&format!(", priority {}", q.priority));
    if let Some(r) = &q.rig {
        out.push_str(&format!(", rig {r}"));
    }
    out.push('\n');
    if !q.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", q.tags.join(", ")));
    }
    if let Some(r) = &q.reply_to {
        out.push_str(&format!("reply to: {r}\n"));
    }
    if let Some(b) = &q.block {
        out.push_str(&format!("blocked on {}; then: {}\n", b.on, b.continuation));
    }
    if let Some(s) = &q.successor {
        out.push_str(&format!("handed off as {s}\n"));
    }
    if !q.body.is_empty() {
        out.push('\n');
        out.push_str(q.body.trim_end());
        out.push('\n');
    }
    if !q.notes.is_empty() {
        out.push('\n');
        for n in &q.notes {
            let st = n.state.map(|s| format!(" [{}]", s.label())).unwrap_or_default();
            out.push_str(&format!("- {} {}{st}: {}\n", ago(now, n.at), n.by, n.text));
        }
    }
    out
}

/// `5s ago`, `3m ago`, `2h ago`, `4d ago`.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let s = ((now_ms - then_ms) / 1000).max(0);
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

pub fn render(resp: &Response) -> String {
    let mut out = String::new();
    match resp {
        Response::Rigs { rigs } => {
            if rigs.is_empty() {
                out.push_str("No rigs yet. /up starter launches the starter team here.\n");
            }
            for r in rigs {
                let running = r
                    .seats
                    .iter()
                    .filter(|s| s.status == ainet_core::SeatStatus::Running)
                    .count();
                out.push_str(&format!(
                    "{:<20} {:<10} {}/{} seats  {}\n",
                    r.name,
                    r.status.label(),
                    running,
                    r.seats.len(),
                    r.root.display()
                ));
                for s in &r.seats {
                    out.push_str(&format!(
                        "    {:<30} {:<10} {:<12} {}\n",
                        s.address,
                        s.status.label(),
                        s.runtime.spec_name(),
                        s.role.as_deref().unwrap_or("")
                    ));
                }
            }
        }
        Response::Discovered { sessions } => {
            if sessions.is_empty() {
                out.push_str("No agents running outside ainet.\n");
            }
            for s in sessions {
                out.push_str(&format!(
                    "{:<38} {:<12} {:<14} {}{}\n",
                    s.id,
                    s.runtime.spec_name(),
                    s.tmux_session.as_deref().unwrap_or("(no tmux)"),
                    s.cwd.display(),
                    s.seat.as_deref().map(|x| format!("  → {x}")).unwrap_or_default()
                ));
            }
            out.push_str("\nAdopt one: /adopt <id> <rig> <pod> <member>\n");
        }
        Response::Specs { specs } => {
            for s in specs {
                out.push_str(&format!(
                    "{:<16} {:>2} seats  {:<8} {}\n",
                    s.name,
                    s.seats,
                    s.source,
                    s.summary.as_deref().unwrap_or("").lines().next().unwrap_or("")
                ));
            }
            out.push_str("\nLaunch one here: /up <name>   ·   preview: /plan <name>\n");
        }
        Response::Snapshots { snapshots } => {
            if snapshots.is_empty() {
                out.push_str("No snapshots.\n");
            }
            for s in snapshots {
                out.push_str(&format!(
                    "{:<36} {} seats  {}\n",
                    s.id,
                    s.seats,
                    s.label.as_deref().unwrap_or("")
                ));
            }
        }
        Response::Watchdogs { watchdogs } => {
            for w in watchdogs {
                out.push_str(&format!(
                    "{:<6} {:<28} every {:>5}s  {}  {}\n",
                    w.id,
                    w.target,
                    w.interval_secs,
                    if w.active { "active " } else { "stopped" },
                    w.message.lines().next().unwrap_or("")
                ));
            }
        }
        Response::Captures { captures } => {
            for c in captures {
                out.push_str(&format!("── {} ──\n", c.label));
                for l in &c.lines {
                    out.push_str(l);
                    out.push('\n');
                }
            }
        }
        Response::Whoami { whoami } => {
            out.push_str(&format!("{} ({})\n", whoami.name, whoami.agent));
            if let Some(s) = &whoami.seat {
                out.push_str(&format!("seat {} ({}) in rig {}\n", s.address, s.logical, s.rig));
            }
            out.push_str(&format!("delegates to: {}\n", whoami.delegates_to.join(", ")));
            out.push_str(&format!("delegated by: {}\n", whoami.delegated_by.join(", ")));
            out.push_str(&format!("peers: {}\n", whoami.peers.join(", ")));
            out.push_str(&format!(
                "{} unread · {} open queue items\n",
                whoami.unread, whoami.open_queue
            ));
        }
        Response::Text { text } => out.push_str(text),
        Response::QItem { item } => out.push_str(&qitem_text(item, ainet_core::now_ms())),
        Response::Settings { settings } => out.push_str(&settings_text(settings)),
        Response::AlwaysRules { rules } => out.push_str(&always_text(rules)),
        Response::Hooks { hooks } => out.push_str(&hooks_text(hooks)),
        Response::Paths { paths } => out.push_str(&paths_text(paths)),
        other => out.push_str(&format!("{other:?}")),
    }
    out
}

/// A JSON value on one line, cut to `max` characters.
fn short_value(v: &serde_json::Value, max: usize) -> String {
    let s = v.to_string();
    if s.chars().count() > max {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        s
    }
}

/// One setting on a line: `*` when it is not its default, the key, the
/// value, and where it comes from and when a change applies when that is
/// not the file and now.
pub fn setting_line(e: &Effective, width: usize) -> String {
    let mark = if e.value != e.default { '*' } else { ' ' };
    let mut notes = Vec::new();
    match &e.source {
        Source::Default => {}
        Source::File => notes.push("file".to_string()),
        Source::Env { var } => notes.push(format!("${var}")),
    }
    if e.applies != Applies::Live {
        notes.push(format!("applies {}", e.applies.label()));
    }
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!("  ({})", notes.join("; "))
    };
    format!("{mark} {:<width$} {}{notes}", e.key, short_value(&e.value, 48))
}

/// The settings in force, one per line, under the file's name and what
/// is wrong with it: `ainet config list`, the console's `/config`.
pub fn settings_text(r: &SettingsReport) -> String {
    let mut out = String::new();
    match &r.file {
        Some(f) => out.push_str(&format!("settings.json: {}\n", f.display())),
        None => out.push_str("no settings file: the defaults\n"),
    }
    if let Some(n) = &r.notice {
        out.push_str(&format!("{n}\n"));
    }
    if let Some(e) = &r.error {
        out.push_str(&format!("not in force (the last good settings are): {e}\n"));
    }
    if !r.unknown.is_empty() {
        out.push_str(&format!("ignored, not settings: {}\n", r.unknown.join(", ")));
    }
    out.push('\n');
    let width = r.entries.iter().map(|e| e.key.chars().count()).max().unwrap_or(0);
    for e in &r.entries {
        out.push_str(&setting_line(e, width));
        out.push('\n');
    }
    out.push_str("\n* not the default.  /set <key> <value>  /unset <key>\n");
    out
}

/// `/always` rules, one line each: whose (or global), the key and what it
/// allows.
pub fn always_text(rules: &[AlwaysRule]) -> String {
    if rules.is_empty() {
        return "no /always rules\n".into();
    }
    rules
        .iter()
        .map(|r| {
            let who = r
                .agent
                .as_deref()
                .map(|a| a.chars().take(8).collect::<String>())
                .unwrap_or_else(|| "global".into());
            format!("{who:<8}  {}  ({})\n", r.key, r.scope)
        })
        .collect()
}

/// ainet's hooks in Claude Code's settings: what was done, and how they
/// stand.
pub fn hooks_text(h: &HookReport) -> String {
    let mut out = String::new();
    if let Some(done) = &h.done {
        out.push_str(done);
        out.push('\n');
    }
    if let Some(b) = &h.backup {
        out.push_str(&format!("the file as it was: {}\n", b.display()));
    }
    out.push_str(&h.summary);
    out.push('\n');
    out
}

/// The paths in use and versions, one per line.
pub fn paths_text(p: &PathsReport) -> String {
    let mut out = String::new();
    let mut line = |name: &str, value: String| out.push_str(&format!("{name:<18} {value}\n"));
    line(
        "version",
        match p.pid {
            0 => format!(
                "ainet {} (protocol {}), ainetd not running",
                p.version, p.protocol
            ),
            pid => format!("ainetd {} (protocol {}, pid {pid})", p.version, p.protocol),
        },
    );
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
    out
}
