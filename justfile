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
