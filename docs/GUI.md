# ainet-gui: building against Zed

`crates/gui` (`ainet-gui`) is built on GPUI and Zed's `ui` and `theme`
crates, taken from Zed's git repository and pinned to one release. This
note records the pin and how to move it. The plan and the reasons are in
[GUI-PLAN.md](GUI-PLAN.md).

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
| `item.rs` | `Item` (tab label, icon or critter slot, tooltip, `can_split`/`clone_on_split`, `activated`/`deactivated`/`on_removed`, `ItemEvent::CloseItem`/`UpdateTab`), `ItemHandle`, and `Placeholder`, the stand-in for phase 4's views | `workspace::item` |
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
center. Dock sizes are kept **in memory**
for the session, per panel (the Fleet panel's starts from
`fleet_panel.width` and follows that setting until dragged); saving them
with the open tabs and splits is phase 7's workspace restore. Moving the
Fleet panel to the other side moves both side panels with their sizes and
open state.

Keys (Linux; macOS has the `cmd` equivalents), in the keymaps:

| Keys | Action |
|---|---|
| `ctrl-tab` | the tab switcher (the latest used first; let go of `ctrl` to switch) |
| `ctrl-shift-tab`, `ctrl-pagedown`, `ctrl-pageup` | previous, next, previous tab |
| `ctrl-w` | close the tab |
| `ctrl-\`, `ctrl-alt-\` | split right, down |
| `ctrl-alt-left/right/up/down` | the pane in that direction (Zed's are `ctrl-k` chords, which a terminal needs) |
| `shift-escape` | zoom the active pane, or put it back |
| `ctrl-b`, `ctrl-alt-b`, `ctrl-j` | left, right, bottom dock |
| `ctrl-shift-c`, `ctrl-shift-i`, `ctrl-t` | Comms, Inbox, Map tabs |
| `ctrl-y`, `ctrl-a`, `ctrl-n` | allow, always allow, deny (outside the Inbox: open it on the oldest request) |
| `ctrl-p` | the agent switcher |
| `ctrl-shift-p` | the command palette |
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
`VisualTestContext`): tabs, splits and focus, zoom, dock resizing by a
simulated drag, the switcher modal, toasts expiring on the test clock,
text editing through the platform input handler, and the picker.
`crate::tests::init_test` sets the app up as `main` does, without the
user's config; `demo_workspace`, `sent` and `modal::<V>` there are the
shared helpers for a window on the demo, the requests it sent, and the
modal open.

## The Fleet and Agent panels (phase 4, steps 1 and 2)

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

## The terminal (phase 4 step 3)

An agent's terminal is a tab, `TerminalItem`, showing the copy of its
screen ainet keeps (a `vt100::Parser` per agent in the `Store`), streamed
from ainetd (`Request::Attach`: the screen, then what the program
prints, fed into the copy as it comes; phase 6) or, from a daemon that
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
  demo draws its screens at the asked size.
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
- **Tile all.** `fleet::TileAll` (`ctrl-g`) lays out every live terminal
  of the selected agent's rig (every live terminal when it has none) as a
  grid of panes, as the TUI's Terminals view: columns chosen by the TUI's
  `grid_cols` (now `ainet_model::terminal::grid_cols`) from the room in
  cells, rows of tiles, the last row's tiles wider. Panes left from tiling
  before make way, so tiling twice gives one grid.

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
bracketed paste, `SendCtrlC`, and Tile all. In tests the window must be
activated (`window.activate_window()`) for focus events to fire.

## Performance and live terminals (phase 6)

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
  overlap or leave a gap.
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
resizes and lag resync, dropping detaches, styled rows draw the same
anywhere), `ainet-daemon` (`tests/attach.rs`, a real ainetd: screen then
bytes, a screen once settled and after a resize, closing either end
detaches, a refused attach leaves the connection usable, coloured
scrollback only when asked; `stream` unit test: characters split between
reads), `ainet-core` (the wire shapes old peers see), `ainet-model`
(`tests/attach.rs` against pretend daemons: streaming into the store
until detached, an old daemon's refusal and the fallback to fetching, a
stream that ends; `Store` feeding bytes and keeping styled history), and
the GUI tests listed under the terminal.

## The Inbox (phase 4 step 4)

`InboxItem` (`inbox.rs`) is the TUI's Inbox as a tab, opened by
`workspace::OpenInbox` (`ctrl-shift-i`), `--view inbox`, the waiting
badge in the title and status bars, a toast's Review button, or the
answer keys from anywhere else. One card per item of the Store's inbox,
drawn like the tool-call approval cards of Zed's agent panel
(`agent_ui`'s `render_tool_call`): a faint header with the agent's
critter, name (in its colour), the kind's chip (the TUI's `kind_chip`:
permission, plan, question, finished, failed; faded once answered) and
age; the summary; the whole detail in a block that scrolls past 18 rem
(a permission's command or edit in the buffer font, `+`/`-`/`@` lines
coloured as the TUI's; a plan as light markdown: headings, lists, fenced
code); and, while it waits on you, **Allow**, **Always** and **Deny**
with their key hints on the selected card, plus a button to the agent's
own terminal. Failed items get a dashed border, as Zed's failed tool
calls.

Order: requests waiting on you first, oldest first (where the keys go);
then the history (answered requests, finished and failed reports,
questions sent as messages), newest first, folded under a disclosure
until opened. A history row opens to its full card (enter, or a click).
The selection is a card's id; when the selected request is answered it
moves on to the next one waiting.

Questions (`AskUserQuestion`) show their options as Zed's `ChoiceCard`s,
radio or checkbox by `multi_select`; digits `1`–`9` pick them while the
Inbox has the keyboard, as the TUI's. A field takes your own words;
enter (or **Answer**) sends them if typed, else the numbers picked
(`command::answer_choices`, "1 3"), through `command::answer_question`.
A click anywhere in the Inbox (its buttons too) gives it the keyboard,
and a field that goes when its question is answered hands the keyboard
back to the cards.
**Leave to Terminal** (`ctrl-n`) denies it as the TUI's `^N` does.
Permissions and plans go through `Store::answer`, so the request and what
the toast says (`allowed Lead's permission`) are the TUI's.

