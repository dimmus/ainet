# ainet-gui: building against Zed

`crates/gui` (`ainet-gui`) is built on GPUI and Zed's `ui` and `theme`
crates, taken from Zed's git repository and pinned to one release. This
note records the pin and how to move it. The plan and the reasons are in
[GUI-PLAN.md](GUI-PLAN.md).

## The pin

| | |
|---|---|
| Zed release | **v1.24.2** (latest stable on 2026-10-15) |
| Commit | `fb69067729cb16a7f6135e2c1600c26a28c59659` (see `Cargo.lock`) |
| Crates used directly | `gpui`, `gpui_platform` (features `font-kit`, `wayland`, `x11`), `theme`, `ui`, `settings_content` (the theme JSON schema only); `gpui` with `test-support` as a dev-dependency |
| Zed's toolchain | Rust 1.98.1 (`rust-toolchain.toml` at the tag); `crates/gui` declares `rust-version = "1.98"`, the console crates keep 1.88 |
| Licences | `gpui*` and its support crates Apache-2.0; `ui`, `ui_macros`, `theme`, `syntax_theme`, `icons`, `component`, `menu`, `settings_content` (with `settings_json`, `settings_macros`, `language_model_core`) GPL-3.0-or-later, so `ainet-gui` is GPL-3.0-or-later (THIRD_PARTY_NOTICES.md) |
| Assets | Zed's `assets/themes` (One, Ayu, Gruvbox), `assets/icons` (top level), `assets/fonts` (IBM Plex Sans, Lilex) at the tag, copied into `crates/gui/assets` |

All Zed dependencies in `crates/gui/Cargo.toml` (and the dev-dependency)
name the same `tag`; never mix tags or revisions.

## Patches copied from Zed

Cargo applies `[patch.crates-io]` only from the root manifest, so the
patches Zed builds with do not follow its crates into ours. The root
`Cargo.toml` copies the ones GPUI's dependency tree needs, from Zed's root
`Cargo.toml` at the tag:

| Crate | Source | Used by |
|---|---|---|
| `async-process` | `github.com/zed-industries/async-process` @ `0b6d6713570af61806e1e5cb40e0f757cb93fd9d` | `smol`, `zbus` (GPUI's desktop portal client) |
| `async-task` | `github.com/smol-rs/async-task` @ `b4486cd71e4e94fbda54ce6302444de14f4d190e` | GPUI's executors, `async-executor` |
| `calloop` | `github.com/zed-industries/calloop` @ `3759371fee14c40066d64777e2a36b6ffcc22590` | `gpui_linux`: the X11/Wayland event loop |

Zed patches more (`tree-sitter-language`, `livekit`, `libwebrtc`,
`webrtc-sys`, `windows-capture`, `notify`, `notify-types`, `scratch`); none
of them is in `ainet-gui`'s tree today. When a new Zed crate is added, check
`cargo tree -p ainet-gui -i <crate>` for each of them and copy the ones that
appear.

These patches only affect crates that use the patched packages; the console
crates use none of them, so `cargo build` of `ainet` is unchanged.

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

## Upgrading Zed

1. Pick the release: the newest `vX.Y.Z` tag (not `-pre`) on
   <https://github.com/zed-industries/zed/releases>.
2. Change the `tag` on every Zed dependency in `crates/gui/Cargo.toml`.
3. Fetch Zed's root manifest at the new tag:
   `curl -fsSL https://raw.githubusercontent.com/zed-industries/zed/<tag>/Cargo.toml`.
   Compare its `[patch.crates-io]` with ours; update the revisions of the
   three we copy and add any new entry whose crate appears in
   `cargo tree -p ainet-gui`.
4. Check Zed's `rust-toolchain.toml` at the tag and raise `rust-version` in
   `crates/gui/Cargo.toml` if it moved.
5. `cargo update -p gpui` (refreshes the git checkout in `Cargo.lock`), then
   `cargo build -p ainet-gui`. The new source is under
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
7. Run the GUI gate: `cargo fmt --all --check`,
   `cargo clippy -p ainet-gui --all-targets -- -D warnings`,
   `cargo test -p ainet-gui`; and the console gate, which must not change:
   `cargo clippy --workspace --exclude ainet-gui --all-targets -- -D warnings`,
   `cargo test --workspace --exclude ainet-gui`.
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
`AINET_WRITE_THEME=1 cargo test -p ainet-gui bundled_deepharness`. Their
player colours are the TUI's agent hues, so `themes::agent_color` gives an
agent the same colour as in the TUI (other themes give their own player
colours).

Keys: `crates/gui/assets/keymaps/default-{linux,macos}.json`, in Zed's
format, with the contexts `Workspace`, `FleetPanel`, `Terminal`, `Inbox`,
`Comms` and `Picker`. `~/.config/ainet/keymap.json` adds to them and
overrides them (`null` unbinds), and reloads on save; bindings it cannot
read are listed in the status bar and the rest apply. The actions are in
`crates/gui/src/actions.rs`; `ui::KeyBinding::for_action` shows any of
them as a hint.

## Build notes

- The first build compiles a few hundred crates (GPUI, wgpu, font
  rasterisation, Wayland and X11 clients); budget tens of minutes on a small
  machine. Incremental builds of `crates/gui` are quick.
- `cargo test -p ainet-gui` builds GPUI a second time with `test-support`.
- `ainet-gui` is outside `default-members`: plain `cargo build`, `cargo
  install --path crates/ainet` and `just check` never compile GPUI.
- `AINET_WINDOW_DECORATIONS=server` asks the window manager for its own
  title bar instead of ours (Zed's `ZED_WINDOW_DECORATIONS`).
