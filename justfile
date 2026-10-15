# ainet tasks. `just` lists them.

default:
    @just --list

# The TUI against the pretend fleet: no daemon, no Claude Code needed.
demo:
    cargo run -- --demo

# The real thing: starts ainetd in the background if it is not running.
run:
    cargo run

# ainetd in the foreground, with debug logs.
daemon:
    AINET_LOG=debug cargo run -- daemon

# Format, lint, test the console crates: what CI should run. The GUI is a
# workspace of its own, so this never compiles or fetches GPUI (see
# check-gui).
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# The desktop app against ainetd (starts it through the console binary).
# crates/gui is its own workspace, hence --manifest-path.
gui:
    cargo build -p ainet
    cargo run --manifest-path crates/gui/Cargo.toml

# The desktop app against the pretend fleet.
gui-demo:
    cargo run --manifest-path crates/gui/Cargo.toml -- --demo

# Format, lint, test the desktop app (slow the first time: builds GPUI).
check-gui:
    cargo fmt --manifest-path crates/gui/Cargo.toml --check
    cargo clippy --manifest-path crates/gui/Cargo.toml --all-targets -- -D warnings
    cargo test --manifest-path crates/gui/Cargo.toml

test:
    cargo nextest run --workspace || cargo test --workspace

# Rebuild and rerun the demo on every save (needs bacon).
watch:
    bacon run -- -- --demo

# Draw some critters in your terminal.
critters *ids:
    cargo run -q -- critter {{ids}}

# Re-render the README screenshots (needs ImageMagick with SVG support, or any SVG rasteriser).
screenshots:
    for v in fleet inbox map critters; do cargo run -q -p ainet-tui --example frame -- 150 46 svg $v > docs/images/$v.svg; done

release:
    cargo build --release
    @ls -lh target/release/ainet

install:
    cargo install --path crates/ainet --locked

# The desktop app: ainet-gui next to ainet (cargo's bin directory, where
# `ainet gui` looks), its desktop entry, icon and settings schema under
# ~/.local/share. The entry names the binary by its full path, since a
# desktop session's PATH often lacks ~/.cargo/bin.
install-gui: install
    #!/usr/bin/env bash
    set -euo pipefail
    cargo install --path crates/gui --locked
    bin="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin/ainet-gui"
    data="${XDG_DATA_HOME:-$HOME/.local/share}"
    mkdir -p "$data/applications"
    sed "s|^Exec=ainet-gui$|Exec=$bin|" crates/gui/assets/ainet-gui.desktop > "$data/applications/ainet-gui.desktop"
    install -Dm644 crates/gui/assets/ainet-gui.svg "$data/icons/hicolor/scalable/apps/ainet-gui.svg"
    install -Dm644 crates/gui/assets/settings-schema.json "$data/ainet/settings-schema.json"
    if command -v update-desktop-database >/dev/null; then update-desktop-database -q "$data/applications" || true; fi
    if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -qtf "$data/icons/hicolor" || true; fi
    echo "installed $bin and its desktop entry in $data"
