//! How agents talk to each other (and to you).
//!
//! A message is stored in the recipient's mailbox and typed into its
//! terminal with a header saying who sent it and how to answer. Replies
//! thread back to the sender, and `WaitReply` lets an agent ask a question
//! and block until the answer comes (`ask_agent`). Each rig also has a
//! chatroom with topics; `@mentions` in it are delivered like messages.
//!
//! Delivery never lands under what you are typing: while you are at an
//! agent's terminal (the TUI says so with `Interact`), or its seat has a
//! typing guard, messages are held and delivered when that ends. The
//! pauses, the note that ends a message and whether being at a terminal
//! holds messages are `messaging` in settings.json.

use std::time::Duration;

use ainet_core::{ChatMessage, Event, InboxItem, InboxKind, Message, Response, now_ms};
use tokio::sync::broadcast;

use crate::store::num_key;
use crate::{Daemon, Shared, err, lock};

pub(crate) const OPERATOR: &str = "operator";

impl Daemon {
    fn save_message(&mut self, m: &Message) {
        self.store.put("message", &num_key(m.id), m);
        match self.messages.iter_mut().find(|x| x.id == m.id) {
            Some(x) => *x = m.clone(),
            None => {
                self.messages.push(m.clone());
                self.trim_messages();
            }
        }
        self.emit(Event::Message { message: m.clone() });
    }

    pub(crate) fn recount_unread(&mut self, id: &str) {
        let unread = self
            .messages
            .iter()
            .filter(|m| m.to == id && m.read.is_none())
            .count() as u32;
        if let Some(a) = self.agents.get_mut(id)
            && a.unread != unread
        {
            a.unread = unread;
            self.publish(id);
        }
    }

    /// Keeps as many messages in memory as `retention.messages_in_memory`
    /// says (all stay on disk).
    pub(crate) fn trim_messages(&mut self) {
        let keep = self.settings().retention.messages_in_memory;
        let excess = self.messages.len().saturating_sub(keep);
        self.messages.drain(..excess);
    }

    /// Keeps as many chat posts in memory as `retention.chat_in_memory`
    /// says.
    pub(crate) fn trim_chat(&mut self) {
        let keep = self.settings().retention.chat_in_memory;
        let excess = self.chat.len().saturating_sub(keep);
        self.chat.drain(..excess);
    }

    /// Whether messages for this agent must wait: you are at its terminal
    /// (unless `messaging.hold_while_interacting` is off), or its seat has a
    /// typing guard.
    fn holding(&self, id: &str) -> bool {
        if self.interactive.contains(id) && self.settings().messaging.hold_while_interacting {
            return true;
        }
        let seat = self.agents.get(id).and_then(|a| a.seat.clone());
        seat.and_then(|s| self.find_seat(&s, None).map(|s| s.typing_guard))
            .unwrap_or(false)
    }

    /// Stores a new message and returns it. Delivery is separate.
    pub(crate) fn new_message(
        &mut self,
        from: &str,
        to: &str,
        body: String,
        reply_to: Option<u64>,
    ) -> Message {
        self.next_message += 1;
        let thread = reply_to
            .and_then(|r| self.messages.iter().find(|m| m.id == r).map(|m| m.thread))
            .unwrap_or(self.next_message);
        let m = Message {
            id: self.next_message,
            from: from.to_string(),
            from_label: if from == OPERATOR {
                "you (operator)".into()
            } else {
                self.label(from)
            },
            to: to.to_string(),
            to_label: if to == OPERATOR {
                "you (operator)".into()
            } else {
                self.label(to)
            },
            body,
            reply_to,
            thread,
            created: now_ms(),
            delivered: None,
            read: None,
            held: false,
        };
        self.save_message(&m);
        if to == OPERATOR {
            // Agents can write to you: it lands in your Inbox.
            self.add_item(InboxItem {
                id: 0,
                agent: from.to_string(),
                kind: InboxKind::Question,
                summary: format!("message #{}: {}", m.id, first_line(&m.body, 100)),
                detail: m.body.clone(),
                created: m.created,
                waiting: false,
                options: vec![],
                multi_select: false,
            });
        } else {
            self.recount_unread(to);
        }
        m
    }

