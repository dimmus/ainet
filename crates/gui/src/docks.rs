//! The workspace's docks and center, as placeholders for the views of
//! phase 4: the Fleet panel (left by default) already lists the Store's
//! Fleet rows and selects agents, by click or keys; the Agent panel names
//! the selected agent; the center and the bottom dock say what will live
//! there.

use ainet_core::AgentState;
use ainet_model::Row;
use gpui::{Entity, FocusHandle};
use ui::{Indicator, ListItem, ListItemSpacing, prelude::*};

use crate::args::View;
use crate::fleet::FleetModel;
use crate::themes;

/// Default dock sizes, Zed's.
const SIDE_DOCK_WIDTH: Pixels = px(260.);
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

/// A dock's title strip.
fn header(title: &'static str, icon: IconName, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(px(32.))
        .px_2()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().colors().border_variant)
        .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
        .child(Label::new(title).size(LabelSize::Small).color(Color::Muted))
}

/// The Fleet tree, one plain row per Store row, `width` wide, docked left
/// or right. Its keys work while `focus` is focused (`FleetPanel` context).
pub fn fleet_panel(
    fleet: &Entity<FleetModel>,
    focus: &FocusHandle,
    width: Pixels,
    on_left: bool,
    focused: bool,
    cx: &App,
) -> impl IntoElement {
    let model = fleet.read(cx);
    let store = &model.store;
    let rows: Vec<AnyElement> = store
        .rows
        .iter()
        .enumerate()
        .map(|(ix, row)| match row {
            Row::Header(title) => div()
                .px_2()
                .pt_2()
                .child(
                    Label::new(title.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Row::Rig(name) => {
                let status = store.rigs.get(name).map(|r| r.status.label()).unwrap_or_default();
                ListItem::new(("rig", ix))
                    .spacing(ListItemSpacing::Sparse)
                    .start_slot(
                        Icon::new(IconName::ListTree)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new(name.clone()))
                    .end_slot(Label::new(status).size(LabelSize::Small).color(Color::Muted))
                    .into_any_element()
            }
            Row::Agent { id, prefix } => {
                let Some(agent) = store.agents.get(id) else {
                    return div().into_any_element();
                };
                let id = id.clone();
                let fleet = fleet.clone();
                ListItem::new(SharedString::from(format!("agent-{id}")))
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(store.selected.as_deref() == Some(id.as_str()))
                    .start_slot(Indicator::dot().color(state_color(agent.state)))
                    .child(Label::new(format!("{prefix}{}", agent.name)).truncate())
                    .end_slot(
                        Label::new(agent.state.label())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(move |_, _, cx| fleet.update(cx, |fleet, cx| fleet.select(&id, cx)))
                    .into_any_element()
            }
            Row::Seat { address, prefix, .. } => ListItem::new(("seat", ix))
                .spacing(ListItemSpacing::Sparse)
                .start_slot(Indicator::dot().color(Color::Disabled))
                .child(
                    Label::new(format!("{prefix}{address}"))
                        .color(Color::Disabled)
                        .truncate(),
                )
                .into_any_element(),
        })
        .collect();
    let empty = rows.is_empty();

    v_flex()
        .id("fleet-panel")
        .key_context("FleetPanel")
        .track_focus(focus)
        .flex_none()
        .w(width)
        .h_full()
        .bg(cx.theme().colors().panel_background)
        .map(|el| docked(el, on_left, cx))
        .child(header("Fleet", IconName::ListTree, cx).when(focused, |el| {
            el.border_color(cx.theme().colors().panel_focused_border)
        }))
        .child(
            v_flex()
                .id("fleet-rows")
                .flex_1()
                .overflow_y_scroll()
                .p_1()
                .children(rows)
                .when(empty, |el| {
                    el.child(div().p_2().child(Label::new("No agents yet").color(Color::Muted)))
                }),
        )
}

/// A side dock's border, on the side facing the center.
fn docked<E: Styled>(el: E, on_left: bool, cx: &App) -> E {
    let el = if on_left { el.border_r_1() } else { el.border_l_1() };
    el.border_color(cx.theme().colors().border)
}

/// Who is selected, docked opposite the Fleet. Phase 4 adds the critter,
/// context gauge, cost, task and queue.
pub fn agent_panel(fleet: &FleetModel, on_left: bool, cx: &App) -> impl IntoElement {
    let field = |name: &'static str, value: String| {
        h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(64.))
                    .child(Label::new(name).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(Label::new(value).size(LabelSize::Small).truncate())
    };
    let body = match fleet.store.selected_agent() {
        Some(agent) => v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .pb_1()
                    .child(Indicator::dot().color(state_color(agent.state)))
                    // The agent's body colour, as its critter wears it.
                    .child(
                        Label::new(agent.name.clone())
                            .size(LabelSize::Large)
                            .color(Color::Custom(themes::agent_color(&agent.id, cx).cursor)),
                    ),
            )
            .child(field("role", agent.role.clone().unwrap_or_else(|| "—".into())))
            .child(field("state", agent.state.label().into()))
            .child(field("runtime", format!("{:?}", agent.runtime).to_lowercase()))
            .when_some(agent.seat.clone(), |el, seat| el.child(field("seat", seat)))
            .when_some(agent.model.clone(), |el, model| el.child(field("model", model)))
            .when_some(agent.task.clone(), |el, task| el.child(field("task", task))),
        None => v_flex().child(Label::new("No agent selected").color(Color::Muted)),
    };
    v_flex()
        .id("agent-panel")
        .flex_none()
        .w(SIDE_DOCK_WIDTH)
        .h_full()
        .bg(cx.theme().colors().panel_background)
        .map(|el| docked(el, on_left, cx))
        .child(header("Agent", IconName::UserGroup, cx))
        .child(div().p_3().child(body))
}

/// The center: the pane where terminals, Comms and the Inbox will open as
/// tabs.
pub fn center(view: Option<&View>, cx: &App) -> impl IntoElement {
    let title = view.map_or_else(|| "Fleet".to_string(), View::title);
    v_flex()
        .id("center")
        .flex_1()
        .min_w_0()
        .bg(cx.theme().colors().editor_background)
        .child(
            h_flex()
                .flex_none()
                .h(px(32.))
                .bg(cx.theme().colors().tab_bar_background)
                .border_b_1()
                .border_color(cx.theme().colors().border)
                .child(
                    h_flex()
                        .h_full()
                        .px_3()
                        .gap_2()
                        .bg(cx.theme().colors().tab_active_background)
                        .border_r_1()
                        .border_color(cx.theme().colors().border)
                        .child(
                            Icon::new(IconName::Terminal)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(Label::new(title).size(LabelSize::Small)),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .child(Label::new("Terminals, Comms and the Inbox open here as tabs").color(Color::Muted))
                .child(
                    Label::new("Select an agent in the Fleet")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                ),
        )
}

/// The bottom dock: Queue, Chatroom and Events log, for now as counts.
pub fn bottom_dock(fleet: &FleetModel, cx: &App) -> impl IntoElement {
    let store = &fleet.store;
    let tab = |name: &'static str, active: bool| {
        Label::new(name)
            .size(LabelSize::Small)
            .color(if active { Color::Default } else { Color::Muted })
    };
    v_flex()
        .id("bottom-dock")
        .flex_none()
        .h(BOTTOM_DOCK_HEIGHT)
        .bg(cx.theme().colors().panel_background)
        .border_t_1()
        .border_color(cx.theme().colors().border)
        .child(
            h_flex()
                .flex_none()
                .h(px(32.))
                .px_2()
                .gap_3()
                .border_b_1()
                .border_color(cx.theme().colors().border_variant)
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
