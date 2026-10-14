# OpenRig parity

What ainet does of [OpenRig](https://github.com/mvschwarz/openrig) (0.6.x) and
how. OpenRig's command names work: `ainet up`, `ainet send`, `ainet queue …`,
and the binary answers to `rig` when you symlink it (`ln -s $(which ainet)
~/.local/bin/rig`).

Status: **done** works end to end and is tested; **partial** works with the
limits noted; **not yet** is planned; **won't** is out of scope.

## Teams (rigs)

| OpenRig | ainet | Status |
|---|---|---|
| RigSpec 0.2 YAML (pods, members, edges, culture, startup files and actions) | `crates/spec`, parses OpenRig's own shipped rigs unchanged | done |
| AgentSpec (`agent.yaml`: defaults, profiles, guidance, skills, startup, imports) | resolved into each seat's system prompt and startup texts | done (plugins, subagents, runtime resources are warned and skipped) |
| `rig up <spec\|library\|rig>`, `--cwd`, `--plan` | `ainet up …` | done |
| Launch order from `delegates_to` / `spawned_by`, cycle errors | dependency levels, each level at once | done |
| Startup `send_text`, `slash_command`, `guidance_merge`, `applies_on` | system prompt (`--append-system-prompt`, no files touched) and typed after readiness | done |
| `rig down` with snapshot, `rig snapshot create/list`, `rig restore` | snapshots in SQLite; restore resumes every Claude conversation (`--resume`) | done |
| `rig start` (restore what was running) | `ainet start` | done |
| `rig grow` / `add` / `expand` / `shrink` / `remove` / `launch` / `create` | same names | done |
| `rig export` / `import` | YAML round trip | done (pod labels and non-built-in agent refs are not recovered) |
| `rig specs ls/preview`, built-in teams | `ainet specs`, `specs show`; built-ins `starter`, `workshop`, `factory`, `code-review`, `research`, `pm`, `operator` | done (own content) |
| Seats with stable addresses `pod-member@rig`, logical ids `pod.member` | same | done |
| `rig ps`, `ps --nodes`, `status`, `whoami` | same | done |
| `rig discover` / `adopt` / `bind` / `release` / `unclaim` (tmux sessions) | tmux panes running Claude Code or Codex, and every Claude session on the machine | done |
| Kernel operator (`saved:kernel`) | the `operator` rig; `Ctrl+O` in the TUI | done |
| `restore_policy` (`resume_if_possible`, `relaunch_fresh`, `checkpoint_only`) | resume or fresh | partial (checkpoint artifacts not yet) |
| Bundles (`rig bundle create/inspect/install`, SHA-256) | | not yet |
| Services (Docker Compose), managed apps, `rig env` | warned and ignored | not yet |
| Permission policies (`builtin:locked` … `yolo`) | warned and ignored; approvals go through the Inbox | not yet |
| Pi / Oh My Pi runtimes, `stub` | rejected with a clear error | won't (for now) |

## Agents talking

| OpenRig | ainet | Status |
|---|---|---|
| `rig send` (to seats, pod, rig; `--from`) | `ainet send`, MCP `send_message`; delivered into the terminal and kept in a mailbox | done |
| `rig broadcast` | `ainet broadcast`, MCP `broadcast` | done |
| Reading and answering | `ainet msg inbox/reply/thread`, MCP `read_messages`, `reply_message`; replies thread back | done |
| Asking and waiting for the answer | `ainet msg ask`, MCP `ask_agent` (blocks until the reply) | done (ainet's own) |
| `rig chatroom send/history/watch/wait`, topics | same, plus `@mentions` delivered to the mentioned seat | done |
| `rig capture` / `transcript` | `ainet capture`, `transcript`; MCP `read_terminal` | done |
| Typing guard (`seat set-typing-guard`, held messages) | same; also automatic while you type into a terminal in the TUI | done |
| `seat continue` | same | done |
| MCP server for agents (`rig mcp serve`) | `ainet mcp`, injected into every Claude seat with its tools pre-allowed | done |
| Messages to agents you did not start | Claude Code's own inbox socket (as Den) | done |

## Owned work

| OpenRig | ainet | Status |
|---|---|---|
| `rig queue create/show/claim/unclaim/update/list` | same; creating nudges the destination, finishing tells the source | done |
| `queue handoff`, `queue block --on <qid\|external:x> --wake-after` | same; blocked items wake when their blocker is done | done |
| `rig watchdog register/list/stop` | same | done |
| Missions and slices (`rig scope`), workflow specs, proofs | | not yet |
| `rig stream`, `heartbeat`, `parked`, `proof`, `telemetry`, `usage` | | not yet (usage: cost per seat from transcripts is next) |

## Watching

| OpenRig | ainet | Status |
|---|---|---|
| TUI: rigs, pods, seats, topology graph | Fleet (rig → seats as a tree), Map (graph with messages in flight) | done |
| `rig terminal open <rig\|pod\|saved>` (tiles via herdr/cmux) | the Terminals view: every terminal live in rows and columns, zoom, type into any (`ainet terminal open rig:<name>`) | done (built in, no herdr needed) |
| Attention feed | Inbox: permissions, plans, questions, finished turns, failures | done |
| Web UI | | won't (maintenance mode in OpenRig too) |
| Slack connector, gateway, multi-host registry | | not yet |
| Context packs (`rig context`), `walk`, `compact`, `handover`, `fork`, agent images | | not yet |

## What ainet adds

- Its own PTYs: no tmux needed, messages written straight to the terminal.
- Den's control room: one approval inbox (`^Y`, `^A` always, `^N`), plan
  approval, answering an agent's questions, notifications of failures.
- Every Claude Code session on the machine in the Fleet, not only managed ones.
- Pixel critters, one face per agent, the same as in Den and agent-badge.
