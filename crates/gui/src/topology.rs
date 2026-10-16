//! The Topology view, after OpenRig's TUI: on the left an explorer tree
//! (TOPOLOGY, this host, each rig with its status, its pods with their
//! seat counts, the agents in no rig); on the right, for the rig, pod or
//! loose agents chosen there, five tabs:
//!
//! - **Table**: the seats grouped by pod (pod, seat, runtime, model,
//!   context, state, open queue items, the item in progress, what it is
//!   doing now) with a filter over them and the summary line under them.
//!   Enter opens the seat's terminal; a right click gives the Fleet's agent
//!   menu.
//! - **Recent**: the queue's recorded changes, oldest first; enter shows
//!   the item in full, as the Queue panel's Show does.
//! - **Overview**: the rig in a few lines.
//! - **Graph**: the Map of the rig (or pod) alone.
//! - **Health**: what the checks found, and which checks were made.
//!
//! What is shown comes from `ainet_model::topology`, which knows nothing
//! of drawing; the view keeps its rows in step with the fleet.

use std::cell::Cell;
use std::rc::Rc;

use ainet_core::{AgentState, RigStatus, SeatStatus};
use ainet_model::describe::{age, short_path};
use ainet_model::topology::{
    self, CHECKS, Node, QueueChange, RowState, Scope, SeatRow, Severity, Summary, TreeRow,
};
use ainet_model::{QueueCommand, command};
use gpui::{
    Action, Bounds, ClickEvent, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyContext,
    MouseButton, MouseDownEvent, Point, ScrollStrategy, Stateful, Subscription, anchored, canvas, deferred,
    point, relative,
};
use ui::{
    Chip, ColumnWidthConfig, ContextMenu, Indicator, ListItem, ListItemSpacing, Table, TableInteractionState,
    ToggleButtonGroup, ToggleButtonGroupStyle, ToggleButtonSimple, Tooltip, prelude::*,
};

use crate::actions::topology::{
    CycleRig, FocusFilter, NextView, PreviousView, ShowGraph, ShowHealth, ShowOverview, ShowRecent, ShowTable,
};
use crate::actions::{OpenContextMenu, OpenTerminal};
use crate::fleet::FleetModel;
use crate::fleet_panel::agent_menu;
use crate::item::{Item, ItemEvent};
use crate::map::MapItem;
use crate::panels::state_color;
use crate::persistence::SavedItem;
use crate::queue_panel::state_colors;
use crate::text_input::{InputEvent, TextInput};
use crate::themes;

/// The table's columns: pod, seat, runtime, model, context, state, queue,
/// work, now.
const COLS: usize = 9;
/// Recent's columns: when, number, seat, state, change, item.
const RECENT_COLS: usize = 6;
/// The explorer's width.
const EXPLORER_WIDTH: Pixels = px(232.);

/// The tabs on the right, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubView {
    #[default]
    Table,
    Recent,
    Overview,
    Graph,
    Health,
}

impl SubView {
    pub const ALL: [SubView; 5] = [
        SubView::Table,
        SubView::Recent,
        SubView::Overview,
        SubView::Graph,
        SubView::Health,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SubView::Table => "Table",
            SubView::Recent => "Recent",
            SubView::Overview => "Overview",
            SubView::Graph => "Graph",
            SubView::Health => "Health",
        }
    }

    /// Its name in the saved workspace.
    pub fn name(self) -> &'static str {
        match self {
            SubView::Table => "table",
            SubView::Recent => "recent",
            SubView::Overview => "overview",
            SubView::Graph => "graph",
            SubView::Health => "health",
        }
    }

    pub fn from_name(name: &str) -> Option<SubView> {
        SubView::ALL.into_iter().find(|v| v.name() == name)
    }

    fn index(self) -> usize {
        SubView::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }

    /// The tab `delta` along, round again.
    fn step(self, delta: isize) -> SubView {
        let n = SubView::ALL.len() as isize;
        SubView::ALL[(self.index() as isize + delta).rem_euclid(n) as usize]
    }
}

/// This machine's name, for the explorer's host node.
fn host_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().trim().to_string();
    if name.is_empty() { "this host".into() } else { name }
}

struct DeployedMenu {
    menu: Entity<ContextMenu>,
    position: Point<Pixels>,
    _dismissed: Subscription,
}

pub struct TopologyItem {
    fleet: Entity<FleetModel>,
    focus: FocusHandle,
    host: String,
    /// What the right side is about; `None` while there is nothing at all.
    scope: Option<Scope>,
    view: SubView,
    filter: Entity<TextInput>,
    table: Entity<TableInteractionState>,
    recent_table: Entity<TableInteractionState>,
    /// The explorer tree, as of the last change.
    tree: Vec<TreeRow>,
    /// Every row of the scope, and the rows the filter lets through.
    rows: Vec<SeatRow>,
    shown: Vec<SeatRow>,
    /// The selected row, by [`SeatRow::key`].
    selected: Option<String>,
    changes: Vec<QueueChange>,
    /// The selected change, by its number.
    change: Option<usize>,
    /// The Graph, made the first time it is shown.
    map: Option<Entity<MapItem>>,
    context_menu: Option<DeployedMenu>,
    /// Where the selected row was drawn, to open its menu from the keys.
    selected_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subscriptions: Vec<Subscription>,
}