    /// The text typed into the recipient's terminal. A shell seat
    /// (OpenRig's `terminal` node) gets the bare text, as a command.
    fn delivery_text(&self, m: &Message) -> String {
        if self
            .agents
            .get(&m.to)
            .is_some_and(|a| a.runtime == ainet_core::Runtime::Shell)
        {
            return m.body.trim().to_string();
        }
        // Made where it is read back, so conversations know it again.
        let note = &self.settings().messaging.message_note;
        ainet_core::conversation::delivery_with(note, m.id, m.reply_to, &m.from_label, &m.body)
    }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or("");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Types `text` into an agent's terminal as a paste, then presses Enter.
/// Panes ainet owns get it directly, adopted tmux panes through tmux, and
/// other Claude Code sessions through their own inbox socket.
pub(crate) async fn deliver_text(d: &Shared, id: &str, text: &str) -> anyhow::Result<()> {
    enum Via {
        Pane,
        Tmux(String),
        Peer,
    }
    let (via, settle) = {
        let mut g = lock(d);
        let settle = Duration::from_millis(g.settings().messaging.paste_settle_ms);
        let via = if let Some(p) = g.panes.get_mut(id) {
            p.paste(text)?;
            Via::Pane
        } else if let Some(t) = g.tmux.get(id) {
            Via::Tmux(t.clone())
        } else if g
            .agents
            .get(id)
            .is_some_and(|a| a.runtime == ainet_core::Runtime::ClaudeCode)
        {
            Via::Peer
        } else {
            anyhow::bail!("{} has no terminal ainet can type into", g.label(id));
        };
        (via, settle)
    };
    match via {
        Via::Pane => {
            tokio::time::sleep(settle).await;
            let mut g = lock(d);
            match g.panes.get_mut(id) {
                Some(p) => p.enter(),
                None => anyhow::bail!("the agent exited"),
            }
        }
        Via::Tmux(target) => {
            crate::discover::tmux_paste(&target, text)?;
            tokio::time::sleep(settle).await;
            crate::discover::tmux_keys(&target, "\r")
        }
        Via::Peer => crate::discover::peer_send(id, text),
    }
}

/// Delivers a stored message now, or holds it.
async fn deliver(d: &Shared, id: u64, force: bool) {
    let (to, text, hold) = {
        let g = lock(d);
        let Some(m) = g.messages.iter().find(|m| m.id == id) else {
            return;
        };
        if m.to == OPERATOR {
            return;
        }
        (m.to.clone(), g.delivery_text(m), !force && g.holding(&m.to))
    };
    let result = if hold {
        Ok(false)
    } else {
        deliver_text(d, &to, &text).await.map(|_| true)
    };
    let mut g = lock(d);
    let Some(mut m) = g.messages.iter().find(|m| m.id == id).cloned() else {
        return;
    };
    match result {
        Ok(true) => {
            m.delivered = Some(now_ms());
            m.held = false;
        }
        Ok(false) => m.held = true,
        Err(e) => tracing::info!("message #{id} stays in the mailbox: {e:#}"),
    }
    g.save_message(&m);
}

/// Delivers what was held for an agent, oldest first.
pub(crate) async fn flush_held(d: &Shared, agent: &str) {
    let ids: Vec<u64> = lock(d)
        .messages
        .iter()
        .filter(|m| m.to == agent && m.held)
        .map(|m| m.id)
        .collect();
    for id in ids {
        deliver(d, id, false).await;
        let gap = lock(d).settings().messaging.flush_gap_ms;
        tokio::time::sleep(Duration::from_millis(gap)).await;
    }
}

/// `Send`: you type into an agent's terminal.
pub(crate) async fn type_text(d: &Shared, key: &str, text: &str) -> Response {
    let Some(id) = lock(d).find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    match deliver_text(d, &id, text).await {
        Ok(()) => {
            lock(d).store.log(
                now_ms(),
                Some(&id),
                "send",
                &serde_json::json!({ "chars": text.len() }),
            );
            Response::Ok
        }
        Err(e) => err(format!("{e:#}")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn message(
    d: &Shared,
    from: Option<String>,
    to: &[String],
    rig: Option<&str>,
    pod: Option<&str>,
    body: String,
    reply_to: Option<u64>,
    force: bool,
) -> Response {
    if body.trim().is_empty() {
        return err("the message is empty");
    }
    let created: Vec<Message> = {
        let mut g = lock(d);
        let sender = match from.as_deref() {
            None | Some(OPERATOR) => OPERATOR.to_string(),
            Some(f) => match g.find_agent(f, None) {
                Some(id) => id,
                None => return err(format!("unknown sender {f}")),
            },
        };
        let ctx_rig = rig.map(str::to_string).or_else(|| g.rig_of(&sender));
        let mut recipients: Vec<String> = Vec::new();
        // A reply goes back to whoever sent the original.
        if let Some(r) = reply_to {
            match g.messages.iter().find(|m| m.id == r) {
                Some(orig) if to.is_empty() => recipients.push(if orig.from == sender {
                    orig.to.clone()
                } else {
                    orig.from.clone()
                }),
                Some(_) => {}
                None => return err(format!("no message #{r}")),
            }
        }
        for t in to {
            if t.eq_ignore_ascii_case(OPERATOR) || t.eq_ignore_ascii_case("you") || t == "human" {
                recipients.push(OPERATOR.into());
                continue;
            }
            match g.find_agent(t, ctx_rig.as_deref()) {
                Some(id) => recipients.push(id),
                None => {
                    return err(format!(
                        "no agent or seat {t:?} (try list_agents / ainet ps --nodes)"
                    ));
                }
            }
        }
        if to.is_empty() && reply_to.is_none() {
            // Broadcast to a rig, or one pod of it.
            let Some(r) = ctx_rig.as_deref() else {
                return err("say who to send to: to, rig or pod");
            };
            let Some(state) = g.rigs.get(r) else {
                return err(format!("no rig {r}"));
            };
            recipients.extend(
                state
                    .rig
                    .seats
                    .iter()
                    .filter(|s| pod.is_none_or(|p| s.pod == p))
                    .filter_map(|s| s.agent.clone())
                    .filter(|a| *a != sender),
            );
        }
        recipients.dedup();
        if recipients.is_empty() {
            return err("nobody to send to");
        }
        recipients
            .into_iter()
            .map(|to| g.new_message(&sender, &to, body.clone(), reply_to))
            .collect()
    };
    for m in &created {
        deliver(d, m.id, force).await;
    }
    let g = lock(d);
    let messages = created
        .iter()
        .filter_map(|m| g.messages.iter().find(|x| x.id == m.id).cloned())
        .collect();
    Response::Messages { messages }
}

pub(crate) fn inbox(d: &Shared, key: &str, all: bool, limit: usize, mark_read: bool) -> Response {
    let mut g = lock(d);
    let id = if key == OPERATOR {
        OPERATOR.to_string()
    } else {
        match g.find_agent(key, None) {
            Some(id) => id,
            None => return err(format!("no agent {key}")),
        }
    };
    let mut picked: Vec<Message> = g
        .messages
        .iter()
        .filter(|m| m.to == id && (all || m.read.is_none()))
        .cloned()
        .collect();
    if limit > 0 && picked.len() > limit {
        picked.drain(..picked.len() - limit);
    }
    if mark_read {
        let now = now_ms();
        for m in &mut picked {
            if m.read.is_none() {
                m.read = Some(now);
                let saved = m.clone();
                g.save_message(&saved);
            }
        }
        g.recount_unread(&id);
    }
    Response::Messages { messages: picked }
}

pub(crate) fn thread(d: &Shared, id: u64) -> Response {
    let g = lock(d);
    let Some(t) = g.messages.iter().find(|m| m.id == id).map(|m| m.thread) else {
        return err(format!("no message #{id}"));
    };
    Response::Messages {
        messages: g.messages.iter().filter(|m| m.thread == t).cloned().collect(),
    }
}

/// Blocks until a reply to message `id` arrives, or the timeout passes.
pub(crate) async fn wait_reply(d: &Shared, id: u64, timeout_secs: u64) -> Response {
    let mut rx = {
        let g = lock(d);
        if !g.messages.iter().any(|m| m.id == id) {
            return err(format!("no message #{id}"));
        }
        if let Some(r) = g.messages.iter().find(|m| m.reply_to == Some(id)) {
            return Response::Messages {
                messages: vec![r.clone()],
            };
        }
        g.events.subscribe()
    };
    let wait = Duration::from_secs(timeout_secs.clamp(1, 3600));
    let found = tokio::time::timeout(wait, async {
        loop {
            match rx.recv().await {
                Ok(Event::Message { message }) if message.reply_to == Some(id) => return Some(message),
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .await;
    match found {
        Ok(Some(m)) => Response::Messages { messages: vec![m] },
        _ => err(format!(
            "timed out: no reply to #{id} after {timeout_secs}s; it may still come (read_messages)"
        )),
    }
}

pub(crate) async fn interact(d: &Shared, key: &str, on: bool) -> Response {
    let Some(id) = lock(d).find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    {
        let mut g = lock(d);
        if on {
            g.interactive.insert(id.clone());
        } else {
            g.interactive.remove(&id);
        }
    }
    if !on {
        flush_held(d, &id).await;
    }
    Response::Ok
}

pub(crate) async fn typing_guard(d: &Shared, seat: &str, on: bool) -> Response {
    let agent = {
        let mut g = lock(d);
        let Some(s) = g.find_seat(seat, None).cloned() else {
            return err(format!("no seat {seat}"));
        };
        let Some(state) = g.rigs.get_mut(&s.rig) else {
            return err(format!("no rig {}", s.rig));
        };
        if let Some(x) = state.rig.seats.iter_mut().find(|x| x.address == s.address) {
            x.typing_guard = on;
        }
        let rig = s.rig.clone();
        g.save_rig(&rig);
        s.agent
    };
    if !on && let Some(a) = agent {
        flush_held(d, &a).await;
    }
    Response::Ok
}

pub(crate) fn held(d: &Shared, seat: &str) -> Response {
    let g = lock(d);
    let Some(agent) = g
        .find_seat(seat, None)
        .and_then(|s| s.agent.clone())
        .or_else(|| g.find_agent(seat, None))
    else {
        return err(format!("no seat {seat}"));
    };
    Response::Messages {
        messages: g
            .messages
            .iter()
            .filter(|m| m.to == agent && m.held)
            .cloned()
            .collect(),
    }
}

// ---- chatroom -----------------------------------------------------------

const DEFAULT_TOPIC: &str = "general";

pub(crate) async fn chat_send(
    d: &Shared,
    rig: &str,
    topic: Option<String>,
    sender: Option<String>,
    body: String,
) -> Response {
    if body.trim().is_empty() {
        return err("the message is empty");
    }
    let (msg, mentions, sender_id) = {
        let mut g = lock(d);
        if !g.rigs.contains_key(rig) {
            return err(format!("no rig {rig}"));
        }
        let sender_id = match sender.as_deref() {
            None | Some(OPERATOR) => OPERATOR.to_string(),
            Some(s) => g.find_agent(s, Some(rig)).unwrap_or_else(|| s.to_string()),
        };
        g.next_chat += 1;
        let msg = ChatMessage {
            id: g.next_chat,
            rig: rig.to_string(),
            topic: topic.unwrap_or_else(|| DEFAULT_TOPIC.into()),
            sender: g.label(&sender_id),
            body,
            created: now_ms(),
        };
        g.store.put("chat", &num_key(msg.id), &msg);
        g.chat.push(msg.clone());
        g.trim_chat();
        g.emit(Event::Chat { message: msg.clone() });
        // @mentions reach the mentioned agent's terminal.
        let mentions: Vec<String> = msg
            .body
            .split_whitespace()
            .filter_map(|w| w.strip_prefix('@'))
            .map(|w| w.trim_end_matches([',', '.', ':', ';', '!', '?']))
            .filter_map(|w| g.find_agent(w, Some(rig)))
            .filter(|a| *a != sender_id)
            .collect();
        (msg, mentions, sender_id)
    };
    let note = lock(d).settings().messaging.mention_note.clone();
    for agent in mentions {
        let text = ainet_core::conversation::mention_with(
            &note,
            &msg.sender,
            &msg.rig,
            msg.id,
            &msg.topic,
            &msg.body,
        );
        let holding = lock(d).holding(&agent);
        if holding {
            let mut g = lock(d);
            let m = g.new_message(
                &sender_id,
                &agent,
                format!("(chatroom #{}) {}", msg.id, msg.body),
                None,
            );
            let mut m2 = m.clone();
            m2.held = true;
            g.save_message(&m2);
        } else if let Err(e) = deliver_text(d, &agent, &text).await {
            tracing::info!("chat mention not delivered: {e:#}");
        }
    }
    Response::Chat { messages: vec![msg] }
}

pub(crate) fn chat_history(
    d: &Shared,
    rig: &str,
    topic: Option<&str>,
    after: Option<u64>,
    limit: usize,
) -> Response {
    let g = lock(d);
    let mut picked: Vec<ChatMessage> = g
        .chat
        .iter()
        .filter(|m| m.rig == rig && topic.is_none_or(|t| m.topic == t) && after.is_none_or(|a| m.id > a))
        .cloned()
        .collect();
    let limit = if limit == 0 { 50 } else { limit };
    if picked.len() > limit {
        picked.drain(..picked.len() - limit);
    }
    Response::Chat { messages: picked }
}

/// Blocks until a chat message newer than `after` arrives in `rig`.
pub(crate) async fn chat_wait(
    d: &Shared,
    rig: &str,
    topic: Option<String>,
    after: Option<u64>,
    timeout_secs: u64,
) -> Response {
    let mut rx = {
        let g = lock(d);
        let after = after.unwrap_or(g.next_chat);
        let newer: Vec<ChatMessage> = g
            .chat
            .iter()
            .filter(|m| m.rig == rig && m.id > after && topic.as_deref().is_none_or(|t| m.topic == t))
            .cloned()
            .collect();
        if !newer.is_empty() {
            return Response::Chat { messages: newer };
        }
        g.events.subscribe()
    };
    let wait = Duration::from_secs(timeout_secs.clamp(1, 3600));
    let got = tokio::time::timeout(wait, async {
        loop {
            match rx.recv().await {
                Ok(Event::Chat { message })
                    if message.rig == rig && topic.as_deref().is_none_or(|t| message.topic == t) =>
                {
                    return Some(message);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .await;
    match got {
        Ok(Some(m)) => Response::Chat { messages: vec![m] },
        _ => Response::Chat { messages: vec![] },
    }
}
