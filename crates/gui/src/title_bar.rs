//! The title bar, after Zed's `platform_title_bar`: drags and double-click
//! zooms the window, carries the window controls when decorations are
//! client-side, and shows the app name, the rig switcher (the selected
//! agent's rig; a click picks another, `fleet::SwitchRig`), the requests
//! waiting on you (a badge that glows when one arrives and opens the
//! Inbox) and the connection.

use gpui::{
    Decorations, Entity, MouseButton, WeakEntity, WindowButton, WindowButtonLayout, WindowControlArea, svg,
};
use theme::{CLIENT_SIDE_DECORATION_BORDER, CLIENT_SIDE_DECORATION_ROUNDING};
use ui::utils::platform_title_bar_height;
use ui::{Indicator, Tooltip, prelude::*};

use ainet_model::Store;

use crate::actions::SwitchRig;
use crate::fleet::{Connection, FleetModel};
use crate::inbox::waiting_badge;
use crate::quit;
use crate::rig_modals::RigAction;
use crate::workspace::Workspace;

pub struct TitleBar {
    fleet: Entity<FleetModel>,
    workspace: WeakEntity<Workspace>,
    /// Set on mouse down; the first move after it starts a window move.
    should_move: bool,
}

impl TitleBar {
    pub fn new(fleet: Entity<FleetModel>, workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        TitleBar {
            fleet,
            workspace,
            should_move: false,
        }
    }
}

/// The dot and words for a connection state.
pub fn connection_badge(connection: &Connection) -> (Color, &'static str) {
    match connection {
        Connection::Demo => (Color::Info, "demo fleet"),
        Connection::Starting => (Color::Muted, "starting ainetd"),
        Connection::Connected => (Color::Success, "connected"),
        Connection::Offline => (Color::Warning, "offline"),
        Connection::Failed(_) => (Color::Error, "ainetd not running"),
        Connection::Incompatible(_) => (Color::Error, "ainetd incompatible"),
    }
}

/// The rig switcher's words: the rig commands act on (the selected
/// agent's, or the only one).
pub fn rig_label(store: &Store) -> String {
    match store.rig_for(None, None) {
        Some(rig) => rig,
        None if store.rigs.is_empty() => "no rigs".into(),
        None => "choose a rig".into(),
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let decorations = window.window_decorations();
        let client_side = matches!(decorations, Decorations::Client { .. });
        let height = platform_title_bar_height(window);
        let colors = cx.theme().colors();
        let background = if window.is_window_active() && !self.should_move {
            colors.title_bar_background
        } else {
            colors.title_bar_inactive_background
        };
        let fleet = self.fleet.read(cx);
        let (dot, words) = connection_badge(&fleet.connection());
        let (waiting, alerts) = (fleet.tally().waiting, fleet.alerts());
        let rig = rig_label(&fleet.store);
        let layout = client_side.then(|| button_layout(cx)).flatten();
        let controls = |side: &[Option<WindowButton>]| {
            let buttons: Vec<WindowButton> = side.iter().flatten().copied().collect();
            (!buttons.is_empty()).then(|| window_controls(buttons, window, cx))
        };
        let left_controls = layout.and_then(|l| controls(&l.left));
        let right_controls = layout.and_then(|l| controls(&l.right));
        let maximizable = window.window_controls().maximize && window.is_resizable();

        let start = h_flex()
            .gap_2()
            .child(Label::new("ainet").weight(gpui::FontWeight::SEMIBOLD))
            .child({
                let workspace = self.workspace.clone();
                // Not a window drag: a click here switches rig.
                div()
                    .debug_selector(|| "TITLE-RIG-SWITCHER".into())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("rig-switcher", rig)
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .end_icon(
                                Icon::new(IconName::ChevronDown)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .tooltip(Tooltip::for_action_title("Switch Rig", &SwitchRig))
                            .on_click(move |_, window, cx| {
                                let _ =
                                    workspace.update(cx, |w, cx| w.rig_action(RigAction::Select, window, cx));
                            }),
                    )
            });
        let end = h_flex()
            .gap_3()
            .pr_2()
            .when(waiting > 0, |el| {
                let workspace = self.workspace.clone();
                el.child(
                    // Not a window drag: a click here opens the Inbox.
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(waiting_badge(
                            "title-waiting",
                            waiting,
                            alerts,
                            move |_, window, cx| {
                                let _ = workspace.update(cx, |w, cx| w.open_inbox(None, window, cx));
                            },
                            cx,
                        )),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(Indicator::dot().color(dot))
                    .child(Label::new(words).size(LabelSize::Small).color(Color::Muted)),
            );

        h_flex()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .bg(background)
            .border_b_1()
            .border_color(colors.border)
            .content_stretch()
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.should_move = false))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = false),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = true),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.should_move {
                    this.should_move = false;
                    window.start_window_move();
                }
            }))
            .when(maximizable, |el| {
                el.on_click(|event, window, _| {
                    if event.click_count() == 2 {
                        window.zoom_window();
                    }
                })
            })
            .when(client_side, |el| {
                el.on_mouse_down(MouseButton::Right, |ev, window, _| {
                    window.show_window_menu(ev.position)
                })
            })
            .map(|el| match decorations {
                Decorations::Server => el,
                // Pulled over the frame's border, so the rounded corners have
                // no transparent gap (as Zed does).
                Decorations::Client { tiling } => el
                    .when(!(tiling.top || tiling.right), |el| {
                        el.rounded_tr(CLIENT_SIDE_DECORATION_ROUNDING)
                    })
                    .when(!(tiling.top || tiling.left), |el| {
                        el.rounded_tl(CLIENT_SIDE_DECORATION_ROUNDING)
                    })
                    .mt(-CLIENT_SIDE_DECORATION_BORDER),
            })
            .map(|el| match left_controls {
                Some(controls) => el.child(controls),
                None => el.pl_2(),
            })
            .child(
                h_flex()
                    .flex_1()
                    .justify_between()
                    .overflow_x_hidden()
                    .child(start)
                    .child(end),
            )
            .children(right_controls)
    }
}

