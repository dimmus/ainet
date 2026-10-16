//! Starting a seat: the command line for its runtime, the system prompt that
//! tells it who it is and how to reach its team, readiness, and the startup
//! texts OpenRig specs deliver once the agent is up.
//!
//! Every Claude Code seat gets, without touching your files:
//! - `--session-id` (fresh) or `--resume` (restore) with the seat's session,
//! - `--append-system-prompt` with its identity, team map, guidance and how
//!   to talk to the team,
//! - `--mcp-config` with the `ainet` MCP server, and `--allowedTools
//!   mcp__ainet` so talking to the team never needs a permission prompt,
//! - `--settings` with ainet's hooks, so its state is tracked even if you
//!   never ran `ainet install-hooks`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ainet_core::settings::RuntimeSettings;
use ainet_core::{Agent, AgentState, Edge, EdgeKind, InboxItem, InboxKind, Runtime, SeatStatus, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::JoinSet;

use crate::agents::{Command, new_agent};
use crate::{Daemon, Shared, lock};

/// One seat's launch plan: everything needed to start (or restart) it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SeatPlan {
    pub pod: String,
    pub member: String,
    pub logical: String,
    pub address: String,
    pub label: Option<String>,
    pub role: Option<String>,
    pub runtime: Runtime,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: PathBuf,
    pub restore_policy: String,
    pub agent_name: Option<String>,
    /// (title, text) pairs for the system prompt.
    pub guidance: Vec<(String, String)>,
    pub send_text: Vec<String>,
    pub restore_send_text: Vec<String>,
}

impl From<&ainet_spec::ResolvedSeat> for SeatPlan {
    fn from(s: &ainet_spec::ResolvedSeat) -> Self {
        SeatPlan {
            pod: s.pod.clone(),
            member: s.member.clone(),
            logical: s.logical.clone(),
            address: s.address.clone(),
            label: s.label.clone(),
            role: s.role.clone(),
            runtime: s.runtime,
            model: s.model.clone(),
            effort: s.effort.clone(),
            cwd: s.cwd.clone(),
            restore_policy: s.restore_policy.clone(),
            agent_name: s.agent_name.clone(),
            guidance: s
                .guidance
                .iter()
                .map(|g| (g.title.clone(), g.text.clone()))
                .collect(),
            send_text: s.send_text.clone(),
            restore_send_text: s.restore_send_text.clone(),
        }
    }
}

impl SeatPlan {
    pub(crate) fn to_resolved(&self) -> ainet_spec::ResolvedSeat {
        ainet_spec::ResolvedSeat {
            pod: self.pod.clone(),
            member: self.member.clone(),
            logical: self.logical.clone(),
            address: self.address.clone(),
            label: self.label.clone(),
            role: self.role.clone(),
            runtime: self.runtime,
            model: self.model.clone(),
            effort: self.effort.clone(),
            cwd: self.cwd.clone(),
            restore_policy: self.restore_policy.clone(),
            agent_name: self.agent_name.clone(),
            guidance: self
                .guidance
                .iter()
                .map(|(t, x)| ainet_spec::Doc {
                    title: t.clone(),
                    text: x.clone(),
                })
                .collect(),
            send_text: self.send_text.clone(),
            restore_send_text: self.restore_send_text.clone(),
            skills: Vec::new(),
        }
    }

    /// The seat's role as people say it: role, else label, else member id.
    pub(crate) fn title(&self) -> String {
        self.role
            .clone()
            .or_else(|| self.label.clone())
            .unwrap_or_else(|| self.member.clone())
    }
}

