# ainet GUI: a Zed-style desktop front end

Plan for adding a native GUI to ainet, built on Zed's UI stack (GPUI and
Zed's component library) and following Zed's look and interaction model,
while the console version (CLI + TUI) stays exactly as it is.

Status: **phases 0–8 done** (2026-10-16). `ainet-model` is shared by
both front ends (0); `crates/gui` runs on Zed v1.24.2 (1) with Zed's
themes, fonts, settings and keymaps (2), panes, docks, modals, toasts, a
text field and a picker (3), every TUI view: the Fleet and Agent panels,
live terminals, the Inbox, Comms, Queue, Chatroom, Events, the Map and
reports (4), the command palette, pickers and forms (5), terminals
streamed from ainetd and drawn only where they changed (6), the
workspace restored per daemon socket, a settings schema and packaging
(7), and a coverage review of every module with the missing headless
tests added, a two-job CI and offscreen screenshots (8). What is still
missing is listed in one place, under "Known gaps" in [GUI.md](GUI.md#known-gaps); in short:
dragging tabs between panes, URL hover, IME and mouse forwarding in
terminals, cost per agent, a few menus and pickers (`/adopt`,
`/discover`, messages, map nodes, rigs), the TUI on the stream, several
windows, an Arch package and a macOS build that were never built, and a
CI workflow that has not yet run on GitHub. The phase notes below say
what each phase did. Written 2026-10-15 against commit `c0dbb5a`.

**Decided (2026-10-15): option A.** The GUI uses Zed's real `ui` and `theme`
crates from a pinned Zed git revision, and the `ainet-gui` crate and binary
are licensed **GPL-3.0-or-later**. Everything else in the workspace stays MIT.

---

## 1. What ainet is today (analysis)

```
 ainet (TUI) ─┐                                ┌─ seat PTYs (claude, codex, shell)
 ainet CLI   ─┼── Unix socket, JSON lines ──►  │  ainetd
 ainet mcp   ─┤   ($XDG_RUNTIME_DIR/ainet.sock) │
 ainet hook  ─┘                                └─ SQLite
```

| Crate | Lines | Role | Matters for the GUI because |
|---|---|---|---|
| `core` | ~1.2k | types, wire protocol (`Request`/`Response`/`Event`), blocking `Client`, paths | the GUI speaks exactly this protocol; no daemon change is needed to start |
| `daemon` | ~4.6k | ainetd: PTYs, rigs, mailbox, chat, queue, hooks | unchanged; streamed terminals (`Attach`) and coloured scrollback added in phase 6 |
| `tui` | ~5.2k | ratatui front end | holds the **state model** the GUI needs, mixed with ratatui types |
| `ainet` | ~4.3k | the binary: CLI, TUI launch, `daemon`, `hook`, `mcp` | stays the console binary; agents and hooks call it by path |
| `critter` | 0.7k | 16×16 pixel critters | UI-agnostic (`pixels() -> Grid`), reusable as GPUI avatars as is |
| `pty`, `spec`, `claude` | — | daemon internals | not touched |

The important findings:

1. **The daemon is already the whole backend.** Everything the TUI does goes
   through `Request`s over the socket, plus a `Snapshot` and a `Subscribe`
   event stream (`crates/tui/src/backend.rs`). A GUI is a second client, like
   the TUI; it needs no new daemon features to reach parity.
2. **The TUI's `App` (`crates/tui/src/app.rs`, 1.7k lines) mixes three
   things:** the fleet state (agents, inbox, rigs, messages, chat, queue,
   terminal parsers), the domain logic over it (`apply_event`, `rebuild` of
   the fleet tree, `find_agent`, `entries`, the `command()` mini-language,
   `answer`), and ratatui UI state (`ListState`, `Rect`, `EffectManager`,
   key handling). The first two must move into a shared crate, or the GUI
   would duplicate ~900 lines of logic and drift.
3. **Terminals are rendered client-side from `vt100`.** The daemon sends the
   whole screen as escape sequences on request (`Request::Screen {
   formatted: true }`) after each `Event::Output`; the client feeds it into a
   local `vt100::Parser`. A GPUI terminal element can render that same
   `vt100::Screen` cell grid. Zed's own terminal (alacritty_terminal + PTY)
   is not needed: ainetd owns the PTYs.
4. **The binary path is load-bearing.** `conn.rs` starts the daemon with
   `current_exe() daemon`, `hook.rs` prints hooks that run `current_exe()
   hook`, and agents run `ainet msg …` in their shells. So the GUI must be a
   *separate* binary that starts the daemon through the console binary, never
   through itself.
5. **Demo mode is a backend, not a view.** `Demo` emits the daemon's own
   `Event`s, so the GUI gets `--demo` for free once the state model is shared.

## 2. Zed's UI stack: what can be used, and the licence catch

Checked on 2026-10-15 against `zed-industries/zed@main` and crates.io:

| Piece | Licence | Published | Notes |
|---|---|---|---|
| `gpui` (+ `gpui_macros`, `gpui_platform`, `gpui_util`, `gpui_shared_string`) | Apache-2.0 | crates.io `gpui 0.2.2` is stale; Zed itself builds from git | the framework: windows, elements, flex layout, actions, keymaps, focus, async executors, animation |
| `ui` (Button, IconButton, Label, ListItem, Tab, TabBar, Disclosure, ContextMenu, PopoverMenu, Tooltip, Modal, KeyBinding, Indicator, Avatar, Facepile, Scrollbar, IndentGuides, Notification, Callout, Chip, DataTable, TreeViewItem, ToggleButton, Banner, ChoiceCard, Progress …) | **GPL-3.0-or-later** | no (`publish = false`) | depends on `theme`, `settings`, `component`, `icons`, `menu` — all GPL |
| `theme`, `settings`, `workspace`, `picker`, `command_palette`, `terminal_view`, `editor` | **GPL-3.0-or-later** | no | `workspace` and `editor` pull in most of Zed; not usable as libraries in practice |
| `gpui-component` 0.7.3 (Longbridge, gpui-kit) | Apache-2.0 | yes | 60+ components (Input, Table, Tree, Dock, Resizable, Notification, Menu…), Zed-like but not Zed |
| Zed theme JSON files (`assets/themes/{one,ayu,gruvbox}`) | per-theme licences in `assets/themes/LICENSES` | — | the *format* is plain JSON; any Zed theme (incl. extension themes) can be read without Zed's code |

**ainet is MIT.** Linking Zed's `ui` crate makes the GUI binary a derivative
of GPL-3.0 code: the `ainet-gui` crate and binary would have to be licensed
GPL-3.0-or-later (the rest of the workspace can stay MIT, because nothing
else links it). This is the one decision this plan cannot make for you:

| Option | Fidelity to Zed | Licence of `ainet-gui` | Cost |
|---|---|---|---|
| **A. Zed `ui` + `theme` from git** (pinned rev) | exact: same components, same theme system, same feel | GPL-3.0-or-later | lowest component work; highest upgrade friction (unpublished internal API, Zed's `[patch.crates-io]` must be mirrored, `settings` global must be initialised the Zed way) |
| **B. `gpui` + own `ainet-ui` crate** mirroring Zed's `ui` API and design tokens, reading Zed theme JSON | very close: same tokens, spacing, components re-implemented to match | MIT (gpui is Apache-2.0) | ~2–3k lines of components; full control; stable |
| C. `gpui` + `gpui-component` | Zed-like, not Zed | MIT | least work, visibly a different design language |

**Chosen: A.** (B was the original recommendation, for keeping a single
licence.) Phases 0–2 and 4–7 are the same under A and B; only phase 3
differs, and under A no `crates/ui` is created. To keep the GPL boundary
clean, only `crates/gui` may depend on Zed's GPL crates; `ainet-model`,
`ainet-tui` and the console binary must never depend on `ainet-gui`.

## 3. Target architecture

```
                         ┌──────────── ainet-model (new, UI-agnostic) ────────────┐
 ainet (console) ─ tui ──┤ Store: agents, inbox, rigs, msgs, chat, queue, terms   │
                         │ apply(Snapshot/Event), fleet tree, entries, find_agent │
 ainet-gui ── gui ───────┤ Command parser → Action / Request, Demo, Source threads │
                         └────────────────── core (protocol, Client) ─────────────┘
                                                   │ Unix socket
                                                 ainetd
```

### Crates and binaries

| Crate | Binary | Contents |
|---|---|---|
| `crates/ainet` | `ainet` (unchanged name and CLI) | CLI, TUI, `daemon`, `hook`, `mcp`; new subcommand `ainet gui [--demo]` that execs `ainet-gui` from the same directory or `PATH` |
| `crates/tui` | — | ratatui rendering + key handling only, on top of `ainet-model` |
| `crates/model` (**new**) | — | `Store`, `Msg`, `Source` (daemon threads / demo), `Demo` (moved from tui), fleet tree, comms entries, command language, popups-as-text |
| `crates/ui` (**new**, option B) | — | `ainet-ui`: Zed-style components + theme loading (option A: not created; depend on Zed's `ui`/`theme`) |
| `crates/gui` (**new**) | `ainet-gui` | the GPUI application |

Why `ainet` stays the console binary (rather than renaming it `ainet-cli`):
hooks already installed in users' Claude settings, MCP configs written into
seats, the `rig` symlink and every agent's shell all invoke `ainet` by name
or path. Renaming would break running fleets. The console experience
"`ainet-cli`" is therefore simply `ainet`; if you want the literal name, the
install task adds an `ainet-cli` symlink to it, and `invoked_as()` in
`main.rs` already handles any name.

`default-members` stays `["crates/ainet"]`, so `cargo build`, `cargo
install --path crates/ainet` and CI for the console version never compile
GPUI. The GUI is built with `cargo build --manifest-path crates/gui/Cargo.toml`
/ `just gui` (`crates/gui` is a workspace of its own, see GUI.md).

### Threading model in the GUI

- `Store` lives in an `Entity<FleetStore>` (GPUI model). Views observe it and
  re-render on `cx.notify()`.
- The existing blocking backend (`listen`, `request`, `fetcher` threads)
  is reused as is, but sends into a `futures::channel::mpsc` instead of
  `std::sync::mpsc`; a `cx.spawn` task on the foreground executor drains it
  and calls `store.apply(msg)`. No tokio in the GUI process.
- Daemon autostart: `ainet-gui` locates the console binary (sibling of
  `current_exe()`, then `PATH`, then `$AINET_BIN`) and runs `ainet daemon`
  detached, the way `conn::ensure_daemon` does. That logic moves from
  `crates/ainet/src/conn.rs` into `ainet-model` (taking the binary path as a
  parameter) so both front ends share it.
- Terminal screens: `Event::Output` marks a terminal dirty; a throttled
  (≤ 30 fps per terminal, visible terminals only) fetch reuses
  `backend::fetcher`. Phase 6 adds an optional streaming path (done:
  `Request::Attach`, see GUI.md).

## 4. The GUI, Zed-style

The layout is Zed's workspace, with ainet's views mapped onto it:

```
┌ Title bar ─ [ainet ▾] [rig: docs ▾] ······· ● connected  3 waiting  [_][□][×] ┐
├──────────────┬──────────────────────────────────────────────┬────────────────┤
│ Fleet panel  │ Pane group (tabs, splits)                    │ Agent panel    │
│ (left dock)  │ ┌ ◐ lead ─┬ ◑ ui ─┬ Comms ─┬ Inbox (3) ─┐    │ (right dock)   │
│ ▾ docs       │ │                                       │    │ critter, role, │
│   ◐ lead     │ │  live terminal of the focused seat    │    │ model, state,  │
│   ├ ◑ ui     │ │  (type straight into it when focused) │    │ context gauge, │
│   └ ◒ app    │ │                                       │    │ cost, task,    │
│ ▸ infra      │ └───────────────────────────────────────┘    │ queue items    │
│ ◓ loose      │                                              │                │
├──────────────┴──────────────────────────────────────────────┴────────────────┤
│ Bottom dock: Queue · Chatroom · Events log                                   │
├──────────────────────────────────────────────────────────────────────────────┤
│ Status bar: ⎇ rig docs · 7 agents · 2 working · ⚠ 3 waiting · ainetd ✓       │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Zed features to reproduce, and what each becomes in ainet

| Zed feature | ainet GUI | TUI equivalent today |
|---|---|---|
| Workspace with left / right / bottom docks, toggle and resize (`ctrl-b`, `ctrl-alt-b`, `ctrl-j`) | Fleet panel left, Agent panel right, Queue / Chat / Events bottom | Fleet list + Details pane |
| Pane group: tabs, split right/down, drag tabs between panes, close, pin, preview tab | items: `TerminalItem(agent)`, `CommsItem`, `InboxItem`, `MapItem`, `ReviewItem`, `TranscriptItem(agent)`, `SpecItem(name)` (read-only YAML) | `Tab` enum + Terminals grid |
| "Terminals grid" | a "Tile all" action that lays the rig's terminals out as splits | `^G`, `+ - =`, `^R` |
| Project panel tree (disclosures, indent guides, sticky headers, context menu) | Fleet tree: rig → pod → seat (`delegates_to` nesting from `rebuild`), critter avatar, state indicator, unread badge; right-click: message, open terminal, transcript, relaunch, continue, release, remove, typing guard | Fleet rows |
| Command palette (`ctrl-shift-p`), fuzzy, shows bindings | every action, plus the TUI's command language (`up factory ~/app`, `spawn codex …`) as free-text commands | `:` command line |
| File finder (`ctrl-p`) | agent switcher: fuzzy over names, seats, roles, rigs; Enter opens the terminal tab | `↑↓` + Enter |
| Tab switcher (`ctrl-tab`) | same | `Tab` |
| Pickers / modals (theme selector, …) | spec picker for `up` (with `--plan` preview), snapshot picker for `restore`, theme selector, rig switcher | popups |
| Notifications / toasts, status bar indicators | daemon flashes, "X joined the fleet", connection state, waiting count (pulses like the TUI's glowing Inbox tab via `with_animation`) | flash line |
| Agent panel tool-call approval UI | Inbox items: Bash command / plan / question cards with **Allow** `ctrl-y`, **Always** `ctrl-a`, **Deny** `ctrl-n`; questions as choice cards + free text | Inbox view, `^Y ^A ^N` |
| Keymap JSON with contexts, user override, hot reload | `assets/keymaps/default-linux.json`, `default-macos.json`; user `~/.config/ainet/keymap.json`; contexts `Workspace`, `FleetPanel`, `Terminal`, `Inbox`, `Comms`, `Picker` | hard-coded keys |
| Settings JSON, hot reload, `ctrl-,` | `~/.config/ainet/settings.json`: `theme` (light/dark/system), `ui_font_family/size`, `terminal.font_family/size/line_height`, `fleet_panel.dock/width`, `critters.size`, `confirm_quit`; JSON-schema'd | `config.toml` (paths only) |
| Theme system (Zed theme JSON, light/dark, system appearance) | read Zed theme JSON (bundled One Dark/Light; user themes in `~/.config/ainet/themes/*.json`, compatible with Zed extension themes); the Omarchy palette (`theme.rs`) becomes one more theme source; agent hues map onto the theme's `players` colours | `Palette::load()` |
| Zed fonts (Zed Plex / IBM Plex Sans + Mono, `.ZedSans`) | bundle Zed Plex (OFL) as UI and terminal fonts by default | terminal's font |
| Client-side decorations on Linux, custom title bar | same as Zed: `WindowDecorations::Client`, own title bar with app menu, rig switcher, window controls | header + wordmark |
| Keybinding hints in buttons and tooltips (`KeyBinding`) | everywhere an action has a binding | footer hints |
| Context menus, right-click | fleet rows, tabs, terminal (copy/paste/clear selection/send `ctrl-c`), messages (reply, open thread) | — |
| Window state restore (workspace serialization) | dock sizes, open tabs, splits, focused agent saved per daemon in `$XDG_STATE_HOME/ainet/gui.json` | — |
| Zoom pane (`shift-esc`) | zoom terminal | `^Z` |
| Vim mode, LSP, editor, collab | **not** in scope | — |

### Views in detail

- **TerminalItem (the core).** A custom GPUI `Element` modelled on Zed's
  `TerminalElement`: measure the terminal font's cell size, compute rows ×
  cols from bounds, send `Request::Resize` on change (debounced), paint
  background rects merged per run, text as `ShapedLine`s per run of equal
  style, cursor (block/bar/underline, blinking), selection, URL hover.
  Input: when focused, GPUI keystrokes go through a port of
  `term::encode_key` (from crossterm `KeyEvent` to GPUI `Keystroke`) into
  `Request::Input`; paste uses bracketed paste if the screen requested it.
  Focus in / out sends `Request::Interact { on }` so messages are held while
  you type, replacing the TUI's explicit typing mode and `Ctrl+]`.
  Scroll wheel fetches history (`Screen { history: n }`). Header shows the
  critter, seat address, state, and a "message agent" button.
- **Comms.** A virtualised `uniform_list`/`list` of messages and chat posts
  with sender critters and hues, threads expandable (Disclosure), reply and
  compose box at the bottom (single-line input with `@seat` completion).
- **Inbox.** Cards per item as described above; waiting items first;
  resolved items collapse into history.
- **Map.** A `canvas` painting rigs as clusters, seats as critter nodes,
  `delegates_to` / `spawned_by` edges as paths, messages in flight as
  animated dots along edges (port of `draw_map` geometry).
- **Review.** Placeholder in the TUI today; the GUI version is where diffs
  will live later (out of scope here, kept as an empty item).
- **Agent panel.** Details pane: critter (animated, 4× scale), role, model,
  runtime, cwd, state with Indicator, smoothed context gauge (`Progress`),
  cost, task, owned queue items, last messages.
- **Text input.** Zed's text field is the whole `editor` crate, which is not
  usable here. Write a small single-line/multi-line `TextInput` element on
  GPUI's `EntityInputHandler` (GPUI's own `examples/input.rs` is the
  pattern): IME, selection, clipboard, undo. Used by the compose box,
  pickers and modals.
- **Critters.** `critter::pixels(seed, colors, pose)` gives a 16×16 grid;
  paint it as quads in a `canvas` at any scale (crisp, no half-blocks), and
  advance `Pose::at(seed, tick, working)` on a 120 ms timer only while
  visible. Used as avatars in tree rows, tabs, messages and notifications.

### Actions

Declared with GPUI's `actions!` macro in namespaces mirroring Zed's style:
`workspace::{ToggleLeftDock, ToggleRightDock, ToggleBottomDock, NewWindow,
CloseWindow, OpenSettings}`, `pane::{SplitRight, SplitDown, CloseActiveItem,
ActivateNextItem, …}`, `fleet::{SelectNext, SelectPrev, OpenTerminal,
MessageAgent, CloseAgent, Spawn, Up, Down, Restore, Grow, Operator,
NextWaiting}`, `inbox::{Allow, Always, Deny, Answer}`, `terminal::{Copy,
Paste, Clear, SendCtrlC, ScrollUp, ScrollDown, ToggleZoom}`,
`ainet::{ToggleCritterSize, Quit}`. Each maps to the same `Request` the TUI
sends, via `ainet-model`'s command layer, so the two front ends cannot
disagree about what an action does.

## 5. Phases

Each phase ends green on `just check` and leaves the console version
untouched in behaviour.

### Phase 0 — Extract `ainet-model` (console refactor, no GUI yet) · ~3–4 days

1. Create `crates/model` (`ainet-model`), deps: `ainet-core`,
   `ainet-critter`, `vt100`, `anyhow`. No ratatui.
2. Move from `tui/src/app.rs` into `model::Store`: the fleet fields
   (`agents`, `inbox`, `rigs`, `messages`, `chat`, `queue`, `screens`,
   `terms`, `connected`, `selected`), `apply`/`apply_event` (minus the
   tachyonfx glow, which becomes a returned `Notice` the TUI turns into an
   effect), `rebuild` → `fleet_rows()`, `row_of`, `entries`,
   `selected_entry`, `find_agent`, `rig_arg`, `selected_item`.
3. Move `backend.rs` → `model::source` (generic over a `Sink` trait so the
   TUI keeps `std::sync::mpsc` and the GUI uses a futures channel), and
   `demo.rs` → `model::demo`.
4. Split `command()` (≈300 lines) into `model::command::parse(&Store, &str)
   -> Result<Command>` and `Command::to_request()`; the TUI keeps executing
   the UI-only commands (`help`, `view`, `cols`, `crit`).
5. Move `popup::render` (Response → text) into the model too.
6. Move daemon autostart from `crates/ainet/src/conn.rs` into the model as
   `ensure_daemon(socket, ainet_bin)`.
7. `tui::App` becomes `{ store: Store, ui state… }`. Existing tests,
   `demo_buffer` snapshot test and `ainet-tui --example frame` must produce
   identical output; add unit tests for `Store::apply`, fleet tree order and
   the command parser that used to be untestable without ratatui.

### Phase 1 — GPUI skeleton · ~3 days

1. Add `crates/gui` with `gpui` from git, pinned to one Zed commit:
   `gpui = { git = "https://github.com/zed-industries/zed", rev = "…" }`
   (plus `gpui_platform` with `wayland`, `x11`, `font-kit`). Copy the
   `[patch.crates-io]` entries GPUI needs from Zed's root `Cargo.toml`
   (`async-task`, `calloop`, `async-process` at least) into ainet's root
   manifest; record the rev in `docs/GUI.md` with an upgrade procedure.
2. Crate-level `rust-version` = Zed's toolchain (1.98 at the time of
   writing); the workspace's 1.88 stays for the console crates.
3. `ainet-gui [--demo] [--socket PATH]` opens one window with client-side
   decorations, a title bar, an empty workspace and a status bar showing the
   connection state from `Store`.
4. `ainet gui` subcommand in the console binary execs `ainet-gui`.
5. `just gui`, `just gui-demo`; README section "Desktop app" with the Arch
   packages GPUI needs (`vulkan-icd-loader` + a Vulkan driver, `wayland`,
   `libxkbcommon`, `libx11`, `libxcb`, `fontconfig`) — verify the exact list
   against Zed's `script/linux` at the pinned rev.

### Phase 2 — Theme, fonts, settings, keymap · ~4 days

1. Theme: under **B**, `ainet-ui::theme` parses Zed theme JSON (the
   `ThemeFamily`/`ThemeContent` schema: `colors`, `status`, `players`,
   `syntax` — only the colour keys ainet uses are required) into a
   `Theme` global; `cx.theme().colors().panel_background` etc. with Zed's
   key names. Bundle One Dark / One Light (check
   `assets/themes/LICENSES`), plus a "Deepharness" theme generated from
   `tui/src/theme.rs` and the Omarchy reader. Under **A**, use Zed's `theme`
   crate and `ThemeRegistry` directly.
2. Fonts: bundle Zed Plex Sans/Mono (OFL) via `cx.text_system().add_fonts`.
3. Settings: `~/.config/ainet/settings.json` with serde defaults, watched
   with `notify`; changes re-apply the theme/fonts live.
4. Keymap: default JSON per platform, user overrides, contexts, reload on
   change; `KeyBinding` hints read from it.

Done, with two items of §4 left for later: user themes
(`~/.config/ainet/themes/*.json`) were read at start-up but not reloaded on
change (done in phase 5, with the theme selector), and `settings.json` had no JSON
schema (done in phase 7: generated from the settings' types, named in the
template's `"$schema"` for external editors).

### Phase 3 — Component library · ~1–2 weeks (B) / ~2 days (A)

Under **B**, `crates/ui` (`ainet-ui`, MIT) implements, matching Zed's
`ui` API names and visual spec (heights, paddings, radii, `DynamicSpacing`,
`TextSize`, hover/active/selected/focus states from theme tokens):
`Label`, `Icon` (bundle Lucide SVGs, ISC — Zed's own icon set is GPL),
`Button`, `IconButton`, `ToggleButton`, `ListItem`, `ListHeader`,
`Disclosure`, `IndentGuides`, `Tab`, `TabBar`, `Divider`, `Indicator`,
`Avatar` (critter-backed), `KeyBinding`, `Tooltip`, `ContextMenu`,
`PopoverMenu`, `RightClickMenu`, `Modal`, `Picker` (fuzzy matcher: `nucleo`
or `fuzzy-matcher`), `Scrollbar`, `Progress`, `Chip`, `CountBadge`,
`Callout`, `Notification`/toast, `TextInput`, plus `Dock`, `Pane`,
`PaneGroup` (resizable splits) and `StatusBar` — the workspace pieces Zed
keeps in its GPL `workspace` crate, written small for ainet's needs.
A `component_preview` example window renders every component in every
state, as Zed's component preview does, for visual review.

Under **A**: wire Zed's `ui`, `theme`, `settings`, `component`, `icons`,
`menu` from the same pinned rev, initialise their globals, license
`crates/gui` as GPL-3.0-or-later with a notice in README and
`THIRD_PARTY_NOTICES.md`. `Dock`/`Pane`/`Picker`/`TextInput` still have to
be written (Zed's `workspace`, `picker` and `editor` are not usable as
libraries).

Done under A: `Item`, `Pane` (Zed's `TabBar`/`Tab`, hover close, middle
click, tab menu), `PaneGroup` (split right/down, draggable dividers, focus
by order and direction, zoom on `shift-escape`), `Dock`/`Panel` (left,
right, bottom; toggled, dragged; sizes saved since phase 7), the
modal layer, toasts from the fleet's notices, `TextInput` (also behind
Zed's `ui_input::InputField`), and `Picker` on Zed's `fuzzy`, with the
agent switcher (`ctrl-p`) as its first use. Left for later: dragging tabs
between panes, pinned and preview tabs, a `component_preview` window.

### Phase 4 — Views · ~2–3 weeks

In this order, each usable on its own against `--demo` and a live daemon:

1. Fleet panel (tree, critters, selection synced with `Store.selected`,
   context menu).
2. Agent panel (details, gauge, cost, queue).
3. TerminalItem element (render, resize, input, focus ⇄ `Interact`,
   copy/paste, history scroll, zoom) and "Tile all".
4. Inbox item (approval cards, question answering) + status bar waiting
   count with pulse animation and toast on new waiting items.
5. Comms item (stream, threads, compose with `@` completion, chat topics).
6. Bottom dock: Queue table (claim/update/handoff/block from context menu),
   Chatroom, Events log.
7. Map item (canvas).
8. Transcript item and Spec viewer (read-only text).

Steps 1 and 2 done: the Fleet panel (tree in the TUI's order, foldable
rigs, indent guides, critters, waiting and unread badges, the agent's
right-click menu and keys, sending what the TUI's commands send through
`command::for_agent`), the Agent panel (details, gliding context gauge,
activity, messages, queue), the `Critter` avatar element, a message modal,
and `TerminalItem` as a stand-in tab that step 3 fills with the live
terminal. Not yet: cost (ainetd does not report it), the TUI's spinner and
pulsing glyph for working and waiting agents (a state dot instead).

Step 3 done: `TerminalItem` with `TerminalElement` (cell metrics from the
terminal font settings, merged backgrounds, runs shaped per style, theme
ANSI colours, cursor shapes and blink, wide characters), resize after
150 ms of quiet, screens fetched on output for shown terminals at most
every 66 ms (the TUI's pump, moved into `ainet_model::Pump`), keys through
the TUI's xterm table (moved into `ainet_model::terminal`), focus ⇄
`Interact`, selection and copy, bracketed paste, scrollback on the wheel
(`Screen { history }`, new `Link::history` / `Msg::History`), the
terminal's menu, and `fleet::TileAll` as a grid of panes. Not yet: URL
hover, IME composition in terminals (no input handler; keys come from
key-down events), forwarding the mouse to programs that ask for it, `--view
terminals`/`rig:` tiling at start. (Colours in scrollback came in phase 6.)

Step 4 done: `InboxItem`, cards after Zed's tool-call approval cards
(critter, name, kind chip, summary, scrolling detail: command and edit
coloured as the TUI's, plans as light markdown), waiting first and the
history folded; Allow/Always/Deny with key hints through `Store::answer`;
questions as `ChoiceCard`s plus a free-text field, sent by
`command::answer_question` (new `command::answer_choices` for picks);
`ctrl-y`/`ctrl-a`/`ctrl-n` outside the Inbox open it on
`Store::next_waiting` first, as the TUI's; alerts as toasts with the
agent's critter and a Review button (text from the new
`Store::inbox_alert`, which the TUI's flash now uses) and a waiting badge
in the title and status bars that glows three times per alert. The
Inbox's single-letter keys (`y`, `a`, `n`) were dropped. Not yet: the
agent's role in the card, markdown beyond headings, lists and code
fences, keyboard focus on individual buttons.

Step 5 done: `CommsItem`, the stream of messages and chat posts in the
Store's order as a `gpui::list` that sticks to its end like a chat
(critters, names in the agents' colours, held and unread marks, bodies
with `@name`s coloured and fenced code), threads opening under their
message with indent guides (`Store::replies`), and a multi-line compose
box that sends what the TUI's input line sends in Comms (the new
`command::comms`, shared with the TUI), `tab` completing `@names`. Not
yet: a right-click menu on messages, an `@` completion list while
typing, Markdown beyond fenced code, older history than the Store keeps.

Step 6 done: the bottom dock holds three panels, switched by tabs in
their strip or their status bar buttons (Zed's `ToggleFocus` per panel):
the Queue as Zed's `Table` (the TUI's order and chip colours, open or
all, a menu that claims, unclaims, moves the state, adds a note, hands
off, blocks and shows, each the request `ainet queue` sends, through the
new `command::queue`), a Chatroom per rig and topic with a box posting
`ChatSend` (`command::chat`), and the Events log (the new
`ainet_model::journal`, capped at 500, newest last). Not yet: creating a
queue item from the panel (`/queue` in Comms does), editing priority and
tags, a reason when unclaiming, `@` completion in the handoff form.

Step 7 done: `MapItem`, the TUI's Map as a canvas tab (`ctrl-t`): each
rig a labelled cluster of its live agents and the loose agents another,
laid out by the new pure `ainet_model::map::layout` (the TUI's layers by
depth per cluster, clusters packed to fit the tab, nodes shrinking
rather than overlapping), critter nodes with name, role and state,
`delegates_to`/`spawned_by`/parent edges as curves with arrowheads, the
TUI's work flowing along edges (`map::pulses`, which the TUI now calls),
and each new message between two agents flying from sender to recipient
over a second, frame by frame while any is in the air; hover lights a
node and its edges, a click selects, a double click opens the terminal,
the wheel and dragging pan and `ctrl`-wheel zooms, fitted at first. Not
yet: empty and stopped seats on the map, edge kinds other than who
directs whom, a menu on nodes. (The view is kept across sessions since
phase 7.)

Step 8 done: `ReportItem`, a read-only, selectable text tab in the buffer
font for replies that are long text (transcripts, `/spec`, `/export`, a
queue item), opened instead of a toast; the reply is routed to it by the
new `Link::ask` (a token, `Msg::Reply`). Not yet: searching inside a
report, YAML highlighting, transcript filters (`tail`, `grep`).

### Phase 5 — Command palette, pickers, modals · ~1 week

Command palette over all actions with bindings and the free-text command
language; agent switcher (`ctrl-p`); spec picker + `Up` modal (cwd chooser
via `cx.prompt_for_paths`, name, fresh seats, `--plan` preview); restore /
snapshot picker; spawn modal (runtime, cwd, role); grow modal; theme
selector. (Operator, `ctrl-o`, came early, in phase 4.)

Done: the palette (`ctrl-shift-p`) lists the actions available where the
keyboard is, in words with their key bindings, lately used first, and runs
a `/command` line (or words matching no action) through
`command::parse`, saying in its row what it will do or what is wrong;
the agent switcher opens the terminal (`ctrl-enter` only selects); `Up`
is a spec picker (the library through the new `Link::list` /
`Msg::Listing`, or a typed path) then a form with Browse…, name, fresh
seats, Plan (shown in the form) and Launch; `Restore` picks a rig, a
snapshot and the seats to start fresh; `Down` asks with a snapshot on;
`Spawn` (`s`) and `Grow` are forms; the forms' requests are
`command::{up, down, restore, spawn, grow}`, which the typed commands
now share. The theme selector previews, restores on escape and saves
only `"theme"` into settings.json's text; user themes reload when their
directory changes; `ctrl-tab` is Zed's tab switcher; the title bar's
rig switcher (`fleet::SwitchRig`) picks a rig and selects its first live
agent. The demo's added answers (transcripts, specs, plans, listings,
queue requests, chat topics) are the GUI's only (`Demo::full`): the
console's `ainet --demo` answers as before. Not yet: a picker
for `/adopt` or `/discover`, editing a rig's edges when growing, and a
rig's own menu in the Fleet with these entries. (The palette's history is
kept across sessions since phase 7.)

### Phase 6 — Performance and live terminals · ~1 week

1. Measure: N terminals visible, Output events per second, fetch cost of
   full-screen ANSI per update.
2. If needed, add to the protocol (VERSION 3, backward compatible) a
   `Request::Attach { agent }` that streams raw PTY output as
   `Event::Bytes { agent, data }` after an initial formatted screen; the GUI
   then feeds `vt100` incrementally like Zed feeds alacritty. The TUI can
   adopt it later; old clients keep using `Screen`.
3. Render only dirty terminals; cache shaped lines per row keyed by row
   content; cap critter animation to visible avatars.

Done. Measured without a display (`terminal_cost` in `ainet-model`,
`layout_cost` in the GUI, both ignored tests run by hand, on the new
`ainet_model::sample` busy screen): a refetch moves the whole formatted
screen (3.6 KB as a reply line at 24×90, 10.9 KB at 50×200) where the
spinner frame it shows is 1–2 KB, and costs the front end 35–153 µs
against 12–18 µs to feed the bytes; with 16 busy 50×200 terminals 2.5
MB/s and about 85 ms of CPU a second between ainetd and the UI thread,
every screen up to 66 ms late. So streaming was added, without bumping
`protocol::VERSION`: `Request::Attach { agent }` answers with the
formatted screen, then `Event::Bytes` on that connection only (UTF-8
split across reads kept whole), the whole screen again after a resize,
when the client fell behind, once output settles and every 5 s of
continuous output; it ends when either side closes. The pane taps its
reader under the parser's lock (`ainet_pty::Pane::attach`). An old daemon
answers "bad request" and the link falls back to fetching until it
reconnects; old clients never send it; the TUI still fetches. The GUI
streams each shown terminal once, however many tabs show it, and falls
back to fetching when a stream ends. Screens that change no longer
notify the fleet: `ScreenChanged` redraws only their terminals, and panes
and dock panels are cached views. The element keeps shaped rows keyed by
a hash of their formatted cells (`RowCache`, two frames, anew on another
palette or font): a spinner frame lays out one row. Critters already
animated only when painted. Colour scrollback was cheap:
`Screen { history, formatted: true }` also answers `styled_rows` (new
`ainet_pty::styled_row`), which the GUI draws. Not yet: the TUI on the
stream, a per-cell fast path for whole-screen layout (only the first
frame and new rows pay it), measuring shaping on a real display.

### Phase 7 — Persistence, polish, packaging · ~1 week

Workspace restore (docks, tabs, splits, focused agent); window bounds;
`confirm_quit`; desktop entry and icon (`ainet-gui.desktop`); `just
install-gui`; optional AUR `PKGBUILD` split into `ainet` and `ainet-gui`;
macOS build check (GPUI's Metal backend) without promising support yet.

Done. Workspace restore as Zed's `workspace::persistence`, simplified to
one versioned JSON file, `$XDG_STATE_HOME/ainet/gui.json`, with a
workspace per daemon socket (and the demo's): the window's bounds and
maximized or full-screen state, each dock's panel and whether it is
open, panels dragged to a size, the pane tree with its splits, shares,
tabs and active tab and pane, the Fleet's folded rigs and selected agent
(selected again if still live), the Map's zoom and pan, and the
palette's recent actions. Terminals are saved by agent id
and come back after ainetd's first snapshot, without the agents gone
(dropped quietly, their panes and splits folding away); reports are not
restored. Saved once a change has held still for a second's check, and
at once when the window closes or the app quits (`confirm_quit` asks
after saving). A corrupt file or another version is ignored and written
anew, a newer version's left alone. Polish: `settings-schema.json`
generated with `schemars` (already in Zed's tree) and named in the
settings template's `"$schema"`. Packaging: `ainet-gui.desktop`, an SVG
icon of ainet's critter (written once, checked by a test), `just
install-gui`, and `packaging/arch/PKGBUILD` (split packages; untested,
only `makepkg --printsrcinfo` was run). macOS: plausible on reading
(same `gpui_platform` features as Zed, Linux-only code behind `cfg`, the
`cmd` keymap), not built. Not yet: the zoomed pane and the Comms or
Inbox selection across sessions, several windows.

### Phase 8 — Tests and CI · continuous

- `ainet-model`: plain unit tests (event application, fleet ordering,
  command parsing, demo determinism).
- GUI: GPUI's `TestAppContext` / `VisualTestContext` (`gpui` feature
  `test-support`) for headless tests: actions dispatch the right `Request`
  (against a fake socket / the demo source), focus moves, keymap contexts,
  terminal key encoding table.
- Optional: screenshot tests of `--demo` frames on Linux under a virtual
  Wayland/X display.
- CI keeps `just check` for the console crates; a separate `just check-gui`
  job (clippy `-D warnings`, tests) that installs GPUI's system packages.

Done. Coverage reviewed module by module (the table under "Tests and CI"
in GUI.md); headless tests added where earlier phases left paths
untested: the tab's right-click menu, the status bar's dock buttons, the
Inbox's Allow/Always/Deny buttons, the terminal's right-click menu
(which found and fixed a bug: the item's own focus-on-click took the
keyboard back from the menu, so its arrow keys and enter went to the
agent), the theme selector's edge cases, and the restore through
`main`'s `open_window` with a state file. Screenshots: GPUI v1.24.2 has
a headless renderer (`gpui_platform::current_headless_renderer`, wgpu
without a surface) and `gpui::HeadlessAppContext`, so an ignored test
draws the demo workspace into `target/screenshots/` (`just
screenshot-gui`; it needs a GPU or a software Vulkan driver, so CI does
not run it). CI: `.github/workflows/ci.yml`, a `console` job (fmt,
clippy, test; fails if Zed was fetched) and a `gui` job (Zed's system
packages that the GUI's tree links, the toolchain named by
`rust-version`, cargo and `target/` cached by `crates/gui/Cargo.lock`).
`just check-all` runs both gates. cargo-deny was left out: no config for
the two licences could be checked here.

### Phase 9 — OpenRig-style views (asked for after first use, 2026-10-16)

From trying the app on a real display:

1. **Terminals grid.** Every live agent's terminal at once, in rows and
   columns, as one tab (the TUI's `^G` view), rather than as pane splits:
   tiles reuse `TerminalItem` (header, live screen, typing with messages
   held), columns from `terminal::grid_cols` with `+ - =` like the TUI,
   a rig filter (`ctrl-r`), click a tile to type into it, zoom one tile.
2. **Command line.** OpenRig's `cmd ▸` line, always there: `/commands`,
   `@Name message`, plain text to the selected agent, Tab completion,
   through the shared command language (`commands::run`,
   `command::message`); a key to reach it from anywhere but a terminal.
3. **Topology view.** OpenRig's TUI topology: an explorer tree (host →
   rigs → pods with seat counts) and, for the chosen rig, a table grouped
   by pod: seat, runtime, model, context, state, queue count, work item,
   what it is doing now; a summary line (seats, working, needing you, open
   rows); filter (`/`); sub-views Table, Recent (queue changes from the
   journal), Overview, Graph (the Map), Health.
4. **Conversation view.** Each agent's session as a readable chat rather
   than a raw terminal, side by side like OpenRig's: your prompts as
   blocks, the agent's replies, tool calls with an output excerpt
   (ainet/rig commands highlighted), and a prompt box under each that
   messages the agent; built from Claude Code's transcript (Codex as far
   as its transcript allows).

Done in this phase so far: the Inbox as a table above the selected
request, with a reply field that denies with a reason (a new optional
`message` on `Request::Answer`) or messages the agent; and item 1, the
Terminals grid (`terminals.rs`, see GUI.md "The Terminals grid"): one tab
whose tiles are `TerminalItem`s told when they show and hide as a pane
tells its tabs, replacing the split-based tiling (`Workspace::tile_rig`
is gone) for `ctrl-g`, `--view terminals`/`rig:NAME`/`<rig>` and `/view`;
`+ - =` and `ctrl-r` on the grid itself (context `Terminals && grid`),
`ctrl-alt` arrows between tiles, `ctrl-alt-g` back to the grid, a tile
zoomed inside the grid; the grid saved with its rig and columns. Two
rules came with it: the view of an agent shown or focused last sizes its
pane (`FleetModel::claim_size`), so a tile and a tab of one agent never
take turns; and `ainet-model` has the grid's arithmetic
(`terminal::{more_cols, fewer_cols, next_rig, grid_step}`, the first
three now used by the TUI too, which behaves as before). Tiles in one
pane redraw together (GPUI redraws a view's subtree), from their row
caches.

Item 2, the command line (`command_line.rs`, see GUI.md "The command
line"): a strip above the status bar (`cmd ▸`, a field, what enter will
do), on by default (`"command_line": false` shows it only while it has
the keyboard). A line means what the TUI's input line means:
`/command` through `commands::parse`/`run` (which now returns what was
wrong), `@Name text` through `command::message`, other text to the
selected agent, or, while the Inbox shows a waiting question, its answer
(`InboxItem::waiting_question`). Errors show in red in the line and as a
toast, and the line stays; done, it clears. `tab` completes `@names` and
`/command` names, `up`/`down` recall the last 50 lines, saved with the
workspace (`command_history`, left out while empty). `ctrl-;` (`cmd-;`)
reaches it from anywhere but a terminal and escape gives the keyboard
back where it was. `ainet-model` gained the list of `/commands` with
their usage (`command::COMMANDS`, checked against `command::parse`) and
their completion (`complete_command`, `complete_line`); the TUI does not
use them, so it behaves as before.

Item 3, the Topology view (`topology.rs`, see GUI.md "The Topology
view"): a tab (`ctrl-shift-t`, `--view topology[:RIG]`, `/view
topology[:RIG]`) with an explorer tree on the left (TOPOLOGY, this host,
rigs with their status, pods with seat counts, the loose agents) choosing
what the right shows: Table (Zed's `Table` grouped by pod: pod, seat,
RT, model, ctx, state, Q, work, now; a `/` filter; OpenRig's summary
line; enter opens the terminal, a right click the Fleet's agent menu,
now `fleet_panel::agent_menu`), Recent (the queue's changes from the
items' notes, oldest first; enter shows the item as the Queue panel's
Show), Overview, Graph (the Map of that rig or pod, `MapItem::set_scope`
over the new `map::layout_where`) and Health (its checks named, then
what they found, never more). The bare keys (`/`, `tab`, `1`–`5`,
`ctrl-r`) live in `Topology && view`, only while the view itself has the
keyboard. `ainet-model` gained `topology` (tree, rows, filter, summary,
changes, overview, health, scopes), all tested; no protocol change, the
TUI untouched.

Item 4, the Conversation view (`conversation.rs`, see GUI.md "The
Conversation view"): a tab per agent (`ctrl-shift-o`, `c` in the Fleet,
its menu, the terminal strip's chat button; splits; and
`workspace::TileConversations` for a rig side by side) showing the
session as Zed's agent panel would: your prompts on lighter blocks, the
agent's words after a dot, tool calls as `Bash(cargo test)` with an
output excerpt behind `⎿`, the messages ainetd typed in as framed blocks
(From, To, Sent, body, `↩ Reply:`), `ainet …`/`rig …` commands in amber,
a header with runtime and seat, a footer with permission mode, model and
cwd, and a `❯` prompt box sending what the Message modal sends. The data
is a new, additive `Request::Conversation { agent, tail, after }`
answered with `ainet_core::Conversation` entries (serde defaults, empty
fields left off, unknown kinds read as `other`), read by ainetd with
`ainet_claude::transcript::thread` (forwards, whole lines, ids growing
along the file); the tab asks for what came after its last entry (or
from a call still running) when the agent moves and every 3 s while
shown. An older ainetd's `bad request` falls back to `Transcript`'s text
with a notice; Codex and shell seats get a notice. The text ainetd types
for a message moved into `ainet-core` (`conversation::delivery`,
`mention`, unchanged) so it is known again as an inbound message. The
app's demo tells conversations that grow as it talks; the console's
demo answers as before. The TUI only gained a no-op match arm for the
new `Change::Conversation`.

After review: `--view topology:RIG` keeps its rig until ainetd's first
snapshot; a terminal size that comes after its view was hidden is not
heard (nor kept as an heir), and a hidden `TerminalItem` drops the size
it was about to send; a Conversation tab drops a reply asked of the
session before a new transcript (a generation count) and asks again
once the fleet is done applying the reply, never inside its update;
the command line's `/commands` act on the rig the grid or the Topology
view shows and `/answer` on the Inbox's selected question
(`commands::parse_in`), as the TUI's; `/cols [n]` sets the grid's
columns; a mention's reply hint is a real command (`ainet chat send
--rig docs --topic general "…"`); the Topology table's line between
pods is a child, so the selection's left border keeps its colour.

### Phase 10 — Settings for all of ainet (asked for 2026-10-16)

"Sophisticated Settings to set up all aspects of ainet's workflow and
infrastructure." Today only the GUI's look is configurable
(`settings.json`, `keymap.json`); everything else is environment
variables, CLI flags, spec YAML or compile-time constants, and
`paths::config_file()` (`config.toml`) is never read. An inventory of
what exists and what is hard-coded was taken on 2026-10-16; the plan:

1. **One settings file for everything.** `~/.config/ainet/settings.json`
   (JSON with comments, as Zed's and the GUI's today) gains sections the
   daemon and the console read: `daemon`, `runtimes`, `permissions`,
   `messaging`, `queue`, `rigs`, `discovery`, `hooks`, `retention`,
   `cost`. The types live in ainet-core (MIT) with a JSON schema; the
   GUI's own sections stay in the GUI. Unknown keys are kept, bad values
   reported and the last good settings kept, as the GUI does now.
2. **Wire the daemon to it**, replacing constants:
   - *daemon*: autostart on/off, log level and file (appended, rotated),
     socket, restore rigs on start.
   - *runtimes* (Claude Code, Codex, shell): binary path, default model,
     effort (passed at last: it is parsed and dropped today), permission
     mode, extra args, environment, allowed/disallowed tools, extra MCP
     servers, `--strict-mcp-config`, readiness timeouts; skills installed.
   - *permissions*: how long a request is held, `/always` rules persisted
     with list/revoke, the sensitive path lists, spec `permission_policy`
     honoured (`locked` … `yolo`) where Claude Code allows it.
   - *messaging*: paste settle and flush delays, delivery header, typing
     guard default per seat, hold while interacting.
   - *queue and watchdogs*: default interval and message, nudge default.
   - *rigs and specs*: user spec directories, default cwd, restore policy,
     snapshot on down, snapshot retention.
   - *discovery*: auto-track Claude sessions on/off, tmux scanning on/off.
   - *hooks*: install/uninstall/repair ainet's hooks in Claude Code's
     settings (the existing unused installer), status shown.
   - *retention*: in-memory caps, event log and snapshot pruning, inbox
     expiry.
   - *cost*: price table overrides, context window guesses.
   Applied live where possible (a daemon reload on change), otherwise
   marked "takes effect for new seats" or "after a daemon restart".
3. **Protocol**: additive requests to read the effective settings and
   their sources, to set values, to list and revoke `/always` rules, to
   see hook status and install hooks, and to list the paths in use.
4. **GUI Settings view** (`workspace::OpenSettings` opens it; the file
   stays available): Zed-style, a searchable category list on the left
   (Appearance, Terminal, Keymap, Workspace, Daemon, Runtimes,
   Permissions, Messaging, Queue, Rigs & Specs, Discovery, Hooks,
   Retention, Cost, Diagnostics) and fields on the right: toggles,
   dropdowns, numbers with units, paths with Browse, list and key-value
   editors; each with its description, default, a modified dot, reset to
   default, and when it applies. Edits write the file keeping comments.
   Diagnostics shows paths, versions, daemon pid and log tail. A keymap
   editor lists bindings by context and records new ones.
5. **Console parity**: `ainet config get|set|unset|list|path|edit` and
   the TUI's `/set`.
6. **Tests**: parsing and defaults, every wired value changing
   behaviour (daemon tests), protocol round trips and old-daemon
   fallback, GUI headless tests per category, comments preserved.

**Done: items 1 and 2, part one.** `ainet_core::settings` (MIT) has the
whole shape of the ten sections with serde defaults equal to the old
constants (a test pins each), a JSON schema (`schemars`, composed into
the GUI's published one), checks of ranges and closed lists, unknown keys
reported, `Follower` (keep the last good file) and `effective` (each
value with its default and source: default, file or an environment
variable, which wins). `ainet_core::jsonc` reads JSON with comments and
sets or removes one key in the text (the GUI's `json_edit`, moved).
`paths::settings_file()`; `config_file()` (`config.toml`) is gone.
ainetd loads the file at start, polls it every second on a blocking
thread and puts a change in force at once; its log says what is set and
from where. Wired: `daemon` (autostart in `ainet_model::daemon`, log
level, file, append and size rotation for the autostarted daemon, socket,
`restore_rigs`), `messaging` (all), `queue` (all; `QSpec.nudge` and a
watchdog's interval may now be left to the daemon), `rigs` (all;
`ainet down` reads `snapshot_on_down`), `discovery` (all), `retention`
(all), `cost` (context windows in the Fleet; prices build an
`ainet_claude::cost::Table`, which no view shows yet), and of `runtimes`
only `binary` and `readiness`. Left for part two: the rest of
`runtimes`, `permissions`, `hooks`. A shutdown no longer saves the
seats it kills as stopped (their exits came after the save), so
`restore_rigs` sees what was running.

**Done: item 2, part two** (`runtimes`, `permissions`, `hooks`). Seats'
command lines are built in one pure place (`ainet_daemon::runtimes`, on
`ainet_claude::launch::LaunchOptions`, which gained
`--disallowedTools`); a test keeps the old builder word for word and
checks that with nothing set every runtime gets exactly its old
arguments. Wired: per runtime the model, effort (`--effort`; Codex
`model_reasoning_effort`), Claude Code's permission mode, more arguments
and environment (ainet's own variables refused), allowed tools
(`mcp__ainet` always first) and disallowed tools, more MCP servers merged
into `--mcp-config` (Codex: `-c mcp_servers.<name>.<key>=…`) with
`--strict-mcp-config`, and the shell's arguments and environment. Codex
has no permission modes or tool lists, so setting them for Codex is an
error. A spec's `effort` is still dropped unless `runtimes.<rt>.spec_effort`
is on: `--effort` is not confirmed for Claude Code 2.1.289 and an unknown
flag would stop the seat; an effort set in settings.json or asked for in a
request is passed. `SeatSpec` and `SpawnSpec` gained `effort`,
`permission_mode`, `env` (and `SeatSpec` `args`), skipped on the wire when
unset; OpenRig defines none of them on members, so only
`permission_policy` maps from a spec. `permissions.hold_secs` sets the hold,
the seats' `PermissionRequest` hook timeout (hold + 20, 600 by default) and
`ainet hook`'s wait (hold + 10). `/always` rules are kept per agent in the
database with `persist_always` (turning it off forgets them), listed and
revoked with the additive `Request::AlwaysList` / `AlwaysRevoke`
(`Response::AlwaysRules`; an old daemon answers "bad request"), and
`global_rules` (the same four kinds, checked strictly) cover every agent.
`sensitive_paths` add names or paths `/always` and global rules never
cover; the built-in lists always apply. With `spec_policies`,
`builtin:locked` is `plan`, `builtin:default` and `builtin:standard`
Claude Code's default, `builtin:yolo` `bypassPermissions`; others are
reported and ignored, and `up --plan` shows each seat's mode. `ainet
install-hooks` prints the block made from the installer's `SPECS` (it
used to differ: no `AskUserQuestion` matcher, other timeouts), and
`--write`, `--status`, `--remove` install, check and remove it in Claude
Code's settings (`hooks.claude_settings`), keeping a backup of each
change. Not done: installing a spec's skills. Claude Code reads skills
only from `~/.claude/skills`, a project's `.claude/skills` or a plugin;
`--plugin-dir` with a generated plugin would avoid your files but is not
confirmed for 2.1.289, so skills stay resolved and unused. Left for the
protocol step: reading and setting values, paths, and hook status and
install over the socket (the console's `install-hooks` already does the
last two locally).

**Done: items 3 and 5** (protocol and console parity). Additive requests,
each answered "bad request" by an older daemon and handled by clients
falling back: `SettingsGet { key? }` (`Response::Settings`: every setting
with value, default, source, description and when it applies, `live`,
`new_seats` or `restart`; the file's error and unknown keys),
`SettingsSet { key, value }` and `SettingsUnset { key }` (ainetd edits the
file through `ainet_core::settings::set_in_file`, keeping comments, refuses
unknown keys, groups and values that would not read, and puts the file in
force at once), `SettingsReload`, `HookStatus` / `HookInstall` /
`HookUninstall` (`Response::Hooks`, with the backup made), and `Paths`
(`Response::Paths`: socket, state directory, database, log, settings
file, spec directories, Claude Code's config directory, the `ainet`
binary, version, protocol and pid). `AlwaysList` / `AlwaysRevoke` came
with part two. The console: `ainet config get|set|unset|list
[--changed]|path [--all]|edit` (through a running ainetd, else on the file
directly; an older daemon is said to be older and the file is used),
`ainet always list|revoke` (revoking all needs `--all`) and `ainet hooks
status|install|uninstall`; `--json` everywhere. The TUI and the GUI's
command line: `/config [key]` (a popup of the settings in force), `/set
<key> <value>`, `/unset <key>`, in `ainet_model::command`. `ainet-model`
gives the GUI `Link::config` (`Msg::Config`, `ConfigReply`), the fallback
to the file for an older daemon, and `Demo::full` answering all of it
from a pretend file. Values typed as words become the JSON a setting
takes (text unquoted; the schema decides), and a value that loosens
safety is said to be less safe as it is set. Left: item 4, the GUI
Settings view, and item 6's GUI tests.

## 6. Risks

| Risk | Mitigation |
|---|---|
| GPUI is pre-1.0 and its API moves weekly; crates.io copy is stale | pin one Zed rev; upgrade deliberately (procedure in `docs/GUI.md`); all GPUI code stays in `crates/gui` and `crates/ui` |
| Zed's `[patch.crates-io]` does not propagate through git deps | mirror the needed patches in ainet's root manifest at each upgrade |
| Option A: GPL obligations, internal `ui` API with no stability promise, globals expecting Zed's `settings` | choose B (recommended), or accept GPL for `ainet-gui` only |
| Large dependency tree, long first build | GUI outside `default-members`; console build unaffected; `mold` already suggested in README |
| Full-screen ANSI refetch too slow with many visible terminals | phase 6 streaming, throttling, visible-only fetch |
| `unsafe_code = "forbid"` workspace lint | GPUI application code needs no `unsafe`; keep the lint |
| Text input without Zed's editor | small `TextInput` on `EntityInputHandler`; multi-line only where needed (compose box) |
| Wayland/X11/Vulkan environment issues on user machines | document packages; GPUI falls back to X11; `--demo` for quick checks |

## 7. Rough total

Phase 0–2: ~2 weeks · Phase 3: 2 days (A) or 1–2 weeks (B) · Phases 4–7:
~5–6 weeks. Roughly **7–9 weeks** of focused work for parity with the TUI
plus the Zed-specific features above, with a usable GUI (fleet, terminals,
inbox) at the end of phase 4 step 4.

## 8. Decisions to make before starting

1. ~~Component library~~: **decided, A** — Zed `ui`/`theme` from a pinned
   rev; `ainet-gui` is GPL-3.0-or-later.
2. Binary names: `ainet` stays the console binary (recommended, see §3) with
   an optional `ainet-cli` symlink, GUI as `ainet-gui`; or rename.
3. Zed rev to pin: latest Zed stable release tag at the time phase 1 starts.
