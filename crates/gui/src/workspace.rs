//! The window's root view, laid out like Zed's workspace: title bar, the
//! Fleet panel (left dock, or right by setting), the center pane over the
//! bottom dock, the Agent panel (right dock), and the status bar, inside
//! client-side decorations. It handles the workspace and fleet actions that
//! work already: toggling docks and stepping through the Fleet.

use gpui::{Entity, FocusHandle, Focusable};
use ui::prelude::*;

use crate::actions::{
    CloseWindow, OpenKeymap, OpenSettings, SelectNext, SelectPrev, ToggleBottomDock, ToggleCritterSize,
    ToggleLeftDock, ToggleRightDock,
};
use crate::args::View;
use crate::decorations::client_side_decorations;
use crate::docks;
use crate::fleet::FleetModel;
use crate::keymap;
use crate::settings::{self, DockSide};
use crate::status_bar::StatusBar;
use crate::title_bar::TitleBar;

/// Which docks are open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Docks {
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Default for Docks {
    fn default() -> Self {
        Docks {
            left: true,
            right: true,
            bottom: true,
        }
    }
}

pub struct Workspace {
    fleet: Entity<FleetModel>,
    title_bar: Entity<TitleBar>,
    status_bar: Entity<StatusBar>,
    /// The view asked for on the command line.
    view: Option<View>,
    docks: Docks,
    focus: FocusHandle,
    /// The Fleet panel's, so its keys (`FleetPanel` context) work.
    fleet_focus: FocusHandle,
}

impl Workspace {
    pub fn new(
        fleet: Entity<FleetModel>,
        view: Option<View>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        let title_bar = cx.new(|cx| TitleBar::new(fleet.clone(), cx));
        let status_bar = cx.new(|cx| StatusBar::new(fleet.clone(), cx));
        let fleet_focus = cx.focus_handle();
        // The Fleet has the keyboard until a terminal or tab takes it.
        window.focus(&fleet_focus, cx);
        Workspace {
            fleet,
            title_bar,
            status_bar,
            view,
            docks: Docks::default(),
            focus: cx.focus_handle(),
            fleet_focus,
        }
    }

    #[cfg(test)]
    pub fn fleet(&self) -> &Entity<FleetModel> {
        &self.fleet
    }

    #[cfg(test)]
    pub fn docks(&self) -> Docks {
        self.docks
    }

    /// The left and right docks as the user sees them: the Fleet panel is
    /// on the side the settings put it, and the Agent panel opposite. As in
    /// Zed, opening the Fleet focuses it, and closing it hands the keyboard
    /// back to the workspace so its keys keep working.
    fn toggle_side(&mut self, left: bool, window: &mut Window, cx: &mut Context<Self>) {
        let open = if left {
            &mut self.docks.left
        } else {
            &mut self.docks.right
        };
        *open = !*open;
        let open = *open;
        let fleet_side = settings::get(cx).fleet_panel.dock == DockSide::Left;
        if left == fleet_side {
            if open {
                window.focus(&self.fleet_focus, cx);
            } else if self.fleet_focus.contains_focused(window, cx) {
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    /// Opens a config file for editing and says where it is.
    fn open_config(&mut self, file: std::path::PathBuf, template: &str, cx: &mut Context<Self>) {
        let (text, error) = match settings::open_file(&file, template, cx) {
            Ok(()) => (format!("opened {}", file.display()), false),
            Err(e) => (e, true),
        };
        self.fleet.update(cx, |fleet, cx| fleet.show(text, error, cx));
    }

    fn select(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.fleet.update(cx, |fleet, cx| fleet.move_selection(delta, cx));
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ui_font, ui_font_size) = settings::ui_font(cx);
        window.set_rem_size(ui_font_size);
        let colors = cx.theme().colors();
        let panel = settings::get(cx).fleet_panel.clone();
        let fleet = self.fleet.read(cx);
        let fleet_on_left = panel.dock == DockSide::Left;
        let side = |left: bool| if left { self.docks.left } else { self.docks.right };
        let focused = self.fleet_focus.contains_focused(window, cx);
        let fleet_panel = side(fleet_on_left).then(|| {
            docks::fleet_panel(
                &self.fleet,
                &self.fleet_focus,
                px(panel.width),
                fleet_on_left,
                focused,
                cx,
            )
        });
        let agent_panel = side(!fleet_on_left).then(|| docks::agent_panel(fleet, !fleet_on_left, cx));
        let (left, right) = if fleet_on_left {
            (
                fleet_panel.map(IntoElement::into_any_element),
                agent_panel.map(IntoElement::into_any_element),
            )
        } else {
            (
                agent_panel.map(IntoElement::into_any_element),
                fleet_panel.map(IntoElement::into_any_element),
            )
        };

        let body = h_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .children(left)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(docks::center(self.view.as_ref(), cx))
                    .when(self.docks.bottom, |el| el.child(docks::bottom_dock(fleet, cx))),
            )
            .children(right);

        let content = v_flex()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| this.toggle_side(true, window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleRightDock, window, cx| this.toggle_side(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleBottomDock, _, cx| {
                this.docks.bottom = !this.docks.bottom;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.open_config(settings::settings_file(), settings::TEMPLATE, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenKeymap, _, cx| {
                let file = keymap::user_file(cx).unwrap_or_else(keymap::keymap_file);
                this.open_config(file, keymap::TEMPLATE, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.select(-1, cx)))
            .on_action(|_: &ToggleCritterSize, _, cx| {
                let mut next = settings::get(cx).clone();
                next.critters.size = next.critters.size.toggled();
                settings::set(next, cx);
            })
            .size_full()
            .font(ui_font)
            .text_color(colors.text)
            .bg(colors.background)
            .child(self.title_bar.clone())
            .child(body)
            .child(self.status_bar.clone());

        client_side_decorations(content, window, cx)
    }
}