impl TopologyItem {
    /// The view of `scope` (the selected agent's rig, or the first, for
    /// `None`).
    pub fn new(
        fleet: Entity<FleetModel>,
        scope: Option<Scope>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| TextInput::new(window, cx).with_placeholder("filter seats…"));
        let subscriptions = vec![
            cx.observe(&fleet, |this, _, cx| {
                this.sync(cx);
                cx.notify();
            }),
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Edited => {
                        this.sync(cx);
                        cx.notify();
                    }
                    // Enter goes on as `menu::Confirm`, taken by the
                    // filter's strip.
                    InputEvent::Submitted => {}
                    // Escape clears the filter, then leaves it.
                    InputEvent::Cancelled => {
                        if input.read(cx).is_empty() {
                            window.focus(&this.focus, cx);
                        } else {
                            input.update(cx, |input, cx| input.set_text("", cx));
                            this.sync(cx);
                            cx.notify();
                        }
                    }
                    InputEvent::Blurred => {}
                }
            }),
        ];
        let mut this = TopologyItem {
            fleet,
            focus: cx.focus_handle(),
            host: host_name(),
            scope,
            view: SubView::Table,
            filter,
            table: cx.new(|cx| TableInteractionState::new(cx)),
            recent_table: cx.new(|cx| TableInteractionState::new(cx)),
            tree: Vec::new(),
            rows: Vec::new(),
            shown: Vec::new(),
            selected: None,
            changes: Vec::new(),
            change: None,
            map: None,
            context_menu: None,
            selected_bounds: Rc::default(),
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    pub fn scope(&self) -> Option<&Scope> {
        self.scope.as_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn view(&self) -> SubView {
        self.view
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tree(&self) -> &[TreeRow] {
        &self.tree
    }

    /// The rows shown, after the filter.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn shown(&self) -> &[SeatRow] {
        &self.shown
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn changes(&self) -> &[QueueChange] {
        &self.changes
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_change(&self) -> Option<usize> {
        self.change
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn filter(&self) -> &Entity<TextInput> {
        &self.filter
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn map(&self) -> Option<&Entity<MapItem>> {
        self.map.as_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn context_menu(&self) -> Option<&Entity<ContextMenu>> {
        self.context_menu.as_ref().map(|m| &m.menu)
    }

    /// The summary line of the scope's rows (all of them, not only those
    /// the filter shows).
    pub fn summary(&self) -> Summary {
        topology::summary(&self.rows)
    }

    /// Everything anew from the store: the tree, the scope (if it went,
    /// the default), the rows through the filter, the changes. The
    /// selections stay on their row and change while they are shown.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let fleet = self.fleet.read(cx);
        // Before ainetd's first snapshot the store is empty, so a rig
        // asked for (`--view topology:RIG`) is kept until it can be told
        // apart from one that is gone.
        let ready = fleet.snapshots() > 0;
        let store = &fleet.store;
        self.tree = topology::tree(store, &self.host);
        let mut moved = false;
        if self.scope.as_ref().is_none_or(|s| ready && !s.exists(store)) {
            let scope = topology::default_scope(store);
            moved = scope != self.scope;
            self.scope = scope;
        }
        let Some(scope) = self.scope.clone() else {
            self.rows.clear();
            self.shown.clear();
            self.changes.clear();
            if moved {
                cx.emit(ItemEvent::UpdateTab);
            }
            return;
        };
        self.rows = topology::rows(store, &scope);
        self.shown = topology::filtered(&self.rows, self.filter.read(cx).text());
        let keep = self
            .selected
            .as_ref()
            .is_some_and(|s| self.shown.iter().any(|r| r.key() == s));
        if !keep {
            // The selected agent's row, else the first.
            let agent = store.selected.as_deref();
            self.selected = self
                .shown
                .iter()
                .find(|r| agent.is_some() && r.agent.as_deref() == agent)
                .or(self.shown.first())
                .map(|r| r.key().to_string());
        }
        self.changes = topology::changes(store, &scope);
        if self.change.is_none_or(|n| n > self.changes.len()) {
            self.change = self.changes.last().map(|c| c.n);
        }
        if let Some(map) = &self.map {
            map.update(cx, |map, cx| map.set_scope(Some(scope), cx));
        }
        if moved {
            cx.emit(ItemEvent::UpdateTab);
        }
    }

    /// Shows `scope` on the right.
    pub fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope.as_ref() != Some(&scope) {
            self.scope = Some(scope);
            self.selected = None;
            self.change = None;
            self.sync(cx);
            cx.emit(ItemEvent::UpdateTab);
            cx.notify();
        }
    }

    /// `ctrl-r`: the next rig, then the loose agents, round again.
    fn cycle_rig(&mut self, cx: &mut Context<Self>) {
        let next = topology::next_scope(&self.fleet.read(cx).store, self.scope.as_ref());
        if let Some(next) = next {
            self.set_scope(next, cx);
        }
    }

    /// Shows the tab `view`; the Graph's map is made the first time.
    pub fn show_view(&mut self, view: SubView, cx: &mut Context<Self>) {
        if view == SubView::Graph && self.map.is_none() {
            let (fleet, scope) = (self.fleet.clone(), self.scope.clone());
            let map = cx.new(|cx| {
                let mut map = MapItem::new(fleet, cx);
                map.set_scope(scope, cx);
                map
            });
            self.map = Some(map);
        }
        if self.view != view {
            self.view = view;
            self.context_menu = None;
            cx.notify();
        }
    }

    /// Moves the selection of the tab showing `delta` rows (to the first
    /// or last with `i32::MIN` or `i32::MAX`).
    fn select_relative(&mut self, delta: i32, cx: &mut Context<Self>) {
        let step = |at: Option<usize>, len: usize| -> Option<usize> {
            if len == 0 {
                return None;
            }
            let last = len as i64 - 1;
            Some(match (at, delta) {
                (_, i32::MIN) => 0,
                (_, i32::MAX) => last as usize,
                (Some(at), d) => (at as i64 + d as i64).clamp(0, last) as usize,
                (None, _) => 0,
            })
        };
        match self.view {
            SubView::Table => {
                let at = self
                    .selected
                    .as_ref()
                    .and_then(|s| self.shown.iter().position(|r| r.key() == s));
                if let Some(ix) = step(at, self.shown.len()) {
                    self.select_row(ix, cx);
                }
            }
            SubView::Recent => {
                let at = self.change.and_then(|n| n.checked_sub(1));
                if let Some(ix) = step(at, self.changes.len()) {
                    self.change = Some(ix + 1);
                    self.recent_table
                        .read(cx)
                        .scroll_handle
                        .scroll_to_item(ix, ScrollStrategy::Nearest);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Selects table row `ix`, and its agent in the fleet (the Fleet and
    /// Agent panels follow).
    fn select_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.shown.get(ix) else { return };
        self.selected = Some(row.key().to_string());
        if let Some(agent) = row.agent.clone() {
            self.fleet.update(cx, |fleet, cx| fleet.select(&agent, cx));
        }
        self.table
            .read(cx)
            .scroll_handle
            .scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn selected_row(&self) -> Option<&SeatRow> {
        let key = self.selected.as_deref()?;
        self.shown.iter().find(|r| r.key() == key)
    }

    /// Enter: the selected seat's terminal on the Table, the selected
    /// item in full on Recent.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.view {
            SubView::Table => self.open_terminal(window, cx),
            SubView::Recent => self.show_change(cx),
            _ => {}
        }
    }

    /// Opens the selected row's agent's terminal (the workspace's
    /// `OpenTerminal`); an empty seat says it has nobody to show.
    fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        match row.agent {
            Some(agent) => {
                self.fleet.update(cx, |fleet, cx| fleet.select(&agent, cx));
                window.dispatch_action(OpenTerminal.boxed_clone(), cx);
            }
            None => {
                let text = format!("no agent runs in {}; /launch starts it", row.key());
                self.fleet.update(cx, |fleet, cx| fleet.show(text, false, cx));
            }
        }
    }

    /// The selected change's item in full, as a report (the Queue panel's
    /// Show).
    fn show_change(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self
            .change
            .and_then(|n| self.changes.get(n - 1))
            .map(|c| c.item.clone())
        else {
            return;
        };
        self.fleet.update(cx, |fleet, cx| {
            let show = command::queue(&fleet.store, &item, QueueCommand::Show);
            fleet.run(show, cx);
        });
    }

    /// The Fleet's agent menu for the selected row's agent, at `position`.
    fn deploy_context_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.selected_row().and_then(|r| r.agent.clone()) else {
            return;
        };
        self.fleet.update(cx, |fleet, cx| fleet.select(&agent, cx));
        let Some(menu) = agent_menu(&self.fleet, self.focus.clone(), window, cx) else {
            return;
        };
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
            if menu.focus_handle(cx).contains_focused(window, cx) {
                window.focus(&this.focus, cx);
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

    // ---- drawing -----------------------------------------------------------

    fn render_explorer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .id("topology-explorer")
            .flex_none()
            .w(EXPLORER_WIDTH)
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(colors.border_variant)
            .bg(colors.panel_background)
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .py_1p5()
                    .child(Label::new("EXPLORER").size(LabelSize::XSmall).color(Color::Muted)),
            )
            .children(
                self.tree
                    .iter()
                    .enumerate()
                    .map(|(ix, row)| self.render_node(ix, row, cx)),
            )
    }

    fn render_node(&self, ix: usize, row: &TreeRow, cx: &mut Context<Self>) -> AnyElement {
        let scope = row.node.scope();
        let selected = scope.is_some() && scope == self.scope;
        let icon = |name: IconName| {
            Icon::new(name)
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element()
        };
        let count = |text: String| Label::new(text).size(LabelSize::XSmall).color(Color::Muted);
        let (start, label, detail): (AnyElement, SharedString, Option<Label>) = match &row.node {
            Node::Topology => (icon(IconName::GitGraph), "TOPOLOGY".into(), None),
            Node::Host(host) => (icon(IconName::Server), host.clone().into(), None),
            Node::Rig {
                name,
                status,
                running,
                seats,
            } => (
                Indicator::dot().color(rig_color(*status)).into_any_element(),
                name.clone().into(),
                Some(count(format!("{running}/{seats}"))),
            ),
            Node::Pod { pod, seats, .. } => (
                icon(IconName::UserGroup),
                pod.clone().into(),
                Some(count(seats.to_string())),
            ),
            Node::Loose { live, agents } => (
                icon(IconName::Person),
                "loose agents".into(),
                Some(count(format!("{live}/{agents}"))),
            ),
        };
        let tooltip = match &row.node {
            Node::Rig { status, .. } => Some(format!("{} · running/seats", status.label())),
            Node::Pod { running, seats, .. } => Some(format!("{running} of {seats} seats running")),
            Node::Loose { .. } => Some("agents in no rig: live/all".to_string()),
            _ => None,
        };
        let header = matches!(row.node, Node::Topology);
        ListItem::new(("topology-node", ix))
            .spacing(ListItemSpacing::Dense)
            .indent_level(row.depth)
            .indent_step_size(px(12.))
            .toggle_state(selected)
            .selectable(scope.is_some())
            .start_slot(start)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1()
                    .debug_selector(move || format!("TOPOLOGY-NODE-{ix}"))
                    .child(
                        Label::new(label)
                            .size(if header {
                                LabelSize::XSmall
                            } else {
                                LabelSize::Small
                            })
                            .color(if header { Color::Muted } else { Color::Default })
                            .truncate(),
                    )
                    .children(detail),
            )
            .when_some(tooltip, |item, text| item.tooltip(Tooltip::text(text)))
            .when_some(scope, |item, scope| {
                item.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.set_scope(scope.clone(), cx);
                }))
            })
            .into_any_element()
    }

    /// The tabs on the right, and what they are about.
    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let button = |view: SubView| {
            let this = this.clone();
            ToggleButtonSimple::new(view.label(), move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.show_view(view, cx));
            })
        };
        let store = &self.fleet.read(cx).store;
        let status = self
            .scope
            .as_ref()
            .and_then(|s| s.rig())
            .and_then(|r| store.rigs.get(r))
            .map(|r| r.status);
        let title = self
            .scope
            .as_ref()
            .map_or_else(|| "nothing yet".to_string(), Scope::title);
        h_flex()
            .flex_none()
            .w_full()
            .gap_3()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                ToggleButtonGroup::single_row("topology-views", SubView::ALL.map(button))
                    .style(ToggleButtonGroupStyle::Outlined)
                    .auto_width()
                    .selected_index(self.view.index()),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .min_w_0()
                    .when_some(status, |el, status| {
                        el.child(Indicator::dot().color(rig_color(status)))
                    })
                    .child(Label::new(title).size(LabelSize::Small).truncate()),
            )
    }

    fn render_table(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let summary = self.summary();
        let filtering = !self.filter.read(cx).is_empty();
        let shown = self.shown.len();
        let filter_focused = self.filter.focus_handle(cx).is_focused(window);
        // Set again below if the selected row is drawn this frame.
        self.selected_bounds.set(None);
        let table = Table::new(COLS)
            .interactable(&self.table)
            .hide_row_borders()
            .width_config(ColumnWidthConfig::explicit(vec![
                relative(0.08),
                relative(0.11),
                relative(0.04),
                relative(0.11),
                relative(0.06),
                relative(0.11),
                relative(0.04),
                relative(0.08),
                relative(0.37),
            ]))
            .header(vec![
                "Pod", "Seat", "RT", "Model", "Ctx", "State", "Q", "Work", "Now",
            ])
            .uniform_list(
                "topology-table",
                shown,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|ix| this.render_cells(ix, cx)).collect()
                }),
            )
            .map_row(
                cx.processor(|this, (ix, row): (usize, Stateful<Div>), window, cx| {
                    this.render_row(ix, row, window, cx)
                }),
            )
            .empty_table_callback(move |_, _| {
                let text = if filtering {
                    "No seat matches the filter."
                } else {
                    "No seats here."
                };
                div()
                    .p_3()
                    .child(Label::new(text).size(LabelSize::Small).color(Color::Muted))
                    .into_any_element()
            });
        let mut line = summary.to_string();
        if filtering {
            line.push_str(&format!(" · {shown} shown"));
        }
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    // Enter in the filter gives the table the keyboard,
                    // rather than opening the selected row.
                    .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                        window.focus(&this.focus, cx);
                        cx.notify();
                    }))
                    .border_b_1()
                    .border_color(if filter_focused {
                        colors.border_focused
                    } else {
                        colors.border_variant
                    })
                    .child(
                        Label::new("/")
                            .size(LabelSize::Small)
                            .color(if filter_focused {
                                Color::Accent
                            } else {
                                Color::Muted
                            })
                            .buffer_font(cx),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "TOPOLOGY-FILTER".into())
                            .child(self.filter.clone()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(table))
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Label::new(line)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    ),
            )
            .into_any_element()
    }

    fn render_cells(&self, ix: usize, cx: &App) -> Vec<AnyElement> {
        let Some(r) = self.shown.get(ix) else {
            return (0..COLS).map(|_| gpui::Empty.into_any_element()).collect();
        };
        let small = |text: String, color: Color| Label::new(text).size(LabelSize::Small).color(color);
        let dash = || small("—".into(), Color::Disabled).into_any_element();
        let pod = if r.first_in_pod {
            let name = if r.pod.is_empty() { "loose" } else { r.pod.as_str() };
            small(name.to_string(), Color::Muted)
                .truncate()
                .into_any_element()
        } else {
            gpui::Empty.into_any_element()
        };
        let seat_color = match &r.agent {
            Some(id) => Color::Custom(themes::agent_color(id, cx).cursor),
            None => Color::Muted,
        };
        let state = row_state_color(r.state);
        vec![
            pod,
            small(r.member.clone(), seat_color).truncate().into_any_element(),
            small(r.rt().into(), Color::Muted)
                .buffer_font(cx)
                .into_any_element(),
            r.model
                .clone()
                .map_or_else(dash, |m| small(m, Color::Muted).truncate().into_any_element()),
            small(r.ctx(), Color::Muted).into_any_element(),
            h_flex()
                .gap_1()
                .min_w_0()
                .child(Indicator::dot().color(state))
                .child(small(r.state.label().into(), state).truncate())
                .into_any_element(),
            small(
                r.items.len().to_string(),
                if r.items.is_empty() {
                    Color::Disabled
                } else {
                    Color::Default
                },
            )
            .into_any_element(),
            r.work.clone().map_or_else(dash, |(id, _)| {
                small(id, Color::Accent).buffer_font(cx).into_any_element()
            }),
            r.now
                .clone()
                .map_or_else(dash, |now| small(now, Color::Muted).truncate().into_any_element()),
        ]
    }

    /// A row's frame: the line above a pod's first row, the selection, the
    /// click that selects (twice: its terminal), the right click that
    /// opens its agent's menu.
    fn render_row(
        &self,
        ix: usize,
        row: Stateful<Div>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(r) = self.shown.get(ix) else {
            return row.into_any_element();
        };
        let colors = cx.theme().colors();
        let selected = self.selected.as_deref() == Some(r.key());
        let focused = self.focus.is_focused(window);
        let tooltip = r
            .work
            .as_ref()
            .map(|(id, summary)| format!("{id}: {summary}"))
            .or_else(|| r.address.clone());
        row.debug_selector(move || format!("TOPOLOGY-ROW-{ix}"))
            .border_l_2()
            .border_color(if selected && focused {
                colors.border_focused
            } else {
                gpui::transparent_black()
            })
            // A line between pods, drawn as a child: a div has one border
            // colour for all sides, and the left one marks the selection.
            .when(r.first_in_pod && ix > 0, |row| {
                row.child(
                    div()
                        .debug_selector(move || format!("TOPOLOGY-POD-LINE-{ix}"))
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h_px()
                        .bg(colors.border_variant),
                )
            })
            .when(selected, |row| row.bg(colors.ghost_element_selected))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.select_row(ix, cx);
                if event.click_count() == 2 {
                    this.open_terminal(window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.select_row(ix, cx);
                    this.deploy_context_menu(event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .when_some(tooltip, |row, text| row.tooltip(Tooltip::text(text)))
            .when(selected, |row| {
                let bounds = self.selected_bounds.clone();
                row.child(
                    canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
            })
            .into_any_element()
    }

    fn render_recent(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.changes.len();
        let title = self.scope.as_ref().map(Scope::title).unwrap_or_default();
        let table = Table::new(RECENT_COLS)
            .interactable(&self.recent_table)
            .hide_row_borders()
            .width_config(ColumnWidthConfig::explicit(vec![
                relative(0.08),
                relative(0.05),
                relative(0.18),
                relative(0.12),
                relative(0.49),
                relative(0.08),
            ]))
            .header(vec!["When", "#", "Seat", "State", "Change", "Item"])
            .uniform_list(
                "topology-recent",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|ix| this.render_change(ix, cx)).collect()
                }),
            )
            .map_row(
                cx.processor(|this, (ix, row): (usize, Stateful<Div>), window, cx| {
                    let selected = this.change == Some(ix + 1);
                    let colors = cx.theme().colors();
                    let focused = this.focus.is_focused(window);
                    row.debug_selector(move || format!("TOPOLOGY-CHANGE-{ix}"))
                        .border_l_2()
                        .border_color(if selected && focused {
                            colors.border_focused
                        } else {
                            gpui::transparent_black()
                        })
                        .when(selected, |row| row.bg(colors.ghost_element_selected))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.change = Some(ix + 1);
                            cx.notify();
                            if event.click_count() == 2 {
                                this.show_change(cx);
                            }
                        }))
                        .into_any_element()
                }),
            )
            .empty_table_callback(|_, _| {
                div()
                    .p_3()
                    .child(
                        Label::new("No queue changes recorded here yet.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element()
            });
        v_flex()
            .size_full()
            .child(
                h_flex().flex_none().px_2().py_1().child(
                    Label::new(format!(
                        "Recent · {title}: recorded queue changes, oldest to newest; enter shows the item"
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
                ),
            )
            .child(div().flex_1().min_h_0().child(table))
            .into_any_element()
    }

    fn render_change(&self, ix: usize, cx: &App) -> Vec<AnyElement> {
        let Some(c) = self.changes.get(ix) else {
            return (0..RECENT_COLS).map(|_| gpui::Empty.into_any_element()).collect();
        };
        let small = |text: String, color: Color| Label::new(text).size(LabelSize::Small).color(color);
        let (bg, fg) = state_colors(c.state, themes::palette(cx), cx);
        let change = if c.note.is_empty() {
            c.summary.clone()
        } else {
            format!("{} · {}", c.note, c.summary)
        };
        vec![
            small(age(c.at), Color::Muted).into_any_element(),
            small(format!("#{}", c.n), Color::Muted).into_any_element(),
            small(c.by.clone(), Color::Default).truncate().into_any_element(),
            Chip::new(c.state.label())
                .label_size(LabelSize::XSmall)
                .label_color(Color::Custom(fg))
                .bg_color(bg)
                .border_color(bg)
                .into_any_element(),
            small(change, Color::Default).truncate().into_any_element(),
            small(c.item.clone(), Color::Accent)
                .buffer_font(cx)
                .into_any_element(),
        ]
    }

    fn render_overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let fact = |key: &'static str, value: AnyElement| {
            h_flex()
                .gap_3()
                .child(
                    div()
                        .flex_none()
                        .w(rems(7.))
                        .child(Label::new(key).size(LabelSize::Small).color(Color::Muted)),
                )
                .child(div().min_w_0().child(value))
        };
        let text = |s: String| Label::new(s).size(LabelSize::Small).into_any_element();
        let body = v_flex().gap_1p5().p_3();
        let body = match &self.scope {
            Some(Scope::Rig(rig)) | Some(Scope::Pod { rig, .. }) => match topology::overview(store, rig) {
                Some(o) => {
                    let states = h_flex().gap_3().children(o.states.iter().map(|(label, n)| {
                        let color = self
                            .rows
                            .iter()
                            .find(|r| r.state.label() == *label)
                            .map_or(Color::Muted, |r| row_state_color(r.state));
                        h_flex()
                            .gap_1()
                            .child(Indicator::dot().color(color))
                            .child(Label::new(format!("{label} {n}")).size(LabelSize::Small))
                    }));
                    body.child(
                        h_flex()
                            .gap_2()
                            .child(Indicator::dot().color(rig_color(o.status)))
                            .child(Label::new(format!("rig {}", o.name)).size(LabelSize::Large))
                            .child(Label::new(o.status.label()).color(Color::Muted)),
                    )
                    .when_some(o.summary.clone(), |el, summary| {
                        el.child(Label::new(summary).color(Color::Muted))
                    })
                    .child(fact(
                        "spec",
                        text(
                            o.spec
                                .as_deref()
                                .map_or_else(|| "none (not from a spec file)".into(), short_path),
                        ),
                    ))
                    .child(fact("root", text(short_path(&o.root))))
                    .child(fact("made", text(format!("{} ago", age(o.created)))))
                    .child(fact(
                        "seats",
                        text(format!("{} running of {} in {} pods", o.running, o.seats, o.pods)),
                    ))
                    .child(fact("states", states.into_any_element()))
                    .child(fact("edges", text(o.edges.to_string())))
                    .child(fact("open queue", text(o.open.to_string())))
                }
                None => body.child(Label::new("That rig is gone.").color(Color::Muted)),
            },
            Some(Scope::Loose) => {
                let live = self
                    .rows
                    .iter()
                    .filter(|r| r.state != RowState::Agent(AgentState::Gone))
                    .count();
                body.child(Label::new("loose agents").size(LabelSize::Large))
                    .child(
                        Label::new("Agents in no rig: spawned on their own or adopted.").color(Color::Muted),
                    )
                    .child(fact(
                        "agents",
                        text(format!("{live} live of {}", self.rows.len())),
                    ))
            }
            None => body.child(Label::new("No rigs or agents yet.").color(Color::Muted)),
        };
        body.into_any_element()
    }

    fn render_health(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let findings = self
            .scope
            .as_ref()
            .map(|s| topology::health(store, s))
            .unwrap_or_default();
        let checked = format!("Checked: {}.", CHECKS.join(", "));
        v_flex()
            .id("topology-health")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .child(Label::new(checked).size(LabelSize::Small).color(Color::Muted)),
            )
            .when(findings.is_empty(), |el| {
                el.child(
                    h_flex()
                        .flex_none()
                        .gap_1p5()
                        .debug_selector(|| "TOPOLOGY-NO-FINDINGS".into())
                        .child(
                            Icon::new(IconName::Check)
                                .size(IconSize::Small)
                                .color(Color::Success),
                        )
                        .child(Label::new("No findings from these checks.")),
                )
            })
            .children(findings.into_iter().enumerate().map(|(ix, f)| {
                let (icon, color) = match f.severity {
                    Severity::Error => (IconName::XCircle, Color::Error),
                    Severity::Warning => (IconName::Warning, Color::Warning),
                    Severity::Info => (IconName::Info, Color::Muted),
                };
                h_flex()
                    .flex_none()
                    .gap_2()
                    .debug_selector(move || format!("TOPOLOGY-FINDING-{ix}"))
                    .child(Icon::new(icon).size(IconSize::Small).color(color))
                    .child(Label::new(f.seat).size(LabelSize::Small))
                    .child(Label::new(f.text).size(LabelSize::Small).color(Color::Muted))
            }))
            .into_any_element()
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.scope.is_none() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .child(Label::new("No rigs or agents yet").color(Color::Muted))
                .child(
                    Label::new("Rigs show here once one is up")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
                .into_any_element();
        }
        match self.view {
            SubView::Table => self.render_table(window, cx),
            SubView::Recent => self.render_recent(cx),
            SubView::Overview => self.render_overview(cx),
            SubView::Graph => match &self.map {
                Some(map) => div().size_full().child(map.clone()).into_any_element(),
                None => gpui::Empty.into_any_element(),
            },
            SubView::Health => self.render_health(cx),
        }
    }
}

