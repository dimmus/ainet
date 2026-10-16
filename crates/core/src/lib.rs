//! Types shared by the daemon, the TUI and the command line: what an agent
//! is, what the inbox holds, the wire protocol between them, and where files
//! live.

pub mod agent;
pub mod client;
pub mod comms;
pub mod conversation;
pub mod paths;
pub mod protocol;
pub mod rig;

pub use agent::{Agent, AgentState, Decision, InboxItem, InboxKind, Runtime};
pub use comms::{ChatMessage, Message, QBlock, QItem, QNote, QState, Watchdog};
pub use conversation::{Conversation, ConversationEntry};
pub use protocol::{Event, Request, Response};
pub use rig::{
    Discovered, Edge, EdgeKind, Rig, RigStatus, Seat, SeatStatus, SnapshotInfo, Transport, Whoami,
};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
