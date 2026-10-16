//! Owned work (OpenRig's queue) and scheduled wakes (its watchdog).
//!
//! A queue item has a source, a destination and, once claimed, an owner, all
//! recorded by seat address so they survive the agent in the seat being
//! replaced. Creating one nudges the destination with a message; finishing
//! one tells the source; handing off closes it and opens a successor; an
//! item can block on another item (and wakes when that one is done) or on
//! something external (and wakes after a while). A watchdog types a
//! reminder into a seat every so often.

use std::time::Duration;

use ainet_core::protocol::QSpec;
use ainet_core::{Event, QBlock, QItem, QNote, QState, Response, Watchdog, now_ms};

use crate::comms::OPERATOR;
use crate::{Daemon, Shared, err, lock};

impl Daemon {
    /// A stable name for an agent in queue records: its seat address, else
    /// its critter name; "operator" for you.
    fn ident(&self, id: &str) -> String {
        if id == OPERATOR {
            return OPERATOR.into();
        }
        self.agents
            .get(id)
            .map(|a| a.seat.clone().unwrap_or_else(|| a.name.clone()))
            .unwrap_or_else(|| id.to_string())
    }

    /// The agent behind a queue identity (seat address, name, id).
    fn agent_for(&self, ident: &str) -> Option<String> {
        if ident == OPERATOR {
            return None;
        }
        self.find_agent(ident, None)
    }

    fn caller(&self, by: Option<&str>) -> String {
        match by {
            None | Some(OPERATOR) => OPERATOR.into(),
            Some(b) => self.find_agent(b, None).unwrap_or_else(|| b.to_string()),
        }
    }

    fn save_qitem(&mut self, item: &QItem) {
        self.store.put("qitem", &item.id, item);
        self.queue.insert(item.id.clone(), item.clone());
        self.emit(Event::Queue { item: item.clone() });
    }

    fn note(item: &mut QItem, by: &str, text: String, state: Option<QState>) {
        item.notes.push(QNote {
            by: by.to_string(),
            text,
            at: now_ms(),
            state,
        });
        item.updated = now_ms();
    }
}

/// Sends a short coordination note as a message, so it lands in the
/// recipient's terminal and mailbox like any other.
async fn notify(d: &Shared, from: &str, to_ident: &str, text: String) {
    let (to, shell) = {
        let g = lock(d);
        let Some(to) = g.agent_for(to_ident) else { return };
        let shell = g
            .agents
            .get(&to)
            .is_some_and(|a| a.runtime == ainet_core::Runtime::Shell);
        (to, shell)
    };
    if shell {
        // A shell would run the notice as a command: it stays in the mailbox.
        lock(d).new_message(from, &to, text, None);
        return;
    }
    let from = if from == OPERATOR {
        None
    } else {
        Some(from.to_string())
    };
    let _ = crate::comms::message(d, from, &[to], None, None, text, None, false).await;
}

pub(crate) async fn create(d: &Shared, spec: QSpec) -> Response {
    if spec.summary.trim().is_empty() {
        return err("a queue item needs a summary");
    }
    let (item, source_id) = {
        let mut g = lock(d);
        let source_id = g.caller(spec.source.as_deref());
        let Some(dest) = g
            .find_agent(&spec.destination, None)
            .map(|id| g.ident(&id))
            .or_else(|| {
                // A seat that is not running right now still takes work.
                g.find_seat(&spec.destination, spec.rig.as_deref())
                    .map(|s| s.address.clone())
            })
        else {
            return err(format!("no agent or seat {:?}", spec.destination));
        };
        g.next_qitem += 1;
        let n = g.next_qitem;
        g.store.put("counter", "qitem", &n);
        let now = now_ms();
        let rig = spec
            .rig
            .clone()
            .or_else(|| g.rig_of(&source_id))
            .or_else(|| g.find_seat(&dest, None).map(|s| s.rig.clone()));
        let item = QItem {
            id: format!("q-{n}"),
            rig,
            source: g.ident(&source_id),
            destination: dest,
            owner: None,
            summary: spec.summary.trim().to_string(),
            body: spec.body,
            priority: spec.priority.unwrap_or_else(|| "normal".into()),
            tags: spec.tags,
            state: QState::Pending,
            reply_to: spec.reply_to,
            block: None,
            successor: None,
            notes: Vec::new(),
            created: now,
            updated: now,
        };
        g.save_qitem(&item);
        (item, source_id)
    };
    let nudge = spec.nudge.unwrap_or_else(|| lock(d).settings().queue.nudge);
    if nudge {
        let text = format!(
            "Queue item {} is for you, from {}: {}\n{}\nClaim it with the ainet tool queue_claim id={} (shell: ainet queue claim {}), then work it and close it with queue_update state=done.",
            item.id, item.source, item.summary, item.body, item.id, item.id
        );
        notify(d, &source_id, &item.destination, text).await;
    }
    Response::QItem { item: Box::new(item) }
}

