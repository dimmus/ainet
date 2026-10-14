//! Turns daemon replies into text for the TUI's result popup.

use ainet_core::Response;

pub(crate) fn render(resp: &Response) -> String {
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
        other => out.push_str(&format!("{other:?}")),
    }
    out
}
