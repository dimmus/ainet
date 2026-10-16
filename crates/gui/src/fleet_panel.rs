//! The Fleet panel (left dock by default), after Zed's project panel: the
//! Store's Fleet tree in the TUI's order, rigs folding away behind a
//! disclosure, who directs whom drawn as indent guides, each agent with
//! its critter, state, waiting and unread counts, task and the TUI's meta
//! line. The selection is the Store's, so every view follows it; a right
//! click (or `shift-f10`) opens the agent's menu. The actions it offers
//! are carried out by the workspace, which can open tabs and modals.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use ainet_core::{Agent, AgentState, RigStatus};
use ainet_model::Row;
use ainet_model::describe;
use gpui::{
    Action, Anchor, Bounds, ClickEvent, DismissEvent, Entity, FocusHandle, Focusable, FontWeight,
    MouseButton, MouseDownEvent, Pixels, Point, ScrollStrategy, Subscription, UniformListScrollHandle,
    anchored, canvas, deferred, point, relative, uniform_list,
};
use smallvec::SmallVec;
use ui::{
    ContextMenu, CountBadge, Disclosure, IndentGuideColors, Indicator, ListItem, ListItemSpacing,
    ListSubHeader, prelude::*,
};

use crate::actions::{
    Capture, CloseAgent, CollapseRig, Continue, ExpandRig, MessageAgent, OpenContextMenu, OpenConversation,
    OpenTerminal, Relaunch, ReleaseSeat, RemoveSeat, SelectNext, SelectPrev, ShowTranscript,
    ToggleTypingGuard,
};
use crate::critter::{Animation, Critter};
use crate::dock::Panel;
use crate::fleet::{FleetModel, Tick};
use crate::panels::{header, state_color};
use crate::settings::{self, CritterSize};
use crate::themes;

/// Every row is this tall, so the list can be virtualised (Zed's
/// `uniform_list`): three lines for an agent.
const ROW_HEIGHT: f32 = 56.;
/// How far each level of the tree is indented.
const INDENT: Pixels = px(16.);
/// Where the indent guides sit, under the disclosure or critter column.
const GUIDE_OFFSET: Pixels = px(14.);

/// The selected agent's menu (`None` with nobody selected), as the Fleet
/// shows it and the Topology view's table too. Its entries are the
/// workspace's actions on the selected agent, dispatched from `focus`.
pub fn agent_menu(
    fleet: &Entity<FleetModel>,
    focus: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<ContextMenu>> {
    let store = &fleet.read(cx).store;
    let agent = store.selected_agent()?;
    let seated = agent.seat.is_some();
    let guarded = store.seat_of(agent).is_some_and(|s| s.typing_guard);
    Some(ContextMenu::build(window, cx, move |menu, _, _| {
        menu.context(focus)
            .action("Message…", MessageAgent.boxed_clone())
            .action("Open Terminal", OpenTerminal.boxed_clone())
            .action("Open Conversation", OpenConversation.boxed_clone())
            .action("Show Transcript", ShowTranscript.boxed_clone())
            .action("Capture Screen", Capture.boxed_clone())
            .separator()
            .action_disabled_when(!seated, "Relaunch", Relaunch.boxed_clone())
            .action_disabled_when(!seated, "Continue", Continue.boxed_clone())
            .action_checked_with_disabled("Typing Guard", ToggleTypingGuard.boxed_clone(), guarded, !seated)
            .separator()
            .action_disabled_when(!seated, "Release Seat", ReleaseSeat.boxed_clone())
            .action_disabled_when(!seated, "Remove Seat…", RemoveSeat.boxed_clone())
            .action("Close Agent…", CloseAgent.boxed_clone())
    }))
}

/// A row the panel shows: a Store row, and how deep in the tree it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Index into the Store's rows.
    pub row: usize,
    pub depth: usize,
}

/// How many levels a tree prefix (`├─ `, `│  └─ `) stands for.
fn prefix_depth(prefix: &str) -> usize {
    prefix.chars().count() / 3
}