pub(crate) fn list(
    d: &Shared,
    rig: Option<&str>,
    who: Option<&str>,
    state: Option<QState>,
    all: bool,
) -> Response {
    let g = lock(d);
    let who = who.map(|w| {
        g.find_agent(w, None)
            .map(|id| g.ident(&id))
            .unwrap_or_else(|| w.to_string())
    });
    let items = g
        .queue
        .values()
        .filter(|q| rig.is_none_or(|r| q.rig.as_deref() == Some(r)))
        .filter(|q| {
            who.as_deref()
                .is_none_or(|w| q.destination == w || q.owner.as_deref() == Some(w))
        })
        .filter(|q| state.is_none_or(|s| q.state == s))
        .filter(|q| all || state.is_some() || q.state.is_open())
        .cloned()
        .collect();
    Response::QItems { items }
}

pub(crate) fn show(d: &Shared, id: &str) -> Response {
    match lock(d).queue.get(id) {
        Some(q) => Response::QItem {
            item: Box::new(q.clone()),
        },
        None => err(format!("no queue item {id}")),
    }
}

pub(crate) async fn claim(d: &Shared, id: &str, by: Option<String>) -> Response {
    let (item, by_ident) = {
        let mut g = lock(d);
        let caller = g.caller(by.as_deref());
        let me = g.ident(&caller);
        let Some(mut item) = g.queue.get(id).cloned() else {
            return err(format!("no queue item {id}"));
        };
        match item.state {
            QState::Pending => {}
            QState::InProgress if item.owner.as_deref() == Some(&me) => {
                return Response::QItem { item: Box::new(item) };
            }
            s => {
                return err(format!(
                    "{id} is {} (owner {})",
                    s.label(),
                    item.owner.as_deref().unwrap_or("none")
                ));
            }
        }
        item.owner = Some(me.clone());
        item.state = QState::InProgress;
        Daemon::note(&mut item, &me, "claimed".into(), Some(QState::InProgress));
        g.save_qitem(&item);
        (item, me)
    };
    if item.source != by_ident {
        notify(
            d,
            &by_ident_agent(d, &by_ident),
            &item.source,
            format!("{by_ident} claimed {}: {}", item.id, item.summary),
        )
        .await;
    }
    Response::QItem { item: Box::new(item) }
}

/// The agent id behind an identity, or the identity itself (operator).
fn by_ident_agent(d: &Shared, ident: &str) -> String {
    lock(d).agent_for(ident).unwrap_or_else(|| OPERATOR.into())
}

pub(crate) async fn unclaim(d: &Shared, id: &str, by: Option<String>, reason: Option<String>) -> Response {
    let mut g = lock(d);
    let caller = g.caller(by.as_deref());
    let me = g.ident(&caller);
    let Some(mut item) = g.queue.get(id).cloned() else {
        return err(format!("no queue item {id}"));
    };
    if item.state != QState::InProgress {
        return err(format!("{id} is {}, not in progress", item.state.label()));
    }
    item.owner = None;
    item.state = QState::Pending;
    Daemon::note(
        &mut item,
        &me,
        format!("released: {}", reason.unwrap_or_default()),
        Some(QState::Pending),
    );
    g.save_qitem(&item);
    Response::QItem { item: Box::new(item) }
}

