//! What happened in the fleet, one line each, for an events log: agents
//! joining, leaving and changing state, rigs changing status, requests
//! and reports in the inbox, messages, chat posts, queue items moving on,
//! the connection to ainetd. Each is read off a [`Msg`] before it is
//! applied, against the store as it was, so a change can say what it was
//! before (`idle → working`).

use ainet_core::{Event, now_ms};

use crate::link::Msg;
use crate::store::Store;

/// What kind of thing happened, for an icon and a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Joined,
    Left,
    /// An agent's state changed.
    State,
    /// A rig came, went or changed status.
    Rig,
    /// Something joined the inbox; `waiting` when it waits on you.
    Inbox {
        waiting: bool,
    },
    /// A request was answered.
    Answered,
    Message,
    Chat,
    Queue,
    Connection {
        up: bool,
    },
}

/// One line of the events log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Happening {
    /// When it was heard of (Unix ms).
    pub at: i64,
    pub kind: Kind,
    /// The agent it is about (its id), if one.
    pub agent: Option<String>,
    pub text: String,
}

/// What applying `msg` to `store` will change, worth a line in the log.
/// Most messages are worth none: output, screens, a message read.
pub fn happened(store: &Store, msg: &Msg) -> Option<Happening> {
    let line = |kind, agent: Option<&str>, text: String| Happening {
        at: now_ms(),
        kind,
        agent: agent.map(str::to_string),
        text,
    };
    let label = |id: &str| {
        store
            .agents
            .get(id)
            .map(|a| a.label())
            .unwrap_or_else(|| id.to_string())
    };
    match msg {
        Msg::Connected(up) if *up != store.connected => Some(line(
            Kind::Connection { up: *up },
            None,
            if *up {
                "connected to ainetd".into()
            } else {
                "lost ainetd".into()
            },
        )),
        Msg::Event(ev) => match ev.as_ref() {
            Event::Agent { agent } => match store.agents.get(&agent.id) {
                None => Some(line(
                    Kind::Joined,
                    Some(&agent.id),
                    format!("{} joined the fleet", agent.label()),
                )),
                Some(was) if was.state != agent.state => Some(line(
                    Kind::State,
                    Some(&agent.id),
                    format!(
                        "{}: {} → {}",
                        agent.label(),
                        was.state.label(),
                        agent.state.label()
                    ),
                )),
                Some(_) => None,
            },
            Event::AgentRemoved { id } => store
                .agents
                .get(id)
                .map(|a| line(Kind::Left, Some(id), format!("{} left the fleet", a.label()))),
            Event::Rig { rig } => {
                let was = store.rigs.get(&rig.name).map(|r| r.status);
                (was != Some(rig.status)).then(|| {
                    line(
                        Kind::Rig,
                        None,
                        format!("rig {} is {}", rig.name, rig.status.label()),
                    )
                })
            }
            Event::RigRemoved { name } => store
                .rigs
                .contains_key(name)
                .then(|| line(Kind::Rig, None, format!("rig {name} is gone"))),
            Event::Inbox { item } => Some(line(
                Kind::Inbox {
                    waiting: item.waiting,
                },
                Some(&item.agent),
                format!(
                    "{}: {} ({})",
                    label(&item.agent),
                    first_line(&item.summary),
                    item.kind.label()
                ),
            )),
            Event::InboxResolved { id } => store.inbox.iter().find(|i| i.id == *id && i.waiting).map(|i| {
                line(
                    Kind::Answered,
                    Some(&i.agent),
                    format!("{}'s {} answered", label(&i.agent), i.kind.label()),
                )
            }),
            Event::Message { message: m } => (!store.messages.iter().any(|x| x.id == m.id)).then(|| {
                line(
                    Kind::Message,
                    Some(&m.from),
                    format!("{} → {}: {}", m.from_label, m.to_label, first_line(&m.body)),
                )
            }),
            Event::Chat { message: c } => Some(line(
                Kind::Chat,
                None,
                format!("#{} {} · {}: {}", c.topic, c.rig, c.sender, first_line(&c.body)),
            )),
            Event::Queue { item: q } => match store.queue.get(&q.id) {
                None => Some(line(
                    Kind::Queue,
                    None,
                    format!("{} for {}: {}", q.id, q.destination, first_line(&q.summary)),
                )),
                Some(was) if was.state != q.state => {
                    let by = q.owner.as_deref().map(|o| format!(" ({o})")).unwrap_or_default();
                    Some(line(
                        Kind::Queue,
                        None,
                        format!("{} is {}{by}", q.id, q.state.label()),
                    ))
                }
                Some(_) => None,
            },
            Event::Output { .. } | Event::Watchdog { .. } => None,
        },
        _ => None,
    }
}