/// The rows to show, in the Store's (the TUI's) order: everything but the
/// agents and seats of folded rigs. A rig's members sit one level under
/// it, each team member one more under whoever directs it.
pub fn entries(rows: &[Row], collapsed: &HashSet<String>) -> Vec<Entry> {
    let mut out = Vec::with_capacity(rows.len());
    let mut rig: Option<&str> = None;
    for (row, r) in rows.iter().enumerate() {
        let depth = match r {
            Row::Rig(name) => {
                rig = Some(name);
                0
            }
            Row::Header(_) => {
                rig = None;
                0
            }
            Row::Agent { prefix, .. } | Row::Seat { prefix, .. } => {
                if rig.is_some_and(|r| collapsed.contains(r)) {
                    continue;
                }
                usize::from(rig.is_some()) + prefix_depth(prefix)
            }
        };
        out.push(Entry { row, depth });
    }
    out
}

/// The rig whose rows hold Store row `row`, if any.
fn rig_of_row(rows: &[Row], row: usize) -> Option<&str> {
    rows[..=row.min(rows.len().saturating_sub(1))]
        .iter()
        .rev()
        .find_map(|r| match r {
            Row::Rig(name) => Some(Some(name.as_str())),
            Row::Header(_) => Some(None),
            _ => None,
        })
        .flatten()
}

struct DeployedMenu {
    menu: Entity<ContextMenu>,
    position: Point<Pixels>,
    _dismissed: Subscription,
}

pub struct FleetPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    /// Rigs folded away.
    collapsed: HashSet<String>,
    /// What is shown, as of the last render.
    entries: Vec<Entry>,
    /// The selection as last seen, to reveal and scroll to a new one.
    last_selected: Option<String>,
    /// Where the selected row was drawn, to open its menu from the keys.
    selected_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    context_menu: Option<DeployedMenu>,
    animation: Animation,
    _subscriptions: Vec<Subscription>,
}

