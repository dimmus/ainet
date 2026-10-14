//! Rigs, seats and topology: OpenRig's model, as the daemon tracks it.
//!
//! A rig is a team launched from a RigSpec. It has pods, and each pod has
//! members; a member running as an agent is a seat. A seat has a stable
//! address that outlives the conversation in it: `{pod}-{member}@{rig}`
//! (OpenRig's canonical session name), and a logical id `{pod}.{member}`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::Runtime;

/// OpenRig's edge kinds. Only `delegates_to` and `spawned_by` change
/// anything at runtime (launch order, and who directs whom in the UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    DelegatesTo,
    SpawnedBy,
    CanObserve,
    CollaboratesWith,
    EscalatesTo,
}

impl EdgeKind {
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::DelegatesTo => "delegates_to",
            EdgeKind::SpawnedBy => "spawned_by",
            EdgeKind::CanObserve => "can_observe",
            EdgeKind::CollaboratesWith => "collaborates_with",
            EdgeKind::EscalatesTo => "escalates_to",
        }
    }
}

/// An edge between two seats, by logical id (`pod.member`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: String,
    pub to: String,
}

/// Where a seat's terminal lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transport {
    /// A PTY ainetd owns.
    Pane,
    /// A tmux pane ainet adopted; written with `tmux paste-buffer`.
    Tmux { pane: String },
    /// Seen only through hooks: no way to type into it.
    None,
}

/// What a seat is doing in its rig's lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatStatus {
    /// Waiting for the seats it depends on.
    Pending,
    Launching,
    Running,
    /// Its process is gone; `ainet launch` or a restore brings it back.
    Stopped,
    Failed,
}

impl SeatStatus {
    pub fn label(self) -> &'static str {
        match self {
            SeatStatus::Pending => "pending",
            SeatStatus::Launching => "launching",
            SeatStatus::Running => "running",
            SeatStatus::Stopped => "stopped",
            SeatStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Seat {
    /// `{pod}-{member}@{rig}`.
    pub address: String,
    /// `{pod}.{member}`.
    pub logical: String,
    pub rig: String,
    pub pod: String,
    pub member: String,
    pub label: Option<String>,
    pub role: Option<String>,
    pub runtime: Runtime,
    pub model: Option<String>,
    pub cwd: PathBuf,
    /// The agent in the seat now. For Claude Code, also the session id that
    /// a restore resumes.
    pub agent: Option<String>,
    pub status: SeatStatus,
    pub transport: Transport,
    /// `resume_if_possible`, `relaunch_fresh` or `checkpoint_only`.
    pub restore_policy: String,
    /// While on, automatic messages are held instead of typed into the seat.
    #[serde(default)]
    pub typing_guard: bool,
    /// Messages held by the typing guard or while you are at the terminal.
    #[serde(default)]
    pub held: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RigStatus {
    Launching,
    Running,
    /// Some seats are running, some are not.
    Partial,
    Stopped,
}

impl RigStatus {
    pub fn label(self) -> &'static str {
        match self {
            RigStatus::Launching => "launching",
            RigStatus::Running => "running",
            RigStatus::Partial => "partial",
            RigStatus::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rig {
    pub name: String,
    pub summary: Option<String>,
    /// The spec file it was launched from, if any.
    pub spec_path: Option<PathBuf>,
    /// Where relative paths in the spec resolve.
    pub root: PathBuf,
    pub status: RigStatus,
    pub seats: Vec<Seat>,
    pub edges: Vec<Edge>,
    pub created: i64,
}

impl Rig {
    pub fn seat(&self, logical_or_address: &str) -> Option<&Seat> {
        self.seats
            .iter()
            .find(|s| s.logical == logical_or_address || s.address == logical_or_address)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub rig: String,
    pub label: Option<String>,
    pub created: i64,
    pub seats: usize,
}

/// A session found outside ainet: a tmux pane running an agent, or a Claude
/// Code session that reports through hooks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discovered {
    /// A tmux pane id (`%12`) or a session id.
    pub id: String,
    pub runtime: Runtime,
    pub cwd: PathBuf,
    pub tmux_session: Option<String>,
    pub pid: Option<u32>,
    pub title: Option<String>,
    /// Already adopted into this seat.
    pub seat: Option<String>,
}

/// What `whoami` tells an agent about itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Whoami {
    pub agent: String,
    pub name: String,
    pub seat: Option<Seat>,
    /// Seats this one delegates to, is delegated by, and its other edges.
    pub delegates_to: Vec<String>,
    pub delegated_by: Vec<String>,
    pub peers: Vec<String>,
    pub edges: Vec<Edge>,
    pub unread: u32,
    pub open_queue: u32,
}
