//! How agents talk: direct messages with replies, a chatroom per rig, and a
//! queue of owned work (OpenRig's "Coordination L3").

use serde::{Deserialize, Serialize};

/// A message from one agent (or you) to another. Delivered into the
/// recipient's terminal and kept in its mailbox until read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    /// Sender: an agent id, or "operator" for you.
    pub from: String,
    /// Sender's display address: a seat address or a critter name.
    pub from_label: String,
    pub to: String,
    pub to_label: String,
    pub body: String,
    /// The message this answers.
    pub reply_to: Option<u64>,
    /// The first message of the conversation.
    pub thread: u64,
    pub created: i64,
    pub delivered: Option<i64>,
    pub read: Option<i64>,
    /// Held by a typing guard, or while you are at the recipient's terminal.
    #[serde(default)]
    pub held: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub rig: String,
    pub topic: String,
    pub sender: String,
    pub body: String,
    pub created: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QState {
    Pending,
    InProgress,
    Blocked,
    Done,
    Canceled,
}

impl QState {
    pub fn label(self) -> &'static str {
        match self {
            QState::Pending => "pending",
            QState::InProgress => "in-progress",
            QState::Blocked => "blocked",
            QState::Done => "done",
            QState::Canceled => "canceled",
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, QState::Pending | QState::InProgress | QState::Blocked)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QNote {
    pub by: String,
    pub text: String,
    pub at: i64,
    pub state: Option<QState>,
}

/// What a blocked item waits on.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QBlock {
    /// Another qitem id, or `external:<name>`.
    pub on: String,
    /// What to do once unblocked.
    pub continuation: String,
    /// Wake the owner after this time (Unix ms), even if still blocked.
    pub wake_at: Option<i64>,
}

/// A scheduled wake: OpenRig's watchdog. Every `interval_secs` the target
/// seat gets `message` typed in, to make it check its queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Watchdog {
    pub id: String,
    pub target: String,
    pub interval_secs: u64,
    pub message: String,
    pub registered_by: String,
    pub reason: Option<String>,
    pub created: i64,
    pub last_fired: Option<i64>,
    pub active: bool,
}

/// A queue item ("qitem"): a piece of work with one owner.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QItem {
    /// `q-<n>`.
    pub id: String,
    pub rig: Option<String>,
    /// Who asked: a seat address, agent id or "operator".
    pub source: String,
    /// Who should do it.
    pub destination: String,
    /// Who has claimed it, if anyone.
    pub owner: Option<String>,
    pub summary: String,
    pub body: String,
    pub priority: String,
    pub tags: Vec<String>,
    pub state: QState,
    pub reply_to: Option<String>,
    /// Set while blocked.
    #[serde(default)]
    pub block: Option<QBlock>,
    /// The item this one was handed off to.
    #[serde(default)]
    pub successor: Option<String>,
    pub notes: Vec<QNote>,
    pub created: i64,
    pub updated: i64,
}