impl FleetPanel {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&fleet, |this, _, cx| {
                this.follow_selection(cx);
                cx.notify();
            }),
            cx.subscribe(&fleet, |this, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.notify();
                }
            }),
        ];
        let mut panel = FleetPanel {
            fleet,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            collapsed: HashSet::new(),
            entries: Vec::new(),
            last_selected: None,
            selected_bounds: Rc::default(),
            context_menu: None,
            animation: Animation::default(),
            _subscriptions: subscriptions,
        };
        panel.follow_selection(cx);
        panel
    }

    /// What the panel shows now, in order.
    pub fn entries(&self, cx: &App) -> Vec<Entry> {
        entries(&self.fleet.read(cx).store.rows, &self.collapsed)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_collapsed(&self, rig: &str) -> bool {
        self.collapsed.contains(rig)
    }

    /// The rigs folded away, sorted: kept with the workspace.
    pub fn folded_rigs(&self) -> Vec<String> {
        let mut rigs: Vec<String> = self.collapsed.iter().cloned().collect();
        rigs.sort();
        rigs
    }

    /// Folds away `rigs` (a saved workspace's) and unfolds the rest. Rigs
    /// not running yet stay folded for when they come.
    pub fn set_folded_rigs(&mut self, rigs: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
        self.collapsed = rigs.into_iter().collect();
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn context_menu(&self) -> Option<&Entity<ContextMenu>> {
        self.context_menu.as_ref().map(|m| &m.menu)
    }

    /// A newly selected agent (by a click elsewhere, the switcher, a key)
    /// is revealed: its rig unfolds and the list scrolls to it, as Zed's
    /// project panel reveals the active file.
    fn follow_selection(&mut self, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        if store.selected == self.last_selected {
            return;
        }
        self.last_selected = store.selected.clone();
        let Some(row) = store.selected_row() else {
            return;
        };
        if let Some(rig) = rig_of_row(&store.rows, row) {
            self.collapsed.remove(rig);
        }
        let shown = entries(&store.rows, &self.collapsed);
        if let Some(ix) = shown.iter().position(|e| e.row == row) {
            self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);
        }
    }

    /// Moves the selection over the agents shown, skipping folded rigs.
    pub fn select_relative(&mut self, delta: i32, cx: &mut Context<Self>) {
        let shown: HashSet<usize> = self.entries(cx).into_iter().map(|e| e.row).collect();
        self.fleet.update(cx, |fleet, cx| {
            fleet.move_selection_where(delta, |row| shown.contains(&row), cx)
        });
    }

    fn set_collapsed(&mut self, rig: &str, collapsed: bool, cx: &mut Context<Self>) {
        let changed = if collapsed {
            self.collapsed.insert(rig.to_string())
        } else {
            self.collapsed.remove(rig)
        };
        if changed {
            cx.notify();
        }
    }

    fn toggle_rig(&mut self, rig: &str, cx: &mut Context<Self>) {
        let collapsed = !self.collapsed.contains(rig);
        self.set_collapsed(rig, collapsed, cx);
    }

    /// The rig of the selected agent, for the fold keys.
    fn selected_rig(&self, cx: &App) -> Option<String> {
        let store = &self.fleet.read(cx).store;
        rig_of_row(&store.rows, store.selected_row()?).map(str::to_string)
    }

    fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.fleet.update(cx, |fleet, cx| fleet.select(id, cx));
    }

    /// Opens the selected agent's menu at `position`, as Zed's project
    /// panel does: the menu takes the keyboard, and gives it back to the
    /// panel when dismissed without choosing.
    pub fn deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = agent_menu(&self.fleet, self.focus_handle.clone(), window, cx) else {
            return;
        };
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
            if menu.focus_handle(cx).contains_focused(window, cx) {
                window.focus(&this.focus_handle, cx);
            }
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some(DeployedMenu {
            menu,
            position,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    /// `shift-f10`: the menu under the selected row, or at the mouse.
    fn open_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let position = match self.selected_bounds.get() {
            Some(b) => point(b.left() + px(24.), b.bottom()),
            None => window.mouse_position(),
        };
        self.deploy_context_menu(position, window, cx);
    }

    fn row_height(window: &Window) -> Pixels {
        rems_from_px(ROW_HEIGHT).to_pixels(window.rem_size())
    }

    fn render_entry(&self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let height = Self::row_height(window);
        let store = &self.fleet.read(cx).store;
        let row = store.rows[entry.row].clone();
        let agent = match &row {
            Row::Agent { id, .. } => store.agents.get(id).cloned(),
            _ => None,
        };
        let row = match row {
            Row::Header(title) => div()
                .size_full()
                .flex()
                .flex_col()
                .justify_end()
                .child(ListSubHeader::new(title))
                .into_any_element(),
            Row::Rig(name) => self.render_rig(entry, &name, cx),
            Row::Agent { .. } => match agent {
                Some(agent) => self.render_agent(entry, &agent, window, cx),
                None => div().into_any_element(),
            },
            Row::Seat { rig, address, .. } => self.render_seat(entry, &rig, &address, cx),
        };
        div()
            .id(("fleet-entry", entry.row))
            .debug_selector(|| format!("FLEET-ROW-{}", entry.row))
            .h(height)
            .w_full()
            .child(row)
            .into_any_element()
    }

    fn render_rig(&self, entry: Entry, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let Some(rig) = store.rigs.get(name) else {
            return div().into_any_element();
        };
        let counts = store.rig_counts(name);
        let status = match rig.status {
            RigStatus::Running => Color::Success,
            RigStatus::Launching => Color::Info,
            RigStatus::Partial => Color::Warning,
            RigStatus::Stopped => Color::Muted,
        };
        let mut line = format!(
            "{} · {}/{} seats",
            rig.status.label(),
            counts.running,
            counts.seats
        );
        if counts.open > 0 {
            line.push_str(&format!(" · {} open", counts.open));
        }
        let summary = rig
            .summary
            .as_ref()
            .map(|s| s.lines().next().unwrap_or_default().to_string());
        let open = !self.collapsed.contains(name);
        let (disclosed, clicked) = (name.to_string(), name.to_string());
        ListItem::new(("fleet-rig", entry.row))
            .height(relative(1.))
            .spacing(ListItemSpacing::Dense)
            .indent_level(entry.depth)
            .indent_step_size(INDENT)
            .start_slot(
                Disclosure::new(("fleet-rig-disclosure", entry.row), open)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_rig(&disclosed, cx))),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Indicator::dot().color(status))
                            .child(Label::new(name.to_uppercase()).weight(FontWeight::BOLD))
                            .child(Label::new(line).size(LabelSize::Small).color(Color::Muted)),
                    )
                    .when_some(summary, |el, summary| {
                        el.child(
                            Label::new(summary)
                                .size(LabelSize::Small)
                                .color(Color::Placeholder)
                                .truncate(),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_rig(&clicked, cx)))
            .into_any_element()
    }

    fn render_seat(&self, entry: Entry, rig: &str, address: &str, cx: &mut Context<Self>) -> AnyElement {
        let seat = self
            .fleet
            .read(cx)
            .store
            .rigs
            .get(rig)
            .and_then(|r| r.seat(address));
        let status = seat.map_or("gone", |s| s.status.label());
        let role = seat.and_then(|s| s.role.clone()).unwrap_or_default();
        ListItem::new(("fleet-seat", entry.row))
            .height(relative(1.))
            .spacing(ListItemSpacing::Dense)
            .selectable(false)
            .indent_level(entry.depth)
            .indent_step_size(INDENT)
            .start_slot(
                Icon::new(IconName::Circle)
                    .size(IconSize::Small)
                    .color(Color::Disabled),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new(address.to_string()).color(Color::Muted).truncate())
                            .child(Label::new(role).size(LabelSize::Small).color(Color::Placeholder))
                            .child(
                                Label::new(status)
                                    .size(LabelSize::Small)
                                    .color(Color::Placeholder),
                            ),
                    )
                    .child(
                        Label::new("no agent in this seat · /launch to start it")
                            .size(LabelSize::Small)
                            .color(Color::Placeholder)
                            .truncate(),
                    ),
            )
            .into_any_element()
    }

    fn render_agent(
        &self,
        entry: Entry,
        a: &Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fleet = self.fleet.read(cx);
        let selected = fleet.store.selected.as_deref() == Some(a.id.as_str());
        let waiting = fleet.store.waiting_for(&a.id);
        let directs = fleet.store.directs(&a.id);
        let critter_size = match settings::get(cx).critters.size {
            CritterSize::Large => px(32.),
            CritterSize::Small => px(16.),
        };
        let critter = Critter::agent(a, fleet.tick(), cx)
            .size(critter_size)
            .animate(&self.animation);
        let focused = self.focus_handle.contains_focused(window, cx);
        let gone = a.state == AgentState::Gone;
        let hue = themes::agent_color(&a.id, cx).cursor;

        // The name is who it is: it keeps its width, and the role gives
        // way first when the panel is narrow.
        let mut first = h_flex().gap_1p5().min_w_0().child(
            div().flex_none().child(
                Label::new(a.name.clone())
                    .color(if gone { Color::Muted } else { Color::Default })
                    .when(selected, |l| l.weight(FontWeight::BOLD)),
            ),
        );
        if let Some(role) = &a.role {
            first = first.child(
                div().min_w_0().child(
                    Label::new(role.clone())
                        .size(LabelSize::Small)
                        .color(if gone { Color::Muted } else { Color::Custom(hue) })
                        .truncate(),
                ),
            );
        }
        if directs > 0 {
            first = first.child(
                div().flex_none().child(
                    Label::new(format!("directs {directs}"))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            );
        }
        let task = a.task.clone().unwrap_or_else(|| "no task yet".into());
        let state = a.state;
        let (id, menu_id) = (a.id.clone(), a.id.clone());
        let bounds = self.selected_bounds.clone();

        ListItem::new(SharedString::from(format!("fleet-agent-{}", a.id)))
            .height(relative(1.))
            .spacing(ListItemSpacing::Dense)
            .indent_level(entry.depth)
            .indent_step_size(INDENT)
            .toggle_state(selected)
            .when(selected, |item| item.focused(focused))
            .start_slot(
                div()
                    .relative()
                    .child(critter)
                    .when(waiting > 0, |el| el.child(CountBadge::new(waiting))),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(first)
                    .child(
                        Label::new(format!("▸ {task}"))
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    // The state leads the last line: an end slot is the
                    // first thing a narrow panel cuts off.
                    .child(
                        h_flex()
                            .gap_1()
                            .min_w_0()
                            .child(Indicator::dot().color(state_color(state)))
                            .child(
                                div().flex_none().child(
                                    Label::new(state.label())
                                        .size(LabelSize::XSmall)
                                        .color(state_color(state)),
                                ),
                            )
                            .child(
                                Label::new(describe::meta(a))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Placeholder)
                                    .truncate(),
                            ),
                    ),
            )
            .end_slot(v_flex().items_end().gap_0p5().when(a.unread > 0, |el| {
                el.child(
                    h_flex()
                        .gap_0p5()
                        .child(
                            Icon::new(IconName::Envelope)
                                .size(IconSize::XSmall)
                                .color(Color::Accent),
                        )
                        .child(
                            Label::new(a.unread.to_string())
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        ),
                )
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.select(&id, window, cx);
                if event.click_count() == 2 {
                    window.dispatch_action(OpenTerminal.boxed_clone(), cx);
                }
            }))
            .on_secondary_mouse_down(cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.select(&menu_id, window, cx);
                this.deploy_context_menu(event.position, window, cx);
                cx.stop_propagation();
            }))
            .when(selected, |item| {
                // Where it was drawn, for a menu opened from the keys.
                item.child(
                    canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
            })
            .into_any_element()
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

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(crate::actions::panel::fleet_panel::ToggleFocus)
    }
}