/// A rig's launch plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RigPlan {
    pub name: String,
    pub summary: Option<String>,
    pub root: PathBuf,
    pub spec_path: Option<PathBuf>,
    pub edges: Vec<Edge>,
    pub seats: Vec<SeatPlan>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl RigPlan {
    pub(crate) fn from_resolved(r: &ainet_spec::ResolvedRig) -> Self {
        RigPlan {
            name: r.name.clone(),
            summary: r.summary.clone(),
            root: r.root.clone(),
            spec_path: r.spec_path.clone(),
            edges: r.edges.clone(),
            seats: r.seats.iter().map(SeatPlan::from).collect(),
            warnings: r.warnings.clone(),
        }
    }

    pub(crate) fn to_resolved(&self) -> ainet_spec::ResolvedRig {
        ainet_spec::ResolvedRig {
            name: self.name.clone(),
            summary: self.summary.clone(),
            root: self.root.clone(),
            spec_path: self.spec_path.clone(),
            seats: self.seats.iter().map(SeatPlan::to_resolved).collect(),
            edges: self.edges.clone(),
            warnings: self.warnings.clone(),
        }
    }

    pub(crate) fn seat(&self, address: &str) -> Option<&SeatPlan> {
        self.seats.iter().find(|s| s.address == address)
    }

    /// Renames the rig: addresses carry its name.
    pub(crate) fn rename(&mut self, name: &str) {
        self.name = name.to_string();
        for s in &mut self.seats {
            s.address = format!("{}-{}@{name}", s.pod, s.member);
        }
    }
}

