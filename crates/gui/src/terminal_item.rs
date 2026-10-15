//! An agent's terminal as a tab: [`TerminalItem`], opened from the Fleet
//! (enter, a double click, the agent's menu) by `Workspace::open_terminal`,
//! one tab per agent. Its tab wears the agent's critter. For now the body
//! shows the agent and the last plain-text screen ainetd sent; the live
//! terminal element (phase 4 step 3) replaces [`TerminalItem::render_body`].

use ainet_core::AgentState;
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, Subscription};
use ui::{Indicator, prelude::*};

use crate::critter::{Animation, Critter};
use crate::fleet::{FleetModel, Tick};
use crate::item::{Item, ItemEvent};
use crate::panels::state_color;

/// The screen size asked of the demo until the element measures one.
const SCREEN: (u16, u16) = (24, 90);

pub struct TerminalItem {
    fleet: Entity<FleetModel>,
    agent: String,
    focus_handle: FocusHandle,
    animation: Animation,
    _subscriptions: Vec<Subscription>,
}

impl TerminalItem {
    /// The terminal of agent `agent` (its id). Asks for a fresh screen.
    pub fn new(fleet: Entity<FleetModel>, agent: String, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // Name, state and critter show in the tab.
            cx.observe(&fleet, |_, _, cx| {
                cx.emit(ItemEvent::UpdateTab);
                cx.notify();
            }),
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.emit(ItemEvent::UpdateTab);
                    cx.notify();
                }
            }),
        ];
        fleet.read(cx).fetch_screen(&agent, SCREEN);
        TerminalItem {
            fleet,
            agent,
            focus_handle: cx.focus_handle(),
            animation: Animation::default(),
            _subscriptions: subscriptions,
        }
    }

    /// The agent whose terminal this is.
    pub fn agent_id(&self) -> &str {
        &self.agent
    }

    /// What fills the tab under its header.
    fn render_body(&self, cx: &App) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let rows = store.screens.get(&self.agent).cloned().unwrap_or_default();
        if rows.is_empty() {
            return div()
                .p_3()
                .child(Label::new("No screen yet: this session may run outside ainetd").color(Color::Muted))
                .into_any_element();
        }
        v_flex()
            .id("terminal-screen")
            .p_3()
            .overflow_y_scroll()
            .font_buffer(cx)
            .text_color(cx.theme().colors().terminal_foreground)
            .children(rows.into_iter().map(|row| div().child(row)))
            .into_any_element()
    }
}

impl EventEmitter<ItemEvent> for TerminalItem {}

impl Focusable for TerminalItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for TerminalItem {
    fn tab_content_text(&self, cx: &App) -> SharedString {
        let store = &self.fleet.read(cx).store;
        store
            .agents
            .get(&self.agent)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Terminal".into())
            .into()
    }

    fn tab_icon(&self, _: &Window, cx: &App) -> Option<AnyElement> {
        let fleet = self.fleet.read(cx);
        let agent = fleet.store.agents.get(&self.agent)?;
        Some(
            Critter::agent(agent, fleet.tick(), cx)
                .size(px(16.))
                .animate(&self.animation)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        let agent = self.fleet.read(cx).store.agents.get(&self.agent)?;
        Some(format!("{} · {}", agent.label(), agent.state.label()).into())
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Self>> {
        let (fleet, agent) = (self.fleet.clone(), self.agent.clone());
        Some(cx.new(|cx| TerminalItem::new(fleet, agent, cx)))
    }
}

impl Render for TerminalItem {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fleet = self.fleet.read(cx);
        let header = fleet.store.agents.get(&self.agent).map(|a| {
            h_flex()
                .flex_none()
                .gap_2()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    Critter::agent(a, fleet.tick(), cx)
                        .size(px(32.))
                        .animate(&self.animation),
                )
                .child(Label::new(a.label()))
                .child(Indicator::dot().color(state_color(a.state)))
                .child(Label::new(a.state.label()).size(LabelSize::Small).color(
                    if a.state == AgentState::Gone {
                        Color::Disabled
                    } else {
                        Color::Muted
                    },
                ))
        });
        v_flex()
            .id("terminal-item")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().terminal_background)
            .children(header)
            .child(div().flex_1().min_h_0().child(self.render_body(cx)))
    }
}