impl Render for FleetPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        self.entries = self.entries(cx);
        // Set again below if the selected row is drawn this frame.
        self.selected_bounds.set(None);
        let count = self.entries.len();
        let list = uniform_list(
            "fleet-entries",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let shown: Vec<Entry> = this.entries.get(range).map(<[Entry]>::to_vec).unwrap_or_default();
                shown
                    .into_iter()
                    .map(|entry| this.render_entry(entry, window, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .with_decoration(
            ui::indent_guides(INDENT, IndentGuideColors::panel(cx))
                .with_left_offset(GUIDE_OFFSET)
                .with_compute_indents_fn(cx.entity(), |this, range, _, _| {
                    let depths: SmallVec<[usize; 64]> = this
                        .entries
                        .get(range)
                        .unwrap_or_default()
                        .iter()
                        .map(|e| e.depth)
                        .collect();
                    depths
                }),
        )
        .flex_1()
        .size_full()
        .track_scroll(&self.scroll_handle);

        v_flex()
            .id("fleet-panel")
            .key_context("FleetPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select_relative(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.select_relative(-1, cx)))
            .on_action(cx.listener(|this, _: &CollapseRig, _, cx| {
                if let Some(rig) = this.selected_rig(cx) {
                    this.set_collapsed(&rig, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExpandRig, _, cx| {
                if let Some(rig) = this.selected_rig(cx) {
                    this.set_collapsed(&rig, false, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &OpenContextMenu, window, cx| this.open_context_menu(window, cx)),
            )
            .child(header("Fleet", IconName::ListTree, focused, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .when(count == 0, |el| {
                        el.child(div().p_2().child(Label::new("No agents yet").color(Color::Muted)))
                    })
                    .when(count > 0, |el| el.child(list)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }),
            )
            .children(self.context_menu.as_ref().map(|deployed| {
                deferred(
                    anchored()
                        .position(deployed.position)
                        .anchor(Anchor::TopLeft)
                        .child(deployed.menu.clone()),
                )
                .with_priority(3)
            }))
    }
}

#[cfg(test)]
mod tests {
    use ainet_core::Request;
    use gpui::{Modifiers, MouseUpEvent, TestAppContext, VisualTestContext};

    use super::*;
    use crate::fleet::TICK;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        (workspace, cx)
    }

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    fn panel(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetPanel> {
        w.read_with(cx, |w, _| w.fleet_panel().clone())
    }

    fn selected(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> Option<String> {
        fleet.read_with(cx, |f, _| f.store.selected.clone())
    }

    /// The agent in seat `seat`.
    fn seat(fleet: &Entity<FleetModel>, seat: &str, cx: &mut VisualTestContext) -> String {
        fleet.read_with(cx, |f, _| {
            f.store
                .agents
                .values()
                .find(|a| a.seat.as_deref() == Some(seat))
                .map(|a| a.id.clone())
                .unwrap()
        })
    }

    /// Where Store row `row` was drawn, if it was.
    fn row_bounds(row: usize, cx: &mut VisualTestContext) -> Option<Bounds<Pixels>> {
        // Debug selectors are looked up by a static name.
        cx.debug_bounds(format!("FLEET-ROW-{row}").leak())
    }

    /// The last request sent, and what it says when done.
    fn last_sent(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> (Request, String) {
        fleet.read_with(cx, |f, _| f.sent.last().cloned().expect("a request was sent"))
    }

    #[gpui::test]
    async fn the_demo_fleet_is_drawn_in_the_tuis_order(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let (fleet, panel) = (fleet(&w, cx), panel(&w, cx));
        let rows = fleet.read_with(cx, |f, _| f.store.rows.clone());
        let shown = panel.read_with(cx, |p, cx| p.entries(cx));
        assert_eq!(
            shown.iter().map(|e| e.row).collect::<Vec<_>>(),
            (0..rows.len()).collect::<Vec<_>>(),
            "every row, in the Store's order"
        );
        // The docs rig: lead under the rig, its team under the lead.
        let depth = |seat_name: &str, cx: &mut VisualTestContext| {
            let id = seat(&fleet, seat_name, cx);
            let row = fleet.read_with(cx, |f, _| f.store.row_of(&id).unwrap());
            shown[row].depth
        };
        assert_eq!(shown[0].depth, 0);
        assert_eq!(depth("orch-lead@docs", cx), 1);
        assert_eq!(depth("dev-ui@docs", cx), 2);

        // The rows drawn are top to bottom in that order, one height each.
        let drawn: Vec<Bounds<Pixels>> = (0..rows.len()).map_while(|row| row_bounds(row, cx)).collect();
        assert!(drawn.len() >= 6, "{} rows drawn", drawn.len());
        for pair in drawn.windows(2) {
            assert!(pair[0].top() < pair[1].top());
            assert_eq!(pair[0].size.height, pair[1].size.height, "uniform rows");
        }
    }

    #[gpui::test]
    async fn keys_move_the_selection_and_fold_rigs(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let (fleet, panel) = (fleet(&w, cx), panel(&w, cx));
        let lead = seat(&fleet, "orch-lead@docs", cx);
        let app = seat(&fleet, "dev-app@docs", cx);
        assert_eq!(selected(&fleet, cx), Some(lead.clone()));
        cx.simulate_keystrokes("down");
        assert_eq!(selected(&fleet, cx), Some(app.clone()));
        cx.simulate_keystrokes("k");
        assert_eq!(selected(&fleet, cx), Some(lead.clone()));

        // Left folds the selected agent's rig; down then leaves it.
        cx.simulate_keystrokes("left");
        assert!(panel.read_with(cx, |p, _| p.is_collapsed("docs")));
        let shown = panel.read_with(cx, |p, cx| p.entries(cx));
        assert!(shown.len() < fleet.read_with(cx, |f, _| f.store.rows.len()));
        cx.simulate_keystrokes("down");
        let first_infra = seat(&fleet, "ops-data@infra", cx);
        assert_eq!(selected(&fleet, cx), Some(first_infra));
        cx.simulate_keystrokes("left");
        assert!(panel.read_with(cx, |p, _| p.is_collapsed("infra")));
        cx.simulate_keystrokes("right");
        assert!(!panel.read_with(cx, |p, _| p.is_collapsed("infra")));

        // Selecting a folded agent from elsewhere unfolds its rig.
        fleet.update(cx, |f, cx| f.select(&app, cx));
        cx.run_until_parked();
        assert!(!panel.read_with(cx, |p, _| p.is_collapsed("docs")));

        // So does the workspace's alt-down, over the shown agents.
        cx.simulate_keystrokes("alt-up");
        assert_eq!(selected(&fleet, cx), Some(lead));
    }

    #[gpui::test]
    async fn clicks_select_and_open_terminals(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let ui = seat(&fleet, "dev-ui@docs", cx);
        let row = fleet.read_with(cx, |f, _| f.store.row_of(&ui).unwrap());
        let bounds = row_bounds(row, cx).expect("drawn");
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert_eq!(selected(&fleet, cx), Some(ui.clone()));

        // A double click opens its terminal as a tab, named after it.
        let pane = w.read_with(cx, |w, _| w.active_pane().clone());
        let tabs = |cx: &mut VisualTestContext| {
            pane.read_with(cx, |p, cx| {
                p.items()
                    .map(|i| i.tab_content_text(cx).to_string())
                    .collect::<Vec<_>>()
            })
        };
        let before = tabs(cx).len();
        let at = bounds.center();
        for event in [1, 2] {
            cx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: event,
                first_mouse: false,
            });
            cx.simulate_event(MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: event,
            });
        }
        let name = fleet.read_with(cx, |f, _| f.store.agents[&ui].name.clone());
        assert_eq!(tabs(cx).len(), before + 1);
        assert_eq!(tabs(cx).last(), Some(&name));

        // Enter in the Fleet opens the same tab again rather than another.
        cx.update(|window, cx| {
            let focus = w.read(cx).fleet_panel().focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(tabs(cx).len(), before + 1);
        let item = pane.read_with(cx, |p, _| p.active_item().unwrap().to_any());
        let terminal = item.downcast::<crate::terminal_item::TerminalItem>().unwrap();
        assert_eq!(terminal.read_with(cx, |t, _| t.agent_id().to_string()), ui);
    }

    #[gpui::test]
    async fn the_agent_menu_sends_the_tuis_requests(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let (fleet, panel) = (fleet(&w, cx), panel(&w, cx));
        let ui = seat(&fleet, "dev-ui@docs", cx);
        let row = fleet.read_with(cx, |f, _| f.store.row_of(&ui).unwrap());
        let bounds = row_bounds(row, cx).expect("drawn");

        // A right click selects the agent and opens its menu; picking
        // Relaunch (the sixth entry) relaunches its seat afresh.
        cx.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::none());
        assert_eq!(selected(&fleet, cx), Some(ui.clone()));
        assert!(panel.read_with(cx, |p, _| p.context_menu().is_some()));
        cx.simulate_keystrokes("down down down down down down enter");
        assert!(panel.read_with(cx, |p, _| p.context_menu().is_none()));
        match last_sent(&fleet, cx) {
            (Request::Launch { seat, fresh }, ok) => {
                assert_eq!(seat, "dev-ui@docs");
                assert!(fresh);
                assert_eq!(ok, "launching dev-ui@docs");
            }
            other => panic!("{other:?}"),
        }
        // Escape closes the menu and gives the keyboard back to the Fleet.
        cx.simulate_keystrokes("shift-f10");
        assert!(panel.read_with(cx, |p, _| p.context_menu().is_some()));
        cx.simulate_keystrokes("escape");
        assert!(panel.read_with(cx, |p, _| p.context_menu().is_none()));
        let focused = cx.update(|window, cx| panel.focus_handle(cx).contains_focused(window, cx));
        assert!(focused);

        // Each entry's action, as the menu dispatches it from the Fleet.
        cx.dispatch_action(Continue);
        assert!(matches!(last_sent(&fleet, cx).0, Request::SeatContinue { seat } if seat == "dev-ui@docs"));
        cx.dispatch_action(ShowTranscript);
        assert!(matches!(last_sent(&fleet, cx).0, Request::Transcript { agent, .. } if agent == ui));
        // The transcript opened as a tab and took the keyboard: back to
        // the Fleet for its keys.
        cx.focus(&panel);
        cx.dispatch_action(Capture);
        assert!(
            matches!(last_sent(&fleet, cx).0, Request::Capture { targets, .. } if targets == [ui.clone()])
        );
        cx.dispatch_action(ToggleTypingGuard);
        assert!(matches!(
            last_sent(&fleet, cx).0,
            Request::TypingGuard { seat, on: true } if seat == "dev-ui@docs"
        ));

        cx.dispatch_action(ReleaseSeat);
        assert!(matches!(
            last_sent(&fleet, cx),
            (Request::Release { seat }, ok) if seat == "dev-ui@docs" && ok == "dev-ui@docs released"
        ));
        // Removing a seat asks first, as /remove's request.
        let sent = fleet.read_with(cx, |f, _| f.sent.len());
        cx.dispatch_action(RemoveSeat);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(fleet.read_with(cx, |f, _| f.sent.len()), sent);
        cx.dispatch_action(RemoveSeat);
        cx.simulate_prompt_answer("Remove");
        cx.run_until_parked();
        assert!(matches!(
            last_sent(&fleet, cx),
            (Request::RemoveSeat { seat }, ok) if seat == "dev-ui@docs" && ok == "removed dev-ui@docs"
        ));

        // Closing asks first.
        let sent = fleet.read_with(cx, |f, _| f.sent.len());
        cx.simulate_keystrokes("delete");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(fleet.read_with(cx, |f, _| f.sent.len()), sent);
        cx.simulate_keystrokes("delete");
        cx.simulate_prompt_answer("Close");
        cx.run_until_parked();
        assert!(matches!(last_sent(&fleet, cx).0, Request::Close { agent } if agent == ui));
    }

    #[gpui::test]
    async fn m_writes_to_the_selected_agent(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        let open =
            |cx: &mut VisualTestContext| w.read_with(cx, |w, cx| w.modal_layer().read(cx).has_active_modal());
        cx.simulate_keystrokes("m");
        assert!(open(cx));
        cx.run_until_parked();
        cx.simulate_input("ship it");
        cx.simulate_keystrokes("enter");
        assert!(!open(cx));
        match last_sent(&fleet, cx) {
            (Request::Message { to, body, force, .. }, ok) => {
                assert_eq!(to, std::slice::from_ref(&lead));
                assert_eq!(body, "ship it");
                assert!(force);
                assert_eq!(ok, "sent to orch-lead@docs");
            }
            other => panic!("{other:?}"),
        }
        // The modal's agent gets it, even when it starts with another's
        // address.
        cx.simulate_keystrokes("m");
        cx.run_until_parked();
        cx.simulate_input("@dev-ui can you check this");
        cx.simulate_keystrokes("enter");
        match last_sent(&fleet, cx) {
            (Request::Message { to, body, .. }, _) => {
                assert_eq!(to, std::slice::from_ref(&lead));
                assert_eq!(body, "@dev-ui can you check this");
            }
            other => panic!("{other:?}"),
        }

        // Escape gives up without sending.
        let sent = fleet.read_with(cx, |f, _| f.sent.len());
        cx.simulate_keystrokes("m");
        cx.run_until_parked();
        cx.simulate_input("never mind");
        cx.simulate_keystrokes("escape");
        assert!(!open(cx));
        assert_eq!(fleet.read_with(cx, |f, _| f.sent.len()), sent);
    }

    #[gpui::test]
    async fn critters_move_only_while_drawn(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let panel = panel(&w, cx);
        // Drawn: each tick takes the flag the last drawing set and asks
        // for a redraw, which sets it again (the panel is a cached view:
        // only that redraw paints it).
        for _ in 0..3 {
            cx.executor().advance_clock(TICK);
            cx.run_until_parked();
        }
        assert!(panel.read_with(cx, |p, _| p.animation.take()), "drawn again");
        // Hidden with its dock: no more.
        cx.simulate_keystrokes("ctrl-b");
        cx.run_until_parked();
        panel.read_with(cx, |p, _| p.animation.take());
        cx.executor().advance_clock(TICK * 3);
        cx.run_until_parked();
        assert!(!panel.read_with(cx, |p, _| p.animation.take()));
    }

    fn agent(id: &str, prefix: &str) -> Row {
        Row::Agent {
            id: id.into(),
            prefix: prefix.into(),
        }
    }

    #[test]
    fn entries_follow_the_store_and_fold_rigs() {
        let rows = vec![
            Row::Rig("docs".into()),
            agent("lead", ""),
            agent("app", "├─ "),
            agent("deep", "│  └─ "),
            Row::Seat {
                rig: "docs".into(),
                address: "dev-x@docs".into(),
                prefix: String::new(),
            },
            Row::Rig("infra".into()),
            agent("ops", ""),
            Row::Header("IDLE · 1".into()),
            agent("loose", ""),
        ];
        let all = entries(&rows, &HashSet::new());
        let shown: Vec<(usize, usize)> = all.iter().map(|e| (e.row, e.depth)).collect();
        assert_eq!(
            shown,
            [
                (0, 0),
                (1, 1),
                (2, 2),
                (3, 3),
                (4, 1),
                (5, 0),
                (6, 1),
                (7, 0),
                (8, 0)
            ]
        );

        let folded = entries(&rows, &HashSet::from(["docs".to_string()]));
        let rows_shown: Vec<usize> = folded.iter().map(|e| e.row).collect();
        assert_eq!(rows_shown, [0, 5, 6, 7, 8]);
        assert_eq!(rig_of_row(&rows, 3), Some("docs"));
        assert_eq!(rig_of_row(&rows, 6), Some("infra"));
        assert_eq!(rig_of_row(&rows, 8), None);
    }
}
