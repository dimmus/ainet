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
| `panels.rs` | the Fleet, Agent and Activity panels (placeholders until phase 4) | — |
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
