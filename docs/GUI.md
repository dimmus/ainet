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
| `dock.rs` | `Dock` (left, right, bottom) holding `Panel`s, opened and closed, resized by dragging its inner edge (double click: default size) | `workspace::dock` |
| `panels.rs` | the panels' title strip and state colours, and the Activity panel (a placeholder until phase 4 step 6) | — |
| `modal_layer.rs` | one modal at a time; escape, a click outside or the modal's `DismissEvent` closes it and gives the keyboard back | `workspace::modal_layer` |
| `toast_layer.rs` | the fleet's notices as toasts, bottom right: flashes for 5 s (the TUI's flash time), errors for 15 s, reports (titled) until dismissed; at most 3 | status toasts, notifications |
| `text_input/` | `TextInput`: one line or several (soft-wrapped, `max_lines`), IME, mouse and key selection, clipboard, word moves, undo/redo, placeholder, masking, read-only; `InputEvent::{Edited, Submitted, Cancelled, Blurred}`; `erased.rs` registers it as `ui_input`'s editor | GPUI's `examples/input.rs`, `ui_input::ErasedEditor` |
| `picker.rs` | `Picker<D: PickerDelegate>` in the modal layer: query input, fuzzy matches with highlighted characters (Zed's `fuzzy`), `menu::*` keys | `picker` |
| `switcher.rs` | the agent switcher (`ctrl-p`), the first picker: picking selects the agent | `file_finder` |

The workspace (`workspace.rs`) ties them together: the Fleet panel docks
left (or right, `fleet_panel.dock`) with the Agent panel opposite and the
Activity panel at the bottom, all open at start; the status bar has a
button per dock at its ends, as Zed's. Dock sizes are kept **in memory**
for the session, per panel (the Fleet panel's starts from
`fleet_panel.width` and follows that setting until dragged); saving them
with the open tabs and splits is phase 7's workspace restore. Moving the
Fleet panel to the other side moves both side panels with their sizes and
open state.

Keys (Linux; macOS has the `cmd` equivalents), in the keymaps:

| Keys | Action |
|---|---|
| `ctrl-tab`, `ctrl-shift-tab`, `ctrl-pagedown`, `ctrl-pageup` | next, previous tab |
| `ctrl-w` | close the tab |
| `ctrl-\`, `ctrl-alt-\` | split right, down |
| `ctrl-alt-left/right/up/down` | the pane in that direction (Zed's are `ctrl-k` chords, which a terminal needs) |
| `shift-escape` | zoom the active pane, or put it back |
| `ctrl-b`, `ctrl-alt-b`, `ctrl-j` | left, right, bottom dock |
| `ctrl-shift-c`, `ctrl-shift-i`, `ctrl-t` | Comms, Inbox, Map tabs (placeholders) |
| `ctrl-p` | the agent switcher |
| `escape`, `enter`, `up`, `down`, `home`, `end` | `menu::*`, for menus, pickers and modals |

`workspace::ActivateNextPane`/`ActivatePreviousPane` and
`pane::CloseOtherItems`/`CloseAllItems` have no default keys. In a text
field (`TextInput` context) the usual editing keys apply (`ctrl-z`,
`ctrl-shift-z`, `ctrl-backspace`, `ctrl-left`, …); `enter` submits and
`escape` cancels, and with several lines (`TextInput && multiline`)
`shift-enter` starts a new line, as in Zed's agent panel.

Testing: the pieces have headless tests (`#[gpui::test]` with
`VisualTestContext`): tabs, splits and focus, zoom, dock resizing by a
simulated drag, the switcher modal, toasts expiring on the test clock,
text editing through the platform input handler, and the picker.
`crate::tests::init_test` sets the app up as `main` does, without the
user's config.

## The Fleet and Agent panels (phase 4, steps 1 and 2)

The first views on top of the workspace pieces. They read the same
`Store` as the TUI and send what the TUI sends, through `ainet-model`:

| File | What | After Zed's |
|---|---|---|
| `fleet_panel.rs` | the Fleet tree in the Store's (the TUI's) order, virtualised (`uniform_list`, every row one height): rigs with a `Disclosure`, status dot, running/total seats, open queue items and summary; agents with their critter, name, role in their colour, `directs N`, a `CountBadge` of requests waiting on you, an envelope with unread messages, the state `Indicator`, task and the TUI's meta line (age, runtime, model, context); empty seats; the loose groups' headers. Who directs whom is drawn with indent guides; folded rigs are skipped by the keys. The selection is the Store's: a new one (from the switcher, a click elsewhere) unfolds its rig and scrolls to it. A right click (or `shift-f10`) opens the agent's `ContextMenu` | `project_panel` |
| `agent_panel.rs` | the TUI's details pane: a big moving critter, name, role chip, "you › … › name", task and last tool, model and cwd, state, the context gauge gliding a quarter of the way each tick (`ProgressBar`, warning past 60 %, error past 80 %), the activity sparkline, the facts table, the last messages to or from it and its open queue items; the empty den when nobody is selected | — |
| `critter.rs` | `Critter`, the avatar element: a sprite as quads, one per run of same-coloured pixels, snapped to whole device pixels at any size; `Critter::agent` hops while working and is a still grey ghost once gone. Colours: the palette's own for Deepharness and Omarchy (exactly the TUI's), else the theme's player colour | — |
| `terminal_item.rs` | `TerminalItem`, an agent's tab (critter in the tab), opened by `Workspace::open_terminal(id)`: one per agent, reused wherever it is open; the live terminal of the next section | `terminal_view` |
| `message.rs` | the message modal (`m`): what `command::message` sends for `@Name text`, so delivered through the typing guard and kept in the mailbox | — |

