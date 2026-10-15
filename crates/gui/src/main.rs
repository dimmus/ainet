//! `ainet-gui`: the desktop front end, a Zed-style workspace on GPUI and
//! Zed's `ui` and `theme` crates (docs/GUI-PLAN.md). It is a client of
//! ainetd like the TUI, sharing its state model (`ainet-model`), and starts
//! the daemon through the console binary `ainet`, never through itself.

mod actions;
mod args;
mod assets;
mod decorations;
mod docks;
mod fleet;
mod keymap;
mod problems;
mod settings;
mod sink;
mod status_bar;
mod themes;
mod title_bar;
mod watch;
mod workspace;
mod zed_theme;

use clap::Parser;
use gpui::{
    App, AppContext, Bounds, PromptLevel, TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions,
    point, px, size,
};
use theme::{ActiveTheme, SystemAppearance};

use crate::actions::Quit;
use crate::args::Args;
use crate::assets::Assets;
use crate::fleet::FleetModel;
use crate::workspace::Workspace;

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
            init(ConfigFiles::user(), cx);
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

/// Where the user's config is read from; tests read none.
#[derive(Clone, Default)]
struct ConfigFiles {
    settings: Option<std::path::PathBuf>,
    keymap: Option<std::path::PathBuf>,
}

impl ConfigFiles {
    fn user() -> Self {
        ConfigFiles {
            settings: Some(settings::settings_file()),
            keymap: Some(keymap::keymap_file()),
        }
    }
}

/// Fonts, themes, settings, the keymap and the app-wide actions.
fn init(files: ConfigFiles, cx: &mut App) {
    let mut startup = themes::init(cx);
    if let Err(e) = Assets::load_fonts(cx) {
        startup.push(format!("{e:#}"));
    }
    if !startup.is_empty() {
        problems::set(problems::Source::Startup, Some(startup.join("; ")), cx);
    }
    settings::init(files.settings, cx);
    keymap::init(files.keymap, cx);

    cx.on_action(quit);
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// Quits, asking first in the active window if `confirm_quit` is set.
fn quit(_: &Quit, cx: &mut App) {
    let window = cx.active_window().or_else(|| cx.windows().into_iter().next());
    let Some(window) = window.filter(|_| settings::get(cx).confirm_quit) else {
        cx.quit();
        return;
    };
    let asked = window.update(cx, |_, window, cx| {
        window.prompt(PromptLevel::Info, "Quit ainet?", None, &["Quit", "Cancel"], cx)
    });
    match asked {
        Ok(answer) => cx
            .spawn(async move |cx| {
                if answer.await == Ok(0) {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach(),
        Err(_) => cx.quit(),
    }
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
        let workspace = cx.new(|cx| Workspace::new(fleet, view, window, cx));
        // As Zed: follow the system's light or dark appearance.
        window
            .observe_window_appearance(|window, cx| {
                *SystemAppearance::global_mut(cx) = SystemAppearance(window.appearance().into());
                settings::apply_theme(cx);
            })
            .detach();
        workspace
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
        cx.update(|cx| init(ConfigFiles::default(), cx));
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
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
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

    #[gpui::test]
    async fn bundled_themes_are_registered_and_deepharness_is_the_default(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            let registry = theme::ThemeRegistry::global(cx);
            let names = registry.list_names();
            for name in [
                "One Dark",
                "One Light",
                "Ayu Dark",
                "Gruvbox Dark",
                themes::DEEPHARNESS,
            ] {
                assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
            }
            let active = cx.theme().name.clone();
            assert!(active == themes::default_dark(&registry), "{active}");
            assert!(problems::lines(cx).is_empty(), "{:?}", problems::lines(cx));
        });
    }

    #[gpui::test]
    async fn settings_reload_switches_theme_and_survives_bad_files(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            settings::reload(r#"{ "theme": "One Light", "ui_font_size": 14, }"#, cx);
            assert_eq!(cx.theme().name, "One Light");
            assert_eq!(settings::ui_font(cx).1, px(14.));

            // A broken file keeps the last good settings and says why.
            settings::reload(r#"{ "theme": "Ayu Dark", "ui_font_size": "big" }"#, cx);
            assert_eq!(cx.theme().name, "One Light");
            assert_eq!(settings::ui_font(cx).1, px(14.));
            let problems = problems::lines(cx);
            assert!(
                problems.len() == 1 && problems[0].starts_with("settings.json"),
                "{problems:?}"
            );

            // An unknown theme falls back, and says so.
            settings::reload(r#"{ "theme": "No Such Theme" }"#, cx);
            assert_eq!(cx.theme().name, themes::DEEPHARNESS);
            assert!(problems::lines(cx).iter().any(|p| p.contains("No Such Theme")));

            settings::reload("", cx);
            assert!(problems::lines(cx).is_empty());
            assert_eq!(settings::ui_font(cx).1, px(16.));
        });
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    async fn default_keys_toggle_docks_and_step_through_the_fleet(cx: &mut TestAppContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        let docks = |cx: &mut gpui::VisualTestContext| workspace.read_with(cx, |w, _| w.docks());
        assert_eq!(docks(cx), workspace::Docks::default());
        cx.simulate_keystrokes("ctrl-b");
        assert!(!docks(cx).left && docks(cx).right);
        cx.simulate_keystrokes("ctrl-alt-b");
        assert!(!docks(cx).right, "{:?}", docks(cx));
        cx.simulate_keystrokes("ctrl-j");
        assert!(!docks(cx).bottom, "{:?}", docks(cx));
        cx.simulate_keystrokes("ctrl-b ctrl-alt-b ctrl-j");
        assert_eq!(docks(cx), workspace::Docks::default());
        // A setting can move the Fleet to the right; ctrl-alt-b then hides it.
        cx.update(|_, cx| {
            settings::reload(r#"{ "fleet_panel": { "dock": "right" } }"#, cx);
        });
        cx.simulate_keystrokes("ctrl-alt-b");
        assert!(docks(cx).left && !docks(cx).right);
        cx.simulate_keystrokes("ctrl-alt-b");
        cx.update(|_, cx| settings::reload("", cx));

        // The Fleet panel has the keyboard: down and up move the selection.
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        let selected =
            |cx: &mut gpui::VisualTestContext| fleet.read_with(cx, |f, _| f.store.selected.clone());
        let first = selected(cx);
        cx.simulate_keystrokes("down");
        let second = selected(cx);
        assert_ne!(first, second);
        cx.simulate_keystrokes("up");
        assert_eq!(selected(cx), first);
    }
}
