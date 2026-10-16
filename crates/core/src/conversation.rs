//! An agent's conversation as a front end shows it: your prompts, the
//! agent's words, the tools it ran with the first lines they printed, and
//! the messages ainetd typed into its terminal. ainetd reads it from the
//! session's transcript (`Request::Conversation`).
//!
//! The text ainetd types into an agent's terminal for a message is made
//! here ([`delivery`], [`mention`]) and read back here ([`parse_delivery`]),
//! so a delivered message is known again when it turns up as a prompt.

use serde::{Deserialize, Serialize};

use crate::agent::Runtime;

/// What an entry of a conversation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A prompt typed by you.
    You,
    /// The agent's words.
    Reply,
    /// A tool call, with what it printed once it is done.
    Tool,
    /// A message ainetd typed into the terminal for the agent.
    Inbound,
    /// A kind this version does not know (a newer daemon's).
    #[serde(other)]
    Other,
}

/// One entry of a conversation. Empty fields are left out of the line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationEntry {
    /// Where it is in the transcript: entries further on have larger ids,
    /// so a front end asks for what came `after` the last it has.
    pub id: u64,
    pub kind: EntryKind,
    /// When, in Unix ms, if the transcript says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    /// The prompt, the reply, a tool call's description, a message's body.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A tool call's tool (`Bash`, `Edit`, …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// A tool call's command, path or other main argument.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    /// The first lines a tool call's result says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    /// A tool call's result came.
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
    /// A tool call's result is an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failed: bool,
    /// A message's sender, as ainetd named them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// A message's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<u64>,
    /// The message it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// For a chatroom mention: the room ("docs · general").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ConversationEntry {
    /// An entry of `kind` saying `text`, the rest empty.
    pub fn new(id: u64, kind: EntryKind, text: impl Into<String>) -> Self {
        ConversationEntry {
            id,
            kind,
            at: None,
            text: text.into(),
            tool: String::new(),
            code: String::new(),
            output: String::new(),
            done: false,
            failed: false,
            from: String::new(),
            message: None,
            reply_to: None,
            room: None,
        }
    }

    /// A prompt: a message ainetd delivered if it reads as one, else your
    /// words.
    pub fn prompt(id: u64, at: Option<i64>, text: &str) -> Self {
        Self::prompt_with(id, at, text, &[])
    }

    /// [`ConversationEntry::prompt`] that also knows the `notes` messages
    /// may end with (see [`parse_delivery_with`]).
    pub fn prompt_with(id: u64, at: Option<i64>, text: &str, notes: &[&str]) -> Self {
        let mut entry = match parse_delivery_with(text, notes) {
            Some(d) => ConversationEntry {
                from: d.from,
                message: d.id,
                reply_to: d.reply_to,
                room: d.room,
                ..ConversationEntry::new(id, EntryKind::Inbound, d.body)
            },
            None => ConversationEntry::new(id, EntryKind::You, text),
        };
        entry.at = at;
        entry
    }

    /// A tool call still waiting for its result.
    pub fn is_running(&self) -> bool {
        self.kind == EntryKind::Tool && !self.done
    }
}

/// An agent's conversation, or a word on why there is none to show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// Oldest first.
    #[serde(default)]
    pub entries: Vec<ConversationEntry>,
    /// The model the session runs, if the transcript says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The session's working directory, if the transcript says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Claude Code's permission mode (`default`, `acceptEdits`, `plan`,
    /// …), if the transcript says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Why there is less to show: a runtime whose transcript ainet does
    /// not read, no transcript yet, a daemon too old to send entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// The transcript as plain text, when there are no entries to be had
    /// (a daemon from before `Request::Conversation` answers
    /// `Request::Transcript` instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plain: Option<String>,
}

/// Why a runtime's conversation cannot be shown as entries, if it cannot.
pub fn unreadable(runtime: Runtime) -> Option<&'static str> {
    match runtime {
        Runtime::ClaudeCode => None,
        Runtime::Codex => Some(
            "Codex keeps its session in a log of its own, which ainet does not read yet: its terminal shows the conversation.",
        ),
        Runtime::Shell => Some("A shell has no conversation to read: its terminal is all there is."),
    }
}

/// Said instead of entries while Claude Code has not named its transcript.
pub const NO_TRANSCRIPT: &str =
    "No transcript yet: Claude Code names it in its first hook, once the session has started.";