pub(crate) async fn update(
    d: &Shared,
    id: &str,
    by: Option<String>,
    note: Option<String>,
    state: Option<QState>,
) -> Response {
    if note.is_none() && state.is_none() {
        return err("give a note, a state, or both");
    }
    let (item, me, finished) = {
        let mut g = lock(d);
        let caller = g.caller(by.as_deref());
        let me = g.ident(&caller);
        let Some(mut item) = g.queue.get(id).cloned() else {
            return err(format!("no queue item {id}"));
        };
        if let Some(s) = state {
            item.state = s;
            if s == QState::InProgress && item.owner.is_none() {
                item.owner = Some(me.clone());
            }
            if s != QState::Blocked {
                item.block = None;
            }
        }
        Daemon::note(&mut item, &me, note.unwrap_or_default(), state);
        g.save_qitem(&item);
        let finished = state == Some(QState::Done);
        (item, me, finished)
    };
    if finished {
        let from = by_ident_agent(d, &me);
        if item.source != me {
            let last = item.notes.last().map(|n| n.text.clone()).unwrap_or_default();
            notify(
                d,
                &from,
                &item.source,
                format!("{me} finished {}: {}\n{last}", item.id, item.summary),
            )
            .await;
        }
        unblock_dependents(d, &item.id).await;
    }
    Response::QItem { item: Box::new(item) }
}

pub(crate) async fn handoff(
    d: &Shared,
    id: &str,
    by: Option<String>,
    to: &str,
    body: String,
    summary: Option<String>,
) -> Response {
    let (old, spec) = {
        let mut g = lock(d);
        let caller = g.caller(by.as_deref());
        let me = g.ident(&caller);
        let Some(mut old) = g.queue.get(id).cloned() else {
            return err(format!("no queue item {id}"));
        };
        let spec = QSpec {
            rig: old.rig.clone(),
            source: Some(caller.clone()),
            destination: to.to_string(),
            summary: summary.unwrap_or_else(|| old.summary.clone()),
            body,
            priority: Some(old.priority.clone()),
            tags: old.tags.clone(),
            reply_to: Some(old.id.clone()),
            // A successor is always announced: the hand-off says so.
            nudge: Some(true),
        };
        old.state = QState::Done;
        Daemon::note(&mut old, &me, format!("handed off to {to}"), Some(QState::Done));
        g.save_qitem(&old);
        (old, spec)
    };
    let created = create(d, spec).await;
    if let Response::QItem { item } = &created {
        let mut g = lock(d);
        let mut old = old;
        old.successor = Some(item.id.clone());
        g.save_qitem(&old);
    }
    created
}

pub(crate) async fn block(
    d: &Shared,
    id: &str,
    by: Option<String>,
    on: String,
    continuation: String,
    wake_after_secs: Option<u64>,
) -> Response {
    let mut g = lock(d);
    let caller = g.caller(by.as_deref());
    let me = g.ident(&caller);
    let Some(mut item) = g.queue.get(id).cloned() else {
        return err(format!("no queue item {id}"));
    };
    if !on.starts_with("external:") && !g.queue.contains_key(&on) {
        return err(format!("{on} is neither a queue item nor external:<name>"));
    }
    item.state = QState::Blocked;
    if item.owner.is_none() {
        item.owner = Some(me.clone());
    }
    item.block = Some(QBlock {
        on: on.clone(),
        continuation: continuation.clone(),
        wake_at: wake_after_secs.map(|s| now_ms() + s as i64 * 1000),
    });
    Daemon::note(
        &mut item,
        &me,
        format!("blocked on {on}; next: {continuation}"),
        Some(QState::Blocked),
    );
    g.save_qitem(&item);
    Response::QItem { item: Box::new(item) }
}

/// Items blocked on `done_id` go back to their owners.
async fn unblock_dependents(d: &Shared, done_id: &str) {
    let woken: Vec<QItem> = {
        let mut g = lock(d);
        let ids: Vec<String> = g
            .queue
            .values()
            .filter(|q| q.block.as_ref().is_some_and(|b| b.on == done_id))
            .map(|q| q.id.clone())
            .collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(mut q) = g.queue.get(&id).cloned() else {
                continue;
            };
            q.state = QState::InProgress;
            Daemon::note(
                &mut q,
                "ainet",
                format!("unblocked: {done_id} is done"),
                Some(QState::InProgress),
            );
            out.push(q.clone());
            q.block = None;
            g.save_qitem(&q);
        }
        out
    };
    for q in woken {
        let next = q.block.map(|b| b.continuation).unwrap_or_default();
        if let Some(owner) = q.owner.clone() {
            notify(
                d,
                OPERATOR,
                &owner,
                format!(
                    "{} is unblocked ({done_id} is done): {}\nNext: {next}",
                    q.id, q.summary
                ),
            )
            .await;
        }
    }
}

