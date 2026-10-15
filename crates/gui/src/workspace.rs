//! The window's root view, laid out like Zed's workspace: title bar, the
//! Fleet panel (left dock), the center pane over the bottom dock, the Agent
//! panel (right dock), and the status bar, inside client-side decorations.

use gpui::Entity;
use ui::prelude::*;

use crate::args::View;
use crate::decorations::client_side_decorations;
use crate::docks;
use crate::fleet::FleetModel;
use crate::status_bar::StatusBar;
use crate::title_bar::TitleBar;
use crate::{CloseWindow, settings};

pub struct Workspace {
    fleet: Entity<FleetModel>,
    title_bar: Entity<TitleBar>,
    status_bar: Entity<StatusBar>,
    /// The view asked for on the command line.
    view: Option<View>,
}

impl Workspace {
    pub fn new(fleet: Entity<FleetModel>, view: Option<View>, cx: &mut Context<Self>) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        let title_bar = cx.new(|cx| TitleBar::new(fleet.clone(), cx));
        let status_bar = cx.new(|cx| StatusBar::new(fleet.clone(), cx));
        Workspace {
            fleet,
            title_bar,
            status_bar,
            view,
        }
    }

    #[cfg(test)]
    pub fn fleet(&self) -> &Entity<FleetModel> {
        &self.fleet
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ui_font, ui_font_size) = settings::ui_font(cx);
        window.set_rem_size(ui_font_size);
        let colors = cx.theme().colors();
        let fleet = self.fleet.read(cx);

        let body = h_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(docks::fleet_panel(&self.fleet, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(docks::center(self.view.as_ref(), cx))
                    .child(docks::bottom_dock(fleet, cx)),
            )
            .child(docks::agent_panel(fleet, cx));

        let content = v_flex()
            .key_context("Workspace")
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
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
