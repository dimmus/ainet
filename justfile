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

# Format, lint, test: what CI should run.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

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
