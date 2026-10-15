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
| Crates used directly | `gpui`, `gpui_platform` (features `font-kit`, `wayland`, `x11`), `theme`, `ui`; `gpui` with `test-support` as a dev-dependency |
| Zed's toolchain | Rust 1.98.1 (`rust-toolchain.toml` at the tag); `crates/gui` declares `rust-version = "1.98"`, the console crates keep 1.88 |
| Licences | `gpui*` and its support crates Apache-2.0; `ui`, `ui_macros`, `theme`, `syntax_theme`, `icons`, `component`, `menu` GPL-3.0-or-later, so `ainet-gui` is GPL-3.0-or-later (THIRD_PARTY_NOTICES.md) |

All four dependencies in `crates/gui/Cargo.toml` (and the dev-dependency)
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
- Assets: `crates/gui/src/assets.rs` embeds the icons the views draw, copied
  from Zed's `assets/icons` (Lucide, ISC; `crates/gui/assets/icons/LICENSES`).

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
6. Re-copy any icons that changed (`assets/icons/<name>.svg`) and check
   `script/linux` for new system packages (README, "Desktop app").
7. Run the GUI gate: `cargo fmt --all --check`,
   `cargo clippy -p ainet-gui --all-targets -- -D warnings`,
   `cargo test -p ainet-gui`; and the console gate, which must not change:
   `cargo clippy --workspace --exclude ainet-gui --all-targets -- -D warnings`,
   `cargo test --workspace --exclude ainet-gui`.
8. Update the table at the top of this file and commit the bump on its own.

## Build notes

- The first build compiles a few hundred crates (GPUI, wgpu, font
  rasterisation, Wayland and X11 clients); budget tens of minutes on a small
  machine. Incremental builds of `crates/gui` are quick.
- `cargo test -p ainet-gui` builds GPUI a second time with `test-support`.
- `ainet-gui` is outside `default-members`: plain `cargo build`, `cargo
  install --path crates/ainet` and `just check` never compile GPUI.
- `AINET_WINDOW_DECORATIONS=server` asks the window manager for its own
  title bar instead of ours (Zed's `ZED_WINDOW_DECORATIONS`).
