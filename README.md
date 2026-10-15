# ainet

A control room for teams of coding agents: one Rust binary that owns the
agents' terminals, launches whole teams from OpenRig RigSpecs, lets the
agents talk to each other, holds their permission requests for you, and
shows everything in a TUI with one pixel critter per agent.

![Terminals: seven agents' live terminals in a grid, one of them just answering a message from a teammate](docs/images/terminals.png)

It puts three projects on one solid base:

- [OpenRig](https://github.com/mvschwarz/openrig): teams defined in YAML,
  launched in order, snapshotted and restored, with stable seat addresses, a
  queue of owned work, a chatroom and an operator agent. Its RigSpecs run
  unchanged and its command names work (`ainet` answers to `rig` too). What is
  and is not covered: [docs/OPENRIG-PARITY.md](docs/OPENRIG-PARITY.md).
- [Den](https://github.com/GlacierHubAB/den): the approval inbox (allow,
  always, deny, plans, answering an agent's questions), every Claude Code
  session on the machine in one Fleet, and the critters (a bit-for-bit port).
  Claude Code's undocumented interfaces Den relies on are ported too.
- [Herdr](https://github.com/herdrdev/herdr): the daemon owns the PTYs, so
  agents outlive the UI and messages are written straight into terminals.

## Try it

```bash
cargo run -- --demo        # a pretend fleet: no daemon, no Claude Code needed
```

Views (`Tab`): **Fleet** (rigs and their seats as trees, details and live
gauges), **Terminals** (every agent's terminal at once, in rows and columns),
**Comms** (who says what to whom, threads, the queue), **Inbox** (what needs
you), **Map** (who directs whom, messages in flight), Review.

Keys: `↑↓` select · `Enter` message the agent · `^G` terminals (`Enter` on a
tile types into it, `Ctrl+]` leaves, `^Z` zoom, `+ - =` columns, `^R` one
rig) · `^Y ^A ^N` allow / always / deny · `^O` the operator · `?` help.

For real agents:

```bash
cargo run                                   # TUI; starts ainetd in the background
# then Ctrl+O: the operator recommends a team and launches it, or directly:
ainet up starter --cwd ~/code/app           # a builder and a reviewer
ainet up path/to/openrig/rig.yaml           # any OpenRig RigSpec
```

| | |
|---|---|
| ![Fleet](docs/images/fleet.png) | ![Comms](docs/images/comms.png) |
| ![Inbox](docs/images/inbox.png) | ![Map](docs/images/map.png) |

## Teams

```bash
ainet specs                                 # starter, workshop, factory, code-review, research, pm, operator
ainet up factory --cwd ~/code/app --plan    # what would start, in which order
ainet up factory --cwd ~/code/app
ainet ps --nodes                            # every seat: address, runtime, state
ainet grow factory --pod dev --member perf --role "PERFORMANCE ENGINEER"
ainet down factory                          # snapshot, then stop
ainet up factory                            # restore: every Claude conversation resumes
ainet discover && ainet adopt %12 --rig factory --pod dev --member extra   # a tmux agent joins
```

A seat has a stable address, `pod-member@rig` (logical id `pod.member`), that
outlives the conversation in it. Every Claude Code seat starts with its
identity, team map, the spec's guidance and culture in its system prompt, the
`ainet` MCP server, and ainet's hooks, without ainet touching your files.

## Agents talking

Agents use the `ainet` MCP tools (no permission prompts), or the same on the
shell:

| Tool | Shell | What |
|---|---|---|
| `send_message` | `ainet msg send <to> <text>` | a message, typed into the recipient's terminal and kept in its mailbox |
| `read_messages` / `reply_message` | `ainet msg inbox` / `msg reply <id>` | read and answer; replies thread back to the sender |
| `ask_agent` | `ainet msg ask <to> <text>` | ask and wait for the answer |
| `broadcast` | `ainet broadcast --rig R` | to every seat, or one pod |
| `chat_send` / `chat_read` / `chat_wait` | `ainet chatroom …` | the rig's chatroom; `@dev.review` mentions reach that seat |
| `queue_create` / `queue_claim` / `queue_update` / `queue_handoff` / `queue_block` | `ainet queue …` | owned work: assign, claim, report, finish, hand off, block and wake |
| `read_terminal` | `ainet capture <seat>` | look at a teammate's terminal |
| `whoami` / `list_agents` / `rig_status` | `ainet whoami`, `ps` | where am I, who is here |
| `send_message(to="operator")` | | reach you: it lands in your Inbox and Comms |

Messages never land under what you are typing: while you type into an
agent's terminal, or its seat has a typing guard, they are held and
delivered after. Watchdogs (`ainet watchdog register`) wake idle seats to
check their queue.

## How it works

```
 ainet (TUI) ─┐                                ┌─ seat: claude --session-id … --mcp-config ainet … (PTY)
 ainet / rig ─┼── Unix socket, JSON lines ──►  │  ainetd ─ seat: codex -c mcp_servers.ainet=… (PTY)
 ainet mcp  ──┤   ($XDG_RUNTIME_DIR/ainet.sock) │          adopted: a tmux pane
 ainet hook ──┘                                └─ SQLite: rigs, snapshots, mailbox, chat, queue, events
```

| Crate | What |
|---|---|
| `crates/ainet` | the binary: TUI, `daemon`, the OpenRig-style CLI, `mcp`, `hook` |
| `crates/daemon` | ainetd: panes, rigs, delivery, mailbox, chat, queue, watchdogs, hooks, discovery |
| `crates/spec` | OpenRig RigSpec/AgentSpec parsing, validation, launch order, the built-in library |
| `crates/claude` | Claude Code integration ported from Den: sessions, inbox socket, hook answers, `/always`, questions, transcripts, cost, agent-badge, hook install |
| `crates/pty` | panes: spawn, write, resize, screen and scrollback |
| `crates/model` | what a front end knows: the fleet store fed by ainetd's snapshot and events, the Fleet tree, the `/command` language, the demo fleet |
| `crates/tui` | ratatui + tachyonfx + tui-term: Fleet, Terminals, Comms, Inbox, Map |
| `crates/core` | agents, rigs, messages, queue, the wire protocol, a blocking client |
| `crates/critter` | the seeded 16×16 critters and names |

## Development setup

### Required

| Tool | Why | Arch Linux |
|---|---|---|
| Rust (stable, 1.88 or newer) via rustup | edition 2024; `rust-toolchain.toml` adds clippy and rustfmt | `sudo pacman -S rustup && rustup default stable` |
| A C compiler | `rusqlite` builds a bundled SQLite | already there (`gcc`) |
| git | branch info, and future review/worktrees | already there |

### To run real agents

| Tool | Why | Install |
|---|---|---|
| Claude Code (`claude`), 2.1.289 or newer | the main runtime; every built-in team uses it. 2.1.289 is what the session files, inbox socket and hook answers were validated against | `curl -fsSL https://claude.ai/install.sh \| bash`, then `claude` once to sign in |
| Codex CLI (`codex`), optional | `runtime: codex` seats in OpenRig specs | `npm i -g @openai/codex`, then `codex login` |
| tmux, optional | only to adopt agents already running in tmux (`ainet discover` / `adopt`) | already there |
| A truecolor terminal | critters are 24-bit colour | Ghostty, kitty, foot, WezTerm, Alacritty |
| A Unicode 16 font, optional | octant critters (`^E` twice) | a recent Ghostty/kitty/WezTerm/foot, or Iosevka / JetBrains Mono |

### Nice to have

| Tool | Why | Arch Linux |
|---|---|---|
| rust-analyzer | IDE support | `rustup component add rust-analyzer` |
| just | the tasks in `justfile` | `sudo pacman -S just` |
| bacon | rebuild-and-rerun on save (`just watch`) | `sudo pacman -S bacon` |
| cargo-nextest | faster tests | `sudo pacman -S cargo-nextest` |
| mold | much faster linking | `sudo pacman -S mold`, then `RUSTFLAGS="-C link-arg=-fuse-ld=mold"` |
| cargo-deny | license and advisory checks | `cargo install cargo-deny --locked` |
| sqlite3 | peek at the event log | already there |

Then:

```bash
just check     # fmt, clippy -D warnings, tests
just demo
```

## Roadmap

See [docs/OPENRIG-PARITY.md](docs/OPENRIG-PARITY.md) for the full list. Next:
Review (per-agent diffs with comments, worktree per agent), cost per seat and
team, rig bundles, missions and slices, permission policies, remote hosts.

## License

MIT. Critter code and design are ported from Den and agent-badge (MIT); see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