Keys (the TUI's): `ctrl-y` allow, `ctrl-a` always, `ctrl-n` deny. In the
Inbox they answer the selected card; anywhere else (but a terminal, which
takes every ctrl letter) they first open the Inbox on the oldest request
waiting, the selected agent's first (`Store::next_waiting`), so you read
before you allow. `up`/`down` (`home`/`end`) move between cards, `enter`
is `inbox::Answer`, `escape` leaves a question's field. There are no
single-letter answer keys: a stray key must never allow a command.

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
selection moves on), cards answered by selection and click, questions
(digits, several or one, your words over picks, escape, deny, the
keyboard back on the cards after an answer by text), a click giving the
Inbox the keys, `ctrl-shift-y`, the history folding, `--view inbox`, and
the alert's toast and glow count.
`FleetModel::receive` (tests only) applies link messages directly.

## Comms (phase 4 step 5)

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

## The bottom dock and reports (phase 4 steps 6 and 8)

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

## The Map (phase 4 step 7)

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

## The command palette, pickers and modals (phase 5)

All in the modal layer, on the picker and the text field of phase 3:

| File | What | After Zed's |
|---|---|---|
| `command_palette.rs` | `CommandPalette` (`ctrl-shift-p`): every action available where the keyboard was (`Window::available_actions`; `menu::*` left out), named in words as Zed's (`humanize`: `workspace::ToggleLeftDock` is "workspace: toggle left dock"), with its key binding there (`ui::KeyBinding::for_action_in`), fuzzy filtered (`normalize` lets `workspace::toggle_left` find it too), the ones run from it lately first (kept for the session). Picking one goes back where the keyboard was and runs it there. A line starting with `/`, or one that matches no action, is a console command: `commands::parse` (`command::parse`, no rig in view, so the selected agent's rig) says in its row what it does (`launching docs`) or what is wrong (the usage, `no command /x`); enter runs it, or stays open while it is wrong | `command_palette` |
| `commands.rs` | the console's command language for the app, shared with Comms: requests through the fleet, the console's own commands (`/view`, `/allow`, `/quit`, …) as the app's actions. `/answer <text>` answers the question waiting on you (`commands::question`: the selected agent's first, else the oldest), as the TUI's answers the one selected. `/view fleet` focuses the Fleet, `/view rig:<name>` and `/view <rig>` tile that rig's terminals (`Workspace::tile_rig`), whichever agent is selected; another name says there is no such view or rig | — |
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
as a toast. The new text goes to a file beside it, then is renamed over
it (through a symlink, over the file it points at), so a crash never
leaves it cut short. The settings then reload from the new text.

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
inbox` run, `/answer` answering the waiting question, `/view rig:infra`
and `/view docs` tiling that rig), the rig switcher (a click, the rig
picked, its first agent selected, the label following), saving the
theme (an unreadable file left alone, a missing one written, a symlink
kept), the switcher opening a terminal (`ctrl-enter` only
selecting), the Up flow against the demo (the library listed, Browse…,
the plan shown, the `UpSpec` for plan and launch, a typed name),
restore (rig, snapshot by keys, a fresh seat), down (escape, snapshot
on, off, the rig picker with nobody selected), spawn by `s`, grow (the
missing pod said, then the seat), the theme selector (preview, escape
and a click outside restoring, the pick saved into the file's text) and
the tab switcher (by recency, `ctrl` held and let go, typing). In
`ainet-model`: the forms' commands are the typed ones, listings by token,
the demo's plan.

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

## Settings, themes and keys

`~/.config/ainet/settings.json` (`$XDG_CONFIG_HOME` respected) is JSON with
comments and trailing commas, every field optional; saved changes apply at
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
`Comms`, `CommsCompose`, `QueuePanel`, `Map`, `Picker`, `TabSwitcher`, `Form` and `TextInput` (see "The workspace pieces" for the
pane, dock and text keys). `~/.config/ainet/keymap.json` adds to them and
overrides them (`null` unbinds), and reloads on save; bindings it cannot
read are listed in the status bar and the rest apply. `ctrl-alt-,` opens
it (`cmd-k cmd-s` on macOS). In a focused terminal every ctrl-letter goes
to the agent, so the workspace's ctrl keys (`ctrl-b`, `ctrl-j`, `ctrl-q`, …)
do nothing there; a test keeps it so. The actions are in
`crates/gui/src/actions.rs`; `ui::KeyBinding::for_action` shows any of
them as a hint.

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
