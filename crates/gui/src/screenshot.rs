//! Pictures of the app for a person to look at: the demo workspace drawn
//! by GPUI's own renderer, offscreen, as PNGs in `target/screenshots/`.
//!
//! Ignored by default, since it needs a GPU (or a software Vulkan driver
//! such as lavapipe) that wgpu can draw on without a window, which CI
//! machines lack. `just screenshot-gui` runs it. Nothing is compared: the
//! pictures are for review, as Zed's visual tests are.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext, HeadlessAppContext, WindowHandle, px, size};

use crate::fleet::FleetModel;
use crate::topology::SubView;
use crate::workspace::Workspace;

/// What a picture shows, opened in the workspace.
type OpenView = fn(&mut Workspace, &mut gpui::Window, &mut gpui::Context<Workspace>);

/// Where the pictures go: the shared `target/` (see `.cargo/config.toml`).
fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots")
}

/// Lets the demo move on and the window draw what it now shows.
fn settle(cx: &HeadlessAppContext) {
    cx.run_until_parked();
    cx.advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
}

fn capture(cx: &mut HeadlessAppContext, window: WindowHandle<Workspace>, name: &str) {
    settle(cx);
    let image = cx
        .capture_screenshot(window.into())
        .unwrap_or_else(|e| panic!("cannot draw {name}: {e:#}"));
    let file = out_dir().join(format!("{name}.png"));
    image
        .save(&file)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", file.display()));
    eprintln!("wrote {}", file.display());
}

#[test]
#[ignore = "needs a GPU wgpu can draw on without a window; run it with `just screenshot-gui`"]
fn the_demo_workspace() {
    // As the Linux app's text system: the bundled fonts first, the
    // system's for what they lack (Claude Code's ⏺ and ⎿, for one).
    let text = Arc::new(gpui_wgpu::CosmicTextSystem::new("IBM Plex Sans"));
    let mut cx = HeadlessAppContext::with_platform(
        text,
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| crate::init(crate::ConfigFiles::default(), cx));
    let window = cx
        .open_window(size(px(1600.), px(1000.)), |window, cx| {
            let fleet = cx.new(FleetModel::demo);
            cx.new(|cx| Workspace::new(fleet, None, window, cx))
        })
        .expect("a window");
    std::fs::create_dir_all(out_dir()).expect("target/screenshots");

    capture(&mut cx, window, "workspace");
    let views: [(&str, OpenView); 7] = [
        ("terminal", |w, window, cx| {
            let selected = w.fleet().read(cx).store.selected.clone();
            if let Some(id) = selected {
                w.open_terminal(&id, window, cx);
            }
        }),
        ("inbox", |w, window, cx| {
            w.open_inbox(None, window, cx);
        }),
        ("map", |w, window, cx| {
            w.open_map(window, cx);
        }),
        ("topology", |w, window, cx| {
            w.open_topology(window, cx);
        }),
        ("topology-recent", |w, window, cx| {
            let t = w.open_topology(window, cx);
            t.update(cx, |t, cx| t.show_view(SubView::Recent, cx));
        }),
        ("topology-overview", |w, window, cx| {
            let t = w.open_topology(window, cx);
            t.update(cx, |t, cx| t.show_view(SubView::Overview, cx));
        }),
        ("topology-health", |w, window, cx| {
            let t = w.open_topology(window, cx);
            t.update(cx, |t, cx| t.show_view(SubView::Health, cx));
        }),
    ];
    for (name, open) in views {
        window.update(&mut cx, open).expect("the window is open");
        capture(&mut cx, window, name);
    }
}