The agent's menu and keys, all handled by the workspace for the selected
agent so the menu shows their bindings:

| Menu entry | Key in the Fleet | Sends (as the TUI's command) |
|---|---|---|
| Message… | `m` | `Message { to: [id], force: true }` (`@Name text`) |
| Open Terminal | `enter`, double click | — (opens the tab) |
| Show Transcript | | `Transcript { agent }`, shown as a report |
| Capture Screen | | `Capture` (`/capture`) |
| Relaunch | | `Launch { fresh: true }` (`/relaunch`) |
| Continue | | `SeatContinue` (`/continue`) |
| Typing Guard (checked when on) | | `TypingGuard` (`/guard`) |
| Close Agent… | `delete` | `Close` (`/close`), after asking |

`up`/`down` (or `j`/`k`, and `alt-up`/`alt-down` anywhere) move the
selection, `left`/`right` fold and unfold the selected agent's rig. Seat
entries are disabled for agents outside a rig. The requests come from
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
screen ainet keeps (a `vt100::Parser` per agent in the `Store`, filled by
`Request::Screen { formatted: true }`, as the TUI's). ainetd owns the
PTYs, so there is no alacritty and no local process: the GUI paints the
screen, sends keys as `Input` and sizes as `Resize`, and asks for a fresh
screen when the agent prints.

| File | What | After Zed's |
|---|---|---|
| `terminal_item.rs` | the tab: critter, name and a state dot; a strip with the seat address, state, "Typing · messages held" while focused, the scroll position and a message button (`fleet::MessageAgent`); keys, mouse, menu and actions | `terminal_view::TerminalView` |
| `terminal_element.rs` | `TerminalElement`: measures a cell of the terminal font, fits rows × columns in its bounds, paints backgrounds, selection, text and cursor | `terminal_view::TerminalElement` |
| `terminal_layout.rs` | the pure part: a screen as merged background rectangles and runs of text of one style (bold, italic, underline, inverse, dim; 16, 256 and true colours through the theme's terminal ANSI colours), wide characters, selections and their text, scrollback laid out as a screen | `layout_grid`, `convert_color` |
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
  quiet `Resize` (dragging a divider asks once, at the end), and the screen
  is fetched again. The demo draws its screens at the asked size.
- **Fresh screens.** A tab on show (active in its pane) registers with
  `FleetModel::show_terminal`; `Event::Output` for its agent marks the
  screen stale, and stale screens are fetched at most every 66 ms each
  (`ainet_model::Pump`, the TUI's pump, which the TUI now uses too).
  Hidden tabs cost nothing; in the demo, shown screens move with the
  clock.
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
  A terminal that gets the keyboard also becomes the Fleet's selection.
- **Mouse.** A left drag selects (shift-click extends), `terminal::Copy`
  copies it, `terminal::Clear` drops it. The wheel scrolls back through the
  scrollback: the first step asks for it (`Screen { history: n }`, plain
  text, in chunks of 200 rows), which then shows in place of the screen,
  without colours, with "N lines back" in the strip; on the alternate
  screen (full-screen programs) the wheel sends arrow keys instead, as
  xterm. Right click: Copy, Paste, Clear Selection, Send Ctrl-C,
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
panes), `ctrl-tab` (other tabs), `alt-up`/`alt-down` (the Fleet's
selection).

Tests: `terminal_layout` (colours, attributes, merging, wide characters,
selections, scrollback), `terminal_input` (the TUI's key table as GPUI
keystrokes), `fleet` (output fetches at most so often, one `Resize` per
size), and headless `terminal_item` tests: the demo's screen painted,
focus sending `Interact` on and off (and nothing between two views of one
agent), keys reaching the agent past the workspace's bindings, one
`Resize` after the window is dragged through several sizes, the wheel
asking for scrollback and typing coming back, selection and copy,
bracketed paste, `SendCtrlC`, and Tile all. In tests the window must be
activated (`window.activate_window()`) for focus events to fire.

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
once (the file is checked every 750 ms). If it does not parse, the last good
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
theme from them in `theme`. They are read at start-up, after the bundled
ones, so a file can replace a bundled theme of the same name; files that do
not parse are listed in the status bar.

Keys: `crates/gui/assets/keymaps/default-{linux,macos}.json`, in Zed's
format, with the contexts `Workspace`, `FleetPanel`, `Terminal`, `Inbox`,
`Comms`, `Picker` and `TextInput` (see "The workspace pieces" for the
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
