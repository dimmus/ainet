//! What keeping terminals fresh costs, measured without a display: the
//! bytes a screen refetch moves against what the agent actually printed,
//! the time each side spends on them, and how many refetches the pump lets
//! through for N busy terminals. Ignored by default; run with
//!
//! ```text
//! cargo test -p ainet-model --release --test terminal_cost -- --ignored --nocapture
//! ```
//!
//! The numbers are recorded in docs/GUI.md ("Performance").

use std::hint::black_box;
use std::time::{Duration, Instant};

use ainet_core::{Event, Response};
use ainet_model::terminal::{FETCH_EVERY, Pump};
use ainet_model::{Backend, Demo, Link, Msg, Store, sample};

/// The daemon's cap on output notices per pane (`pane_events` in ainetd).
const OUTPUT_EVERY: Duration = Duration::from_millis(50);

/// Runs `f` for about `budget` and gives the mean time per call.
fn time(budget: Duration, mut f: impl FnMut()) -> Duration {
    let start = Instant::now();
    let mut n = 0u32;
    while start.elapsed() < budget || n < 10 {
        f();
        n += 1;
    }
    start.elapsed() / n
}

/// What ainetd sends for `Screen { formatted: true }`: plain rows and the
/// formatted state, as one JSON line.
fn screen_line(parser: &vt100::Parser) -> Vec<u8> {
    let screen = parser.screen();
    let (_, cols) = screen.size();
    let mut rows: Vec<String> = screen.rows(0, cols).collect();
    while rows.last().is_some_and(|r| r.trim().is_empty()) {
        rows.pop();
    }
    ainet_core::protocol::encode(&Response::Screen {
        rows,
        ansi: Some(String::from_utf8_lossy(&screen.state_formatted()).into_owned()),
        size: screen.size(),
        styled_rows: None,
    })
}

/// What a stream of raw output would send for `bytes`: one JSON line.
fn bytes_line(bytes: &[u8]) -> Vec<u8> {
    ainet_core::protocol::encode(&Response::Event {
        event: Event::Bytes {
            agent: "0f6c2a9e-1d4b-4c7e-9a51-3b8e2f7d6c10".into(),
            data: String::from_utf8_lossy(bytes).into_owned(),
        },
    })
}

#[test]
#[ignore = "a measurement; run by hand"]
fn screen_refetch_against_raw_output() {
    let budget = Duration::from_millis(400);
    println!(
        "\n{:>8} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "size", "screen", "ansi B", "line B", "frame B", "f-line B", "serve", "apply", "feed"
    );
    for (rows, cols) in [(24u16, 90u16), (50, 200)] {
        for (name, bytes) in [
            ("sample", sample::screen(rows, cols, 12)),
            ("demo", demo_screen(rows, cols)),
        ] {
            let mut parser = vt100::Parser::new(rows, cols, 1000);
            parser.process(&bytes);
            let ansi = parser.screen().state_formatted();
            let line = screen_line(&parser);
            let frame = sample::frame(rows, cols, 12);
            let frame_line = bytes_line(&frame);
            // ainetd: format the screen and encode it.
            let serve = time(budget, || {
                black_box(screen_line(black_box(&parser)));
            });
            // The front end: decode it and build its copy of the screen.
            let mut store = Store::new();
            let apply = time(budget, || {
                let resp: Response = serde_json::from_slice(&line).unwrap();
                let Response::Screen { rows, ansi, size, .. } = resp else {
                    unreachable!()
                };
                black_box(store.apply(Msg::Term {
                    id: "a".into(),
                    ansi: ansi.unwrap_or_default(),
                    size,
                    rows,
                }));
            });
            // Streaming instead: decode a frame's bytes and feed them in.
            let feed = time(budget, || {
                let resp: Response = serde_json::from_slice(&frame_line).unwrap();
                let Response::Event {
                    event: Event::Bytes { data, .. },
                } = resp
                else {
                    unreachable!()
                };
                parser.process(data.as_bytes());
            });
            println!(
                "{:>8} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9.1?} {:>9.1?} {:>9.1?}",
                format!("{rows}x{cols}"),
                name,
                ansi.len(),
                line.len(),
                frame.len(),
                frame_line.len(),
                serve,
                apply,
                feed
            );
        }
    }
}

/// The demo's screen of its first agent, as the GUI's demo shows it.
fn demo_screen(rows: u16, cols: u16) -> Vec<u8> {
    let demo = Demo::new();
    let Response::Snapshot { agents, .. } = demo.snapshot() else {
        unreachable!()
    };
    demo.terminal(&agents[0], 40, rows, cols).into_bytes()
}

#[test]
#[ignore = "a measurement; run by hand"]
fn refetches_for_n_busy_terminals() {
    // What one refetch and one streamed frame move, per size.
    let sizes = [(24u16, 90u16), (50, 200)];
    let lines: Vec<(usize, usize)> = sizes
        .iter()
        .map(|&(rows, cols)| {
            let mut p = vt100::Parser::new(rows, cols, 0);
            p.process(&sample::screen(rows, cols, 12));
            (
                screen_line(&p).len(),
                bytes_line(&sample::frame(rows, cols, 12)).len(),
            )
        })
        .collect();
    println!(
        "\n{:>3} {:>10} {:>12} {:>12} {:>12} {:>12} {:>14}",
        "N", "fetches/s", "fetch 24x90", "fetch 50x200", "stream 24x90", "stream 50x200", "demo apply"
    );
    for n in [1usize, 4, 8, 16] {
        // Simulated time: every terminal prints all the time, so ainetd
        // sends an output notice every 50 ms each; the pump decides.
        let ids: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();
        let mut pump = Pump::new();
        let t0 = Instant::now();
        let secs = 10;
        let mut fetches = 0usize;
        for ms in 0..secs * 1000 {
            let now = t0 + Duration::from_millis(ms);
            for (i, id) in ids.iter().enumerate() {
                if (ms + i as u64 * 7).is_multiple_of(OUTPUT_EVERY.as_millis() as u64) {
                    pump.mark(id);
                }
            }
            fetches += pump.due(now).len();
        }
        let per_sec = fetches as f64 / secs as f64;
        // Streamed instead, each program drawing a frame as often as
        // ainetd would say it printed (every 50 ms), every frame sent.
        let frames = n as f64 * 1000. / OUTPUT_EVERY.as_millis() as f64;

        // The demo through a real link and store: what N refetches cost
        // the front end in a second (the demo draws its screens too).
        let (tx, rx) = std::sync::mpsc::channel();
        let (link, snapshot) = Link::new(Backend::Demo(Box::new(Demo::new())), tx);
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(snapshot.unwrap())));
        let agents: Vec<_> = store.agents.values().cloned().cycle().take(n).collect();
        let rounds = (per_sec / n as f64).round() as usize;
        let start = Instant::now();
        for tick in 0..rounds as u64 {
            for agent in &agents {
                link.fetch(agent, tick, (50, 200));
            }
            for msg in rx.try_iter() {
                store.apply(msg);
            }
        }
        let busy = start.elapsed();
        let kb = |count: f64, bytes: usize| format!("{:.0} KB/s", count * bytes as f64 / 1024.);
        println!(
            "{:>3} {:>10.0} {:>12} {:>12} {:>12} {:>12} {:>11.1?}/s",
            n,
            per_sec,
            kb(per_sec, lines[0].0),
            kb(per_sec, lines[1].0),
            kb(frames, lines[0].1),
            kb(frames, lines[1].1),
            busy
        );
    }
    println!(
        "(each terminal refetched at most every {FETCH_EVERY:?}, output notices every {OUTPUT_EVERY:?})"
    );
}