/// Is `program` on PATH?
pub(crate) fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// The program a runtime runs: `runtimes.<runtime>.binary` from
/// settings.json, else `claude`, `codex` or `$SHELL` (else `/bin/sh`).
pub(crate) fn program(runtimes: &RuntimeSettings, runtime: Runtime) -> String {
    let configured = match runtime {
        Runtime::ClaudeCode => &runtimes.claude.binary,
        Runtime::Codex => &runtimes.codex.binary,
        Runtime::Shell => &runtimes.shell.binary,
    };
    if let Some(b) = configured.as_deref().filter(|b| !b.trim().is_empty()) {
        return ainet_core::paths::expand(Path::new(b)).display().to_string();
    }
    match runtime {
        Runtime::ClaudeCode => "claude".into(),
        Runtime::Codex => "codex".into(),
        Runtime::Shell => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl Daemon {
    /// The `ainet` MCP server, as Claude Code's `--mcp-config` wants it.
    fn mcp_config(&self, id: &str) -> serde_json::Value {
        json!({
            "mcpServers": {
                "ainet": {
                    "type": "stdio",
                    "command": self.exe.display().to_string(),
                    "args": ["mcp"],
                    "env": { "AINET_AGENT_ID": id, "AINET_SOCKET": self.socket.display().to_string() }
                }
            }
        })
    }

    /// ainet's hooks for one session, passed with `--settings`. They are
    /// marked so the daemon can tell them from your global ones.
    fn hook_settings(&self) -> serde_json::Value {
        let cmd = format!(
            "AINET_HOOK_SOURCE=seat {} hook",
            shell_quote(&self.exe.display().to_string())
        );
        let entry =
            |timeout: u32| json!([{ "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }] }]);
        let mut hooks = serde_json::Map::new();
        for event in [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "Notification",
            "Stop",
            "StopFailure",
            "PostToolUse",
            "PostToolUseFailure",
        ] {
            hooks.insert(event.into(), entry(10));
        }
        hooks.insert(
            "PreToolUse".into(),
            json!([{ "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": cmd, "timeout": 10 }] }]),
        );
        hooks.insert("PermissionRequest".into(), entry(600));
        json!({ "hooks": hooks })
    }

    fn claude_args(
        &self,
        id: &str,
        resume: bool,
        model: Option<&str>,
        prompt: &str,
        extra: &[String],
    ) -> Vec<String> {
        let mut args = vec![
            if resume { "--resume" } else { "--session-id" }.to_string(),
            id.to_string(),
        ];
        if let Some(m) = model {
            args.extend(["--model".into(), m.into()]);
        }
        args.extend(["--append-system-prompt".into(), prompt.to_string()]);
        args.extend(["--mcp-config".into(), self.mcp_config(id).to_string()]);
        args.extend(["--allowedTools".into(), "mcp__ainet".into()]);
        args.extend(["--settings".into(), self.hook_settings().to_string()]);
        args.extend(extra.iter().cloned());
        args
    }

    fn codex_args(
        &self,
        id: &str,
        model: Option<&str>,
        prompt: Option<&str>,
        extra: &[String],
    ) -> Vec<String> {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let mut args = Vec::new();
        if let Some(m) = model {
            args.extend(["-m".into(), m.into()]);
        }
        args.extend([
            "-c".into(),
            format!("mcp_servers.ainet.command={}", q(&self.exe.display().to_string())),
            "-c".into(),
            "mcp_servers.ainet.args=[\"mcp\"]".into(),
            "-c".into(),
            format!(
                "mcp_servers.ainet.env={{AINET_AGENT_ID={}, AINET_SOCKET={}}}",
                q(id),
                q(&self.socket.display().to_string())
            ),
        ]);
        args.extend(extra.iter().cloned());
        if let Some(p) = prompt {
            args.push(p.to_string());
        }
        args
    }

    /// Who this seat is and how to work with the team, for its system prompt.
    pub(crate) fn system_prompt(&self, plan: &RigPlan, seat: &SeatPlan, agent: &Agent) -> String {
        let mut p = String::new();
        p.push_str(&format!(
            "# You are a seat in an ainet rig\n\nYou are the {} of the rig \"{}\": seat address {}, logical id {}. On the team map you appear as {}.\n",
            seat.title(),
            plan.name,
            seat.address,
            seat.logical,
            agent.name
        ));
        if let Some(s) = &plan.summary {
            p.push_str(&format!("The rig: {}\n", s.trim()));
        }
        p.push_str("\n## Your team\n");
        for other in plan.seats.iter().filter(|o| o.address != seat.address) {
            let mut rel: Vec<&str> = Vec::new();
            for e in &plan.edges {
                let (a, b) = (e.from.as_str(), e.to.as_str());
                match e.kind {
                    EdgeKind::DelegatesTo if a == seat.logical && b == other.logical => {
                        rel.push("you delegate to them")
                    }
                    EdgeKind::DelegatesTo if b == seat.logical && a == other.logical => {
                        rel.push("they delegate to you")
                    }
                    EdgeKind::SpawnedBy if a == seat.logical && b == other.logical => {
                        rel.push("they spawned you")
                    }
                    EdgeKind::SpawnedBy if b == seat.logical && a == other.logical => {
                        rel.push("you spawned them")
                    }
                    EdgeKind::EscalatesTo if a == seat.logical && b == other.logical => {
                        rel.push("you escalate to them")
                    }
                    EdgeKind::EscalatesTo if b == seat.logical && a == other.logical => {
                        rel.push("they escalate to you")
                    }
                    EdgeKind::CanObserve if a == seat.logical && b == other.logical => {
                        rel.push("you can observe them")
                    }
                    EdgeKind::CanObserve if b == seat.logical && a == other.logical => {
                        rel.push("they observe you")
                    }
                    EdgeKind::CollaboratesWith
                        if (a == seat.logical && b == other.logical)
                            || (b == seat.logical && a == other.logical) =>
                    {
                        rel.push("you collaborate")
                    }
                    _ => {}
                }
            }
            let rel = if rel.is_empty() {
                String::new()
            } else {
                format!(" — {}", rel.join(", "))
            };
            p.push_str(&format!(
                "- {} ({}): {}, {}{rel}\n",
                other.logical,
                other.address,
                other.title(),
                other.runtime.label()
            ));
        }
        p.push_str(
            "\n## Talking to your team\n\
Use the `ainet` MCP tools (they need no permission):\n\
- send_message(to, body): a direct message. Address teammates by logical id (dev.review) or seat address.\n\
- ask_agent(to, question): ask and wait for the answer, when you need it before going on.\n\
- read_messages / reply_message(id, body): messages for you also arrive typed into this terminal as \"[ainet] message #N from …\". Answer each with reply_message(id=N, …), never only in your own chat: the sender cannot see your chat.\n\
- queue_create(destination, summary, body): give a piece of work an owner. Work for you arrives as a message: queue_claim it, report with queue_update(note=…), close it with queue_update(state=done). queue_list(mine=true) shows what you own.\n\
- chat_send / chat_read / chat_wait: the rig's chatroom; mention someone with @logical-id.\n\
- read_terminal(agent): look at a teammate's terminal (for review or to see why they are stuck).\n\
- whoami, list_agents, rig_status: where you are.\n\
- send_message(to=\"operator\", body): reach your human.\n\
The same on the shell: ainet msg send|inbox|reply|ask, ainet queue create|list|claim|update, ainet chatroom send|history, ainet whoami.\n\
Keep messages short and concrete; say what you need and by when. Do not wait in silence for an answer you need: use ask_agent, or carry on and check read_messages later.\n",
        );
        for (title, text) in &seat.guidance {
            p.push_str(&format!("\n## {title}\n{}\n", text.trim()));
        }
        p
    }

    /// The command for a seat.
    pub(crate) fn seat_command(
        &self,
        plan: &RigPlan,
        seat: &SeatPlan,
        agent: &Agent,
        resume: bool,
    ) -> Command {
        let prompt = self.system_prompt(plan, seat, agent);
        let seat_rec = self.find_seat(&seat.address, None).cloned();
        let env = self.agent_env(&agent.id, seat_rec.as_ref());
        let args = match seat.runtime {
            Runtime::ClaudeCode => self.claude_args(&agent.id, resume, seat.model.as_deref(), &prompt, &[]),
            Runtime::Codex => self.codex_args(&agent.id, seat.model.as_deref(), Some(&prompt), &[]),
            Runtime::Shell => Vec::new(),
        };
        Command {
            program: program(&self.settings().runtimes, seat.runtime),
            args,
            env,
        }
    }
}

/// The command for an agent started outside any rig (`ainet spawn`).
pub(crate) fn loose_command(d: &Daemon, agent: &Agent, extra: &[String]) -> Command {
    let prompt = format!(
        "# ainet\nYou run under ainet as {} (agent id {}). Other agents may message you: messages arrive typed into this terminal as \"[ainet] message #N from …\"; answer them with the ainet MCP tool reply_message(id=N, …). Use list_agents to see who else is running, send_message / ask_agent to talk to them, and send_message(to=\"operator\") to reach your human.",
        agent.name, agent.id
    );
    let env = d.agent_env(&agent.id, None);
    let args = match agent.runtime {
        Runtime::ClaudeCode => d.claude_args(&agent.id, false, None, &prompt, extra),
        Runtime::Codex => d.codex_args(&agent.id, None, None, extra),
        Runtime::Shell => extra.to_vec(),
    };
    Command {
        program: program(&d.settings().runtimes, agent.runtime),
        args,
        env,
    }
}

/// Starts one seat and waits for it to be ready, then types its startup
/// texts. Returns the agent id in the seat.
pub(crate) async fn start_seat(
    d: Shared,
    rig: String,
    address: String,
    fresh: bool,
) -> anyhow::Result<String> {
    let (id, runtime, resume, texts, gap) = {
        let mut g = lock(&d);
        let state = g.rigs.get(&rig).ok_or_else(|| anyhow::anyhow!("no rig {rig}"))?;
        let plan = state.plan.clone();
        let seat_plan = plan
            .seat(&address)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no seat {address}"))?;
        let seat = state
            .rig
            .seat(&address)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no seat {address}"))?;
        let program = program(&g.settings().runtimes, seat_plan.runtime);
        if !on_path(&program) {
            g.set_seat_status(&rig, &address, SeatStatus::Failed);
            anyhow::bail!("{program} is not installed (needed by {address})");
        }
        let resume = !fresh
            && seat_plan.restore_policy == "resume_if_possible"
            && seat_plan.runtime == Runtime::ClaudeCode
            && seat.agent.is_some();
        let id = match (&seat.agent, resume) {
            (Some(a), true) => a.clone(),
            _ => uuid::Uuid::new_v4().to_string(),
        };
        let mut agent = new_agent(&id, seat_plan.runtime, &seat_plan.cwd);
        agent.role = Some(seat_plan.title());
        agent.model = seat_plan.model.clone();
        agent.seat = Some(address.clone());
        agent.rig = Some(rig.clone());
        g.set_seat_agent(&rig, &address, &id, SeatStatus::Launching);
        let cmd = g.seat_command(&plan, &seat_plan, &agent, resume);
        if let Err(e) = g.start_pane(agent, &cmd) {
            g.set_seat_status(&rig, &address, SeatStatus::Failed);
            return Err(e);
        }
        g.relink(&rig);
        let texts = if resume {
            seat_plan.restore_send_text.clone()
        } else {
            seat_plan.send_text.clone()
        };
        let gap = Duration::from_millis(g.settings().runtimes.readiness.startup_text_gap_ms);
        (id, seat_plan.runtime, resume, texts, gap)
    };
    let ready = wait_ready(&d, &id, runtime).await;
    if !ready {
        let mut g = lock(&d);
        if !g.panes.contains_key(&id) {
            g.set_seat_status(&rig, &address, SeatStatus::Failed);
            anyhow::bail!("{address} exited while starting");
        }
        tracing::warn!("{address} did not look ready; delivering its startup anyway");
    }
    for text in texts {
        if let Err(e) = crate::comms::deliver_text(&d, &id, &text).await {
            tracing::warn!("startup text for {address}: {e:#}");
        }
        tokio::time::sleep(gap).await;
    }
    let mut g = lock(&d);
    // Shells and Codex send no hooks: ready means idle.
    if runtime != Runtime::ClaudeCode && g.agents.get(&id).is_some_and(|a| a.state == AgentState::Unknown) {
        g.set_state(&id, AgentState::Idle);
        g.publish(&id);
    }
    g.set_seat_status(&rig, &address, SeatStatus::Running);
    g.store.log(
        now_ms(),
        Some(&id),
        if resume { "seat_resumed" } else { "seat_started" },
        &json!({ "seat": address }),
    );
    Ok(id)
}

/// Waits until the agent in a fresh pane can take input
/// (`runtimes.readiness` says how long and how quiet).
async fn wait_ready(d: &Shared, id: &str, runtime: Runtime) -> bool {
    let started = Instant::now();
    let mut asked_trust = false;
    let mut deadline = Duration::from_secs(lock(d).settings().runtimes.readiness.timeout_secs);
    loop {
        let poll = lock(d).settings().runtimes.readiness.poll_ms;
        tokio::time::sleep(Duration::from_millis(poll)).await;
        let mut g = lock(d);
        let r = g.settings().runtimes.readiness.clone();
        let Some(pane) = g.panes.get(id) else { return false };
        let screen = pane.rows().join("\n");
        let quiet = g
            .last_output
            .get(id)
            .map(|t| t.elapsed())
            .unwrap_or(started.elapsed());
        let printed = g.last_output.contains_key(id);
        if screen.contains("Do you trust the files in this folder") || screen.contains("trust this folder") {
            // Your call, not ours: say so and wait for you.
            if !asked_trust {
                asked_trust = true;
                deadline = started.elapsed() + Duration::from_secs(r.trust_timeout_secs);
                let cwd = g
                    .agents
                    .get(id)
                    .map(|a| a.cwd.display().to_string())
                    .unwrap_or_default();
                g.set_state(id, AgentState::NeedsYou);
                g.add_item(InboxItem {
                    id: 0,
                    agent: id.to_string(),
                    kind: InboxKind::Question,
                    summary: format!("Claude Code asks whether to trust {cwd}"),
                    detail: "Answer it in the agent's terminal (Terminals view: select it, Enter to zoom in and type).".into(),
                    created: now_ms(),
                    waiting: false,
                    options: vec![],
                    multi_select: false,
                });
                g.publish(id);
            }
            continue;
        }
        let ready = match runtime {
            Runtime::ClaudeCode => {
                let hooked = g.agents.get(id).is_some_and(|a| a.state != AgentState::Unknown);
                let prompt = screen.contains("? for shortcuts")
                    || screen.contains("│ >")
                    || screen.lines().any(|l| l.trim_start().starts_with('>'));
                (hooked || prompt) && quiet > Duration::from_millis(r.quiet_ms.claude)
            }
            Runtime::Codex => printed && quiet > Duration::from_millis(r.quiet_ms.codex),
            Runtime::Shell => printed && quiet > Duration::from_millis(r.quiet_ms.shell),
        };
        if ready {
            return true;
        }
        if started.elapsed() > deadline {
            return false;
        }
    }
}

/// Launches seats in dependency levels: OpenRig's order (a delegator before
/// its delegates, a parent before what it spawned), each level at once.
pub(crate) fn launch_seats(d: &Shared, rig: &str, addresses: Vec<String>, fresh: Vec<String>) {
    let levels = {
        let g = lock(d);
        let Some(state) = g.rigs.get(rig) else { return };
        levels(&state.plan, &addresses)
    };
    let d = d.clone();
    let rig = rig.to_string();
    tokio::spawn(async move {
        for level in levels {
            let mut set = JoinSet::new();
            for address in level {
                let fresh = fresh
                    .iter()
                    .any(|f| f == &address || address.starts_with(&format!("{}-", f.replace('.', "-"))));
                set.spawn(start_seat(d.clone(), rig.clone(), address, fresh));
            }
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(Err(e)) => tracing::warn!("launch: {e:#}"),
                    Err(e) => tracing::warn!("launch task: {e}"),
                    Ok(Ok(_)) => {}
                }
            }
        }
        let mut g = lock(&d);
        g.relink(&rig);
        g.save_rig(&rig);
    });
}

/// Groups seats into levels so every seat comes after the seats it waits for.
fn levels(plan: &RigPlan, addresses: &[String]) -> Vec<Vec<String>> {
    let logical: HashMap<&str, &str> = plan
        .seats
        .iter()
        .map(|s| (s.logical.as_str(), s.address.as_str()))
        .collect();
    // deps[x] = seats that must be up before x.
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &plan.edges {
        let (Some(from), Some(to)) = (logical.get(e.from.as_str()), logical.get(e.to.as_str())) else {
            continue;
        };
        match e.kind {
            EdgeKind::DelegatesTo => deps.entry(to).or_default().push(from),
            EdgeKind::SpawnedBy => deps.entry(from).or_default().push(to),
            _ => {}
        }
    }
    let wanted: Vec<&str> = plan
        .seats
        .iter()
        .map(|s| s.address.as_str())
        .filter(|a| addresses.iter().any(|x| x == a))
        .collect();
    let mut level: BTreeMap<&str, usize> = BTreeMap::new();
    // Longest path from a root; cycles were rejected when the spec loaded.
    fn depth<'a>(
        s: &'a str,
        deps: &HashMap<&'a str, Vec<&'a str>>,
        memo: &mut BTreeMap<&'a str, usize>,
        guard: usize,
    ) -> usize {
        if let Some(d) = memo.get(s) {
            return *d;
        }
        if guard > 64 {
            return 0;
        }
        let d = deps
            .get(s)
            .map(|ds| {
                ds.iter()
                    .map(|x| depth(x, deps, memo, guard + 1) + 1)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        memo.insert(s, d);
        d
    }
    let mut memo = BTreeMap::new();
    for s in &wanted {
        level.insert(s, depth(s, &deps, &mut memo, 0));
    }
    let max = level.values().copied().max().unwrap_or(0);
    let mut out = vec![Vec::new(); max + 1];
    for s in &wanted {
        out[level[s]].push(s.to_string());
    }
    out.retain(|l| !l.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(pod: &str, member: &str) -> SeatPlan {
        SeatPlan {
            pod: pod.into(),
            member: member.into(),
            logical: format!("{pod}.{member}"),
            address: format!("{pod}-{member}@r"),
            label: None,
            role: None,
            runtime: Runtime::Shell,
            model: None,
            effort: None,
            cwd: PathBuf::from("/"),
            restore_policy: "resume_if_possible".into(),
            agent_name: None,
            guidance: vec![],
            send_text: vec![],
            restore_send_text: vec![],
        }
    }

    #[test]
    fn delegators_launch_first() {
        let plan = RigPlan {
            name: "r".into(),
            summary: None,
            root: PathBuf::from("/"),
            spec_path: None,
            edges: vec![
                Edge {
                    kind: EdgeKind::DelegatesTo,
                    from: "o.lead".into(),
                    to: "d.build".into(),
                },
                Edge {
                    kind: EdgeKind::DelegatesTo,
                    from: "d.build".into(),
                    to: "d.qa".into(),
                },
                Edge {
                    kind: EdgeKind::SpawnedBy,
                    from: "d.helper".into(),
                    to: "d.build".into(),
                },
                Edge {
                    kind: EdgeKind::CanObserve,
                    from: "r.rev".into(),
                    to: "d.build".into(),
                },
            ],
            seats: vec![
                seat("d", "qa"),
                seat("d", "build"),
                seat("o", "lead"),
                seat("d", "helper"),
                seat("r", "rev"),
            ],
            warnings: vec![],
        };
        let all: Vec<String> = plan.seats.iter().map(|s| s.address.clone()).collect();
        let lv = levels(&plan, &all);
        assert_eq!(lv[0], vec!["o-lead@r".to_string(), "r-rev@r".to_string()]);
        assert_eq!(lv[1], vec!["d-build@r".to_string()]);
        assert_eq!(lv[2], vec!["d-qa@r".to_string(), "d-helper@r".to_string()]);
    }
}
