# ainet GUI: a Zed-style desktop front end

Plan for adding a native GUI to ainet, built on Zed's UI stack (GPUI and
Zed's component library) and following Zed's look and interaction model,
while the console version (CLI + TUI) stays exactly as it is.

Status: **phase 4 done** (the Fleet panel with
critters and the agent menu, the Agent panel, the live terminal with
Tile all, the Inbox with approval cards, questions and the glowing
waiting badge, Comms with threads and a compose box, the bottom dock's
Queue, Chatroom and Events panels, the Map with rig clusters and
messages in flight, transcripts, specs and queue items as
report tabs; review fixes applied, with `ctrl-o`, `ctrl-shift-y` and the
Release and Remove seat entries; see [GUI.md](GUI.md)), phase 5 next;
phase 3 done (option A: panes, splits, docks,
modals, toasts, the text input and the picker); phase 2 done (themes,
fonts, settings and keymaps); phase 1 done (`crates/gui`: the GPUI
skeleton on Zed v1.24.2); phase 0 done (`crates/model`). Written 2026-10-15 against commit `c0dbb5a`.

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
| `daemon` | ~4.6k | ainetd: PTYs, rigs, mailbox, chat, queue, hooks | unchanged; optional streaming addition in phase 6 |
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
  `backend::fetcher`. Phase 6 adds an optional streaming path.

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
(`~/.config/ainet/themes/*.json`) are read at start-up but not reloaded on
change (phase 5, with the theme selector), and `settings.json` has no JSON
schema yet (phase 7: without an editor in the app, a schema only helps in
an external editor that is told about it).

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
right, bottom; toggled, dragged; sizes in memory until phase 7), the
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
key-down events), forwarding the mouse to programs that ask for it,
colours in scrollback (ainetd sends history as plain text), `--view
terminals`/`rig:` tiling at start.

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
directs whom, a menu on nodes, keeping the view across sessions.

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

### Phase 7 — Persistence, polish, packaging · ~1 week

Workspace restore (docks, tabs, splits, focused agent); window bounds;
`confirm_quit`; desktop entry and icon (`ainet-gui.desktop`); `just
install-gui`; optional AUR `PKGBUILD` split into `ainet` and `ainet-gui`;
macOS build check (GPUI's Metal backend) without promising support yet.

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