/// The first line of `s`, cut to a log line's length.
fn first_line(s: &str) -> String {
    const MAX: usize = 120;
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > MAX {
        format!("{}…", line.chars().take(MAX - 1).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use ainet_core::{AgentState, Event, InboxKind, QState, RigStatus};

    use super::{Kind, happened};
    use crate::demo::Demo;
    use crate::link::Msg;
    use crate::store::Store;

    fn demo() -> Store {
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        store
    }

    /// The line for `ev`, then applies it.
    fn see(store: &mut Store, ev: Event) -> Option<(Kind, String)> {
        let msg = Msg::Event(Box::new(ev));
        let line = happened(store, &msg).map(|h| (h.kind, h.text));
        store.apply(msg);
        line
    }

    #[test]
    fn agents_join_change_state_and_leave() {
        let mut store = demo();
        let mut lead = store.find_agent("orch-lead@docs").unwrap().clone();
        let was = lead.state;
        lead.state = if was == AgentState::Working {
            AgentState::Idle
        } else {
            AgentState::Working
        };
        let now = lead.state;
        assert_eq!(
            see(&mut store, Event::Agent { agent: lead.clone() }),
            Some((
                Kind::State,
                format!("orch-lead@docs: {} → {}", was.label(), now.label())
            ))
        );
        // The same state again (a new task, say) is no news.
        lead.task = Some("something else".into());
        assert_eq!(see(&mut store, Event::Agent { agent: lead.clone() }), None);

        let mut new = lead.clone();
        new.id = "new-agent".into();
        new.seat = Some("dev-new@docs".into());
        assert_eq!(
            see(&mut store, Event::Agent { agent: new }),
            Some((Kind::Joined, "dev-new@docs joined the fleet".into()))
        );
        assert_eq!(
            see(
                &mut store,
                Event::AgentRemoved {
                    id: "new-agent".into()
                }
            ),
            Some((Kind::Left, "dev-new@docs left the fleet".into()))
        );
        assert_eq!(see(&mut store, Event::AgentRemoved { id: "nobody".into() }), None);
    }

    #[test]
    fn rigs_inbox_messages_chat_and_queue() {
        let mut store = demo();
        let mut rig = store.rigs["docs"].clone();
        assert_eq!(see(&mut store, Event::Rig { rig: rig.clone() }), None);
        rig.status = RigStatus::Stopped;
        assert_eq!(
            see(&mut store, Event::Rig { rig }),
            Some((Kind::Rig, "rig docs is stopped".into()))
        );

        let mut item = store.inbox[0].clone();
        item.id = 4242;
        item.waiting = true;
        item.kind = InboxKind::Permission;
        item.summary = "Bash: ls\nmore".into();
        let who = store.agents[&item.agent].label();
        assert_eq!(
            see(&mut store, Event::Inbox { item }),
            Some((
                Kind::Inbox { waiting: true },
                format!("{who}: Bash: ls (permission)")
            ))
        );
        assert_eq!(
            see(&mut store, Event::InboxResolved { id: 4242 }),
            Some((Kind::Answered, format!("{who}'s permission answered")))
        );
        assert_eq!(see(&mut store, Event::InboxResolved { id: 4242 }), None, "once");

        let mut m = store.messages[0].clone();
        assert_eq!(
            see(&mut store, Event::Message { message: m.clone() }),
            None,
            "an update"
        );
        m.id = 9999;
        m.body = "\nhello there\nsecond".into();
        let text = format!("{} → {}: hello there", m.from_label, m.to_label);
        assert_eq!(
            see(&mut store, Event::Message { message: m }),
            Some((Kind::Message, text))
        );

        let mut c = store.chat[0].clone();
        c.body = "standup in 5".into();
        let text = format!("#{} {} · {}: standup in 5", c.topic, c.rig, c.sender);
        assert_eq!(
            see(&mut store, Event::Chat { message: c }),
            Some((Kind::Chat, text))
        );

        let mut q = store.queue.values().next().unwrap().clone();
        assert_eq!(see(&mut store, Event::Queue { item: q.clone() }), None);
        q.state = QState::Done;
        q.owner = Some("dev-app@docs".into());
        let text = format!("{} is done (dev-app@docs)", q.id);
        assert_eq!(
            see(&mut store, Event::Queue { item: q.clone() }),
            Some((Kind::Queue, text))
        );
        q.id = "q-77".into();
        let text = format!("q-77 for {}: {}", q.destination, q.summary);
        assert_eq!(
            see(&mut store, Event::Queue { item: q }),
            Some((Kind::Queue, text))
        );
    }

    #[test]
    fn the_connection_is_logged_when_it_changes() {
        let store = Store::new();
        assert_eq!(happened(&store, &Msg::Connected(false)), None);
        let up = happened(&store, &Msg::Connected(true)).unwrap();
        assert_eq!(
            (up.kind, up.text.as_str()),
            (Kind::Connection { up: true }, "connected to ainetd")
        );
        assert_eq!(happened(&store, &Msg::Flash("x".into(), false)), None);
    }
}
