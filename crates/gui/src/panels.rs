//! The docks' panels, still placeholders for the views of phase 4: the
//! Fleet panel (left by default) lists the Store's Fleet rows and selects
//! agents, by click or keys; the Agent panel names the selected agent; the
//! bottom dock's Activity panel counts the queue, chat and messages.

use ainet_core::AgentState;
use ainet_model::Row;
use gpui::{Entity, FocusHandle, Focusable};
use ui::{Indicator, ListItem, ListItemSpacing, prelude::*};

use crate::dock::Panel;
use crate::fleet::FleetModel;
use crate::{settings, themes};

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

/// A panel's title strip; its border lights up while the panel has the
/// keyboard, as Zed's do.
fn header(title: &'static str, icon: IconName, focused: bool, cx: &App) -> Div {
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

/// The Fleet tree, one plain row per Store row. Its keys work while it has
/// the keyboard (`FleetPanel` context).
pub struct FleetPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
}

impl FleetPanel {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        FleetPanel {
            fleet,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for FleetPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FleetPanel {
    fn persistent_name() -> &'static str {
        "FleetPanel"
    }

    fn default_size(&self, cx: &App) -> Pixels {
        px(settings::get(cx).fleet_panel.width)
    }

    fn icon(&self) -> IconName {
        IconName::ListTree
    }

    fn icon_tooltip(&self) -> &'static str {
        "Fleet Panel"
    }
}

impl Render for FleetPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        let store = &self.fleet.read(cx).store;
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
                    let fleet = self.fleet.clone();
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
            .track_focus(&self.focus_handle)
            .size_full()
            .child(header("Fleet", IconName::ListTree, focused, cx))
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
}

/// Who is selected, docked opposite the Fleet. Phase 4 adds the critter,
/// context gauge, cost, task and queue.
pub struct AgentPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
}

impl AgentPanel {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        AgentPanel {
            fleet,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AgentPanel {
    fn persistent_name() -> &'static str {
        "AgentPanel"
    }

    fn default_size(&self, _: &App) -> Pixels {
        SIDE_DOCK_WIDTH
    }

    fn icon(&self) -> IconName {
        IconName::UserGroup
    }

    fn icon_tooltip(&self) -> &'static str {
        "Agent Panel"
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
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
        let body = match self.fleet.read(cx).store.selected_agent() {
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
            .track_focus(&self.focus_handle)
            .size_full()
            .child(header("Agent", IconName::UserGroup, focused, cx))
            .child(div().p_3().child(body))
    }
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