pub(crate) fn watchdog_register(
    d: &Shared,
    target: &str,
    interval_secs: u64,
    message: Option<String>,
    registered_by: Option<String>,
) -> Response {
    let mut g = lock(d);
    let w = g.settings().queue.watchdog.clone();
    // None given: the default from settings.json.
    let interval_secs = if interval_secs == 0 {
        w.default_interval_secs
    } else {
        interval_secs
    };
    if interval_secs < w.min_interval_secs {
        return err(format!(
            "a watchdog interval must be at least {} seconds",
            w.min_interval_secs
        ));
    }
    let target_ident = match g.find_agent(target, None) {
        Some(id) => g.ident(&id),
        None => match g.find_seat(target, None) {
            Some(s) => s.address.clone(),
            None => return err(format!("no agent or seat {target}")),
        },
    };
    let by = g.caller(registered_by.as_deref());
    g.next_watchdog += 1;
    let n = g.next_watchdog;
    g.store.put("counter", "watchdog", &n);
    let w = Watchdog {
        id: format!("w-{n}"),
        target: target_ident,
        interval_secs,
        message: message.unwrap_or(w.message),
        registered_by: g.ident(&by),
        reason: None,
        created: now_ms(),
        last_fired: None,
        active: true,
    };
    g.store.put("watchdog", &w.id, &w);
    g.watchdogs.insert(w.id.clone(), w.clone());
    g.emit(Event::Watchdog { watchdog: w.clone() });
    Response::Watchdogs { watchdogs: vec![w] }
}

pub(crate) fn watchdog_stop(d: &Shared, id: &str, reason: Option<String>) -> Response {
    let mut g = lock(d);
    let Some(mut w) = g.watchdogs.get(id).cloned() else {
        return err(format!("no watchdog {id}"));
    };
    w.active = false;
    w.reason = reason;
    g.store.put("watchdog", &w.id, &w);
    g.watchdogs.insert(w.id.clone(), w.clone());
    g.emit(Event::Watchdog { watchdog: w.clone() });
    Response::Watchdogs { watchdogs: vec![w] }
}

/// Fires due watchdogs and wakes blocked items whose wait is over, every
/// `queue.watchdog.tick_secs`.
pub(crate) async fn ticker(d: Shared) {
    loop {
        let tick = lock(&d).settings().queue.watchdog.tick_secs;
        tokio::time::sleep(Duration::from_secs(tick)).await;
        let now = now_ms();
        let (dogs, wakes) = {
            let mut g = lock(&d);
            let due: Vec<Watchdog> = g
                .watchdogs
                .values()
                .filter(|w| {
                    w.active && now - w.last_fired.unwrap_or(w.created) >= w.interval_secs as i64 * 1000
                })
                .cloned()
                .collect();
            for w in &due {
                let mut w2 = w.clone();
                w2.last_fired = Some(now);
                g.store.put("watchdog", &w2.id, &w2);
                g.watchdogs.insert(w2.id.clone(), w2);
            }
            let wakes: Vec<QItem> = g
                .queue
                .values()
                .filter(|q| {
                    q.state == QState::Blocked
                        && q.block.as_ref().and_then(|b| b.wake_at).is_some_and(|t| t <= now)
                })
                .cloned()
                .collect();
            for q in &wakes {
                let mut q2 = q.clone();
                if let Some(b) = q2.block.as_mut() {
                    b.wake_at = None;
                }
                g.save_qitem(&q2);
            }
            (due, wakes)
        };
        for w in dogs {
            let agent = lock(&d).agent_for(&w.target);
            if let Some(a) = agent {
                let busy = lock(&d)
                    .agents
                    .get(&a)
                    .is_some_and(|x| x.state == ainet_core::AgentState::Working);
                // A wake is for idle agents; a busy one will get the next.
                if !busy && let Err(e) = crate::comms::deliver_text(&d, &a, &w.message).await {
                    tracing::info!("watchdog {} not delivered: {e:#}", w.id);
                }
            }
        }
        for q in wakes {
            let Some(owner) = q.owner.clone() else { continue };
            let b = q.block.clone().unwrap_or(QBlock {
                on: String::new(),
                continuation: String::new(),
                wake_at: None,
            });
            notify(
                &d,
                OPERATOR,
                &owner,
                format!(
                    "Wake for {} (still blocked on {}): {}\nNext: {}",
                    q.id, b.on, q.summary, b.continuation
                ),
            )
            .await;
        }
    }
}
