//! The ainet terminal UI.
//!
//! Den's design (a wordmark header, Fleet and Inbox views, one pixel critter
//! per agent) on ratatui, with tachyonfx for transitions: the screen
//! coalesces in on start, views sweep in, the details pane fades to the new
//! agent, the Inbox tab glows when something needs you, and the screen
//! dissolves on quit.

mod app;
mod sprite;
mod term;
mod theme;
mod view;
mod view_comms;
mod view_terms;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyEventKind,
};
use ratatui::crossterm::execute;

use ainet_model::Backend;
pub use ainet_model::Demo;
pub use app::App;
use app::TICK;

pub struct Options {
    /// Run against the pretend fleet instead of ainetd.
    pub demo: bool,
    pub socket: PathBuf,
    pub mouse: bool,
}

pub fn run(opts: Options) -> Result<()> {
    let backend = if opts.demo {
        Backend::Demo(Box::default())
    } else {
        Backend::Daemon(opts.socket)
    };
    let view = std::env::var("AINET_VIEW").ok();
    let mut app = App::new(backend, view.as_deref());

    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste)?;
    if opts.mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    let result = event_loop(&mut terminal, &mut app);
    if opts.mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_frame = Instant::now();
    let mut last_tick = Instant::now();
    loop {
        app.drain();
        let elapsed = last_frame.elapsed();
        last_frame = Instant::now();
        terminal.draw(|f| {
            view::draw(f, app);
            let area = f.area();
            app.fx.process_effects(elapsed.into(), f.buffer_mut(), area);
        })?;
        app.sync_sizes();
        app.pump();
        if app.done() {
            return Ok(());
        }
        // ~60 fps while an effect plays, ~16 fps otherwise: enough for the
        // critters, cheap when nothing moves.
        let budget = if app.fx.is_running() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(60)
        };
        if event::poll(budget)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(m) => app.on_mouse(m),
                Event::Paste(text) => app.on_paste(text),
                _ => {}
            }
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            app.on_tick();
        }
    }
}

/// Draws the demo after `ticks` ticks and the given keys, into a buffer:
/// for snapshot tests and screenshots without a terminal.
pub fn demo_buffer(
    width: u16,
    height: u16,
    ticks: u64,
    keys: &[ratatui::crossterm::event::KeyEvent],
) -> Result<ratatui::buffer::Buffer> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = App::new(Backend::Demo(Box::default()), None);
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    // One frame first, so effects and mouse hits know where things are.
    terminal.draw(|f| view::draw(f, &mut app))?;
    for key in keys {
        app.on_key(*key);
    }
    for _ in 0..ticks {
        app.on_tick();
        app.drain();
    }
    // Let the opening and transition effects finish, and the terminals
    // take their tiles' sizes and fill in.
    app.fx = tachyonfx::EffectManager::default();
    for _ in 0..2 {
        terminal.draw(|f| view::draw(f, &mut app))?;
        app.sync_sizes();
        app.pump();
        app.drain();
    }
    terminal.draw(|f| view::draw(f, &mut app))?;
    Ok(terminal.backend().buffer().clone())
}

/// The demo as plain text, one line per row.
pub fn render_demo_frame(width: u16, height: u16, ticks: u64) -> Result<String> {
    let buf = demo_buffer(width, height, ticks, &[])?;
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_frame_draws_the_fleet() {
        let frame = super::render_demo_frame(140, 44, 3).unwrap();
        assert!(frame.contains("Agents"), "{frame}");
        assert!(frame.contains("TECH LEAD"), "{frame}");
        assert!(frame.contains("Details"), "{frame}");
    }
}