/// A rig's status as a colour, as the Fleet colours it.
fn rig_color(status: RigStatus) -> Color {
    match status {
        RigStatus::Running => Color::Success,
        RigStatus::Launching => Color::Info,
        RigStatus::Partial => Color::Warning,
        RigStatus::Stopped => Color::Muted,
    }
}

/// A row's state as a colour: an agent's as the Fleet colours it, an
/// empty seat's by its status.
fn row_state_color(state: RowState) -> Color {
    match state {
        RowState::Agent(s) => state_color(s),
        RowState::Seat(SeatStatus::Failed) => Color::Error,
        RowState::Seat(SeatStatus::Pending | SeatStatus::Launching) => Color::Info,
        RowState::Seat(SeatStatus::Running) => Color::Success,
        RowState::Seat(SeatStatus::Stopped) => Color::Disabled,
    }
}

impl EventEmitter<ItemEvent> for TopologyItem {}

impl Focusable for TopologyItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Item for TopologyItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        match &self.scope {
            Some(Scope::Rig(rig)) => format!("Topology · {rig}").into(),
            Some(Scope::Pod { rig, pod }) => format!("Topology · {rig}/{pod}").into(),
            Some(Scope::Loose) => "Topology · loose".into(),
            None => "Topology".into(),
        }
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        Some(
            Icon::new(IconName::GitGraph)
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some("The rigs, their pods and seats".into())
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        let (rig, pod, loose) = match &self.scope {
            Some(Scope::Rig(rig)) => (Some(rig.clone()), None, false),
            Some(Scope::Pod { rig, pod }) => (Some(rig.clone()), Some(pod.clone()), false),
            Some(Scope::Loose) => (None, None, true),
            None => (None, None, false),
        };
        Some(SavedItem::Topology {
            rig,
            pod,
            loose,
            view: (self.view != SubView::Table).then(|| self.view.name().to_string()),
        })
    }
}