/// How a message delivered into a terminal ends: what to answer with.
/// `{id}` is the message's number. `messaging.message_note` in
/// settings.json replaces it.
pub const MESSAGE_NOTE: &str = "(Answer with the ainet tool reply_message id={id} — or on the shell: ainet msg reply {id} \"…\". Do not answer in this chat; the sender cannot see it.)";
/// How a chatroom mention ends (`messaging.mention_note`).
pub const MENTION_NOTE: &str = "(Answer with the ainet tool chat_send, or read more with chat_read.)";
/// How every note of ainet's own starts, cut off or not.
const ANSWER_START: &str = "(Answer with the ainet tool ";
const PREFIX: &str = "[ainet] ";

/// What ainetd types into an agent's terminal for message `id` from
/// `from` (its label), answering `reply_to` if it does.
pub fn delivery(id: u64, reply_to: Option<u64>, from: &str, body: &str) -> String {
    delivery_with(MESSAGE_NOTE, id, reply_to, from, body)
}

/// [`delivery`] ending with `note` (`{id}` is the message's number). The
/// first line stays as it is: ainet reads it back.
pub fn delivery_with(note: &str, id: u64, reply_to: Option<u64>, from: &str, body: &str) -> String {
    let what = match reply_to {
        Some(r) => format!("reply #{id} to your message #{r}"),
        None => format!("message #{id}"),
    };
    format!(
        "{PREFIX}{what} from {from}:\n{}\n{}",
        body.trim(),
        note_for(note, id)
    )
}

fn note_for(note: &str, id: u64) -> String {
    note.replace("{id}", &id.to_string())
}

/// What ainetd types into an agent's terminal when `sender` names it in
/// `rig`'s chatroom (post `id` on `topic`).
pub fn mention(sender: &str, rig: &str, id: u64, topic: &str, body: &str) -> String {
    mention_with(MENTION_NOTE, sender, rig, id, topic, body)
}

/// [`mention`] ending with `note`.
pub fn mention_with(note: &str, sender: &str, rig: &str, id: u64, topic: &str, body: &str) -> String {
    format!(
        "{PREFIX}{sender} mentioned you in the {rig} chatroom (#{id}, topic {topic}):\n{body}\n{}",
        note_for(note, id)
    )
}

/// A message read back from what ainetd typed ([`delivery`], [`mention`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivered {
    /// The message's number (the chat post's, for a mention).
    pub id: Option<u64>,
    pub reply_to: Option<u64>,
    pub from: String,
    pub body: String,
    /// For a mention: the room ("docs · general").
    pub room: Option<String>,
}

/// The message in `text` if ainetd typed it, `None` for anything else.
/// The trailer saying how to answer may be cut off (a long prompt kept
/// in part); the body then runs to the end.
pub fn parse_delivery(text: &str) -> Option<Delivered> {
    parse_delivery_with(text, &[])
}

