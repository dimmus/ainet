//! `ainet-gui`: the desktop front end, a Zed-style workspace on GPUI and
//! Zed's `ui` and `theme` crates (docs/GUI-PLAN.md). It is a client of
//! ainetd like the TUI, sharing its state model (`ainet-model`), and starts
//! the daemon through the console binary `ainet`, never through itself.

mod args;
mod assets;
mod decorations;
mod docks;
mod fleet;
mod settings;
mod sink;
mod status_bar;
mod title_bar;
mod workspace;

use clap::Parser;
use gpui::{
    App, AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions,
    actions, point, px, size,
};
use theme::ActiveTheme;

use crate::args::Args;
use crate::fleet::FleetModel;
use crate::workspace::Workspace;

actions!(
    workspace,
    [
        /// Closes the window.
        CloseWindow
    ]
);
actions!(
    ainet,
    [
        /// Quits the app.
        Quit
    ]
);

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    // Without a display server GPUI falls back to running headless, which
    // would leave ainet-gui running with no window at all.
    if !has_display(&std::env::var_os("WAYLAND_DISPLAY"), &std::env::var_os("DISPLAY")) {
        eprintln!(
            "ainet-gui: no display: run it in a Wayland or X11 session (WAYLAND_DISPLAY and DISPLAY are unset)"
        );
        return std::process::ExitCode::FAILURE;
    }
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            init(cx);
            if let Err(e) = open_window(&args, cx) {
                eprintln!("ainet-gui: cannot open a window: {e:#}");
                cx.quit();
            }
        });
    std::process::ExitCode::SUCCESS
}

/// Whether a Wayland or X11 display is named in the environment. Only Linux
/// and the BSDs have this question; elsewhere there is always a display.
fn has_display(wayland: &Option<std::ffi::OsString>, x11: &Option<std::ffi::OsString>) -> bool {
    let set = |v: &Option<std::ffi::OsString>| v.as_ref().is_some_and(|v| !v.is_empty());
    !cfg!(any(target_os = "linux", target_os = "freebsd")) || set(wayland) || set(x11)
}

/// Theme, appearance settings, app-wide actions and their keys.
fn init(cx: &mut App) {
    theme::init(theme::LoadThemes::JustBase, cx);
    settings::init(settings::Appearance::default(), cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("ctrl-shift-w", CloseWindow, Some("Workspace")),
    ]);
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// The fleet the window shows: the demo, or ainetd at the chosen socket.
fn fleet(args: &Args, cx: &mut App) -> gpui::Entity<FleetModel> {
    if args.demo {
        cx.new(FleetModel::demo)
    } else {
        let socket = args.socket();
        cx.new(|cx| FleetModel::daemon(socket, cx))
    }
}

fn open_window(args: &Args, cx: &mut App) -> anyhow::Result<()> {
    let fleet = fleet(args, cx);
    let view = args.view.clone();
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    cx.open_window(window_options(bounds, cx), |window, cx| {
        window.activate_window();
        cx.new(|cx| Workspace::new(fleet, view, cx))
    })?;
    Ok(())
}

fn window_options(bounds: Bounds<gpui::Pixels>, cx: &App) -> WindowOptions {
    // As Zed: client-side decorations unless asked for the server's
    // (`AINET_WINDOW_DECORATIONS=server`, Zed's `ZED_WINDOW_DECORATIONS`).
    let decorations = match std::env::var("AINET_WINDOW_DECORATIONS").as_deref() {
        Ok("server") => WindowDecorations::Server,
        _ => WindowDecorations::Client,
    };
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("ainet".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.0), px(9.0))),
        }),
        app_owns_titlebar_drag: true,
        window_background: cx.theme().window_background_appearance(),
        app_id: Some("ainet-gui".into()),
        window_decorations: Some(decorations),
        window_min_size: Some(size(px(480.), px(320.))),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::status_bar::Status;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(init);
    }

    #[test]
    fn needs_a_display_server() {
        let some = |s: &str| Some(std::ffi::OsString::from(s));
        assert!(has_display(&some("wayland-0"), &None));
        assert!(has_display(&None, &some(":0")));
        assert!(!has_display(&None, &None));
        assert!(!has_display(&some(""), &some("")));
    }

    #[gpui::test]
    async fn demo_workspace_shows_the_fleet_numbers(cx: &mut TestAppContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|_, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, cx)
        });
        cx.run_until_parked();

        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        let (status, tally, rows, selected) = fleet.read_with(cx, |f, _| {
            (
                Status::of(f),
                f.store.tally(),
                f.store.rows.len(),
                f.store.selected.clone(),
            )
        });
        assert_eq!(status.connection, fleet::Connection::Demo);
        assert!(tally.live > 0, "the demo has agents");
        assert!(rows > 0 && selected.is_some());
        // The same numbers as the TUI header, from the same Store.
        let counts: Vec<String> = status.counts().into_iter().map(|(t, _)| t).collect();
        assert_eq!(counts[0], format!("{} agents", tally.live));
        assert!(counts.contains(&format!("{} working", tally.working)));
        if tally.waiting > 0 {
            assert!(counts.contains(&format!("{} waiting", tally.waiting)));
        }

        // The demo moves on with the ticker, through the channel sink.
        cx.executor().advance_clock(fleet::TICK * 50);
        cx.run_until_parked();
        let ticked = fleet.read_with(cx, |f, _| f.store.tally());
        assert!(ticked.live > 0);
    }

    #[gpui::test]
    async fn selecting_an_agent_updates_the_store(cx: &mut TestAppContext) {
        init_test(cx);
        let fleet = cx.new(FleetModel::demo);
        let other = fleet.read_with(cx, |f, _| {
            f.store
                .agents
                .keys()
                .find(|id| Some(*id) != f.store.selected.as_ref())
                .cloned()
                .expect("the demo has two agents")
        });
        fleet.update(cx, |f, cx| f.select(&other, cx));
        fleet.read_with(cx, |f, _| assert_eq!(f.store.selected.as_ref(), Some(&other)));
        fleet.update(cx, |f, cx| f.select("no-such-agent", cx));
        fleet.read_with(cx, |f, _| assert_eq!(f.store.selected.as_ref(), Some(&other)));
    }
}