/// The window buttons the desktop asks for (GNOME's `button-layout`), else
/// minimize, maximize and close on the right.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn button_layout(cx: &App) -> Option<WindowButtonLayout> {
    Some(
        cx.button_layout()
            .unwrap_or_else(WindowButtonLayout::linux_default),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn button_layout(_cx: &App) -> Option<WindowButtonLayout> {
    None
}

/// Minimize, maximize and close, drawn like Zed's Linux window controls.
fn window_controls(buttons: Vec<WindowButton>, window: &Window, cx: &App) -> AnyElement {
    let supported = window.window_controls();
    let maximized = window.is_maximized();
    let colors = cx.theme().colors();
    let (icon, icon_hover, background_hover, disabled) = (
        colors.icon,
        colors.icon_muted,
        colors.ghost_element_hover,
        colors.icon_disabled,
    );
    h_flex()
        .id("window-controls")
        .gap_3()
        .px_3()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(
            buttons
                .into_iter()
                .filter(|b| match b {
                    WindowButton::Minimize => supported.minimize,
                    WindowButton::Maximize => supported.maximize,
                    WindowButton::Close => true,
                })
                .map(|button| {
                    let (glyph, enabled) = match button {
                        WindowButton::Minimize => (IconName::GenericMinimize, window.is_minimizable()),
                        WindowButton::Maximize if maximized => {
                            (IconName::GenericRestore, window.is_resizable())
                        }
                        WindowButton::Maximize => (IconName::GenericMaximize, window.is_resizable()),
                        WindowButton::Close => (IconName::GenericClose, true),
                    };
                    h_flex()
                        .id(button.id())
                        .group("window-control")
                        .justify_center()
                        .content_center()
                        .rounded_2xl()
                        .w_5()
                        .h_5()
                        .when(enabled, |el| {
                            el.cursor_pointer()
                                .hover(|el| el.bg(background_hover))
                                .active(|el| el.bg(background_hover))
                        })
                        .child(
                            svg()
                                .size_4()
                                .flex_none()
                                .path(glyph.path())
                                .text_color(if enabled { icon } else { disabled })
                                .when(enabled, |el| {
                                    el.group_hover("window-control", |el| el.text_color(icon_hover))
                                }),
                        )
                        .on_mouse_move(|_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            if !enabled {
                                return;
                            }
                            match button {
                                WindowButton::Minimize => window.minimize_window(),
                                WindowButton::Maximize => window.zoom_window(),
                                // Directly, not as an action: the action needs
                                // the focus inside the workspace.
                                WindowButton::Close => quit::close_window(window, cx),
                            }
                        })
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}
