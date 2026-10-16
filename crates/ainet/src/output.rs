//! Turning daemon replies into text: aligned tables for people, pretty JSON
//! for scripts. Pure functions, so they are tested without a daemon.

use ainet_core::protocol::{Capture, SpecEntry};
use ainet_core::{
    Agent, ChatMessage, Discovered, Message, QItem, Response, Rig, SeatStatus, SnapshotInfo, Watchdog, Whoami,
};
use serde_json::Value;

/// Columns padded to their widest cell, two spaces apart; the last column
/// is not padded.
#[derive(Default)]
pub struct Table {
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(header: &[&str]) -> Table {
        Table {
            rows: vec![header.iter().map(|s| s.to_string()).collect()],
        }
    }

    pub fn row<I, S>(&mut self, cells: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(cells.into_iter().map(Into::into).collect());
    }

    pub fn render(&self) -> String {
        let cols = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..cols)
            .map(|c| {
                self.rows
                    .iter()
                    .filter_map(|r| r.get(c))
                    .map(|s| s.chars().count())
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let mut out = String::new();
        for row in &self.rows {
            let mut line = String::new();
            for (c, cell) in row.iter().enumerate() {
                if c + 1 == row.len() {
                    line.push_str(cell);
                } else {
                    line.push_str(cell);
                    let pad = widths[c] - cell.chars().count() + 2;
                    line.extend(std::iter::repeat_n(' ', pad));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// The payload of a reply, without its `type` tag, pretty-printed.
pub fn json(resp: &Response) -> String {
    let mut v = serde_json::to_value(resp).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.remove("type");
    }
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

fn or_dash(s: Option<&str>) -> String {
    match s {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "-".to_string(),
    }
}

/// `5s ago`, `3m ago`, `2h ago`, `4d ago`.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    ainet_model::popup::ago(now_ms, then_ms)
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// Any reply, for people.
pub fn human(resp: &Response) -> String {
    let now = ainet_core::now_ms();
    match resp {
        Response::Ok => "ok\n".into(),
        Response::Error { message } => format!("error: {message}\n"),
        Response::Hello { version, pid } => format!("ainetd running (pid {pid}, protocol {version})\n"),
        Response::Snapshot { agents, inbox, .. } => {
            let waiting = inbox.iter().filter(|i| i.waiting).count();
            format!(
                "{}{} agents, {waiting} waiting on you\n",
                agents_table(agents),
                agents.len()
            )
        }
        Response::Spawned { agent } => format!("{}  {}\n", agent.name, agent.id),
        Response::Screen { rows, .. } => rows.iter().map(|r| format!("{r}\n")).collect(),
        Response::HookReply { output } => output.as_ref().map(|o| format!("{o}\n")).unwrap_or_default(),
        Response::Event { event } => format!("{}\n", serde_json::to_string(event).unwrap_or_default()),
        Response::Rigs { rigs } => rigs_table(rigs),
        Response::Rig { rig } => rig_detail(rig, &[]),
        Response::Snapshots { snapshots } => snapshots_table(snapshots, now),
        Response::Specs { specs } => specs_table(specs),
        Response::Text { text } if text.ends_with('\n') || text.is_empty() => text.clone(),
        Response::Text { text } => format!("{text}\n"),
        Response::Discovered { sessions } => discovered_table(sessions),
        Response::Whoami { whoami } => whoami_text(whoami),
        Response::Captures { captures } => captures_text(captures),
        Response::Messages { messages } => messages_text(messages, now),
        Response::Chat { messages } => chat_text(messages),
        Response::QItems { items } => qitems_table(items),
        Response::QItem { item } => qitem_text(item, now),
        Response::Watchdogs { watchdogs } => watchdogs_table(watchdogs, now),
        // The console never asks for entries; said plainly all the same.
        Response::Conversation { conversation } => conversation_text(conversation),
    }
}

/// A conversation's entries, one "who: words" line each, as `ainet
/// transcript` prints them.
fn conversation_text(c: &ainet_core::Conversation) -> String {
    use ainet_core::conversation::EntryKind;
    let mut out = String::new();
    if let Some(notice) = &c.notice {
        out.push_str(&format!("{notice}\n"));
    }
    if let Some(plain) = &c.plain {
        out.push_str(plain.trim_end());
        out.push('\n');
    }
    for e in &c.entries {
        let line = match e.kind {
            EntryKind::You => format!("user: {}", e.text),
            EntryKind::Reply => format!("assistant: {}", e.text),
            EntryKind::Tool if e.code.is_empty() => format!("tool: {}", e.tool),
            EntryKind::Tool => format!("tool: {}: {}", e.tool, first_line(&e.code, 100)),
            EntryKind::Inbound => format!("message from {}: {}", e.from, e.text),
            EntryKind::Other => continue,
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn agents_table(agents: &[Agent]) -> String {
    let mut t = Table::new(&["NAME", "SEAT", "STATE", "RUNTIME", "UNREAD", "TASK"]);
    for a in agents {
        t.row([
            a.name.clone(),
            or_dash(a.seat.as_deref()),
            a.state.label().to_string(),
            a.runtime.spec_name().to_string(),
            a.unread.to_string(),
            first_line(a.task.as_deref().or(a.role.as_deref()).unwrap_or(""), 60),
        ]);
    }
    t.render()
}

pub fn rigs_table(rigs: &[Rig]) -> String {
    if rigs.is_empty() {
        return "no rigs\n".into();
    }
    let mut t = Table::new(&["RIG", "STATUS", "SEATS", "SUMMARY"]);
    for r in rigs {
        let running = r.seats.iter().filter(|s| s.status == SeatStatus::Running).count();
        t.row([
            r.name.clone(),
            r.status.label().to_string(),
            format!("{running}/{}", r.seats.len()),
            first_line(r.summary.as_deref().unwrap_or(""), 60),
        ]);
    }
    t.render()
}

/// A rig's seats, with each occupant's state when `agents` knows it.
pub fn rig_detail(rig: &Rig, agents: &[Agent]) -> String {
    let mut out = format!("{}  {}", rig.name, rig.status.label());
    if let Some(s) = &rig.summary {
        out.push_str(&format!("  {}", first_line(s, 60)));
    }
    out.push('\n');
    out.push_str(&seats_table(std::slice::from_ref(rig), agents, false));
    out
}

/// One row per seat; `with_rig` adds a RIG column (`ps --nodes`).
pub fn seats_table(rigs: &[Rig], agents: &[Agent], with_rig: bool) -> String {
    let mut header = vec!["SEAT", "RUNTIME", "MODEL", "STATE", "STATUS", "HELD"];
    if with_rig {
        header.insert(0, "RIG");
    }
    let mut t = Table::new(&header);
    for rig in rigs {
        for s in &rig.seats {
            let agent = s
                .agent
                .as_deref()
                .and_then(|id| agents.iter().find(|a| a.id == id));
            let mut row = vec![
                s.address.clone(),
                s.runtime.spec_name().to_string(),
                or_dash(s.model.as_deref().or(agent.and_then(|a| a.model.as_deref()))),
                agent
                    .map(|a| a.state.label().to_string())
                    .unwrap_or_else(|| "-".into()),
                s.status.label().to_string(),
                if s.typing_guard {
                    format!("{} (guard)", s.held)
                } else {
                    s.held.to_string()
                },
            ];
            if with_rig {
                row.insert(0, rig.name.clone());
            }
            t.row(row);
        }
    }
    t.render()
}

pub fn snapshots_table(snaps: &[SnapshotInfo], now: i64) -> String {
    if snaps.is_empty() {
        return "no snapshots\n".into();
    }
    let mut t = Table::new(&["ID", "RIG", "LABEL", "SEATS", "TAKEN"]);
    for s in snaps {
        t.row([
            s.id.clone(),
            s.rig.clone(),
            or_dash(s.label.as_deref()),
            s.seats.to_string(),
            ago(now, s.created),
        ]);
    }
    t.render()
}

pub fn specs_table(specs: &[SpecEntry]) -> String {
    let mut t = Table::new(&["NAME", "SEATS", "SOURCE", "SUMMARY"]);
    for s in specs {
        t.row([
            s.name.clone(),
            s.seats.to_string(),
            s.source.clone(),
            first_line(s.summary.as_deref().unwrap_or(""), 60),
        ]);
    }
    t.render()
}

pub fn discovered_table(sessions: &[Discovered]) -> String {
    if sessions.is_empty() {
        return "no sessions found outside ainet\n".into();
    }
    let mut t = Table::new(&["ID", "RUNTIME", "TMUX", "PID", "SEAT", "CWD"]);
    for d in sessions {
        t.row([
            d.id.clone(),
            d.runtime.spec_name().to_string(),
            or_dash(d.tmux_session.as_deref()),
            d.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            or_dash(d.seat.as_deref()),
            d.cwd.display().to_string(),
        ]);
    }
    t.render()
}

pub fn whoami_text(w: &Whoami) -> String {
    let mut out = format!("agent:    {} ({})\n", w.name, w.agent);
    match &w.seat {
        Some(s) => {
            out.push_str(&format!(
                "seat:     {}  (logical id {}, rig {})\n",
                s.address, s.logical, s.rig
            ));
            if let Some(role) = &s.role {
                out.push_str(&format!("role:     {role}\n"));
            }
        }
        None => out.push_str("seat:     none (not in a rig)\n"),
    }
    let list = |v: &[String]| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(", ")
        }
    };
    out.push_str(&format!("delegates to: {}\n", list(&w.delegates_to)));
    out.push_str(&format!("delegated by: {}\n", list(&w.delegated_by)));
    out.push_str(&format!("peers:        {}\n", list(&w.peers)));
    out.push_str(&format!(
        "unread messages: {}, open queue items: {}\n",
        w.unread, w.open_queue
    ));
    out
}

pub fn captures_text(caps: &[Capture]) -> String {
    let mut out = String::new();
    for c in caps {
        out.push_str(&format!("=== {} ===\n", c.label));
        for l in &c.lines {
            out.push_str(l);
            out.push('\n');
        }
    }
    if caps.is_empty() {
        out.push_str("nothing captured\n");
    }
    out
}

pub fn messages_text(msgs: &[Message], now: i64) -> String {
    if msgs.is_empty() {
        return "no messages\n".into();
    }
    let mut out = String::new();
    for m in msgs {
        let re = m.reply_to.map(|r| format!(" (re #{r})")).unwrap_or_default();
        let held = if m.held { " [held]" } else { "" };
        out.push_str(&format!(
            "#{} {} -> {}{re}{held}, {}\n",
            m.id,
            m.from_label,
            m.to_label,
            ago(now, m.created)
        ));
        for line in m.body.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn chat_line(m: &ChatMessage) -> String {
    format!(
        "#{} [{}] {}: {}\n",
        m.id,
        m.topic,
        m.sender,
        m.body.replace('\n', "\n    ")
    )
}

pub fn chat_text(msgs: &[ChatMessage]) -> String {
    if msgs.is_empty() {
        return "no chat messages\n".into();
    }
    msgs.iter().map(chat_line).collect()
}

pub fn qitems_table(items: &[QItem]) -> String {
    if items.is_empty() {
        return "no queue items\n".into();
    }
    let mut t = Table::new(&["ID", "STATE", "PRIORITY", "TO", "OWNER", "SUMMARY"]);
    for q in items {
        t.row([
            q.id.clone(),
            q.state.label().to_string(),
            q.priority.clone(),
            q.destination.clone(),
            or_dash(q.owner.as_deref()),
            first_line(&q.summary, 60),
        ]);
    }
    t.render()
}

pub fn qitem_text(q: &QItem, now: i64) -> String {
    ainet_model::popup::qitem_text(q, now)
}

pub fn watchdogs_table(w: &[Watchdog], now: i64) -> String {
    if w.is_empty() {
        return "no watchdogs\n".into();
    }
    let mut t = Table::new(&["ID", "TARGET", "EVERY", "ACTIVE", "LAST", "MESSAGE"]);
    for d in w {
        t.row([
            d.id.clone(),
            d.target.clone(),
            format!("{}s", d.interval_secs),
            if d.active { "yes" } else { "no" }.to_string(),
            d.last_fired.map(|t| ago(now, t)).unwrap_or_else(|| "-".into()),
            first_line(&d.message, 50),
        ]);
    }
    t.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainet_core::{QState, RigStatus, Runtime, Seat, Transport};
    use std::path::PathBuf;

    pub(crate) fn seat(rig: &str, pod: &str, member: &str, status: SeatStatus) -> Seat {
        Seat {
            address: format!("{pod}-{member}@{rig}"),
            logical: format!("{pod}.{member}"),
            rig: rig.into(),
            pod: pod.into(),
            member: member.into(),
            label: None,
            role: None,
            runtime: Runtime::ClaudeCode,
            model: Some("opus".into()),
            cwd: PathBuf::from("/w"),
            agent: None,
            status,
            transport: Transport::Pane,
            restore_policy: "resume_if_possible".into(),
            typing_guard: false,
            held: 0,
        }
    }

    pub(crate) fn rig() -> Rig {
        Rig {
            name: "starter".into(),
            summary: Some("two seats".into()),
            spec_path: None,
            root: PathBuf::from("/w"),
            status: RigStatus::Partial,
            seats: vec![
                seat("starter", "dev", "impl", SeatStatus::Running),
                seat("starter", "dev", "review", SeatStatus::Stopped),
            ],
            edges: vec![],
            created: 0,
        }
    }

    #[test]
    fn table_aligns_columns() {
        let mut t = Table::new(&["A", "BB"]);
        t.row(["long", "x"]);
        assert_eq!(t.render(), "A     BB\nlong  x\n");
    }

    #[test]
    fn rigs_show_running_over_total() {
        let out = human(&Response::Rigs { rigs: vec![rig()] });
        assert!(out.starts_with("RIG"), "{out}");
        assert!(out.contains("starter  partial  1/2"), "{out}");
    }

    #[test]
    fn rig_lists_seats() {
        let out = human(&Response::Rig { rig: rig() });
        assert!(out.contains("dev-review@starter"));
        assert!(out.contains("claude-code"));
        assert!(out.contains("stopped"));
    }

    #[test]
    fn json_drops_the_tag() {
        let out = json(&Response::Text { text: "hi".into() });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, serde_json::json!({ "text": "hi" }));
    }

    #[test]
    fn errors_and_text() {
        assert_eq!(
            human(&Response::Error {
                message: "nope".into()
            }),
            "error: nope\n"
        );
        assert_eq!(human(&Response::Text { text: "a: b".into() }), "a: b\n");
        assert_eq!(human(&Response::Ok), "ok\n");
    }

    #[test]
    fn messages_and_queue() {
        let m = Message {
            id: 7,
            from: "x".into(),
            from_label: "dev-impl@starter".into(),
            to: "y".into(),
            to_label: "dev-review@starter".into(),
            body: "look\nat this".into(),
            reply_to: Some(3),
            thread: 3,
            created: ainet_core::now_ms(),
            delivered: None,
            read: None,
            held: false,
        };
        let out = human(&Response::Messages { messages: vec![m] });
        assert!(
            out.starts_with("#7 dev-impl@starter -> dev-review@starter (re #3)"),
            "{out}"
        );
        assert!(out.contains("\n  at this\n"));
        let q = QItem {
            id: "q-1".into(),
            rig: Some("starter".into()),
            source: "operator".into(),
            destination: "dev.impl".into(),
            owner: None,
            summary: "fix it".into(),
            body: String::new(),
            priority: "routine".into(),
            tags: vec![],
            state: QState::InProgress,
            reply_to: None,
            block: None,
            successor: None,
            notes: vec![],
            created: 0,
            updated: 0,
        };
        let out = human(&Response::QItems {
            items: vec![q.clone()],
        });
        assert!(
            out.contains("q-1  in-progress  routine   dev.impl  -      fix it"),
            "{out}"
        );
        assert!(
            human(&Response::QItem { item: Box::new(q) })
                .starts_with("q-1  in-progress  fix it\nfrom operator to dev.impl")
        );
    }

    #[test]
    fn ago_units() {
        assert_eq!(ago(10_000, 5_000), "5s ago");
        assert_eq!(ago(3_600_000, 0), "1h ago");
        assert_eq!(ago(0, 10), "0s ago");
    }
}
