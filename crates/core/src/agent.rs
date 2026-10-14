use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What an agent is doing, as far as the daemon can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Waiting on a permission, plan or question from you.
    NeedsYou,
    Working,
    Idle,
    /// A turn ended on an error, or tool calls keep failing.
    Failed,
    /// The process has exited.
    Gone,
    /// Started, but no hook has reported yet.
    Unknown,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            AgentState::NeedsYou => "needs you",
            AgentState::Working => "working",
            AgentState::Idle => "idle",
            AgentState::Failed => "failed",
            AgentState::Gone => "gone",
            AgentState::Unknown => "starting",
        }
    }

    pub fn is_live(self) -> bool {
        !matches!(self, AgentState::Gone)
    }
}

/// Which agent CLI runs in the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    #[serde(alias = "claude-code", alias = "claude")]
    ClaudeCode,
    Codex,
    /// A plain shell or command: OpenRig's `terminal` nodes.
    #[serde(alias = "terminal")]
    Shell,
}

impl std::str::FromStr for Runtime {
    type Err = String;

    /// Accepts OpenRig's names too: `claude-code`, `codex`, `terminal`.
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claude_code" | "claudecode" => Ok(Runtime::ClaudeCode),
            "codex" => Ok(Runtime::Codex),
            "shell" | "terminal" | "sh" => Ok(Runtime::Shell),
            other => Err(format!(
                "unknown runtime {other:?}: use claude-code, codex or terminal"
            )),
        }
    }
}

impl Runtime {
    /// OpenRig's spelling, as written in rig specs.
    pub fn spec_name(self) -> &'static str {
        match self {
            Runtime::ClaudeCode => "claude-code",
            Runtime::Codex => "codex",
            Runtime::Shell => "terminal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Runtime::ClaudeCode => "Claude Code",
            Runtime::Codex => "Codex",
            Runtime::Shell => "shell",
        }
    }
}

/// One agent: a pane the daemon owns, or a session it was told about.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Agent {
    /// Stable id. For Claude Code it is also the session id (we start it with
    /// `--session-id`), which seeds the critter and the name.
    pub id: String,
    pub name: String,
    pub runtime: Runtime,
    pub role: Option<String>,
    pub task: Option<String>,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub state: AgentState,
    /// When `state` last changed, in Unix milliseconds.
    pub state_since: i64,
    pub started_at: i64,
    /// Share of the context window in use, 0.0..=1.0.
    pub context: Option<f32>,
    /// The agent that directs this one, if any.
    pub parent: Option<String>,
    pub pid: Option<u32>,
    /// The last tool the agent ran, for the details pane.
    pub last_tool: Option<String>,
    /// Hook events per minute over the last few minutes, oldest first, for a
    /// sparkline of how busy the agent is.
    #[serde(default)]
    pub activity: Vec<u64>,
    /// The seat it occupies (`{pod}-{member}@{rig}`), if it is in a rig.
    #[serde(default)]
    pub seat: Option<String>,
    #[serde(default)]
    pub rig: Option<String>,
    /// Claude Code's transcript file, from its hooks.
    #[serde(default)]
    pub transcript: Option<PathBuf>,
    /// Messages it has not read yet.
    #[serde(default)]
    pub unread: u32,
    /// Whether ainetd can type into it: its own pane, an adopted tmux pane,
    /// or a Claude Code session's inbox socket.
    #[serde(default)]
    pub writable: bool,
    /// Whether ainetd can show its screen (its own pane or a tmux pane).
    #[serde(default)]
    pub terminal: bool,
}

impl Agent {
    /// The seat address when it has one, else its critter name.
    pub fn label(&self) -> String {
        self.seat.clone().unwrap_or_else(|| self.name.clone())
    }
}

/// What lands in the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Permission,
    Plan,
    Question,
    Finished,
    Failed,
}

impl InboxKind {
    pub fn label(self) -> &'static str {
        match self {
            InboxKind::Permission => "permission",
            InboxKind::Plan => "plan",
            InboxKind::Question => "question",
            InboxKind::Finished => "finished",
            InboxKind::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: u64,
    pub agent: String,
    pub kind: InboxKind,
    /// One line: the tool and its target, or the end of the agent's message.
    pub summary: String,
    /// The whole command, edit or message.
    pub detail: String,
    pub created: i64,
    /// True while a hook is held open waiting for your answer.
    pub waiting: bool,
    /// For a question: its numbered options, and whether several may be picked.
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub multi_select: bool,
}

/// Your answer to a held permission or plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    /// Allow, and the same again for this session (Den's `/always`).
    Always,
}