impl Render for TopologyItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `view` only while the view itself (not its filter or its map)
        // has the keyboard: its bare keys must not fire while typing.
        let mut context = KeyContext::new_with_defaults();
        context.add("Topology");
        if self.focus.is_focused(window) {
            context.add("view");
        }
        h_flex()
            .id("topology")
            .key_context(context)
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.select_relative(1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.select_relative(-1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectFirst, _, cx| this.select_relative(i32::MIN, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectLast, _, cx| this.select_relative(i32::MAX, cx)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.confirm(window, cx)))
            .on_action(cx.listener(|this, _: &ShowTable, _, cx| this.show_view(SubView::Table, cx)))
            .on_action(cx.listener(|this, _: &ShowRecent, _, cx| this.show_view(SubView::Recent, cx)))
            .on_action(cx.listener(|this, _: &ShowOverview, _, cx| this.show_view(SubView::Overview, cx)))
            .on_action(cx.listener(|this, _: &ShowGraph, _, cx| this.show_view(SubView::Graph, cx)))
            .on_action(cx.listener(|this, _: &ShowHealth, _, cx| this.show_view(SubView::Health, cx)))
            .on_action(cx.listener(|this, _: &NextView, _, cx| this.show_view(this.view.step(1), cx)))
            .on_action(cx.listener(|this, _: &PreviousView, _, cx| this.show_view(this.view.step(-1), cx)))
            .on_action(cx.listener(|this, _: &CycleRig, _, cx| this.cycle_rig(cx)))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                this.show_view(SubView::Table, cx);
                window.focus(&this.filter.focus_handle(cx), cx);
            }))
            .on_action(cx.listener(|this, _: &OpenContextMenu, window, cx| {
                // Under the selected row, or at the mouse.
                let position = match this.selected_bounds.get() {
                    Some(b) => point(b.left() + px(24.), b.bottom()),
                    None => window.mouse_position(),
                };
                this.deploy_context_menu(position, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .child(self.render_explorer(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_tabs(cx))
                    .child(div().flex_1().min_h_0().child(self.render_body(window, cx))),
            )
            .children(self.context_menu.as_ref().map(|deployed| {
                deferred(
                    anchored()
                        .position(deployed.position)
                        .anchor(gpui::Anchor::TopLeft)
                        .child(deployed.menu.clone()),
                )
                .with_priority(3)
            }))
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::{QState, Request};
    use ainet_model::{Command, Msg};
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::args::View;
    use crate::report::ReportItem;
    use crate::terminal_item::TerminalItem;
    use crate::tests::{demo_workspace, init_test, sent};
    use crate::workspace::Workspace;

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    fn active<T: 'static>(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<T>> {
        w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<T>().ok())
        })
    }

    /// The demo with the Topology view opened by its key.
    fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<TopologyItem>, &mut VisualTestContext) {
        let (w, cx) = demo_workspace(cx);
        cx.simulate_keystrokes("ctrl-shift-t");
        cx.run_until_parked();
        let topology = active::<TopologyItem>(&w, cx).expect("ctrl-shift-t opens the Topology view");
        (w, topology, cx)
    }

    fn members(t: &Entity<TopologyItem>, cx: &mut VisualTestContext) -> Vec<String> {
        t.read_with(cx, |t, _| t.shown().iter().map(|r| r.member.clone()).collect())
    }

    fn view_has_keys(t: &Entity<TopologyItem>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| t.focus_handle(cx).is_focused(window))
    }

    fn click(selector: &str, cx: &mut VisualTestContext) {
        cx.run_until_parked();
        let at = cx
            .debug_bounds(selector.to_string().leak())
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        cx.simulate_click(at.center(), Modifiers::none());
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn the_demo_shows_its_tree_and_the_docs_table_grouped_by_pod(cx: &mut TestAppContext) {
        let (w, topology, cx) = open(cx);
        assert!(view_has_keys(&topology, cx));
        let host = host_name();
        let tree: Vec<(usize, Node)> = topology.read_with(cx, |t, _| {
            t.tree().iter().map(|r| (r.depth, r.node.clone())).collect()
        });
        assert_eq!(tree[0], (0, Node::Topology));
        assert_eq!(tree[1], (1, Node::Host(host)));
        let named: Vec<String> = tree
            .iter()
            .filter_map(|(_, n)| match n {
                Node::Rig { name, .. } => Some(name.clone()),
                Node::Pod { rig, pod, .. } => Some(format!("{rig}/{pod}")),
                Node::Loose { .. } => Some("loose".into()),
                _ => None,
            })
            .collect();
        assert_eq!(
            named,
            [
                "docs",
                "docs/orch",
                "docs/dev",
                "docs/rev",
                "infra",
                "infra/ops",
                "loose"
            ]
        );
        for ix in 0..tree.len() {
            assert!(
                cx.debug_bounds(format!("TOPOLOGY-NODE-{ix}").leak()).is_some(),
                "node {ix} drawn"
            );
        }

        // The selected agent's rig, its seats grouped by pod.
        assert_eq!(
            topology.read_with(cx, |t, _| t.scope().cloned()),
            Some(Scope::Rig("docs".into()))
        );
        assert_eq!(members(&topology, cx), ["lead", "ui", "app", "writer", "review"]);
        let pods: Vec<(bool, String)> = topology.read_with(cx, |t, _| {
            t.shown()
                .iter()
                .map(|r| (r.first_in_pod, r.pod.clone()))
                .collect()
        });
        assert_eq!(
            pods,
            [
                (true, "orch"),
                (true, "dev"),
                (false, "dev"),
                (false, "dev"),
                (true, "rev")
            ]
            .map(|(f, p)| (f, p.to_string()))
        );
        for ix in 0..5 {
            assert!(
                cx.debug_bounds(format!("TOPOLOGY-ROW-{ix}").leak()).is_some(),
                "row {ix} drawn"
            );
            // A line above each pod but the first, apart from the
            // selection's left border.
            let line = cx.debug_bounds(format!("TOPOLOGY-POD-LINE-{ix}").leak());
            assert_eq!(line.is_some(), ix == 1 || ix == 4, "pod line above row {ix}");
        }
        assert_eq!(
            topology.read_with(cx, |t, _| t.summary().to_string()),
            "5 seats · 3 working · 1 need attention · 4 open rows"
        );
        let title = w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .unwrap()
                .tab_content_text(cx)
                .to_string()
        });
        assert_eq!(title, "Topology · docs");

        // Opening it again activates the same tab.
        cx.simulate_keystrokes("ctrl-shift-c ctrl-shift-t");
        let tabs = w.read_with(cx, |w, cx| w.active_pane().read(cx).items().count());
        assert_eq!(tabs, 3, "the stand-in, Comms and one Topology");
        assert_eq!(active::<TopologyItem>(&w, cx), Some(topology));
    }

    #[gpui::test]
    async fn the_tree_and_ctrl_r_choose_what_the_right_side_shows(cx: &mut TestAppContext) {
        let (w, topology, cx) = open(cx);
        let scope = |cx: &mut VisualTestContext| topology.read_with(cx, |t, _| t.scope().cloned());
        // infra (node 6), then the dev pod of docs (node 4).
        click("TOPOLOGY-NODE-6", cx);
        assert_eq!(scope(cx), Some(Scope::Rig("infra".into())));
        assert_eq!(members(&topology, cx), ["data", "sre"]);
        let title = w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .unwrap()
                .tab_content_text(cx)
                .to_string()
        });
        assert_eq!(title, "Topology · infra");
        click("TOPOLOGY-NODE-4", cx);
        let dev = Scope::Pod {
            rig: "docs".into(),
            pod: "dev".into(),
        };
        assert_eq!(scope(cx), Some(dev));
        assert_eq!(members(&topology, cx), ["ui", "app", "writer"]);
        // The host chooses nothing.
        click("TOPOLOGY-NODE-1", cx);
        assert_eq!(members(&topology, cx).len(), 3);

        // ctrl-r: the next rig, the loose agents, round again.
        assert!(view_has_keys(&topology, cx));
        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(scope(cx), Some(Scope::Rig("infra".into())));
        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(scope(cx), Some(Scope::Loose));
        assert_eq!(
            topology.read_with(cx, |t, _| t.shown()[0].state),
            RowState::Agent(AgentState::Gone)
        );
        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(scope(cx), Some(Scope::Rig("docs".into())));
    }

    #[gpui::test]
    async fn the_filter_narrows_the_table_and_keeps_its_letters(cx: &mut TestAppContext) {
        let (_w, topology, cx) = open(cx);
        let filter = topology.read_with(cx, |t, _| t.filter().clone());
        cx.simulate_keystrokes("/");
        assert!(cx.update(|window, cx| filter.focus_handle(cx).is_focused(window)));
        // Digits, `/` and tab typed there are the filter's.
        cx.simulate_input("dev");
        assert_eq!(members(&topology, cx), ["ui", "app", "writer"]);
        cx.simulate_keystrokes("2 /");
        assert_eq!(filter.read_with(cx, |f, _| f.text().to_string()), "dev2/");
        assert!(members(&topology, cx).is_empty());
        cx.simulate_keystrokes("tab ctrl-r");
        assert_eq!(
            topology.read_with(cx, |t, _| t.view()),
            SubView::Table,
            "no tab switched"
        );
        assert_eq!(
            topology.read_with(cx, |t, _| t.scope().cloned()),
            Some(Scope::Rig("docs".into())),
            "no rig switched"
        );
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("dev");
        // The summary is the rig's, whatever the filter shows.
        assert_eq!(topology.read_with(cx, |t, _| t.summary().seats), 5);
        // Every word: a pod and a state; then a model.
        cx.simulate_input(" work");
        assert_eq!(members(&topology, cx), ["ui", "writer"]);
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("haiku");
        assert_eq!(members(&topology, cx), ["review"]);
        assert!(topology.read_with(cx, |t, _| t.shown()[0].first_in_pod));

        // Enter gives the table the keyboard, the rows still filtered.
        cx.simulate_keystrokes("enter");
        assert!(view_has_keys(&topology, cx));
        assert_eq!(members(&topology, cx), ["review"]);
        // Escape in the filter clears it, then leaves it.
        cx.simulate_keystrokes("/ escape");
        assert!(cx.update(|window, cx| filter.focus_handle(cx).is_focused(window)));
        assert_eq!(members(&topology, cx).len(), 5);
        cx.simulate_keystrokes("escape");
        assert!(view_has_keys(&topology, cx));
    }

    #[gpui::test]
    async fn enter_opens_the_selected_seats_terminal(cx: &mut TestAppContext) {
        let (w, topology, cx) = open(cx);
        let fleet = fleet(&w, cx);
        let app = fleet.read_with(cx, |f, _| f.store.find_agent("dev-app@docs").unwrap().id.clone());
        // Down to dev.app: the Fleet's selection follows.
        cx.simulate_keystrokes("home down down");
        assert_eq!(
            topology.read_with(cx, |t, _| t.selected().map(str::to_string)),
            Some("dev-app@docs".into())
        );
        assert_eq!(
            fleet.read_with(cx, |f, _| f.store.selected.clone()),
            Some(app.clone())
        );
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let terminal = active::<TerminalItem>(&w, cx).expect("enter opens a terminal");
        assert_eq!(terminal.read_with(cx, |t, _| t.agent_id().to_string()), app);

        // A double click on a row does the same.
        cx.simulate_keystrokes("ctrl-shift-t");
        let row = cx.debug_bounds("TOPOLOGY-ROW-0").expect("drawn");
        cx.simulate_click(row.center(), Modifiers::none());
        cx.simulate_event(gpui::MouseDownEvent {
            position: row.center(),
            modifiers: Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            position: row.center(),
            modifiers: Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count: 2,
        });
        cx.run_until_parked();
        let lead = fleet.read_with(cx, |f, _| {
            f.store.find_agent("orch-lead@docs").unwrap().id.clone()
        });
        let terminal = active::<TerminalItem>(&w, cx).expect("a double click opens a terminal");
        assert_eq!(terminal.read_with(cx, |t, _| t.agent_id().to_string()), lead);
    }

    #[gpui::test]
    async fn a_right_click_gives_the_fleets_agent_menu(cx: &mut TestAppContext) {
        let (w, topology, cx) = open(cx);
        let fleet = fleet(&w, cx);
        let ui = fleet.read_with(cx, |f, _| f.store.find_agent("dev-ui@docs").unwrap().id.clone());
        let row = cx.debug_bounds("TOPOLOGY-ROW-1").expect("drawn");
        cx.simulate_mouse_down(row.center(), gpui::MouseButton::Right, Modifiers::none());
        assert!(topology.read_with(cx, |t, _| t.context_menu().is_some()));
        assert_eq!(
            fleet.read_with(cx, |f, _| f.store.selected.clone()),
            Some(ui.clone())
        );
        click("MENU_ITEM-Show Transcript", cx);
        assert!(topology.read_with(cx, |t, _| t.context_menu().is_none()));
        let last = sent(&w, cx).last().cloned();
        assert!(
            matches!(&last, Some(Request::Transcript { agent, .. }) if *agent == ui),
            "{last:?}"
        );
        // shift-f10 opens it from the keys; escape gives them back.
        cx.simulate_keystrokes("ctrl-shift-t shift-f10");
        assert!(topology.read_with(cx, |t, _| t.context_menu().is_some()));
        cx.simulate_keystrokes("escape");
        assert!(topology.read_with(cx, |t, _| t.context_menu().is_none()));
        assert!(view_has_keys(&topology, cx));
    }

    #[gpui::test]
    async fn the_tabs_switch_by_keys_and_actions(cx: &mut TestAppContext) {
        let (_w, topology, cx) = open(cx);
        let view = |cx: &mut VisualTestContext| topology.read_with(cx, |t, _| t.view());
        cx.simulate_keystrokes("2");
        assert_eq!(view(cx), SubView::Recent);
        cx.simulate_keystrokes("tab");
        assert_eq!(view(cx), SubView::Overview);
        cx.simulate_keystrokes("shift-tab shift-tab shift-tab");
        assert_eq!(view(cx), SubView::Health, "round again");
        cx.dispatch_action(ShowTable);
        assert_eq!(view(cx), SubView::Table);

        // Graph: the Map of the rig alone.
        assert!(
            topology.read_with(cx, |t, _| t.map().is_none()),
            "made when first shown"
        );
        cx.simulate_keystrokes("4");
        cx.run_until_parked();
        let map = topology
            .read_with(cx, |t, _| t.map().cloned())
            .expect("the graph's map");
        let (_, layout) = map.read_with(cx, |m, _| m.drawn().cloned()).expect("drawn");
        assert_eq!(layout.groups.len(), 1);
        assert_eq!(layout.nodes.len(), 5, "docs' five");
        // It follows the scope.
        cx.simulate_keystrokes("ctrl-r");
        cx.run_until_parked();
        let (_, layout) = map.read_with(cx, |m, _| m.drawn().cloned()).expect("drawn");
        assert_eq!(layout.nodes.len(), 2, "infra's two");

        // Health: infra is clean, as far as the checks go; docs has dev.app
        // waiting on you.
        cx.simulate_keystrokes("5");
        cx.run_until_parked();
        assert!(cx.debug_bounds("TOPOLOGY-NO-FINDINGS").is_some());
        cx.simulate_keystrokes("ctrl-r ctrl-r");
        cx.run_until_parked();
        assert!(cx.debug_bounds("TOPOLOGY-NO-FINDINGS").is_none());
        assert!(cx.debug_bounds("TOPOLOGY-FINDING-0").is_some());

        // Overview draws.
        cx.simulate_keystrokes("3");
        cx.run_until_parked();
        assert_eq!(view(cx), SubView::Overview);
    }

    #[gpui::test]
    async fn recent_lists_the_rigs_queue_changes_and_enter_shows_one(cx: &mut TestAppContext) {
        let (w, topology, cx) = open(cx);
        cx.simulate_keystrokes("2");
        cx.run_until_parked();
        let changes: Vec<(usize, String, QState)> = topology.read_with(cx, |t, _| {
            t.changes()
                .iter()
                .map(|c| (c.n, c.item.clone(), c.state))
                .collect()
        });
        assert_eq!(changes.len(), 7);
        assert!(changes.windows(2).all(|w| w[0].0 + 1 == w[1].0));
        assert_eq!(changes[6], (7, "q-4".into(), QState::Blocked));
        for ix in 0..7 {
            assert!(
                cx.debug_bounds(format!("TOPOLOGY-CHANGE-{ix}").leak()).is_some(),
                "change {ix} drawn"
            );
        }
        // The newest is selected; up goes to q-4's creation; enter shows it.
        assert_eq!(topology.read_with(cx, |t, _| t.selected_change()), Some(7));
        cx.simulate_keystrokes("up enter");
        cx.run_until_parked();
        assert!(matches!(sent(&w, cx).last(), Some(Request::QueueShow { id }) if id == "q-4"));
        assert!(
            active::<ReportItem>(&w, cx).is_some(),
            "as the Queue panel's Show"
        );

        // A click selects; home is the oldest.
        cx.simulate_keystrokes("ctrl-shift-t");
        click("TOPOLOGY-CHANGE-2", cx);
        assert_eq!(topology.read_with(cx, |t, _| t.selected_change()), Some(3));
        cx.simulate_keystrokes("home enter");
        cx.run_until_parked();
        assert!(matches!(sent(&w, cx).last(), Some(Request::QueueShow { id }) if id == "q-1"));
    }

    #[gpui::test]
    async fn views_and_commands_open_it_on_a_rig(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(
                fleet,
                Some(View::Topology {
                    rig: Some("infra".into()),
                }),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let topology = active::<TopologyItem>(&w, cx).expect("--view topology:infra");
        assert_eq!(
            topology.read_with(cx, |t, _| t.scope().cloned()),
            Some(Scope::Rig("infra".into()))
        );

        // /view topology:docs moves it; a rig that is not there is said.
        let fleet = fleet(&w, cx);
        cx.update(|window, cx| {
            crate::commands::run(&fleet, Command::View("topology:docs".into()), window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            topology.read_with(cx, |t, _| t.scope().cloned()),
            Some(Scope::Rig("docs".into()))
        );
        let wrong = cx.update(|window, cx| {
            crate::commands::run(&fleet, Command::View("topology:nope".into()), window, cx)
        });
        assert_eq!(wrong.as_deref(), Some("no rig named nope"));
        let tabs = w.read_with(cx, |w, cx| w.active_pane().read(cx).items().count());
        assert_eq!(tabs, 1);
    }

    #[gpui::test]
    async fn a_rig_asked_for_waits_for_the_first_snapshot(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(|_| FleetModel::offline());
            Workspace::new(
                fleet,
                Some(View::Topology {
                    rig: Some("infra".into()),
                }),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let topology = active::<TopologyItem>(&w, cx).expect("--view topology:infra");
        let scope = |cx: &mut VisualTestContext| topology.read_with(cx, |t, _| t.scope().cloned());
        assert_eq!(scope(cx), Some(Scope::Rig("infra".into())));

        // The snapshot arrives later, as ainetd's does: still infra.
        let fleet = fleet(&w, cx);
        let snapshot = Msg::Snapshot(Box::new(ainet_model::Demo::new().snapshot()));
        fleet.update(cx, |f, cx| f.receive(vec![Msg::Connected(true), snapshot], cx));
        cx.run_until_parked();
        assert_eq!(scope(cx), Some(Scope::Rig("infra".into())));
        assert!(!members(&topology, cx).is_empty());
    }

    #[gpui::test]
    async fn an_empty_fleet_says_so(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(|_| FleetModel::offline());
            Workspace::new(fleet, Some(View::Topology { rig: None }), window, cx)
        });
        cx.run_until_parked();
        let topology = active::<TopologyItem>(&w, cx).expect("the view");
        topology.read_with(cx, |t, _| {
            assert_eq!(t.scope(), None);
            assert!(t.shown().is_empty());
            assert_eq!(t.tree().len(), 2, "the root and the host");
        });
    }
}