/// [`parse_delivery`] that also knows the `notes` a message may end with
/// besides ainet's own (those set in settings.json).
pub fn parse_delivery_with(text: &str, notes: &[&str]) -> Option<Delivered> {
    let rest = text.trim_start().strip_prefix(PREFIX)?;
    let (head, body) = rest.split_once('\n').unwrap_or((rest, ""));
    let head = head.trim_end().strip_suffix(':')?;
    let number = |s: &str| s.trim().strip_prefix('#').and_then(|n| n.parse::<u64>().ok());
    let id = head
        .split_whitespace()
        .map(|w| w.trim_start_matches('('))
        .find(|w| w.starts_with('#'))
        .and_then(|w| number(w.trim_end_matches([',', ')'])));
    let own = |body: &str| -> Option<usize> {
        let id = id?;
        notes
            .iter()
            .filter(|n| !n.trim().is_empty())
            .map(|n| note_for(n, id))
            .filter_map(|n| body.trim_end().strip_suffix(n.trim_end()).map(str::len))
            .max()
    };
    let body = match own(body) {
        Some(end) => &body[..end],
        None => match body.rfind(&format!("\n{ANSWER_START}")) {
            Some(at) => &body[..at],
            None if body.starts_with(ANSWER_START) => "",
            None => body,
        },
    };
    let body = body.trim().to_string();
    if let Some(m) = head.strip_prefix("message ") {
        let (id, from) = m.split_once(" from ")?;
        return Some(Delivered {
            id: Some(number(id)?),
            reply_to: None,
            from: from.to_string(),
            body,
            room: None,
        });
    }
    if let Some(r) = head.strip_prefix("reply ") {
        let (id, rest) = r.split_once(" to your message ")?;
        let (to, from) = rest.split_once(" from ")?;
        return Some(Delivered {
            id: Some(number(id)?),
            reply_to: Some(number(to)?),
            from: from.to_string(),
            body,
            room: None,
        });
    }
    let (sender, place) = head.split_once(" mentioned you in the ")?;
    let (rig, post) = place.split_once(" chatroom (")?;
    let post = post.strip_suffix(')')?;
    let (id, topic) = post.split_once(", topic ")?;
    Some(Delivered {
        id: number(id),
        reply_to: None,
        from: sender.to_string(),
        body,
        room: Some(format!("{rig} · {topic}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliveries_read_back() {
        let text = delivery(7, None, "dev-ui@docs", "  Is the API sorted?\n\nThanks.  ");
        assert_eq!(
            text,
            "[ainet] message #7 from dev-ui@docs:\nIs the API sorted?\n\nThanks.\n(Answer with the ainet tool reply_message id=7 — or on the shell: ainet msg reply 7 \"…\". Do not answer in this chat; the sender cannot see it.)"
        );
        assert_eq!(
            parse_delivery(&text),
            Some(Delivered {
                id: Some(7),
                reply_to: None,
                from: "dev-ui@docs".into(),
                body: "Is the API sorted?\n\nThanks.".into(),
                room: None,
            })
        );
        let reply = parse_delivery(&delivery(9, Some(7), "you (operator)", "Yes.")).unwrap();
        assert_eq!(
            (reply.id, reply.reply_to, reply.from.as_str(), reply.body.as_str()),
            (Some(9), Some(7), "you (operator)", "Yes.")
        );
        let chat = parse_delivery(&mention("orch-lead@docs", "docs", 3, "general", "@dev.ui look")).unwrap();
        assert_eq!(chat.from, "orch-lead@docs");
        assert_eq!(chat.room.as_deref(), Some("docs · general"));
        assert_eq!((chat.id, chat.body.as_str()), (Some(3), "@dev.ui look"));
        // Cut before its trailer: the body runs to the end.
        let cut = parse_delivery("[ainet] message #2 from lead:\nfirst\n…").unwrap();
        assert_eq!(cut.body, "first\n…");
        // Anything else is your own words.
        for text in [
            "fix the tests",
            "[ainet] what is this",
            "[ainet] message #x from a:\nb",
        ] {
            assert_eq!(parse_delivery(text), None, "{text}");
        }
        // A note of your own (settings.json) is known by what it says.
        let note = "Reply with reply_message id={id}, please.";
        let own = delivery_with(note, 12, None, "lead", "Ship it.");
        assert_eq!(
            own,
            "[ainet] message #12 from lead:\nShip it.\nReply with reply_message id=12, please."
        );
        assert_eq!(parse_delivery_with(&own, &[note]).unwrap().body, "Ship it.");
        assert_eq!(
            parse_delivery(&own).unwrap().body,
            own.split_once('\n').unwrap().1
        );
        let own_mention = mention_with("Use chat_send.", "lead", "docs", 4, "general", "@dev look");
        assert_eq!(
            parse_delivery_with(&own_mention, &["Use chat_send."])
                .unwrap()
                .body,
            "@dev look"
        );
        let entry = ConversationEntry::prompt(4, Some(10), &delivery(1, None, "lead", "go"));
        assert_eq!(
            (entry.kind, entry.from.as_str(), entry.text.as_str()),
            (EntryKind::Inbound, "lead", "go")
        );
        assert_eq!(ConversationEntry::prompt(5, None, "go").kind, EntryKind::You);
    }

    #[test]
    fn entries_leave_out_what_is_empty_and_read_what_comes_later() {
        let mut tool = ConversationEntry::new(3, EntryKind::Tool, "Run the tests");
        tool.tool = "Bash".into();
        tool.code = "cargo test".into();
        assert_eq!(
            serde_json::to_string(&tool).unwrap(),
            r#"{"id":3,"kind":"tool","text":"Run the tests","tool":"Bash","code":"cargo test"}"#
        );
        assert!(tool.is_running());
        let back: ConversationEntry = serde_json::from_str(r#"{"id":1,"kind":"reply"}"#).unwrap();
        assert_eq!(back, ConversationEntry::new(1, EntryKind::Reply, ""));
        // A kind from a newer daemon, and fields this one does not know.
        let newer: ConversationEntry =
            serde_json::from_str(r#"{"id":2,"kind":"thinking","text":"hm","mood":"calm"}"#).unwrap();
        assert_eq!(newer.kind, EntryKind::Other);
        let empty: Conversation = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Conversation::default());
        assert_eq!(serde_json::to_string(&empty).unwrap(), r#"{"entries":[]}"#);
    }
}
