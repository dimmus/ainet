# ainet-gui: the desktop app

`crates/gui` (`ainet-gui`) is built on GPUI and Zed's `ui` and `theme`
crates, taken from Zed's git repository and pinned to one release. This
guide covers how it is built against Zed (the pin, the workspace of its
own, how to upgrade), what each part of the app does and which keys it
has, how it is tested, and what is known to be missing. The plan it was
built by, and the reasons behind it, are in [GUI-PLAN.md](GUI-PLAN.md).

Contents: [the pin](#the-pin) · [a workspace of its own](#a-workspace-of-its-own) ·
[the workspace pieces](#the-workspace-pieces) · [Fleet and Agent panels](#the-fleet-and-agent-panels) ·
[terminals](#the-terminal) · [the Terminals grid](#the-terminals-grid) ·
[the command line](#the-command-line) · [the Topology view](#the-topology-view) ·
[the Inbox](#the-inbox) · [Comms](#comms) ·
[the bottom dock](#the-bottom-dock-and-reports) · [the Map](#the-map) ·
[palette and modals](#the-command-palette-pickers-and-modals) ·
[restore and packaging](#workspace-restore-packaging-and-macos) ·
[settings and keys](#settings-themes-and-keys) · [tests and CI](#tests-and-ci) ·
[known gaps](#known-gaps) · [upgrading Zed](#upgrading-zed)

## The pin

| | |
|---|---|
| Zed release | **v1.24.2** (latest stable on 2026-10-15) |
| Commit | `fb69067729cb16a7f6135e2c1600c26a28c59659` (see `crates/gui/Cargo.lock`) |
| Crates used directly | `gpui`, `gpui_platform` (features `font-kit`, `wayland`, `x11`), `theme`, `ui`, `ui_input` (`InputField`), `menu` (the list actions), `fuzzy` (the pickers' matcher), `settings_content` (the theme JSON schema only); `gpui` with `test-support` as a dev-dependency |
| Zed's toolchain | Rust 1.98.1 (`rust-toolchain.toml` at the tag); `crates/gui` declares `rust-version = "1.98"`, the console crates keep 1.88 |
| Licences | `gpui*` and its support crates Apache-2.0; `ui`, `ui_macros`, `ui_input`, `theme`, `syntax_theme`, `icons`, `component`, `menu`, `fuzzy`, `settings_content` (with `settings_json`, `settings_macros`, `language_model_core`) GPL-3.0-or-later, so `ainet-gui` is GPL-3.0-or-later (THIRD_PARTY_NOTICES.md) |
| Assets | Zed's `assets/themes` (One, Ayu, Gruvbox), `assets/icons` (top level), `assets/fonts` (IBM Plex Sans, Lilex) at the tag, copied into `crates/gui/assets` |

All Zed dependencies in `crates/gui/Cargo.toml` (and the dev-dependency)
name the same `tag`; never mix tags or revisions.

## A workspace of its own

`crates/gui` is a Cargo workspace by itself, with its own `Cargo.lock`; the
root workspace (the MIT console crates) lists it under `exclude`. Cargo
resolves a whole workspace even to build one package, so if the GUI were a
member, building `ainet` would fetch Zed's repository and the patched git
crates below. As it is, `cargo build`, `cargo install --path crates/ainet`
and `just check` never see Zed, offline or vendored.

So GUI commands name its manifest, from the repository root:

```bash
cargo build --manifest-path crates/gui/Cargo.toml
cargo run   --manifest-path crates/gui/Cargo.toml -- --demo
cargo test  --manifest-path crates/gui/Cargo.toml
```

(or run plain `cargo build` etc. inside `crates/gui`). `.cargo/config.toml`
points both workspaces at the root `target/`, so `ainet` and `ainet-gui`
land side by side (where `ainet gui` looks) and the shared crates are not
built twice. `crates/gui/Cargo.toml` repeats the root's lints and profiles,
since a workspace reads its own; keep them in step.

## Patches copied from Zed

Cargo applies `[patch.crates-io]` only from a workspace's root manifest,
so the patches Zed builds with do not follow its crates into ours.
`crates/gui/Cargo.toml` copies the ones GPUI's dependency tree needs, from
Zed's root `Cargo.toml` at the tag:

| Crate | Source | Used by |
|---|---|---|
| `async-process` | `github.com/zed-industries/async-process` @ `0b6d6713570af61806e1e5cb40e0f757cb93fd9d` | `smol`, `zbus` (GPUI's desktop portal client) |
| `async-task` | `github.com/smol-rs/async-task` @ `b4486cd71e4e94fbda54ce6302444de14f4d190e` | GPUI's executors, `async-executor` |
| `calloop` | `github.com/zed-industries/calloop` @ `3759371fee14c40066d64777e2a36b6ffcc22590` | `gpui_linux`: the X11/Wayland event loop |

Zed patches more (`tree-sitter-language`, `livekit`, `libwebrtc`,
`webrtc-sys`, `windows-capture`, `notify`, `notify-types`, `scratch`); none
of them is in `ainet-gui`'s tree today. When a new Zed crate is added, check
`cargo tree --manifest-path crates/gui/Cargo.toml -i <crate>` for each of
them and copy the ones that appear.

The patches live in the GUI's workspace only, so the console's build and
`Cargo.lock` are unchanged by them.

## What we implement instead of Zed's crates

- `theme::ThemeSettingsProvider`: Zed registers it from its `settings`
  crate; ainet registers its own (`crates/gui/src/settings.rs`). `theme`
  panics without one.
- Client-side decorations and the title bar: ported from
  `workspace::client_side_decorations` and `platform_title_bar` (both pull
  in Zed's whole workspace, so they cannot be dependencies).
- Assets: `crates/gui/src/assets.rs` embeds `crates/gui/assets` with
  `rust-embed` (also in debug builds, so the binary never reads the source
  tree; Zed's own `assets` crate reads its checkout in debug builds).
- Loading themes: Zed does it in `theme_settings`, which needs Zed's
  `settings` crate and with it Zed's fs, git and wasmtime. The few hundred
  lines that turn theme JSON into `theme::Theme` (`refine_theme_family` and
  the colour fallbacks of `schema.rs`) are copied into
  `crates/gui/src/zed_theme.rs`; the JSON types come from
  `settings_content`, which is light.
- Settings: `crates/gui/src/settings.rs` reads ainet's own
  `settings.json` (below) instead of Zed's settings store.
- Keymaps: `crates/gui/src/keymap.rs` reads Zed's keymap format the way
  `settings::keymap_file` does (sections, `null` to unbind, partial loads
  of a broken file), without the settings crate.
- The workspace: Zed's `workspace` crate (panes, splits, docks, modals,
  notifications) pulls in its project, fs, git and client crates, and
  `picker` and `editor` need `workspace`. The pieces are written small
  for ainet instead, after Zed's (next section).
- The text field: Zed's is its `editor` crate. `text_input/` is a small
  one on GPUI's `EntityInputHandler`, and backs `ui_input::InputField`
  through its `ErasedEditor` hook.

## The workspace pieces

Written after Zed's `workspace` and `picker` crates at the tag, with the
same components (`ui::Tab`, `TabBar`, `ContextMenu`, `PopoverMenu`,
`ListItem`, `HighlightedLabel`, `Tooltip`, `KeyBinding`) and the same
behaviour where ainet has the feature. Pure logic stays testable without
GPUI (the pane tree, divider dragging, the text buffer).

| File | What | After Zed's |
|---|---|---|
| `item.rs` | `Item` (tab label, icon or critter slot, tooltip, `can_split`/`clone_on_split`, `activated`/`deactivated`/`on_removed`, `ItemEvent::CloseItem`/`UpdateTab`), `ItemHandle`, and `Placeholder`, the stand-in tab ("Fleet") an empty workspace shows | `workspace::item` |
| `pane.rs` | `Pane`: a `TabBar` of `Tab`s (click activates, middle click or the hover close button closes, right click for Close/Close Others/Close All/Split), a split menu and a zoom button; closing the active tab activates the one used before it; `PaneEvent`s for the workspace | `workspace::pane` |
| `pane_group.rs` | `PaneGroup<P>`: the split tree (generic, so tested with numbers), neighbours by drawn position, and the axis element whose dividers drag (double click evens them) | `workspace::pane_group` |
| `dock.rs` | `Dock` (left, right, bottom) holding one or more `Panel`s and showing one (`activate_panel`), opened and closed, resized by dragging its inner edge (double click: default size); a panel names its `toggle_action` and may put a count on its button (`icon_label`) | `workspace::dock` |
| `panels.rs` | the panels' title strip and state colours, and the bottom panels' strip with a tab per panel (`bottom_header`) | — |
| `modal_layer.rs` | one modal at a time; escape, a click outside or the modal's `DismissEvent` closes it and gives the keyboard back | `workspace::modal_layer` |
| `toast_layer.rs` | the fleet's notices as toasts, bottom right: flashes for 5 s (the TUI's flash time), requests waiting on you for 10 s (with the agent's critter and a Review button), errors for 15 s, reports (titled) until dismissed, their text in the buffer font scrolling under a title row that keeps the close button; at most 3. The empty flash a quiet request (a key, a resize, `Interact`) gets back is dropped by `FleetModel`, which also re-renders only for batches that changed something (output alone does not) | status toasts, notifications |
| `text_input/` | `TextInput`: one line or several (soft-wrapped, `max_lines`), IME, mouse and key selection, clipboard, word moves, undo/redo, placeholder, masking, read-only; `InputEvent::{Edited, Submitted, Cancelled, Blurred}`; `erased.rs` registers it as `ui_input`'s editor | GPUI's `examples/input.rs`, `ui_input::ErasedEditor` |
| `picker.rs` | `Picker<D: PickerDelegate>` in the modal layer: query input, fuzzy matches with highlighted characters (Zed's `fuzzy`), `menu::*` keys, an optional footer; the delegate hears every dismissal (a click outside too) | `picker` |
| `switcher.rs` | the agent switcher (`ctrl-p`): picking opens the agent's terminal (`Workspace::open_terminal`), the secondary pick (`ctrl-enter`) only selects it | `file_finder` |

The workspace (`workspace.rs`) ties them together: the Fleet panel docks
left (or right, `fleet_panel.dock`) with the Agent panel opposite and the
Queue, Chatroom and Events panels at the bottom, all open at start; the
status bar has a button per panel at its ends, as Zed's (the one showing
is lit and closes its dock; another shows its panel). Each panel has a
`ToggleFocus` action in a namespace of its own, as Zed's
(`queue_panel::ToggleFocus`, …): it opens the dock on that panel and gives
it the keyboard, or, if it has the keyboard already, gives it back to the
center. Dock sizes are kept per panel (the Fleet panel's starts from
`fleet_panel.width` and follows that setting until dragged) and saved
with the open tabs and splits (see "Workspace restore"). Moving the
Fleet panel to the other side moves both side panels with their sizes and
open state.

Keys (Linux; macOS has the `cmd` equivalents), in the keymaps:

| Keys | Action |
|---|---|
| `ctrl-tab` | the tab switcher (the latest used first; let go of `ctrl` to switch) |
| `ctrl-shift-tab`, `ctrl-pagedown`, `ctrl-pageup` | previous, next, previous tab |
| `ctrl-w` | close the tab |
| `ctrl-\`, `ctrl-alt-\` | split right, down |
| `ctrl-alt-left/right/up/down` | the pane in that direction (Zed's are `ctrl-k` chords, which a terminal needs); in the Terminals grid, the tile that way first |
| `shift-escape` | zoom the active pane, or put it back |
| `ctrl-b`, `ctrl-alt-b`, `ctrl-j` | left, right, bottom dock |
| `ctrl-shift-c`, `ctrl-shift-i`, `ctrl-t`, `ctrl-shift-t` | Comms, Inbox, Map, Topology tabs |
| `ctrl-y`, `ctrl-a`, `ctrl-n` | allow, always allow, deny (outside the Inbox: open it on the oldest request) |
| `ctrl-p` | the agent switcher |
| `ctrl-shift-p` | the command palette |
| `ctrl-;` | the command line (above the status bar), and back |
| `ctrl-o` | the operator's terminal, to type into; with no operator running, starts one (`command::operator_up`), as the TUI's |
| `ctrl-shift-y` | select the next agent with a request waiting on you (`Store::next_waiting_agent`, round again) |
| `escape`, `enter`, `up`, `down`, `home`, `end` | `menu::*`, for menus, pickers and modals (`ctrl-enter` in a picker: `menu::SecondaryConfirm`) |

`workspace::ActivateNextPane`/`ActivatePreviousPane` and
`pane::CloseOtherItems`/`CloseAllItems` have no default keys. In a text
field (`TextInput` context) the usual editing keys apply (`ctrl-z`,
`ctrl-shift-z`, `ctrl-backspace`, `ctrl-left`, …); `enter` submits and
`escape` cancels, and with several lines (`TextInput && multiline`)
`shift-enter` starts a new line, as in Zed's agent panel. An empty field
also has `empty` in its context, for keys that do something else there.

Testing: the pieces have headless tests (`#[gpui::test]` with
`VisualTestContext`): tabs, their right-click menu, splits and focus,
zoom, dock resizing by a simulated drag, the status bar's dock buttons,
the switcher modal, toasts expiring on the test clock,
text editing through the platform input handler, and the picker.
`crate::tests::init_test` sets the app up as `main` does, without the
user's config; `demo_workspace`, `sent` and `modal::<V>` there are the
shared helpers for a window on the demo, the requests it sent, and the
modal open.

## The Fleet and Agent panels

The first views on top of the workspace pieces. They read the same
`Store` as the TUI and send what the TUI sends, through `ainet-model`:

| File | What | After Zed's |
|---|---|---|
| `fleet_panel.rs` | the Fleet tree in the Store's (the TUI's) order, virtualised (`uniform_list`, every row one height): rigs with a `Disclosure`, status dot, running/total seats, open queue items and summary; agents with their critter, name, role in their colour, `directs N`, a `CountBadge` of requests waiting on you, an envelope with unread messages, the state `Indicator`, task and the TUI's meta line (age, runtime, model, context); empty seats; the loose groups' headers. Who directs whom is drawn with indent guides; folded rigs are skipped by the keys. The selection is the Store's: a new one (from the switcher, a click elsewhere) unfolds its rig and scrolls to it. A right click (or `shift-f10`) opens the agent's `ContextMenu` | `project_panel` |
| `agent_panel.rs` | the TUI's details pane: a big moving critter, name, role chip, "you › … › name", task and last tool, model and cwd, state, the context gauge gliding a quarter of the way each tick (`ProgressBar`, warning past 60 %, error past 80 %), the activity sparkline, the facts table, the last messages to or from it and its open queue items; the empty den when nobody is selected | — |
| `critter.rs` | `Critter`, the avatar element: a sprite as quads, one per run of same-coloured pixels, snapped to whole device pixels at any size; `Critter::agent` hops while working and is a still grey ghost once gone. Colours: the palette's own for Deepharness and Omarchy (exactly the TUI's), else the theme's player colour | — |
| `terminal_item.rs` | `TerminalItem`, an agent's tab (critter in the tab), opened by `Workspace::open_terminal(id)`: one per agent, reused wherever it is open; the live terminal of the next section | `terminal_view` |
| `message.rs` | the message modal (`m`): what `command::message` sends for `@Name text` (through `command::message_to`, so always to the modal's agent, a leading `@word` being part of the text), delivered through the typing guard and kept in the mailbox | — |

The agent's menu and keys, all handled by the workspace for the selected
agent so the menu shows their bindings:

| Menu entry | Key in the Fleet | Sends (as the TUI's command) |
|---|---|---|
| Message… | `m` | `Message { to: [id], force: true }` (`@Name text`) |
| Open Terminal | `enter`, double click | — (opens the tab) |
| Show Transcript | | `Transcript { agent }`, shown as a report tab |
| Capture Screen | | `Capture` (`/capture`) |
| Relaunch | | `Launch { fresh: true }` (`/relaunch`) |
| Continue | | `SeatContinue` (`/continue`) |
| Typing Guard (checked when on) | | `TypingGuard` (`/guard`) |
| Release Seat | | `Release` (`ainet release`): the process runs on outside the rig |
| Remove Seat… | | `RemoveSeat` (`/remove`), after asking |
| Close Agent… | `delete` | `Close` (`/close`), after asking |

`up`/`down` (or `j`/`k`, and `alt-up`/`alt-down` anywhere) move the
selection, `left`/`right` fold and unfold the selected agent's rig. Seat
entries are disabled for agents outside a rig. `s` opens the spawn
modal (`fleet::Spawn`). The requests come from
`ainet_model::command::for_agent(store, id, AgentCommand)`, which the
TUI's typed commands share.

Animation: `FleetModel` ticks every 120 ms (`fleet::TICK`, the TUI's) also
when following ainetd, emitting `fleet::Tick` without re-rendering. A view
that draws critters keeps a `critter::Animation`, passes it to each
`Critter::animate`, and on `Tick` calls `cx.notify()` only if
`animation.take()` says one was painted since: critters in a closed dock,
an inactive tab or rows scrolled away cost nothing.

## The terminal

An agent's terminal is a tab, `TerminalItem`, showing the copy of its
screen ainet keeps (a `vt100::Parser` per agent in the `Store`), streamed
from ainetd (`Request::Attach`: the screen, then what the program
prints, fed into the copy as it comes) or, from a daemon that
cannot stream and in the demo, filled by `Request::Screen { formatted:
true }` as the TUI's. ainetd owns the PTYs, so there is no alacritty and
no local process: the GUI paints the screen, sends keys as `Input` and
sizes as `Resize`.

| File | What | After Zed's |
|---|---|---|
| `terminal_item.rs` | the tab: critter, name and a state dot; a strip with the seat address, state, "Typing · messages held" while focused, the scroll position and a message button (`fleet::MessageAgent`); keys, mouse, menu and actions | `terminal_view::TerminalView` |
| `terminal_element.rs` | `TerminalElement`: measures a cell of the terminal font, fits rows × columns in its bounds, paints backgrounds, selection, text and cursor; rows shaped once and kept while they show the same (`ShapedRow` in the item's `RowCache`) | `terminal_view::TerminalElement` |
| `terminal_layout.rs` | the pure part: a screen as merged background rectangles and runs of text of one style (bold, italic, underline, inverse, dim; 16, 256 and true colours through the theme's terminal ANSI colours), wide characters, selections and their text, scrollback laid out as a screen (with its colours when ainetd sends them), `row_keys` and `RowCache` | `layout_grid`, `convert_color` |
| `terminal_input.rs` | a GPUI `Keystroke` as the bytes xterm sends, through `ainet_model::terminal::encode_in` (the TUI's table) | `terminal::mappings::keys` |

What it does:

- **Painting.** The font is `terminal.font_family`/`font_size` with
  ligatures off; a cell is the advance of `m` wide and `font_size ×
  line_height` high. Backgrounds other than the terminal's are merged
  along each row; text is shaped per run of equal style with the cell
  width forced, so columns line up; a wide character ends its run.
  Colours: the theme's sixteen terminal ANSI colours, xterm's 6×6×6 cube
  and grey ramp for the rest of the 256, true colour as is; `inverse`
  swaps (defaults included), `dim` keeps 70 % of the colour, as Zed's.
- **Cursor.** `terminal.cursor_shape` (`block`, the default, `bar`,
  `underline`, `hollow`, as Zed's); a block redraws its character in the
  background colour. Shown only while the terminal has the keyboard and
  the program has not hidden it; blinks every 500 ms with
  `terminal.blinking: "on"` (the default; `off` and Zed's
  `terminal_controlled` keep it still, since ainetd does not pass a
  program's blink request on), and typing restarts the blink.
- **Size.** When the element's rows × columns change and then hold for
  150 ms (`RESIZE_AFTER`), the agent's pane is asked to take them with one
  quiet `Resize` (dragging a divider asks once, at the end); a streamed
  screen comes anew at that size, a fetched one is fetched again. The
  demo draws its screens at the asked size. An agent shown in two places
  at once (its tab and a tile of the Terminals grid, or a split) has one
  pane, so one size: the view **shown or given the keyboard last** owns
  it (`FleetModel::claim_size`, from `show_terminal` and on focus), and
  the others paint the screen at that size. The owner's layout is asked
  for at once if it was laid out, else at its next layout; the others'
  layouts are kept but not sent, so a window resize sends one `Resize`,
  not one per view in turn. When the owner hides, a view still on show
  takes the size over with its own.
- **Fresh screens.** A tab on show (active in its pane) registers with
  `FleetModel::show_terminal`, which streams the agent's terminal
  (`Link::attach`, one stream per agent however many tabs show it,
  closed when the last one hides). Without streaming (a daemon from
  before it, an adopted tmux pane, the demo), `Event::Output` for its
  agent marks the screen stale, and stale screens are fetched at most
  every 66 ms each (`ainet_model::Pump`, the TUI's pump, which the TUI
  now uses too). A stream that ends (ainetd went, the program ended)
  falls back to fetching until the next snapshot tries again. Hidden tabs
  cost nothing; in the demo, shown screens move with the clock.
- **Drawing only what changed.** A new screen (or bytes for one) does not
  notify the fleet: it emits `ScreenChanged(agent)`, once per batch, and
  only the terminals showing that agent redraw. Panes and dock panels are
  cached views (`AnyView::cached`, as Zed's pane group), so a terminal
  that redraws does not redraw the other panes or the panels. In the
  element, each row is keyed by what it shows (`row_keys`: a hash of
  its formatted cells and the columns drawn) and laid out and shaped only
  when the key is new; rows are kept for one more frame, and a new
  palette or font (the theme, the font settings) shapes them all anew.
- **Keys.** While the terminal has the keyboard, every key the keymap
  leaves alone goes to the agent as `Input`: the `Terminal` context
  unbinds the workspace's ctrl letters and the list keys (`escape`,
  `enter`, arrows, `home`, `end`), and the item's key-down listener
  encodes the rest as xterm does (application cursor keys when the screen
  asks, `alt` as an `ESC` prefix, `shift-enter`/`alt-enter` as Claude
  Code's newline). Typing goes back to the live screen.
- **Typing guard by focus.** When a terminal gets the keyboard, ainetd is
  told `Interact { on: true }` and holds the agent's messages so they never
  land under what you type; when it loses it, `on: false` lets them
  through. This replaces the TUI's typing mode and `ctrl-]`. The fleet
  tells ainetd 50 ms after the focus settles (`INTERACT_SETTLE`), so moving
  between two views of one agent says nothing; quitting tells it at once.
  A (re)connected ainetd has forgotten all this, so on `Connected(true)`
  it is told again, and a snapshot refetches the screens on show.
  Opening a terminal whose tab exists (from the Fleet, the Inbox's
  terminal button, `ctrl-o`) focuses the tab itself, not its pane.
  A terminal that gets the keyboard also becomes the Fleet's selection.
- **Mouse.** A left drag selects (shift-click extends), `terminal::Copy`
  copies it, `terminal::Clear` drops it. The wheel scrolls back through the
  scrollback: the first step asks for it (`Screen { history: n,
  formatted: true }`, in chunks of 200 rows), which then shows in place of
  the screen with "N lines back" in the strip, in its colours from a
  daemon that sends them (`styled_rows`), as plain text from one that
  does not. Scrollback shorter
  than a screen is believed for 2 s (`SHORT_HISTORY_FOR`), so a trackpad
  gesture asks once, not per event. A selection reaching past the screen
  (the grid drawn before a resize takes, or a pane that keeps its size)
  copies only what is on it. On the alternate screen (full-screen
  programs) the wheel sends arrow keys instead, as xterm. Right click: Copy, Paste, Clear Selection, Send Ctrl-C,
  Message…, Zoom.
- **Paste.** `terminal::Paste` sends the clipboard bracketed
  (`ESC [200~ … ESC [201~`) when the screen asked for bracketed paste,
  else with its line ends as the Enter key sends them.
- **Every terminal at once.** `fleet::TileAll` (`ctrl-g`) opens the
  Terminals grid, a tab of its own (next section), instead of splitting
  panes.

Keys in a focused terminal (Linux; macOS uses `cmd-c`, `cmd-v`, `cmd-k`):

| Keys | Action |
|---|---|
| `ctrl-shift-c` | `terminal::Copy` |
| `ctrl-shift-v`, `shift-insert` | `terminal::Paste` |
| `ctrl-shift-l` | `terminal::Clear` (the selection) |
| `shift-pageup`, `shift-pagedown` | `terminal::ScrollUp`, `ScrollDown` (a page of scrollback) |
| `shift-escape` | `terminal::ToggleZoom` |
| everything else | typed into the agent |

`terminal::SendCtrlC` is in the menu (`ctrl-c` itself goes to the agent).
Leaving a terminal by keyboard: `ctrl-alt-left/right/up/down` (other
panes), `ctrl-tab` (the tab switcher), `alt-up`/`alt-down` (the Fleet's
selection).

Tests: `terminal_layout` (colours, attributes, merging, wide characters,
selections, scrollback with and without colours, row keys, the row cache
finding and forgetting rows, a resize or another palette laying rows out
anew), `terminal_input` (the TUI's key table as GPUI keystrokes), `fleet`
(output fetches at most so often, one `Resize` per size, streamed
terminals never fetched until their stream ends and streamed again after
a snapshot, changed screens announced once and not notified), and
headless `terminal_item` tests: only the terminal whose screen changed
redrawing, one changed row shaped again and a new theme shaping all,
coloured scrollback, the demo's screen painted,
focus sending `Interact` on and off (and nothing between two views of one
agent), keys reaching the agent past the workspace's bindings, one
`Resize` after the window is dragged through several sizes, the wheel
asking for scrollback and typing coming back, selection and copy,
bracketed paste, `SendCtrlC`, the right-click menu driven by its keys
(which reach the menu, not the agent). In tests the window must be
activated (`window.activate_window()`) for focus events to fire.

## Keeping terminals cheap: measurements and streaming

Measured first, without a display, then changed where the numbers said.
Two measurements, both `#[ignore]`d tests run by hand:

```sh
cargo test -p ainet-model --release --test terminal_cost -- --ignored --nocapture
CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test --manifest-path crates/gui/Cargo.toml \
    layout_cost -- --ignored --nocapture
```

They run on `ainet_model::sample`, a busy Claude Code screen (prose,
tool calls, diffs on coloured backgrounds, the spinner and input box)
and `sample::frame`, what one redraw of its spinner writes. Numbers from
the 2-core development machine (release build):

| Screen | formatted screen | `Screen` reply line | ainetd: format + encode | front end: decode + parse | one spinner frame streamed | front end: decode + feed |
|---|---|---|---|---|---|---|
| 24×90 | 1.8 KB | 3.6 KB | 47 µs | 35 µs | 1.1 KB | 12 µs |
| 50×200 | 5.6 KB | 10.9 KB | 203 µs | 153 µs | 1.9 KB | 18 µs |

With every terminal busy (ainetd says `Output` every 50 ms per pane, the
pump fetches each at most every 66 ms):

| Terminals | fetches/s | refetching, 24×90 | refetching, 50×200 | streaming, 24×90 | streaming, 50×200 |
|---|---|---|---|---|---|
| 1 | 15 | 52 KB/s | 160 KB/s | 22 KB/s | 37 KB/s |
| 4 | 60 | 207 KB/s | 637 KB/s | 89 KB/s | 149 KB/s |
| 8 | 120 | 415 KB/s | 1.3 MB/s | 178 KB/s | 299 KB/s |
| 16 | 239 | 830 KB/s | 2.5 MB/s | 357 KB/s | 598 KB/s |

(streaming counted at a frame every 50 ms per terminal, all sent). So a
refetch moves the whole screen to show a spinner turning: 4–6 times the
bytes of the output itself, and 8–9 times the front end's work per
update; for 16 busy 50×200 terminals that is about 37 ms of the UI
thread and 48 ms of ainetd per second, against about 6 ms and next to
nothing streamed, and every screen is up to 66 ms late on top. The
refetch was the cost worth removing, hence streaming.

Laying a screen out in the element (`terminal_layout`, optimised, before
any shaping, which needs a real text system and is not measured):

| Screen | whole layout | row keys | a frame through the row cache, spinner moved |
|---|---|---|---|
| 24×90 | 189 µs (110 runs) | 34 µs | 62 µs (1 row laid out) |
| 50×200 | 564 µs (303 runs) | 159 µs | 186 µs (1 row laid out) |

Keys were first a hash of each cell through `Screen::cell`, which walks
to the row on every call and cost as much as the layout; they are now
read from `Screen::rows_formatted` in one pass. Shaping, the larger cost
on a real display, is what the cache saves most: only rows whose key is
new are shaped.

**Streaming** (`Request::Attach { agent }`, added without bumping
`protocol::VERSION`):

- ainetd (`crates/daemon/src/stream.rs`) answers with the formatted
  screen (`Response::Screen`), then every chunk the program prints as
  `Response::Event { event: Event::Bytes { agent, data } }`, on that
  connection only (never broadcast: older subscribers would not parse
  it). A character split between two reads is held for the next (never
  two U+FFFDs). The whole screen comes again after a resize, when the
  client fell 256 chunks behind (the pane's tap drops what does not fit
  and the stream resyncs), once the output pauses for 300 ms, and at
  least every 5 s of continuous output, which also mends anything the
  client's parser took differently. It ends when the client closes the
  connection (detached) or the program ends (ainetd closes it).
- The pane (`ainet_pty::Pane::attach`) taps its reader under the same lock
  as the parser, so the screen handed over and the bytes after it never
  overlap or leave a gap. An attach after the program ended (before
  ainetd has dropped the pane) gets the last screen and the end at once,
  so the connection closes instead of waiting for output that never
  comes; dropping the pane ends its attachments too.
- An agent ainetd has no pane for (an adopted tmux pane) gets an error
  and the connection stays an ordinary one.
- Old clients never send it. A new client and an old daemon: the daemon
  cannot parse the request and answers `bad request: unknown variant`,
  the link remembers that until it reconnects (`Link::attach` then
  returns `None`), and the GUI fetches screens on `Output` as before.
- `Link::attach`/`detach`/`ended` (a thread per stream, closed with
  `shutdown`), `Msg::Detached { id, stream }` and `Change::Detached`; the
  stream's number keeps a late end of an old stream from ending a new
  one. `Store` feeds `Event::Bytes` into the agent's parser. The TUI keeps
  fetching (`Change::Detached` is no news to it); its behaviour is
  unchanged.

**Colour scrollback.** `Screen { history: n, formatted: true }` now also
answers `styled_rows` from ainetd's own panes: each row as text with SGR
sequences from the default attributes (`ainet_pty::styled_row`, written
for this: vt100's `rows_formatted` positions the cursor absolutely and so
cannot be moved to another row). Old daemons leave it out (plain rows, as
before); old clients never ask with `formatted` and get the reply they
always did. The GUI draws them on a screen wide enough not to wrap,
painting its first columns.

**Critters.** Already only visible ones animate: a view redraws on the
fleet's tick only if it painted a critter since the last one
(`critter::Animation`), so hidden panels and tabs cost one flag check a
tick. With cached views, the redraw a tick asks for repaints only that
view.

Tests: `ainet-pty` (an attachment keeps an exact copy of the screen,
resizes and lag resync, dropping detaches, attaching after the end or
dropping the pane ends the attachment, styled rows draw the same
anywhere), `ainet-daemon` (`tests/attach.rs`, a real ainetd: screen then
bytes, a screen once settled and after a resize, closing either end
detaches, a refused attach leaves the connection usable, coloured
scrollback only when asked; `stream` unit test: characters split between
reads), `ainet-core` (the wire shapes old peers see), `ainet-model`
(`tests/attach.rs` against pretend daemons: streaming into the store
until detached, an old daemon's refusal and the fallback to fetching, a
stream that ends; `Store` feeding bytes and keeping styled history), and
the GUI tests listed under the terminal.

## The Terminals grid

`TerminalsItem` (`terminals.rs`) is the TUI's Terminals view (`^G`) as a
tab: every live agent's terminal at once, in rows and columns, like
OpenRig's terminal wall. It is opened by `fleet::TileAll` (`ctrl-g`; an
open grid is activated and keeps its filter, a new one shows every rig),
`--view terminals`, `--view rig:NAME` (and `pod:RIG/POD`, which shows the
rig), `--view <rig>`, and `/view terminals`, `/view rig:NAME` or
`/view <rig>` in the palette or Comms (those set the filter). Tile by
tile:

- **Tiles.** One per agent of `Store::terminal_ids(rig)`, in the TUI's
  order (rig, seat, start), each a `TerminalItem` the grid owns: the same
  header (critter, name, seat, state, "Typing · messages held", the
  message button), live screen, scrollback, selection, menu and typing as
  a terminal tab. The grid follows the fleet: an agent that starts gets a
  tile at its place, one that goes loses it (as a closed tab; the
  keyboard, if it was there, goes to the grid). The grid calls the item
  hooks a pane would: `activated` for every tile on show while its tab
  is, `deactivated` when the tab is hidden or a tile is zoomed away,
  `on_removed` when a tile or the grid goes, so the fleet streams or
  fetches exactly the tiles on show and nothing while the grid is hidden.
- **Columns.** `terminal::grid_cols` from the grid's own room in terminal
  cells (measured by a `canvas` behind the tiles, as the workspace
  measures itself): the count that keeps tiles closest to a terminal's
  shape; rows of that many tiles, the last row's tiles wider. `+` and `-`
  fix one more or one fewer (`terminal::more_cols`, `fewer_cols`: at
  least one, at most one per tile), `=` lets the shape pick again, as in
  the TUI. The strip above the tiles says how many terminals and columns
  there are and has Fewer, More and Auto buttons.
- **Rig filter.** `ctrl-r` (`terminals::CycleRig`, the TUI's `^R`, through
  `terminal::next_rig`, which the TUI now uses too): all, then each rig in
  name order, then all again; the strip's rig button does the same. The
  tab reads "Terminals · docs" while filtered.
- **Keyboard.** A click on a tile gives it the keyboard: typing goes to
  that agent and ainetd holds its messages (`Interact`), as in a terminal
  tab; the focused tile's border is the theme's `border_focused`.
  `ctrl-alt-left/right/up/down` (the workspace's pane keys) move from tile
  to tile (`terminal::grid_step`; going down onto a short last row lands
  on its last tile) and, past the grid's edge, on to the pane that way.
  `ctrl-alt-g` (`terminals::FocusGrid`) leaves a tile for the grid itself,
  where `enter` (or a `ctrl-alt` arrow) goes back to the tile last used
  (outlined in `border_selected` meanwhile); a click on the strip does the
  same as `ctrl-alt-g`. Escape is the agent's.
- **Zoom.** `terminal::ToggleZoom` in a tile (`shift-escape`, or Zoom in
  its menu) makes that tile fill the grid, not the pane; the others stop
  being fetched until it is put back (`shift-escape` again, or the strip's
  button). Moving with `ctrl-alt` arrows while zoomed zooms the next tile,
  as the TUI's arrows do. On the grid itself `shift-escape` zooms the pane
  as anywhere else.
- **Size.** Each tile asks its agent's pane for its own size as a tab
  does; an agent also open in a tab of its own follows the rule under
  "The terminal" (the view shown or focused last wins).
- **Opening a tiled agent.** `fleet::OpenTerminal` (enter in the Fleet,
  the switcher, the Inbox's terminal button) for an agent with no tab of
  its own, while a grid on show has its tile, gives that tile the
  keyboard instead of opening a tab.
- **Saved** with the workspace as `{ "kind": "terminals", "rig": …,
  "cols": … }` (both left out when unset).

Cost: the tiles share one pane, and GPUI redraws a view's whole subtree
when anything in it changed (an ancestor redrawn renders its cached
children anew too), so a tile whose screen changed paints the other
tiles again; their rows come from their row caches (keys and lookups,
no shaping). Panes side by side (a split of terminal tabs) still redraw
alone.

Keys (contexts `Terminals`, the grid and its tiles, and `Terminals &&
grid`, only while the grid itself, not a tile, has the keyboard, so the
bare keys never fire while you type in a tile):

| Keys | Where | Action |
|---|---|---|
| `ctrl-g` | anywhere but a terminal | `fleet::TileAll`: open the grid |
| `+` (`shift-=`), `-`, `=` | the grid | `terminals::MoreColumns`, `FewerColumns`, `AutoColumns` |
| `ctrl-r` | the grid | `terminals::CycleRig` |
| `enter` | the grid | back into the tile last used (`menu::Confirm`) |
| `ctrl-alt-left/right/up/down` | a tile, the grid | the tile that way (past the edge, the pane) |
| `ctrl-alt-g` (`cmd-alt-g` on macOS) | a tile | `terminals::FocusGrid` |
| `shift-escape` | a tile | `terminal::ToggleZoom`: the tile fills the grid, or not |

Tests (headless, `terminals.rs`): `ctrl-g` tiling the demo's seven agents
in the TUI's order in rows (every tile painted and kept fresh, one tab
however often it is opened); `ctrl-r` going round the rigs, the tab's
title, only the filtered tiles kept fresh, the rig button, and `ctrl-r`
typed into a tile reaching its agent; `+ - =` and the columns drawn (and
typed into a tile, the agent's); a click giving a tile the keys with
`Interact`, typing and escape reaching its agent, `ctrl-alt` arrows
between tiles with ainetd told once the focus settles, `ctrl-alt-g` and
enter, and stepping out of the grid into the next pane; a new screen
shaping only its tile's rows; zooming a tile (only it fetched, moving
zooms the next, the menu's Zoom); a hidden grid fetching nothing on
output and a closed one letting every tile go; tiles coming and going as
agents start and stop (the keyboard back on the grid); the latest view of
an agent owning its size (the tab beside the grid, then a click in the
tile, then a window resize sending one `Resize`); `OpenTerminal` focusing
a tile; `--view rig:infra` and a rig's name opening the grid. Also
`persist.rs` (the grid restored with its rig and columns, its tiles on
show), `persistence.rs` (its saved form), `fleet.rs` (size ownership),
`keymap.rs` (no grid key reaching into a tile: every ctrl letter, the
list keys and `+ - =` stay the agent's), `command_palette.rs` (`/view
rig:infra`, `/view docs`, `/view terminals`), and in `ainet-model`
`terminal::{more_cols, fewer_cols, next_rig, grid_step}`.

## The command line

`CommandLine` (`command_line.rs`) is OpenRig's `cmd ▸` line: a strip
across the window above the status bar, there all the time
(settings.json `"command_line": false` hides it; it then shows only while
it has the keyboard). It has the prompt (`cmd ▸`, the arrow in the accent
colour while it has the keyboard), a one-line `TextInput`, and beside it
what enter will do, or the hint "/command · @Name message · text to the
selected agent · Tab completes" with its key while nothing is typed.

A line means what it means in the TUI's input line, through the shared
command language (`command_line::command_for`):

- **`/command`**: `commands::parse` and `commands::run`, as in the palette
  and Comms: a request through the fleet (long text as a report tab), the
  console's own commands as the app's actions (`/view inbox` opens the
  Inbox, `/allow` opens it on the oldest request, …). While typing the
  name the strip lists the commands it may be (`/sp`: `/spawn  /spec
  /specs`); once it is whole, its usage and what it does (`/grow <pod>
  <member> [runtime] [ROLE] · add a seat to the rig`), from
  `ainet_model::command::COMMANDS`.
- **`@Name text`**: that agent (`command::message`, delivered through the
  typing guard, kept in its mailbox); the strip says `to orch-lead@docs`.
- **Anything else**: the selected agent (`command::message` with the
  selection as the default; a click in a terminal or a grid tile selects
  its agent, so this is also the TUI's focused tile). When the active tab
  is the Inbox and its selected item is a question waiting on you, the
  line answers it instead (`command::answer_question`, through
  `InboxItem::waiting_question`), as the TUI's line does in its Inbox;
  the strip says `answers orch-lead@docs's question`. `@Name` lines
  still message there.

Done, the line clears and keeps the keyboard, as a shell's does (a
command that opens a view, `/view inbox`, gives that view the keyboard).
Wrong, what is wrong shows beside it in red and as the usual toast
(`commands::run` now returns it), and the line stays to be put right;
editing it clears the red. A command with nothing to act on (`/close`
with nobody selected) says "nothing to act on" there only.

`tab` completes an `@name` as the TUI's (`command::complete`) or a
`/command` name (`command::complete_command`: the whole name and a space,
or as far as several names agree, `/snap` to `/snapshot`). `up` and `down`
recall the lines run before, newest first, and past the newest what was
being typed; the same line twice in a row is kept once, and the last 50
(`persistence::COMMAND_HISTORY`) are saved with the workspace
(`"command_history"`, left out while empty).

`ctrl-;` (`command_line::ToggleFocus`; `cmd-;` on macOS) gives it the
keyboard from anywhere but a terminal (a terminal's keys are the
agent's; the keymap unbinds it there), remembering where the keyboard
was; escape, or `ctrl-;` again, gives it back there (to the active pane
if that view is gone). A click into the line remembers it the same way.
The line sits outside every view, so the bare-letter keys of the Fleet,
the Inbox, Comms and the grid never fire while you type in it.

Keys (context `CommandLine`, around its field):

| Keys | Where | Action |
|---|---|---|
| `ctrl-;` (`cmd-;` on macOS) | anywhere but a terminal | `command_line::ToggleFocus` |
| `enter` | the line | run it |
| `escape` | the line | the keyboard back where it was |
| `tab` | the line | `command_line::Complete` |
| `up`, `down` | the line | `command_line::PreviousLine`, `NextLine` |

Tests: pure ones for the history (recall, the line being typed kept, the
same line once, the last 50), what each kind of line becomes (the
selected agent, `@Name`, a question shown, `/commands`, nobody selected)
and the previews; headless ones for each kind of line sending what the
TUI sends through the demo link (plain text to the selected agent with
its toast, `@Name`, `/ps`, `/view inbox` giving the Inbox the keys, a
question shown in the Inbox answered, `@Name` still a message there),
errors in the line and as toasts (no such command, no such agent, no such
view, a usage) and cleared by editing, "nothing to act on", `tab` on
`@names` and `/command` names, `up` and `down`, `ctrl-;` and escape from
the Fleet, the Inbox and after a click, a terminal keeping `ctrl-;`,
escape after the view it came from closed, letters typed in it reaching
no other view's keys, and the setting hiding it until it has the
keyboard. Also `persist.rs` (its history restored), `persistence.rs` (the
saved field left out while empty), `keymap.rs` (`ctrl-;` and `cmd-;` stay
a terminal's) and, in `ainet-model`, `command::{COMMANDS, command_named,
commands_starting, complete_command, complete_line}` (every listed
command understood by `command::parse`, by name, its usage the one a
usage error gives).

## The Topology view

`TopologyItem` (`topology.rs`) is OpenRig's TUI topology as a tab, opened
by `workspace::OpenTopology` (`ctrl-shift-t`, `cmd-shift-t` on macOS; the
palette lists it as "workspace: open topology"), `--view topology` or
`--view topology:RIG`, and `/view topology` or `/view topology:RIG` in
the palette, Comms or the command line (a rig that is not there is said,
as an error). An open one is activated. It starts on the selected
agent's rig, else the first rig, else the loose agents; the tab reads
"Topology · docs".

Left, the **explorer**: TOPOLOGY, this host (`gethostname`), each rig
(its status dot, running/seats), its pods in the order the rig lists
their seats (seat counts; the tooltip says how many run) and, when there
are any, the loose agents (live/all). A click on a rig, pod or the loose
agents shows it on the right (`ctrl-r` goes round the rigs and the loose
agents); the root and the host choose nothing.

Right, five tabs (Zed's `ToggleButtonGroup`), with the scope's status
and name beside them:

- **Table**: Zed's `Table` (virtualised) of the seats grouped by pod, the
  pod's name on its first row and a line above each pod after the first:
  Pod, Seat (the member, in its agent's colour), RT (`cl`, `cx`, `sh`),
  Model, Ctx (context in use, or `—`), State (the agent's, coloured as in
  the Fleet; a seat with no agent running shows the seat's status:
  stopped, failed, pending), Q (open queue items owned by the seat or
  addressed to it), Work (its item in progress; the tooltip has the
  summary), Now (its task, else its last tool). Loose agents are rows of
  their own, oldest first. Above, the filter (`/`): every word must be in
  the row's seat, pod, address, model, runtime or state, any case; enter
  gives the table the keyboard, escape clears the filter and then leaves
  it, and up and down move through the table while you type. Below, the
  summary, OpenRig's words, over all the scope's rows: "5 seats · 3
  working · 1 need attention · 4 open rows" (needing attention: needs
  you, failed, or a request waiting on you; open rows: the seats' open
  queue items, each once), with "· N shown" while filtering. A click
  selects a row and its agent (the Fleet and Agent panels follow); enter
  or a double click opens the agent's terminal (`fleet::OpenTerminal`; an
  empty seat says so); a right click (or `shift-f10`) gives the Fleet's
  agent menu (`fleet_panel::agent_menu`, now shared).
- **Recent**: the queue's recorded changes for the scope, oldest to
  newest: when, `#n`, who, the state after it (the Queue panel's chips),
  the note and the item's summary, the item. Each item's creation (ainetd
  does not note it; the demo does), then each note (ainetd notes every
  change of state); an item whose state moved without a note gets one
  change more at its last update, so the list ends where each item is.
  The newest is selected; enter or a double click shows the item in full
  as a report (`QueueShow`, as the Queue panel's Show).
- **Overview**: the rig's status and summary, spec file (or none), root,
  age, seats running of seats in pods, rows per state, edges, open queue
  items; for the loose agents, how many live.
- **Graph**: the Map of the scope alone (`MapItem::set_scope`, through
  `map::layout_where`), made the first time it is shown and following the
  scope; its keys and mouse are the Map's.
- **Health**: the checks it makes, listed first (seats without a running
  agent, failed agents, requests waiting on you, working agents with no
  hook event for 5 minutes, typing guards on and messages held), then
  what they found, worst first, or "No findings from these checks." It
  never says more than that.

Everything shown is `ainet_model::topology`, pure and tested: `tree`,
`rows` and `SeatRow` (its cells, `matches`, `needs_attention`),
`filtered`, `summary` (and its words), `changes`, `overview`, `health`
and `CHECKS`, `Scope` (`admits`, `exists`, `title`), `default_scope` and
`next_scope`. The view keeps its rows in step with the fleet; saved as
`{ "kind": "topology", "rig": …, "pod": …, "loose": true, "view": … }`
(each left out when unset or the table).

Keys (contexts `Topology`, and `Topology && view` only while the view
itself, not its filter or its graph, has the keyboard):

| Keys | Where | Action |
|---|---|---|
| `ctrl-shift-t` (`cmd-shift-t` on macOS) | anywhere | `workspace::OpenTopology` |
| `/` | the view | `topology::FocusFilter` |
| `tab`, `shift-tab` | the view | `topology::NextView`, `PreviousView` |
| `1` … `5` | the view | `topology::ShowTable`, `ShowRecent`, `ShowOverview`, `ShowGraph`, `ShowHealth` |
| `ctrl-r` | the view | `topology::CycleRig` |
| `up`, `down`, `home`, `end` | the view, the filter | the table's or Recent's selection (`menu::*`) |
| `enter` | the view | the row's terminal; on Recent, the item in full |
| `enter`, `escape` | the filter | back to the table; clear, then back |
| `shift-f10` | the view | the agent menu (`fleet::OpenContextMenu`) |

Tests: in `ainet-model`, the demo's tree, the docs rig's rows and their
cells by pod, an empty seat as a stopped row still owed its item, the
filter, the summary and its words, Recent (the demo's creations and
moves, a claim noted, a move without a note, an item ainetd made, a pod
and a rig with no queue), the overview, health (only the waiting request
on the demo; a stopped seat, a failed agent, a quiet one and a guard),
the scopes and `map::layout_where`. Headless (`topology.rs`): the demo's
tree drawn and the docs table grouped by pod with its summary and tab
title, one tab however often opened; the tree and `ctrl-r` choosing the
scope; the filter narrowing the rows while digits, `/`, tab and `ctrl-r`
stay its letters, enter and escape; enter and a double click opening the
row's terminal with the Fleet's selection following; the right-click
menu sending the Fleet's request, `shift-f10` and escape; the tabs by
keys and actions, the Graph's map of the rig following the scope, Health
clean on infra and not on docs; Recent's seven changes, enter showing an
item as a report, a click and `home`; `--view topology:infra`, `/view
topology:docs` and a wrong rig; an empty fleet. Also `persist.rs` (back
on its pod and tab) and `persistence.rs` (its saved form).

## The Inbox

`InboxItem` (`inbox.rs`) is the TUI's Inbox as a tab, opened by
`workspace::OpenInbox` (`ctrl-shift-i`), `--view inbox`, the waiting
badge in the title and status bars, a toast's Review button, or the
answer keys from anywhere else. It is a table above a detail pane, so it
stays readable with many requests: Zed's virtualised `Table` (the Queue
panel's) lists who asks (critter and name in its colour), the kind's chip
(the TUI's `kind_chip`: permission, plan, question, finished, failed;
faded once answered), what it asks and how long ago. Below it, the
selected item in full, drawn like the tool-call approval cards of Zed's
agent panel (`agent_ui`'s `render_tool_call`): a faint header with the
critter, name, role, chip and age; the summary; the whole detail in a
block that scrolls past 18 rem (a permission's command or edit in the
buffer font, `+`/`-`/`@` lines coloured as the TUI's; a plan as light
markdown: headings, lists, fenced code); and, while it waits on you,
**Allow**, **Always** and **Deny** with their key hints, plus a button to
the agent's own terminal. Failed items get a dashed border, as Zed's
failed tool calls. A double click on a row opens the agent's terminal.

Talking back: every item but a waiting question (which has its own
answer field) has a field for your words to its agent (`r`, or enter on
the row). While the request waits, enter there **denies it with your
reason**: `Request::Answer` carries an optional `message` (left off the
wire when empty, so older daemons and the TUI see the request they
always did) and ainetd hands it to Claude Code as the deny message
("Denied from ainet by your human: use cargo nextest"; for a plan, "Your
human sent the plan back from ainet: …"), so the agent knows what to do
instead. Once the item no longer waits, the same field sends the agent a
message (`command::message_to`). `Store::deny_with` builds the deny.

Order: requests waiting on you first, oldest first (where the keys go);
with **All** (`h`), the rest follows (answered requests, finished and
failed reports, questions sent as messages), newest first. The selection
is an item's id; when the selected request is answered it moves on to the
next one waiting.

Questions (`AskUserQuestion`) show their options as Zed's `ChoiceCard`s,
radio or checkbox by `multi_select`; digits `1`–`9` pick them while the
Inbox has the keyboard, as the TUI's. A field takes your own words;
enter (or **Answer**) sends them if typed, else the numbers picked
(`command::answer_choices`, "1 3"), through `command::answer_question`.
A click anywhere in the Inbox (its buttons too) gives it the keyboard,
and a field that goes when its item is answered hands the keyboard back
to the table.
**Leave to Terminal** (`ctrl-n`) denies it as the TUI's `^N` does.
Permissions and plans go through `Store::answer`, so the request and what
the toast says (`allowed Lead's permission`) are the TUI's.

Keys (the TUI's): `ctrl-y` allow, `ctrl-a` always, `ctrl-n` deny. In the
Inbox they answer the selected item; anywhere else (but a terminal, which
takes every ctrl letter) they first open the Inbox on the oldest request
waiting, the selected agent's first (`Store::next_waiting`), so you read
before you allow. `up`/`down` (`home`/`end`) move between rows, `enter`
is `inbox::Answer` (a question's answer, else the reply field), `r`
(`inbox::ReplyToAgent`) goes to the reply field, `h`
(`inbox::ToggleHistory`) shows everything or only what waits, `escape`
leaves a field. `r` and `h` are bound in `Inbox && cards`, a context the
Inbox adds only while it (not one of its fields) has the keyboard, so
typing in a field never triggers them. There are no single-letter answer
keys: a stray key must never allow a command.

Attention: an item that asks for you (`Change::InboxAdded { alert: true
}`) becomes a toast with the agent's critter and the TUI's flash text
(`Store::inbox_alert`: "name: summary") and a Review button, and the
waiting badge in the title bar and status bar glows three times
(`with_animation`, 1.8 s, keyed by `FleetModel::alerts()` so each new
request starts it again), like the TUI's glowing Inbox tab. The Inbox's
tab shows a bell and the waiting count.

Tests: pure ones for the order, the detail styles, picking and the glow;
headless ones for the keys opening the Inbox first and then answering
through the demo link (the card resolves on `InboxResolved` and the
selection moves on), items answered by selection and a click on their
row, the Allow, Always and Deny buttons answering their own item, cards
keeping their buttons in a short window, a deny with a reason, a message
from a history item, the Waiting / All filter, questions
(digits, several or one, your words over picks, escape, deny, the
keyboard back on the cards after an answer by text), a click giving the
Inbox the keys, `ctrl-shift-y`, `--view inbox`, and
the alert's toast and glow count.
`FleetModel::receive` (tests only) applies link messages directly.

## Comms

`CommsItem` (`comms.rs`) is the TUI's Comms stream as a tab, opened by
`workspace::OpenComms` (`ctrl-shift-c`, which gives its compose box the
keyboard) or `--view comms`. The queue the TUI shows beside it is the
bottom dock's Queue panel.

- **The stream.** `Store::entries` in its order: messages and chat
  posts interleaved, oldest first (`comms::rows` builds the same order
  from ids without cloning, plus open threads). A `gpui::list`, since
  rows differ in height: only rows on show are drawn, and when the
  Store changes only the rows that changed are spliced in
  (`comms::splices`: first the rows that dropped off the front, as the
  Store at its cap of 2000 drops its oldest for each new one, then what
  changed in the rest), so the scroll position holds. A message shows its
  sender's critter (or a person icon for you), `from → to` in the
  agents' colours (the accent for you), `#id`, `↳ #id` for a reply, a
  **held** chip (the typing guard holds it) or the TUI's unread dot (not
  delivered, or not read; tooltip says which), how long ago, and the
  whole body wrapped: fenced code in the buffer font, `@name`s in the
  named agent's colour (`comms::mentions`, `comms::blocks`). A chat post
  shows its poster, a `#topic rig` chip and the body. Nobody has said
  anything yet: the TUI's words.
- **Sticking to the end.** The list follows its end (`FollowMode::Tail`,
  as Zed's agent panel): at the bottom, what arrives keeps it there;
  scrolled up (the wheel), or with a row above the last selected, it
  stays where you read; sending something, or selecting the last row,
  brings it back down. `comms::sticks` is the decision, tested on its
  own; gpui's list notices the wheel and scrolling back down.
- **Threads.** A message with replies has a `Disclosure` with the count;
  open (a click, or enter on it), its replies and theirs
  (`Store::replies`, depth first) follow it, indented a level each with
  indent guides. They come from the Store; no `Request::Thread` is
  needed. A reply selected in a thread is answered as itself.
- **Compose.** A multi-line `TextInput` at the bottom (enter sends,
  `shift-enter` a new line, at most 6 lines before it scrolls), with what
  a line will do above it and a send button. It is the TUI's input line
  in Comms (`command::comms`, which the TUI now uses too): `@Name text`
  messages that agent (`command::message`, delivered through the typing
  guard); otherwise it answers the selected entry (`command::reply`: a
  threaded reply to a message, a post in a chat's topic); with nothing
  selected it says "select a message to answer, or start with @Name" and
  keeps the words. `/commands` go through `command::parse` (`/chat`,
  `/broadcast`, `/queue`, …); the console's own (`/view`, `/allow`,
  `/quit`, …) become the app's actions. `tab` completes `@Mos` as the
  TUI's (`command::complete`). As in the TUI, the newest entry is
  selected when Comms opens, so a line answers it; the × by "Answering
  #4 …" (or escape in the stream) makes it a new message instead.

Keys (contexts `Comms`, the stream with the keyboard, and
`CommsCompose`, around the compose box's field):

| Keys | Where | Action |
|---|---|---|
| `up`, `down` | the stream, or the compose box while empty | select entries (`menu::*`) |
| `home`, `end` | the stream | the first, the last entry |
| `enter` | the stream | open or close the selected message's thread; with none, answer it |
| `c` | the stream | `comms::Compose`: a new message (no selection, the box starts with `@`) |
| `r` | the stream | `comms::Reply`: answer the selected entry (the newest if none) |
| `escape` | the stream / the box | clear the selection / back to the stream |
| `tab` | the box | `comms::Complete` |
| `enter`, `shift-enter` | the box | send, new line |

`comms::Compose` from anywhere else opens Comms to write one.

Tests: pure ones for the rows (the Store's order with the demo,
interleaved, threads under their message), splicing, the sticking
decision, mentions and code blocks; headless ones for the stream as the
Store's entries following a new post, lines sending exactly what the
TUI's input line sends (reply, chat post with a second line, `@Name`,
nothing selected, `/chat`), `tab` completion and the keys moving through
the stream from the box and the stream, `c` and `r`, reading back
holding the stream while posts arrive until you send, a thread opening
under its message and closing onto the reply's own row, and a click
selecting a row and giving the stream the keys.
`ainet-model` gained `Store::replies` and `command::comms`, with tests.

## The bottom dock and reports

The bottom dock holds three panels, switched by the tabs in their title
strip or their buttons in the status bar (`Workspace::toggle_panel_focus`):

| File | What |
|---|---|
| `queue_panel.rs` | `QueuePanel`: the queue as Zed's `ui::Table` (virtualised): id, a state chip, priority, `source → destination` in the agents' colours, summary, how long since it moved; the TUI's order (`Store::queue_items`: in progress, blocked, pending, done, canceled, latest first), open items only or all (`queue::ToggleOpenOnly`, the Open/All buttons). Chips in the TUI's colours (`Palette::qstate`, shared with the TUI) in ainet's own themes, else the theme's status colours (`queue_panel::state_colors`). The status bar button counts the open items |
| `queue_form.rs` | `QueueFormModal`: the forms for a note, a handoff (to, body) and a block (on, then): enter goes to the next field, then sends; what is missing is said in the form, which stays open |
| `chatroom.rs` | `ChatroomPanel`: one rig's chatroom on one topic (`Store::chat_rooms`: every rig's `general` plus every topic posted in; a dropdown picks one, else it follows the selected agent's rig), posts oldest first with the poster's critter, following the newest (the list tracks the room's post ids and splices what changed, `comms::splices`, so a post dropped at the Store's cap, from this room or another, keeps the rows and a reader's place right); a box (it takes the keyboard when the panel is shown) posts `ChatSend` there as you (`command::chat`) |
| `events_panel.rs` | `EventsPanel`: what happened, one line each, newest last: agents joining, leaving and changing state, rigs, inbox items and answers, messages, chat, queue items, the connection. `FleetModel` logs a line (`ainet_model::journal::happened`, read off each message before the Store applies it, so a change says what it was before) and keeps the last `KEEP_EVENTS` (500); the list drops what the log drops (`events_panel::moved`) |
| `report.rs` | `ReportItem`: a reply that is long text, as a tab: the buffer font in a read-only `TextInput` (select with the mouse or keys, `ctrl-a`, `ctrl-c`), scrolling, with a Copy All and an Ask Again button; "Loading…" until the reply, the daemon's error if it fails |

The queue item's menu (right click, or `shift-f10` with the panel's
keyboard) and the actions behind it, each sending what `ainet queue …`
sends when you run it (`by` none), through `command::queue`:

| Entry | Action | Sends |
|---|---|---|
| Show (`enter`, double click) | `queue::Show` | `QueueShow`, opened as a report |
| Claim | `queue::Claim` | `QueueClaim` |
| Unclaim | `queue::Unclaim` | `QueueUnclaim` |
| Pending, In Progress, Done, Canceled | `queue::MarkPending`, … | `QueueUpdate { state }` |
| Add Note… | `queue::AddNote` | `QueueUpdate { note }` |
| Hand Off… | `queue::Handoff` | `QueueHandoff { to, body }` |
| Block On… | `queue::BlockOn` | `QueueBlock { on, continuation }` |

Reports instead of toasts: `FleetModel::run` hands a request whose reply
is long text (`fleet::is_report`: `Transcript`, `SpecShow`, `Export`,
`QueueShow`) to the workspace as `ReportWanted`, wherever it came from
(the Fleet's Show Transcript, the Queue's Show, `/spec` or `/export` typed
in Comms). The workspace opens a `ReportItem` in the active pane (or asks
again in the one of the same title) and the item asks for its text with
`FleetModel::ask(request, on_reply)`: a new `Link::ask(req, token)` in
`ainet-model` sends it, and its reply comes back as `Msg::Reply { token,
reply }` (`Change::Reply`), the text (`popup::reply`: reports rendered as
the TUI's popups, a queue item as `ainet queue show` prints it) or the
error, to that caller only, never as a flash or popup. The TUI never asks,
so it is unchanged. Short results stay toasts.

Keys: the `QueuePanel` context has `up`/`down` (select), `enter`
(`queue::Show`) and `shift-f10` (the menu). The panels' `ToggleFocus`
actions have no default keys (`ctrl-j` opens and closes the dock).

Tests: pure ones for the queue's rows (the TUI's order, open or all) and
the events list's splicing; headless ones for the chips in the TUI's
colours, every menu entry and form sending what `ainet queue` sends (and
the forms giving the keyboard back), the menu by right click and
`shift-f10`, the filter, a post in another room going to its rig and
topic, the log capped with the newest last (and the demo's chat post in
it), the three panels and their focus actions, and reports: Show
Transcript opening a tab with the demo's transcript (reused when asked
again, its text copied with `ctrl-a ctrl-c`), `/spec docs` and the queue's
Show opening theirs, `/export nope` showing the error, and no toasts.
`ainet-model` gained `Link::ask`/`Msg::Reply`/`Change::Reply`,
`popup::reply` and `popup::qitem_text` (moved from the console's output,
which calls it), `command::queue`/`QueueCommand` and `command::chat`
(which `/chat` now uses), `Store::queue_items` (the TUI's order, which the
TUI now uses), `Store::chat_rooms`/`chat_in`, `Palette::qstate` (the TUI's
chip colours), the `journal` module, and the demo answers transcripts,
specs, exports and the queue requests (the app's `Demo::full` only; the
console's demo is unchanged); all with tests.

## The Map

`MapItem` (`map.rs`) is the TUI's Map (`draw_map`) as a tab, opened by
`workspace::OpenMap` (`ctrl-t`, the TUI's key), `--view map` or `/view
map` in Comms. It is one `canvas`: the clusters, edges and dots are
painted, and the cluster titles and the nodes are ordinary elements laid
out in the canvas's prepaint where the layout puts them, so they are
Zed's components (labels, `Indicator`, tooltips) with their own mouse
handling, clipped to the tab.

- **Layout.** `ainet_model::map::layout(store, width, height)`, pure and
  UI-agnostic: each rig with live agents is a cluster, the loose agents
  another, last. Inside a cluster the agents are in layers by depth, each
  layer spread across the width and the layers spread down, as the TUI
  lays out its Map; an agent sits under the first agent of its cluster
  that directs it, children under their director. Clusters are packed in
  rows, as wide as their widest layer needs and as tall as their layers;
  every number of columns is tried and the one that lets the clusters
  grow most is kept, so a wide tab puts the rigs side by side and a tall
  one stacks them. Nodes are 132 × 88 (a critter, name, role, state) and
  shrink alike (`Layout::scale`) when the tab is too small: nothing
  overlaps or leaves the box.
- **Edges.** Who directs whom: a rig's `delegates_to` (from directs to)
  and `spawned_by` (to directs from, drawn dashed) between seats with
  agents in them, then each agent's `parent` (also across clusters), once
  per pair. Each is a Bézier curve (`map::route`) from under the director
  to over the one it directs (or side to side when level) with an
  arrowhead, in the theme's `border` colour.
- **What moves.** The critters, on the fleet's tick; the TUI's work in
  flight on the same tick (`map::pulses`, now shared with the TUI): dots
  running down an edge to an agent at work in its colour, up from one
  that needs you in the warning colour; and every new message between two
  agents on the map, a dot in the sender's colour with a fading trail
  flying from sender to recipient along their edge (backwards for one
  going up it) or straight across, over a second (`map::FLIGHT`, eased),
  drawn frame by frame (`on_next_frame`) only while one is in the air.
  `map::Flights` is that state, pure: it notes the newest message seen,
  so messages from before the tab opened and edits of old ones (read,
  delivered) do not fly; at most 64 are in the air. Each snapshot
  (`FleetModel::snapshots` counts them) starts the watch anew, so the
  history ainetd's snapshot brings after the tab opened (`--view map`
  at start-up, or a reconnect) does not fly either.
- **Mouse.** Hovering a node lights it and its edges (the accent colour;
  the others fade); a click selects the agent (the Store's selection, so
  the Fleet and Agent panels follow), a double click opens its terminal.
  The wheel pans, as does dragging the background; `ctrl`-wheel (`cmd`
  on macOS) zooms about the pointer (25 % to 400 %). The view starts
  fitted to the tab; the strip above has the legend, the zoom and Zoom
  Out, Zoom In and Fit to View buttons. Nobody running: it says so.

Keys (context `Map`, the tab with the keyboard):

| Keys | Action |
|---|---|
| `up`, `down` | `fleet::SelectPrev`, `SelectNext` (the selection shows on the map) |
| `enter` | `fleet::OpenTerminal`, the selected agent's |
| `ctrl-=`, `ctrl-+`, `ctrl--`, `ctrl-0` | `map::ZoomIn`, `ZoomIn`, `ZoomOut`, `FitView` (Zed's font size keys; `cmd` on macOS) |

Tests: pure ones in `ainet-model` for the layout (the demo's two rigs and
seven agents, the lead above the three it directs, no overlaps and
inside the box at sizes from 1600 × 900 down to 90 × 60, side by side
when wide and stacked when tall, `spawned_by` pointing from the spawner,
loose agents in their own cluster, edges across clusters, a loop in the
edges), routes, and the pulses being the TUI's; pure ones in the GUI for
the flights (taking off once, only between agents on the map, easing,
landing after a second, at most 64, nothing a snapshot brings) and the camera (zooming about the
pointer, mapping back); headless ones for `ctrl-t` drawing every live
agent inside the map without overlaps, zoom and fit, hover, click and
double click, dragging to pan, a message sent through the fleet flying
and landing on the test clock, a late snapshot's history staying put,
and the empty fleet.

## The command palette, pickers and modals

All in the modal layer, on the picker and the text field above:

| File | What | After Zed's |
|---|---|---|
| `command_palette.rs` | `CommandPalette` (`ctrl-shift-p`): every action available where the keyboard was (`Window::available_actions`; `menu::*` left out), named in words as Zed's (`humanize`: `workspace::ToggleLeftDock` is "workspace: toggle left dock"), with its key binding there (`ui::KeyBinding::for_action_in`), fuzzy filtered (`normalize` lets `workspace::toggle_left` find it too), the ones run from it lately first (kept with the workspace). Picking one goes back where the keyboard was and runs it there. A line starting with `/`, or one that matches no action, is a console command: `commands::parse` (`command::parse`, no rig in view, so the selected agent's rig) says in its row what it does (`launching docs`) or what is wrong (the usage, `no command /x`); enter runs it, or stays open while it is wrong | `command_palette` |
| `commands.rs` | the console's command language for the app, shared with Comms and the command line (`run` returns what was wrong, also shown as a toast): requests through the fleet, the console's own commands (`/view`, `/allow`, `/quit`, …) as the app's actions. `/answer <text>` answers the question waiting on you (`commands::question`: the selected agent's first, else the oldest), as the TUI's answers the one selected. `/view fleet` focuses the Fleet, `/view terminals` opens the Terminals grid on every rig, `/view rig:<name>` and `/view <rig>` on that rig (`Workspace::show_rig_terminals`), whichever agent is selected; `/view topology` and `/view topology:<rig>` the Topology view (`Workspace::show_topology`); another name says there is no such view or rig | — |
| `up_modal.rs` | `fleet::Up`: `SpecPicker` over the library (`Request::Specs`, through the new `FleetModel::list`), with what is typed offered as a source of its own (a spec file's path, or a name); then `UpModal`: working directory (Browse… asks the system for a folder, `cx.prompt_for_paths`), rig name, fresh seats (`dev.impl, dev.review`). **Plan** shows what would launch (`UpSpec { plan: true }`, `FleetModel::ask`) in the form; **Launch** (or enter in the last field) sends `Up`. Both are `command::up` | `file_finder`, `ui::Modal` |
| `rig_modals.rs` | `fleet::SwitchRig` (the title bar's rig switcher, which names the selected agent's rig, `title_bar::rig_label`): a `RigPicker`; picking a rig selects its first live agent in the Fleet's order (`rig_modals::first_agent`), so the Fleet, Agent panel, Chatroom and the rig commands follow it. `fleet::Restore`: a `RigPicker`, then `RestoreModal`: the snapshots (`Request::SnapshotList`), the latest by default, picked by click or `up`/`down`; the rig's seats as checkboxes to start fresh; enter restores (`command::restore`). `fleet::Down`: `DownModal` asks about the selected agent's rig (or the only one; else a rig picker first), with **Take a snapshot first** checked; enter takes it down (`command::down`) | — |
| `seat_modals.rs` | `fleet::Spawn` (`s` in the Fleet): `SpawnModal`, the runtime (claude-code, codex, terminal), working directory (Browse…) and role, as `/spawn` (`command::spawn`). `fleet::Grow`: `GrowModal` for the selected agent's rig (else a rig picker): pod, member, runtime (or the rig's default), role, as `/grow` (`command::grow`); it says so when the pod or member is missing | — |
| `form.rs` | what the forms share: Zed's `InputField`s (over `TextInput`) where enter moves to the next field and sends from the last, `tab`/`shift-tab` round the fields (`form::NextField`, `PreviousField`, context `Form`), Browse…, the runtime buttons, the frame (`ui::Modal`, `ModalHeader`, `Section`, `ModalFooter`) | — |
| `theme_selector.rs` | `theme_selector::Toggle`: every registered theme, dark first, the one in use ticked and selected; moving previews it on the window, escape or a click outside puts the one in use back, picking it saves it (`settings::save_theme`) | `theme_selector` |
| `json_edit.rs` | sets one value in settings.json's text (comments, order and the rest stay as written), as Zed's `settings_json` does with tree-sitter | `settings_json` |
| `tab_switcher.rs` | `tab_switcher::Toggle` (`ctrl-tab`): the active pane's tabs, the latest used first (`Pane::items_by_recency`), the one before selected; with `ctrl` held, `ctrl-tab` again moves down and `ctrl-shift-tab` up, and letting go switches; opened from the palette, typing filters and enter switches | `tab_switcher` |

The rig forms send exactly what the TUI's typed commands send: the
request building moved into `ainet-model` (`command::{up, down, restore,
spawn, grow}`), which `/up`, `/plan`, `/down`, `/restore`, `/spawn` and
`/grow` now call. Pickers that need items from ainetd ask for them with
`FleetModel::list(request, on_listing)`: `Link::list` sends it, and the
items come back whole as `Msg::Listing` (`Change::Listing`,
`ainet_model::Listing::{Specs, Snapshots}`), to that caller only. The demo
lists its two rigs as the library, two snapshots of each, and answers
`up --plan` for its rigs (its other rig requests still say it cannot).
These answers, and transcripts, specs, exports, the queue requests and
chat topics, are the app's demo only (`Demo::full`, which
`FleetModel::demo` uses): the console's `ainet --demo` (`Demo::new`)
answers as before, "the demo cannot do that".

Saving the theme: only `"theme"` changes in settings.json's text (with a
theme per appearance, `"light"` or `"dark"` for the system's appearance
now, as Zed's selector); a file that does not parse, or cannot be read
(only a missing one counts as empty), is left alone and the error shows
as a toast. The new text goes to a file beside it, synced to disk, then
is renamed over it (through a symlink, over the file it points at), so a
crash never leaves it cut short or empty. Each write has a file of its
own (named by process and write), so two windows saving gui.json at once
never rename each other's half-written text into place. The settings then reload from the new text.

Keys (Linux; `cmd` for `ctrl` on macOS where Zed has it):

| Keys | Where | Action |
|---|---|---|
| `ctrl-shift-p` | anywhere | `command_palette::Toggle` |
| `ctrl-tab` | the workspace | `tab_switcher::Toggle` (again while open: down) |
| `ctrl-shift-tab` | the tab switcher (context `TabSwitcher`) | up |
| `ctrl-enter` | a picker | `menu::SecondaryConfirm` (the switcher: only select) |
| `s` | the Fleet | `fleet::Spawn` |
| `tab`, `shift-tab` | a form (context `Form`) | `form::NextField`, `PreviousField` |
| `enter` | a form's field | the next field; the last sends |

`fleet::Up`, `Down`, `Restore`, `Grow`, `SwitchRig` and
`theme_selector::Toggle` have no default keys: the palette has them.

Tests: pure ones for action names, queries, seat lists and editing
settings.json's text (in place, a missing key, comments and strings
hiding keys, what it cannot edit); headless ones for the palette (the
actions with their bindings, the one picked run where the keyboard was,
lately used first, a usage error kept in the row, `/down` and `/view
inbox` run, `/answer` answering the waiting question, `/view rig:infra`,
`/view docs` and `/view terminals` filtering the Terminals grid), the rig switcher (a click, the rig
picked, its first agent selected, the label following), saving the
theme (an unreadable file left alone, a missing one written, a symlink
kept), the switcher opening a terminal (`ctrl-enter` only
selecting), the Up flow against the demo (the library listed, Browse…,
the plan shown, the `UpSpec` for plan and launch, a typed name),
restore (rig, snapshot by keys, a fresh seat), down (escape, snapshot
on, off, the rig picker with nobody selected), spawn by `s`, grow (the
missing pod said, then the seat), the theme selector (preview, escape
and a click outside restoring, the pick saved into the file's text) and
the tab switcher (by recency, `ctrl` held and let go, typing); also the
selector's edge cases (no match, a theme per appearance, a broken
settings.json). In
`ainet-model`: the forms' commands are the typed ones, listings by token,
the demo's plan.

## Workspace restore, packaging and macOS

**Workspace restore** (`persistence.rs`, the format and file;
`workspace/persist.rs`, reading the workspace into it and building it
back), after Zed's `workspace::persistence` but one JSON file instead of
SQLite: `$XDG_STATE_HOME/ainet/gui.json` (`~/.local/state/ainet/gui.json`),
one workspace per daemon socket and one for the demo:

```json
{ "version": 1, "workspaces": { "/run/user/1000/ainet.sock": {
  "window": { "x": 0, "y": 0, "width": 1280, "height": 800, "state": "maximized" },
  "docks": [ { "panel": "FleetPanel", "open": true }, … ],
  "panel_sizes": { "FleetPanel": 312 },
  "center": { "kind": "axis", "axis": "horizontal", "flexes": [1.2, 0.8], "members": [
    { "kind": "pane", "items": [ { "kind": "terminal", "agent": "…" }, { "kind": "inbox" } ],
      "active": 0, "active_pane": true },
    { "kind": "pane", "items": [ { "kind": "map", "zoom": 1.5, "pan_x": 0, "pan_y": 0 } ] } ] },
  "folded_rigs": [ "infra" ],
  "selected": "…",
  "recent_actions": [ "workspace: toggle left dock" ],
  "command_history": [ "/ps", "@orch-lead@docs ship it" ] } } }
```

- **What.** The window's bounds and whether it was maximized or full
  screen; each dock's panel showing and whether it is open (by panel, so
  a Fleet moved to the other side keeps its state) and the panels
  dragged to a size; the pane tree with its splits, shares, tabs, each
  pane's active tab and the active pane; the Fleet's folded rigs and
  selected agent; the Map's zoom and pan (with its tab); the palette's recent actions (the
  last 50) and the command line's lines (the last 50). Terminals are saved by agent id, the Inbox, Comms and the Map
  as themselves, the Terminals grid with its rig filter and the columns
  asked for, the Topology view with its rig, pod or loose agents and its
  tab; reports and the stand-in tab are not saved.
- **Restoring.** At start the window opens with the saved bounds (at
  least its smallest size; one left on a display that is gone is centred
  on the main display), then the docks, sizes, folds and recent actions
  come back. The panes wait for ainetd's first snapshot (the demo has
  one at once), since only then are the agents known: tabs of agents that
  are gone (or no longer live) are dropped quietly, panes left empty go
  and splits fold up (`SavedMember::retain`, with the active tab passed to
  the one before it). The saved agent is selected in the Fleet again if
  it is still live. The keyboard stays in the Fleet, where a new window
  puts it (it goes to the restored active tab only if it was in the
  center). Until then, and when ainetd never answers, the saved panes and
  selection are kept as saved. Tabs opened before the snapshot win over the
  saved ones, as does a `--view` given on the command line.
- **Saving.** The workspace is compared with what was written every second
  (`persist::SAVE_CHECK`) and written once it changed and then held still
  for a check (`persistence::Debounce`), so dragging a divider writes once;
  its state lives in many cached views, and one comparison a second is
  cheaper than hearing from each. Closing the window (the title bar's
  button, `ctrl-shift-w`, the window manager) and quitting (`ctrl-q`, with
  or without `confirm_quit`'s question) write at once first
  (`Workspace::save_now`, from the close paths, GPUI's `on_app_quit` and
  the workspace's release). A write goes to a file of its own beside it,
  then is renamed over it (as settings.json, above).
- **Versions.** `persistence::VERSION` is 1. A file that does not parse or
  has another version is ignored and written anew; one from a newer
  version (a higher number) is neither read nor written over. A
  workspace that does not parse costs only itself, and a tab of a kind
  this version does not know is dropped (`SavedItem::Unknown`).

Tests: pure ones for the file (round trip with two sockets side by side,
corrupt, empty, older and newer files, a broken workspace beside a good
one), dropping tabs and folding splits, the active tab passing on,
shares, window bounds (too small, on a display gone, not a number) and
the debounce; headless ones for a workspace coming back as it was left
(splits, tabs, the active one, docks, sizes, folds, the Map's view, the
palette), agents gone dropped without a toast, a change written once it
settles, closing and quitting writing first, saved panes kept while
no snapshot came, and `main`'s `open_window` restoring the window's
bounds, panes and folds from a state file (with `--view` keeping the
panes it asked for).

**Settings schema.** `crates/gui/assets/settings-schema.json` is
generated from the settings' types (`schemars`, already in the tree
through Zed's crates; the field docs become its descriptions), and a test
fails when it falls behind: regenerate with
`AINET_WRITE_SCHEMA=1 cargo test --manifest-path crates/gui/Cargo.toml settings_types`.
The settings template (`ctrl-,` on a new file) names it in `"$schema"` by
its URL in the repository; `just install-gui` also puts a copy in
`~/.local/share/ainet/settings-schema.json` for editors offline.

**Packaging.** `crates/gui/assets/ainet-gui.desktop` (the window's app id
and `StartupWMClass` are both `ainet-gui`, so the desktop matches the
window to its entry) and `crates/gui/assets/ainet-gui.svg`, ainet's
critter (`ainet_critter::pixels` for `hash("ainet")`) on a rounded square
in Deepharness's colours, written once and kept in step by a test
(`AINET_WRITE_ICON=1 cargo test --manifest-path crates/gui/Cargo.toml
ainets_critter` draws it again; another checks that the entry's `Exec`,
`Icon` and `StartupWMClass` match the binary, the icon and
`main::APP_ID`; `src/packaging.rs`). `just install-gui` installs `ainet` and
`ainet-gui` with `cargo install` (side by side in cargo's bin directory,
where `ainet gui` looks), the entry (naming the binary by its full path,
since a desktop session's `PATH` often lacks `~/.cargo/bin`), the icon and
the schema under `$XDG_DATA_HOME` (`~/.local/share`).
`packaging/arch/PKGBUILD` builds both binaries from git as split packages,
`ainet` (MIT) and `ainet-gui` (GPL-3.0-or-later, with GPUI's runtime
dependencies); it was written without running `makepkg` (only
`makepkg --printsrcinfo`), so it is untested.

**macOS: plausible, not supported.** Not built or run here (no Mac, and
GPUI's Metal shaders need Xcode). What is in place: `gpui_platform` takes
the same features as Zed's own build on every platform (`wayland`/`x11`
only reach `gpui_linux`, which macOS does not build, `font-kit` reaches
`gpui_macos`), and `crates/gui/Cargo.lock` holds the macOS crates too;
the display check, the GNOME button layout and the Linux window controls
are behind `cfg(target_os = "linux"/"freebsd")`; client-side decorations
fall back to the native title bar with its traffic lights at (9, 9), as
Zed's; `assets/keymaps/default-macos.json` has the `cmd` equivalents and is
picked by `cfg!(target_os = "macos")` (both keymaps are parsed in tests on
Linux); the workspace and key tests that spell out Linux keys are skipped
there. `ainet-model` and `ainet-core` are plain Rust. Expect work on
`AINET_WINDOW_DECORATIONS`, the app menu and signing before anyone ships
it.

## Settings, themes and keys

`~/.config/ainet/settings.json` (`$XDG_CONFIG_HOME` respected) is JSON with
comments and trailing commas, every field optional, with a JSON schema
for editors (`"$schema"`, see "Workspace restore, packaging and macOS"); saved changes apply at
once (the file is checked every 750 ms: its time, size and a hash of its
contents, so a quick re-save of the same size is seen too). If it does not parse, the last good
settings stay and the status bar says what is wrong. `ctrl-,` opens it
(creating it with every field commented out).

| Field | Default | |
|---|---|---|
| `theme` | `"Omarchy"` if installed, else `"Deepharness"` | a name, or `{ "mode": "system" \| "light" \| "dark", "light": "…", "dark": "…" }` as in Zed |
| `ui_font_family`, `ui_font_size` | `".ZedSans"` (IBM Plex Sans), 16 | sizes are kept within 6–100 |
| `ui_density` | `"default"` | `"compact"`, `"comfortable"` |
| `terminal.font_family`, `.font_size`, `.line_height` | `".ZedMono"` (Lilex), 15, `"standard"` | `"comfortable"`, `"standard"` or `{ "custom": 1.4 }`, as in Zed |
| `terminal.cursor_shape`, `.blinking` | `"block"`, `"on"` | `"bar"`, `"underline"`, `"hollow"`; `"off"`, `"terminal_controlled"` (still), as in Zed |
| `fleet_panel.dock`, `.width` | `"left"`, 260 | the Agent panel takes the other side |
| `critters.size` | `"large"` | `"small"`; `ctrl-e` toggles it |
| `command_line` | `true` | the command line above the status bar; `false` shows it only while it has the keyboard (`ctrl-;`) |
| `confirm_quit` | `false` | ask before quitting |

Themes: Zed's One, Ayu and Gruvbox families, **Deepharness** (the TUI's
palette, `crates/gui/assets/themes/deepharness/deepharness.json`), and
**Omarchy**, made at start-up from Omarchy's current `colors.toml` the way
the TUI reads it. Both come from `themes::family_json` and the palette in
`ainet-model`; after changing the palette, regenerate the bundled file with
`AINET_WRITE_THEME=1 cargo test --manifest-path crates/gui/Cargo.toml bundled_deepharness`. Their
player colours are the TUI's agent hues, so `themes::agent_color` gives an
agent the same colour as in the TUI (other themes give their own player
colours).

Your own themes: put Zed theme files (the JSON a Zed theme extension ships
in its `themes/` directory) in `~/.config/ainet/themes/*.json` and name a
theme from them in `theme`, or pick one in the theme selector. They are
read after the bundled ones, so a file can replace a bundled theme of the
same name, and read again whenever a file there is added, changed or
removed (the directory is checked every 750 ms, as settings.json): the
theme in use shows an edit at once. Files that do not parse are listed in
the status bar (`themes:`).

Keys: `crates/gui/assets/keymaps/default-{linux,macos}.json`, in Zed's
format, with the contexts `Workspace`, `FleetPanel`, `Terminal`, `Inbox`,
`Comms`, `CommsCompose`, `CommandLine`, `QueuePanel`, `Map`, `Terminals` (with `grid`), `Topology` (with `view`), `Picker`, `TabSwitcher`, `Form` and `TextInput` (see "The workspace pieces" for the
pane, dock and text keys). `~/.config/ainet/keymap.json` adds to them and
overrides them (`null` unbinds), and reloads on save; bindings it cannot
read are listed in the status bar and the rest apply. `ctrl-alt-,` opens
it (`cmd-k cmd-s` on macOS). In a focused terminal every ctrl-letter goes
to the agent, so the workspace's ctrl keys (`ctrl-b`, `ctrl-j`, `ctrl-q`, …)
do nothing there; a test keeps it so. The actions are in
`crates/gui/src/actions.rs`; `ui::KeyBinding::for_action` shows any of
them as a hint.

## Tests and CI

Run them with `just check-gui` (the GUI) and `just check` (the console
crates, which the GUI shares through `ainet-model`), or both with
`just check-all`. GUI tests are of two kinds:

- **Unit tests** (`#[test]`) for the pure parts: the pane tree, the text
  buffer, terminal layout and key encoding, the saved-workspace format,
  settings and keymap parsing, JSON editing, map flights and camera.
- **Headless tests** (`#[gpui::test]`) on GPUI's test platform: a real
  `Workspace` on the demo fleet (or an offline one) in a window with no
  display, driven by simulated keys, clicks, drags, wheel events and the
  test clock, and checked by what the fleet sent (`FleetModel::sent`),
  what the views hold and where elements were drawn
  (`debug_selector`/`debug_bounds`). Helpers live in `main.rs`'s tests:
  `init_test` (the app as `main` sets it up, without the user's config),
  `demo_workspace`, `sent` and `modal::<V>`. Windows must be activated
  for focus events to fire. GPUI never delivers frame callbacks in tests,
  so anything that waits for the next frame (Zed's `right_click_menu`
  focusing its menu, say) is driven by clicks instead.

Buttons cannot be named for tests themselves; where a test clicks one
(the Inbox's Allow, Always and Deny, the status bar's dock buttons), a
`div` around it carries the selector. Zed's own context menus name their
entries `MENU_ITEM-<label>`.

What each module is tested by (u: unit tests, h: headless tests in the
module; modules without tests of their own are covered where named):

| Module | u | h | Covered |
|---|---|---|---|
| `main.rs` | 1 | 7 | the display check, the demo's numbers, selection, bundled themes, settings reload, `confirm_quit`, default keys, the window opening as it was left through `open_window` (bounds, panes, folds; `--view` winning over saved panes) |
| `actions.rs`, `keymap.rs` | 1 | 3 | Zed's keymap format, every default binding naming a real action, a focused terminal getting every ctrl letter and the list keys, user overrides and errors |
| `args.rs` | 5 | | flags and view names |
| `assets.rs` | 2 | | every `ui::IconName` served, themes, fonts and keymaps bundled |
| `settings.rs`, `json_edit.rs`, `watch.rs` | 17 | 1 | defaults, the template and schema, lenient JSON, clamping, a theme per appearance, saving into the text as written, never losing the file, writers at once never mixing their files, file watching |
| `themes.rs`, `zed_theme.rs` | 2 | 3 | Deepharness by default, One Dark bundled, user themes loaded, reported and reloaded, palettes as themes |
| `theme_selector.rs` | | 2 | preview, escape and a click outside restoring, the pick saved; a query matching nothing, a theme per appearance changing only that one, a broken settings.json left alone with a toast |
| `workspace.rs`, `pane.rs`, `pane_group.rs`, `dock.rs`, `item.rs`, `panels.rs` | 3 | 9 | tabs opening, cycling and closing (click, middle click), the tab's right-click menu (Close, Close Others, Close All, Split Right and Down from a tab that is not active), splits and focus by direction, zoom, dock resizing by dragging, the status bar's dock buttons, the bottom dock's panels and focus actions, toasts |
| `workspace/persist.rs`, `persistence.rs` | 10 | 7 | the file's format and versions, folding gone tabs, bounds, the debounce; a workspace coming back as left (the Fleet's selection too), gone agents dropped quietly, saving once settled, on close and on quit, saved panes kept while ainetd is silent |
| `modal_layer.rs`, `picker.rs`, `switcher.rs`, `tab_switcher.rs` | | 4 | filtering, keys, escape; the agent switcher (in `workspace.rs`); `ctrl-tab` |
| `text_input/` | 8 | 5 | the buffer's edits, moves, undo and IME offsets; typing, clipboard, several lines, read-only; Zed's `InputField` on it |
| `fleet.rs` | | 9 | notices, quiet requests, reconnects, the fetch pump, streamed terminals, changed screens announced, failed and incompatible daemons, the latest view sizing an agent's pane |
| `fleet_panel.rs`, `message.rs`, `critter.rs` | 6 | 7 | the TUI's order, keys and folds, clicks, the agent menu's requests, `m` and the message modal, critters drawn and animated only while drawn |
| `agent_panel.rs` | 2 | 1 | the selected agent, the gauge, the sparkline |
| `terminal_item.rs`, `terminal_element.rs`, `terminal_layout.rs`, `terminal_input.rs` | 16 | 12 | layout, colours, selections, row cache and keys; key encoding; the screen drawn, focus and `Interact`, keys past the workspace, one `Resize`, scrollback, copy and paste, the right-click menu by its keys (Paste, Send Ctrl-C, Copy, Clear Selection, Message…, Zoom; escape types nothing), redrawing only what changed |
| `terminals.rs` | | 11 | the Terminals grid: tiles in the TUI's order, the rig filter, columns, a click and `ctrl-alt` arrows giving tiles the keys with `Interact`, zoom, a hidden grid fetching nothing, tiles following agents, the size rule, `OpenTerminal` into a tile, views opening it |
| `topology.rs` | | 9 | the Topology view: the tree and the docs table by pod, scopes by click and `ctrl-r`, the filter keeping its letters, enter and a double click opening terminals, the agent menu, the tabs, the Graph, Health, Recent showing items, views and commands opening it, an empty fleet |
| `command_line.rs` | 4 | 6 | the history, lines as the TUI's and their previews; each kind of line sent through the demo link, errors in the line and as toasts, completion, recall, `ctrl-;` and escape, letters never reaching other views' keys, the setting |
| `inbox.rs` | 4 | 8 | order, detail styles, picks, the glow; the answer keys, cards by selection and click, the Allow, Always and Deny buttons answering their own card, questions, `ctrl-shift-y`, history, alerts |
| `comms.rs`, `commands.rs` | 6 | 6 | rows, splices, sticking to the end, mentions, code; lines sent as the TUI's, completion, keys, threads, clicks |
| `queue_panel.rs`, `queue_form.rs`, `chatroom.rs`, `events_panel.rs`, `report.rs` | 1 | 9 | the queue's order and chips, every menu entry and form, right click; chat rooms and the cap; the events log; reports |
| `map.rs` | 4 | 6 | flights, the camera; drawing, hover, clicks, panning, messages flying, the empty fleet |
| `command_palette.rs`, `up_modal.rs`, `rig_modals.rs`, `seat_modals.rs`, `form.rs` | 3 | 11 | actions with keys, commands and their errors, `/answer`, `/view`; Up, Restore, Down, the rig switcher, Spawn, Grow |
| `toast_layer.rs`, `problems.rs`, `status_bar.rs`, `title_bar.rs` | 3 | 3 | toast lifetimes, problems kept until cleared, the counts in the TUI's words (the title bar's rig switcher in `rig_modals.rs`) |
| `decorations.rs`, `quit.rs`, `sink.rs`, `packaging.rs` | 6 | | resize edges, the channel sink, the icon and desktop entry (quitting in `main.rs`) |

Not tested headless: the window controls and client-side decorations
(the test platform has no window manager), drag and drop of files, the
system's folder picker (Browse… is checked to ask for it), IME
composition in a real input method, and anything only a real text
system shows (shaping, fonts).

**Screenshots.** `src/screenshot.rs` is an ignored test that draws the
demo workspace offscreen with GPUI's own renderer
(`gpui::HeadlessAppContext`, `gpui_platform::current_headless_renderer`,
which is wgpu without a surface, and the Linux app's cosmic-text text
system) and writes `workspace.png`, `terminal.png`, `inbox.png`,
`map.png`, `topology.png`, `topology-recent.png`, `topology-overview.png`
and `topology-health.png` into `target/screenshots/`:

```sh
just screenshot-gui   # cargo test --manifest-path crates/gui/Cargo.toml screenshot -- --ignored --nocapture
```

It needs a GPU wgpu can draw on without a window (a Vulkan driver, or
lavapipe in software), which is why it is ignored and CI does not run
it. Nothing is compared against saved pictures: rendering differs
between drivers and fonts, so the pictures are for a person to look at.

**CI.** `.github/workflows/ci.yml` has two jobs on `ubuntu-latest`:
`console` runs `cargo fmt --all --check`, clippy with `-D warnings` and
`cargo test` for the root workspace, then fails if Zed's repository was
fetched; `gui` installs the system packages the GUI's tree links (Zed's
`script/linux` at the tag, less what only Zed's other crates need:
fontconfig, freetype, Wayland, X11, xcb, xkbcommon, Vulkan, zstd),
installs the toolchain named by `crates/gui`'s `rust-version`, and runs
the same three with `--manifest-path crates/gui/Cargo.toml`. Each job
caches cargo's registry and `target/`, keyed by its own `Cargo.lock`.
The workflow has been checked as YAML but has not run on GitHub yet.
cargo-deny is not configured: the two workspaces need different licence
allow-lists (MIT for the console, GPL-3.0-or-later with Zed's licences
for the GUI), and a config that has never been run would only look
right.

## Known gaps

Collected from the plan's phase notes; none of these is started.

- **Panes:** dragging tabs between panes, pinned and preview tabs, a
  component preview window.
- **Terminals grid:** tiles in one pane redraw together (above); no
  dragging tiles into another order or out into tabs; the rig filter is a
  button that goes round, not a list to pick from; `pod:RIG/POD` shows the
  whole rig; the zoomed tile is not saved.
- **Fleet and Agent panels:** cost per agent (ainetd does not report it);
  the TUI's spinner and pulsing glyph (a state dot instead).
- **Terminals:** URL hover, IME composition (keys come from key-down
  events), forwarding the mouse to programs that ask for it. The TUI
  still fetches whole screens rather than streaming. Laying out a whole
  screen still goes cell by cell (only the first frame and new rows pay
  it), and shaping has not been measured on a real display. A detached
  stream's thread can pass on one message in the instant before it sees
  the detach; the next whole screen (at most 5 s later) mends it.
  Cached panes and panels rely on every view that reads the fleet
  observing it, which all do today.
- **Command line:** no list of completions to pick from (`tab` completes
  as far as the names agree, the strip lists them); no completion of a
  command's arguments (rig, seat or view names); the history is per
  workspace, not shared between windows; Comms' own meaning of a line
  (answering the selected entry) is the Comms compose box's, not the
  line's.
- **Topology view:** OpenRig's other explorer sections (specs,
  projects, terminals, feed, system) are not there; one host only (the
  one ainetd runs on); columns cannot be resized or sorted; Recent is
  built from the queue items' notes (what ainetd keeps), not a log of
  its own, so a change ainetd made without a note shows only as where
  the item is now; Health's quiet check relies on the hook counts
  ainetd reports per agent (an agent without them is never called quiet); the explorer is not folded or
  driven by keys.
- **Inbox:** Markdown beyond headings,
  lists and fenced code, keyboard focus on single buttons.
- **Comms:** a right-click menu on messages, a completion list while
  typing `@`, Markdown beyond fenced code, history older than the Store
  keeps.
- **Bottom dock and reports:** creating a queue item from the panel
  (`/queue` in Comms does), editing priority and tags, a reason when
  unclaiming, `@` completion in the handoff form; searching a report,
  YAML highlighting, transcript filters.
- **Map:** empty and stopped seats, edge kinds other than who directs
  whom, a menu on nodes.
- **Palette and modals:** pickers for `/adopt` and `/discover`, editing a
  rig's edges when growing, a rig's own menu in the Fleet.
- **Restore:** the zoomed pane, selections in Comms and the Inbox, scroll
  positions and several windows are not saved (reports deliberately
  not); a dock size saved in a larger window is not capped until
  dragged.
- **Packaging and platforms:** the Arch `PKGBUILD` has not been built
  (only `makepkg --printsrcinfo`); `just install-gui` was run by hand, not
  through `just`; macOS has not been built or run. The settings
  template's `"$schema"` URL resolves once this branch is on `main`.
- **Tests and CI:** the workflow has not run on GitHub; screenshots are
  not compared; no cargo-deny.

## Upgrading Zed

1. Pick the release: the newest `vX.Y.Z` tag (not `-pre`) on
   <https://github.com/zed-industries/zed/releases>.
2. Change the `tag` on every Zed dependency in `crates/gui/Cargo.toml`.
3. Fetch Zed's root manifest at the new tag:
   `curl -fsSL https://raw.githubusercontent.com/zed-industries/zed/<tag>/Cargo.toml`.
   Compare its `[patch.crates-io]` with ours; update the revisions of the
   three we copy and add any new entry whose crate appears in
   `cargo tree --manifest-path crates/gui/Cargo.toml`.
4. Check Zed's `rust-toolchain.toml` at the tag and raise `rust-version` in
   `crates/gui/Cargo.toml` if it moved.
5. `cargo update --manifest-path crates/gui/Cargo.toml -p gpui` (refreshes
   the git checkout in `crates/gui/Cargo.lock`), then
   `cargo build --manifest-path crates/gui/Cargo.toml`. The new source is under
   `~/.cargo/git/checkouts/zed-*/<short-rev>/`; read it for API changes, in
   particular `crates/theme/src/theme_settings_provider.rs`,
   `crates/workspace/src/workspace.rs` (`client_side_decorations`),
   `crates/platform_title_bar` and `crates/ui/src/components`.
6. Re-copy Zed's assets: `assets/icons/*.svg` and `LICENSES`,
   `assets/themes` (with `LICENSES`) and `assets/fonts` into
   `crates/gui/assets` (the `serves_every_icon_ui_can_name` test fails if
   `ui::IconName` gained an icon). Diff Zed's
   `crates/theme_settings/src/{schema.rs,theme_settings.rs}` between the
   two tags and carry the changes into `crates/gui/src/zed_theme.rs`. Check
   `script/linux` for new system packages (README, "Desktop app").
7. Run the GUI gate (`just check-gui`):
   `cargo fmt --manifest-path crates/gui/Cargo.toml --check`,
   `cargo clippy --manifest-path crates/gui/Cargo.toml --all-targets -- -D warnings`,
   `cargo test --manifest-path crates/gui/Cargo.toml`; and the console gate
   (`just check`), which must not change: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`.
8. Update the table at the top of this file and commit the bump on its own.

## Build notes

- The first build compiles a few hundred crates (GPUI, wgpu, font
  rasterisation, Wayland and X11 clients); budget tens of minutes on a small
  machine. Incremental builds of `crates/gui` are quick.
- Testing the GUI builds GPUI a second time with `test-support`.
- The GUI is outside the console's workspace: plain `cargo build`, `cargo
  install --path crates/ainet` and `just check` never fetch or compile
  GPUI.
- `ainet gui` runs `ainet-gui` with `AINET_BIN` set to itself, so the GUI
  starts ainetd through the same console binary the user ran.
- `AINET_WINDOW_DECORATIONS=server` asks the window manager for its own
  title bar instead of ours (Zed's `ZED_WINDOW_DECORATIONS`).
