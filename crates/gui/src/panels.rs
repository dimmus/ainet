//! What the docks' panels share (their title strip, state colours), and
//! the bottom dock's Activity panel, which still counts the queue, chat and
//! messages until phase 4 gives it its views. The Fleet and Agent panels
//! have modules of their own.

use ainet_core::AgentState;
use gpui::{Entity, FocusHandle, Focusable};
use ui::prelude::*;

use crate::dock::Panel;
use crate::fleet::FleetModel;

/// Default dock sizes, Zed's.
pub const SIDE_DOCK_WIDTH: Pixels = px(260.);
const BOTTOM_DOCK_HEIGHT: Pixels = px(160.);

/// How an agent's state is coloured, like the TUI's state glyphs.
pub fn state_color(state: AgentState) -> Color {
    match state {
        AgentState::NeedsYou => Color::Warning,
        AgentState::Working => Color::Success,
        AgentState::Idle => Color::Muted,
        AgentState::Failed => Color::Error,
        AgentState::Gone => Color::Disabled,
        AgentState::Unknown => Color::Placeholder,
    }
}

/// A panel's title strip; its border lights up while the panel has the
/// keyboard, as Zed's do.
pub fn header(title: &'static str, icon: IconName, focused: bool, cx: &App) -> Div {
    let colors = cx.theme().colors();
    h_flex()
        .flex_none()
        .h(px(32.))
        .px_2()
        .gap_2()
        .border_b_1()
        .border_color(if focused {
            colors.panel_focused_border
        } else {
            colors.border_variant
        })
        .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
        .child(Label::new(title).size(LabelSize::Small).color(Color::Muted))
}

/// The bottom dock: Queue, Chatroom and Events log, for now as counts.
pub struct ActivityPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
}

impl ActivityPanel {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        ActivityPanel {
            fleet,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for ActivityPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ActivityPanel {
    fn persistent_name() -> &'static str {
        "ActivityPanel"
    }

    fn default_size(&self, _: &App) -> Pixels {
        BOTTOM_DOCK_HEIGHT
    }

    fn icon(&self) -> IconName {
        IconName::ListTodo
    }

    fn icon_tooltip(&self) -> &'static str {
        "Activity Panel"
    }
}

impl Render for ActivityPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        let store = &self.fleet.read(cx).store;
        let tab = |name: &'static str, active: bool| {
            Label::new(name)
                .size(LabelSize::Small)
                .color(if active { Color::Default } else { Color::Muted })
        };
        let colors = cx.theme().colors();
        v_flex()
            .id("activity-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(32.))
                    .px_2()
                    .gap_3()
                    .border_b_1()
                    .border_color(if focused {
                        colors.panel_focused_border
                    } else {
                        colors.border_variant
                    })
                    .child(tab("Queue", true))
                    .child(tab("Chatroom", false))
                    .child(tab("Events", false)),
            )
            .child(
                div().p_3().child(
                    Label::new(format!(
                        "{} queue items · {} chat posts · {} messages",
                        store.queue.len(),
                        store.chat.len(),
                        store.messages.len()
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
            )
    }
}
