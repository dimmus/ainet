//! What the docks' panels share: their title strip, the agents' state
//! colours, and the tabs that switch between the bottom dock's panels
//! (Queue, Chatroom, Events). Each panel has a module of its own.

use ainet_core::AgentState;
use gpui::Action;
use ui::prelude::*;

use crate::actions::panel::{chatroom_panel, events_panel, queue_panel};

/// Default dock sizes, Zed's.
pub const SIDE_DOCK_WIDTH: Pixels = px(260.);
pub const BOTTOM_DOCK_HEIGHT: Pixels = px(160.);
/// A panel's title strip, as tall as Zed's tab bars.
pub const HEADER_HEIGHT: Pixels = px(32.);

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
        .h(HEADER_HEIGHT)
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

/// The bottom dock's panels, in order: their names and their tabs' words.
pub const BOTTOM_PANELS: [(&str, &str); 3] = [
    ("QueuePanel", "Queue"),
    ("ChatroomPanel", "Chatroom"),
    ("EventsPanel", "Events"),
];

/// What shows the bottom panel named `name` and gives it the keyboard.
fn show_bottom_panel(name: &str) -> Box<dyn Action> {
    match name {
        "ChatroomPanel" => Box::new(chatroom_panel::ToggleFocus),
        "EventsPanel" => Box::new(events_panel::ToggleFocus),
        _ => Box::new(queue_panel::ToggleFocus),
    }
}

/// A bottom panel's title strip: a tab per panel of the dock, `active`
/// lit (a click on another shows that one), then `end`, the panel's own
/// controls. The border lights up while the panel has the keyboard.
pub fn bottom_header(active: &str, focused: bool, end: impl IntoElement, cx: &App) -> Div {
    let colors = cx.theme().colors();
    h_flex()
        .flex_none()
        .h(HEADER_HEIGHT)
        .px_1()
        .gap_1()
        .justify_between()
        .border_b_1()
        .border_color(if focused {
            colors.panel_focused_border
        } else {
            colors.border_variant
        })
        .child(h_flex().gap_0p5().children(BOTTOM_PANELS.map(|(name, label)| {
            let is_active = name == active;
            Button::new(SharedString::from(format!("bottom-tab-{name}")), label)
                .label_size(LabelSize::Small)
                .toggle_state(is_active)
                .color(if is_active { Color::Default } else { Color::Muted })
                .when(!is_active, |button| {
                    button.on_click(move |_, window, cx| window.dispatch_action(show_bottom_panel(name), cx))
                })
        })))
        .child(h_flex().gap_1().pr_1().min_w_0().child(end))
}
