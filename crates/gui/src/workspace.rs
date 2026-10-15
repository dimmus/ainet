//! The window's root view, laid out like Zed's workspace: title bar, the
//! left, right and bottom [`Dock`]s around the center's panes (tabs and
//! splits), the status bar, and over them the modal and toast layers,
//! inside client-side decorations. The Fleet panel docks left (or right,
//! by setting) with the Agent panel opposite; the Activity panel docks at
//! the bottom.
//!
//! It handles what spans panes and docks: toggling and resizing docks,
//! splitting, moving between and zooming panes, opening tabs, modals, and
//! the pane actions when the keyboard is outside the panes.

use ainet_model::{AgentCommand, command};
use gpui::{
    Bounds, DragMoveEvent, Entity, FocusHandle, Focusable, Pixels, PromptLevel, Subscription, WeakEntity,
    canvas,
};
use ui::prelude::*;

use crate::actions::{
    ActivateNextItem, ActivateNextPane, ActivatePaneDown, ActivatePaneLeft, ActivatePaneRight,
    ActivatePaneUp, ActivatePreviousItem, ActivatePreviousPane, Capture, CloseActiveItem, CloseAgent,
    CloseAllItems, CloseOtherItems, CloseWindow, Continue, MessageAgent, OpenComms, OpenInbox, OpenKeymap,
    OpenMap, OpenSettings, OpenTerminal, Relaunch, SelectNext, SelectPrev, ShowTranscript, SplitDown,
    SplitRight, ToggleBottomDock, ToggleCritterSize, ToggleLeftDock, ToggleRightDock, ToggleSwitcher,
    ToggleTypingGuard, ToggleZoom, terminal,
};
use crate::agent_panel::AgentPanel;
use crate::args::View;
use crate::decorations::client_side_decorations;
use crate::dock::{Dock, DockPosition, DraggedDock, Panel, RESIZE_HANDLE_SIZE};
use crate::fleet::FleetModel;
use crate::fleet_panel::FleetPanel;
use crate::item::Placeholder;
use crate::message::MessageModal;
use crate::modal_layer::{ModalLayer, ModalView};
use crate::pane::{Pane, PaneEvent};
use crate::pane_group::{PaneGroup, SplitDirection};
use crate::panels::ActivityPanel;
use crate::picker::Picker;
use crate::settings::{self, DockSide};
use crate::status_bar::StatusBar;
use crate::switcher::AgentSwitcher;
use crate::terminal_item::TerminalItem;
use crate::title_bar::TitleBar;
use crate::toast_layer::ToastLayer;
use crate::{keymap, quit};

/// A dock is never dragged smaller than this.
const MIN_DOCK_SIZE: Pixels = px(120.);
/// Nor so big that the center gets less than this.
const MIN_CENTER_SIZE: Pixels = px(160.);

/// Which panels show. By panel, not by side, so moving the Fleet to the
/// other side (a setting) keeps it open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Docks {
    pub fleet: bool,
    pub agent: bool,
    pub bottom: bool,
}

impl Default for Docks {
    fn default() -> Self {
        Docks {
            fleet: true,
            agent: true,
            bottom: true,
        }
    }
}

pub struct Workspace {
    fleet: Entity<FleetModel>,
    fleet_panel: Entity<FleetPanel>,
    title_bar: Entity<TitleBar>,
    status_bar: Entity<StatusBar>,
    center: PaneGroup<Entity<Pane>>,
    active_pane: Entity<Pane>,
    /// The pane filling the window, if one is zoomed.
    zoomed: Option<WeakEntity<Pane>>,
    left_dock: Entity<Dock>,
    right_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
    /// The side the Fleet panel is on, to notice the setting change.
    fleet_side: DockSide,
    modal_layer: Entity<ModalLayer>,
    toast_layer: Entity<ToastLayer>,
    /// Where the docks and center were last drawn, for resizing docks.
    bounds: Bounds<Pixels>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
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
        let modal_layer = cx.new(|_| ModalLayer::new());
        let toast_layer = cx.new(|cx| ToastLayer::new(&fleet, cx));

        let fleet_panel = cx.new(|cx| FleetPanel::new(fleet.clone(), cx));
        let agent_panel = cx.new(|cx| AgentPanel::new(fleet.clone(), cx));
        let activity_panel = cx.new(|cx| ActivityPanel::new(fleet.clone(), cx));
        let fleet_side = settings::get(cx).fleet_panel.dock;
        let dock = |position, cx: &mut Context<Self>| cx.new(|cx| Dock::new(position, cx));
        let (left_dock, right_dock, bottom_dock) = (
            dock(DockPosition::Left, cx),
            dock(DockPosition::Right, cx),
            dock(DockPosition::Bottom, cx),
        );
        let (fleet_dock, agent_dock) = match fleet_side {
            DockSide::Left => (&left_dock, &right_dock),
            DockSide::Right => (&right_dock, &left_dock),
        };
        fleet_dock.update(cx, |d, cx| d.add_panel(fleet_panel.clone(), cx));
        agent_dock.update(cx, |d, cx| d.add_panel(agent_panel, cx));
        bottom_dock.update(cx, |d, cx| d.add_panel(activity_panel, cx));
        for dock in [&left_dock, &right_dock, &bottom_dock] {
            dock.update(cx, |d, cx| d.set_open(true, window, cx));
        }
        let this = cx.entity().downgrade();
        let docks = [left_dock.clone(), bottom_dock.clone(), right_dock.clone()];
        let status_bar = cx.new(|cx| StatusBar::new(fleet.clone(), this, docks, cx));

        let first = cx.new(|cx| Pane::new(window, cx));
        let mut subscriptions = vec![
            cx.subscribe_in(&first, window, Self::handle_pane_event),
            cx.observe_global_in::<settings::AppSettings>(window, |this, window, cx| {
                this.follow_settings(window, cx)
            }),
        ];
        let placeholder = cx.new(|cx| Placeholder::new(view.unwrap_or(View::Fleet), cx));
        first.update(cx, |pane, cx| {
            pane.add_item(Box::new(placeholder), true, false, window, cx)
        });
        subscriptions.push(cx.observe(&first, |_, _, cx| cx.notify()));

        // The Fleet has the keyboard until a terminal or tab takes it.
        window.focus(&fleet_panel.focus_handle(cx), cx);
        // The window manager's close goes the way of `CloseWindow`.
        window.on_window_should_close(cx, |window, cx| {
            if quit::must_ask_on_close(cx) {
                quit::ask(window, cx);
                false
            } else {
                true
            }
        });
        Workspace {
            fleet,
            fleet_panel,
            title_bar,
            status_bar,
            center: PaneGroup::new(first.clone()),
            active_pane: first,
            zoomed: None,
            left_dock,
            right_dock,
            bottom_dock,
            fleet_side,
            modal_layer,
            toast_layer,
            bounds: Bounds::default(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub fn fleet(&self) -> &Entity<FleetModel> {
        &self.fleet
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fleet_panel(&self) -> &Entity<FleetPanel> {
        &self.fleet_panel
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn active_pane(&self) -> &Entity<Pane> {
        &self.active_pane
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn panes(&self) -> Vec<Entity<Pane>> {
        self.center.panes().into_iter().cloned().collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn zoomed_pane(&self) -> Option<Entity<Pane>> {
        self.zoomed.as_ref().and_then(|z| z.upgrade())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn modal_layer(&self) -> &Entity<ModalLayer> {
        &self.modal_layer
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn toast_layer(&self) -> &Entity<ToastLayer> {
        &self.toast_layer
    }

    pub fn dock(&self, position: DockPosition) -> &Entity<Dock> {
        match position {
            DockPosition::Left => &self.left_dock,
            DockPosition::Right => &self.right_dock,
            DockPosition::Bottom => &self.bottom_dock,
        }
    }

    /// Which panels show.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn docks(&self, cx: &App) -> Docks {
        let showing = |name: &str| {
            [&self.left_dock, &self.right_dock, &self.bottom_dock]
                .into_iter()
                .any(|d| d.read(cx).is_open() && d.read(cx).active_panel_name() == Some(name))
        };
        Docks {
            fleet: showing(FleetPanel::persistent_name()),
            agent: showing(AgentPanel::persistent_name()),
            bottom: showing(ActivityPanel::persistent_name()),
        }
    }

    /// Moves the Fleet and Agent panels across when the setting changes
    /// sides: the two side docks swap what they hold, open or not.
    fn follow_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let side = settings::get(cx).fleet_panel.dock;
        if side != self.fleet_side {
            self.fleet_side = side;
            let left = self.left_dock.update(cx, |d, cx| d.take_contents(cx));
            let right = self.right_dock.update(cx, |d, cx| d.take_contents(cx));
            self.left_dock.update(cx, |d, cx| d.set_contents(right, cx));
            self.right_dock.update(cx, |d, cx| d.set_contents(left, cx));
            self.release_hidden_focus(window, cx);
        }
        cx.notify();
    }

    /// A closed dock's panel cannot keep the keyboard, or the workspace's
    /// keys would go nowhere: hand it to the center.
    fn release_hidden_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for dock in [&self.left_dock, &self.right_dock, &self.bottom_dock] {
            let dock = dock.read(cx);
            if !dock.is_open()
                && dock
                    .active_panel()
                    .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
            {
                let pane = self.active_pane.focus_handle(cx);
                window.focus(&pane, cx);
                return;
            }
        }
    }

    /// Opens or closes a dock. As in Zed, opening one gives its panel the
    /// keyboard and closing it hands the keyboard to the center; a zoomed
    /// pane steps back so the dock shows.
    pub fn toggle_dock(&mut self, position: DockPosition, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_out(cx);
        let dock = self.dock(position).clone();
        let open = !dock.read(cx).is_open();
        dock.update(cx, |d, cx| d.set_open(open, window, cx));
        if open {
            if let Some(panel) = dock.read(cx).active_panel() {
                let handle = panel.focus_handle(cx);
                window.focus(&handle, cx);
            }
        } else {
            self.release_hidden_focus(window, cx);
        }
        cx.notify();
    }

    /// The left and right dock keys, by side as in Zed.
    fn toggle_side(&mut self, left: bool, window: &mut Window, cx: &mut Context<Self>) {
        let position = if left {
            DockPosition::Left
        } else {
            DockPosition::Right
        };
        self.toggle_dock(position, window, cx);
    }

    /// Resizes a dock to `size` along its axis, within what the window
    /// leaves: the center keeps some room, and each dock some size.
    pub fn resize_dock(&mut self, position: DockPosition, size: Pixels, cx: &mut Context<Self>) {
        let room = match position {
            DockPosition::Left | DockPosition::Right => {
                let other = self.dock(if position == DockPosition::Left {
                    DockPosition::Right
                } else {
                    DockPosition::Left
                });
                let other = other.read(cx);
                let taken = if other.is_open() {
                    other.active_size(cx).unwrap_or_default()
                } else {
                    Pixels::ZERO
                };
                self.bounds.size.width - taken
            }
            DockPosition::Bottom => self.bounds.size.height,
        };
        let max = (room - MIN_CENTER_SIZE - RESIZE_HANDLE_SIZE).max(MIN_DOCK_SIZE);
        let size = size.clamp(MIN_DOCK_SIZE, max);
        self.dock(position)
            .update(cx, |d, cx| d.resize_active(Some(size), cx));
        cx.notify();
    }

    fn on_dock_drag(&mut self, e: &DragMoveEvent<DraggedDock>, cx: &mut Context<Self>) {
        let at = e.event.position;
        let b = self.bounds;
        let (position, size) = match e.drag(cx).0 {
            DockPosition::Left => (DockPosition::Left, at.x - b.left()),
            DockPosition::Right => (DockPosition::Right, b.right() - at.x),
            DockPosition::Bottom => (DockPosition::Bottom, b.bottom() - at.y),
        };
        self.resize_dock(position, size, cx);
    }

    fn add_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        let pane = cx.new(|cx| Pane::new(window, cx));
        self._subscriptions
            .push(cx.subscribe_in(&pane, window, Self::handle_pane_event));
        self._subscriptions
            .push(cx.observe(&pane, |_, _, cx| cx.notify()));
        pane
    }

    /// Splits `pane`, copying its active tab into the new pane when the
    /// tab can split, and moves the keyboard to the new pane.
    pub fn split_pane(
        &mut self,
        pane: &Entity<Pane>,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        self.zoom_out(cx);
        let new = self.add_pane(window, cx);
        let item = pane.read(cx).active_item().map(|i| i.boxed_clone());
        if let Some(item) = item.filter(|i| i.can_split(cx))
            && let Some(copy) = item.clone_on_split(window, cx)
        {
            new.update(cx, |p, cx| p.add_item(copy, true, false, window, cx));
        }
        self.center.split(pane, &new, direction);
        self.active_pane = new.clone();
        window.focus(&new.focus_handle(cx), cx);
        cx.notify();
        new
    }

    fn handle_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Split(direction) => {
                self.split_pane(pane, *direction, window, cx);
            }
            PaneEvent::Remove => {
                if self.center.remove(pane) {
                    if self.zoomed_pane().as_ref() == Some(pane) {
                        self.zoomed = None;
                    }
                    if &self.active_pane == pane {
                        self.active_pane = self.center.panes()[0].clone();
                    }
                    let had_focus = pane.read(cx).has_focus(window, cx) || window.focused(cx).is_none();
                    if had_focus {
                        window.focus(&self.active_pane.focus_handle(cx), cx);
                    }
                }
            }
            PaneEvent::Focus => {
                if &self.active_pane != pane {
                    if self.zoomed_pane().is_some_and(|z| &z != pane) {
                        self.zoom_out(cx);
                    }
                    self.active_pane = pane.clone();
                }
            }
            PaneEvent::ZoomIn => {
                self.zoom_out(cx);
                self.active_pane = pane.clone();
                pane.update(cx, |p, cx| p.set_zoomed(true, cx));
                self.zoomed = Some(pane.downgrade());
            }
            PaneEvent::ZoomOut => self.zoom_out(cx),
            PaneEvent::ActivateItem => {}
        }
        cx.notify();
    }

    fn zoom_out(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.zoomed.take().and_then(|z| z.upgrade()) {
            pane.update(cx, |p, cx| p.set_zoomed(false, cx));
            cx.notify();
        }
    }

    /// Moves the keyboard to `to`, if there is such a pane.
    fn activate_pane(&mut self, to: Option<Entity<Pane>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = to {
            self.active_pane = pane.clone();
            window.focus(&pane.focus_handle(cx), cx);
            cx.notify();
        }
    }

    /// Moves the keyboard `delta` panes on in order, wrapping round.
    fn activate_relative_pane(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.panes();
        let at = panes.iter().position(|p| p == &self.active_pane).unwrap_or(0) as isize;
        let next = panes
            .get((at + delta).rem_euclid(panes.len() as isize) as usize)
            .cloned();
        self.activate_pane(next, window, cx);
    }

    /// Moves the keyboard to the neighbouring pane in `direction`.
    fn activate_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = self
            .center
            .find_pane_in_direction(&self.active_pane, direction)
            .cloned();
        self.activate_pane(next, window, cx);
    }

    /// Shows a view of phase 4 as a tab of the active pane, or activates
    /// its tab if one is open there already.
    pub fn open_placeholder(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_pane.clone();
        let existing = pane.read(cx).items().position(|item| {
            item.to_any()
                .downcast::<Placeholder>()
                .is_ok_and(|p| p.read(cx).view() == &view)
        });
        match existing {
            Some(ix) => pane.update(cx, |p, cx| p.activate_item(ix, true, window, cx)),
            None => {
                let item = cx.new(|cx| Placeholder::new(view, cx));
                pane.update(cx, |p, cx| p.add_item(Box::new(item), true, true, window, cx));
            }
        }
    }

    /// Opens agent `id`'s terminal: activates its tab where one is open
    /// (in any pane), else adds one to the active pane. The tab takes the
    /// keyboard.
    pub fn open_terminal(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.fleet.read(cx).store.agents.contains_key(id) {
            return;
        }
        for pane in self.panes() {
            let found = pane.read(cx).items().position(|item| {
                item.to_any()
                    .downcast::<TerminalItem>()
                    .is_ok_and(|t| t.read(cx).agent_id() == id)
            });
            if let Some(ix) = found {
                self.active_pane = pane.clone();
                pane.update(cx, |p, cx| p.activate_item(ix, true, window, cx));
                window.focus(&pane.focus_handle(cx), cx);
                return;
            }
        }
        let (fleet, id) = (self.fleet.clone(), id.to_string());
        let item = cx.new(|cx| TerminalItem::new(fleet, id, cx));
        self.active_pane
            .update(cx, |p, cx| p.add_item(Box::new(item), true, true, window, cx));
    }

    /// The selected agent's id.
    fn selected(&self, cx: &App) -> Option<String> {
        self.fleet.read(cx).store.selected.clone()
    }

    /// Asks ainetd to do `cmd` to the selected agent.
    fn agent_command(&mut self, cmd: AgentCommand, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        self.fleet.update(cx, |fleet, cx| {
            let command = command::for_agent(&fleet.store, &id, cmd);
            fleet.run(command, cx);
        });
    }

    /// Turns the typing guard of the selected agent's seat the other way.
    fn toggle_typing_guard(&mut self, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        let on = store
            .selected_agent()
            .and_then(|a| store.seat_of(a))
            .is_some_and(|s| s.typing_guard);
        self.agent_command(AgentCommand::Guard(!on), cx);
    }

    /// Closes the selected agent once you say so: its process ends.
    fn close_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.fleet.read(cx).store.selected_agent().cloned() else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Close {}?", agent.label()),
            Some("Its process ends. A rig seat can be launched again."),
            &["Close", "Cancel"],
            cx,
        );
        let fleet = self.fleet.clone();
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(0) {
                fleet.update(cx, |fleet, cx| {
                    let close = command::for_agent(&fleet.store, &agent.id, AgentCommand::Close);
                    fleet.run(close, cx);
                });
            }
        })
        .detach();
    }

    /// Opens the message modal for the selected agent.
    fn message_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        let fleet = self.fleet.clone();
        self.toggle_modal(window, cx, move |window, cx| {
            MessageModal::new(fleet, id, window, cx)
        });
    }

    /// Opens a modal of type `V` (closing it if open).
    pub fn toggle_modal<V: ModalView>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) {
        self.modal_layer
            .update(cx, |layer, cx| layer.toggle_modal(window, cx, build));
    }

    fn toggle_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fleet = self.fleet.clone();
        self.toggle_modal(window, cx, move |window, cx| {
            let switcher = AgentSwitcher::new(fleet, cx);
            Picker::uniform_list(switcher, window, cx)
        });
    }

    /// Runs a pane action on the active pane, for when the keyboard is
    /// outside the panes (a pane handles its own actions while focused).
    fn on_active_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Pane, &mut Window, &mut Context<Pane>),
    ) {
        self.active_pane.update(cx, |pane, cx| f(pane, window, cx));
    }

    /// Opens a config file for editing and says where it is.
    fn open_config(&mut self, file: std::path::PathBuf, template: &str, cx: &mut Context<Self>) {
        let (text, error) = match settings::open_file(&file, template, cx) {
            Ok(()) => (format!("opened {}", file.display()), false),
            Err(e) => (e, true),
        };
        self.fleet.update(cx, |fleet, cx| fleet.show(text, error, cx));
    }

    /// Moves the Fleet selection over the agents the Fleet panel shows.
    fn select(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.fleet_panel
            .update(cx, |panel, cx| panel.select_relative(delta, cx));
    }

    /// The docks and center, or the zoomed pane alone.
    fn render_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let body = h_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<DraggedDock>, _, cx| this.on_dock_drag(e, cx)))
            .child(
                canvas(
                    move |bounds, _, cx| this.update(cx, |this, _| this.bounds = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        if let Some(zoomed) = self.zoomed_pane() {
            return body.child(
                div()
                    .size_full()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().background)
                    .child(zoomed),
            );
        }
        body.child(self.left_dock.clone())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .child(self.center.render()),
                    )
                    .child(self.bottom_dock.clone()),
            )
            .child(self.right_dock.clone())
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.release_hidden_focus(window, cx);
        let (ui_font, ui_font_size) = settings::ui_font(cx);
        window.set_rem_size(ui_font_size);
        let colors = cx.theme().colors();

        let content = v_flex()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .on_action(|_: &CloseWindow, window, cx| quit::close_window(window, cx))
            .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| this.toggle_side(true, window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleRightDock, window, cx| this.toggle_side(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleBottomDock, window, cx| {
                this.toggle_dock(DockPosition::Bottom, window, cx)
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
            .on_action(cx.listener(|this, _: &ToggleSwitcher, window, cx| this.toggle_switcher(window, cx)))
            // The selected agent's actions, from the Fleet's keys and menu.
            .on_action(cx.listener(|this, _: &OpenTerminal, window, cx| {
                if let Some(id) = this.selected(cx) {
                    this.open_terminal(&id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &MessageAgent, window, cx| this.message_agent(window, cx)))
            .on_action(cx.listener(|this, _: &CloseAgent, window, cx| this.close_agent(window, cx)))
            .on_action(
                cx.listener(|this, _: &ShowTranscript, _, cx| {
                    this.agent_command(AgentCommand::Transcript, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Relaunch, _, cx| this.agent_command(AgentCommand::Relaunch, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Continue, _, cx| this.agent_command(AgentCommand::Continue, cx)),
            )
            .on_action(cx.listener(|this, _: &Capture, _, cx| this.agent_command(AgentCommand::Capture, cx)))
            .on_action(cx.listener(|this, _: &ToggleTypingGuard, _, cx| this.toggle_typing_guard(cx)))
            .on_action(
                cx.listener(|this, _: &OpenComms, window, cx| this.open_placeholder(View::Comms, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenInbox, window, cx| this.open_placeholder(View::Inbox, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenMap, window, cx| this.open_placeholder(View::Map, window, cx)),
            )
            // Pane actions with the keyboard outside the panes.
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                let pane = this.active_pane.clone();
                this.split_pane(&pane, SplitDirection::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                let pane = this.active_pane.clone();
                this.split_pane(&pane, SplitDirection::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseActiveItem, window, cx| {
                this.on_active_pane(window, cx, Pane::close_active_item)
            }))
            .on_action(cx.listener(|this, _: &CloseOtherItems, window, cx| {
                this.on_active_pane(window, cx, |pane, window, cx| {
                    if let Some(id) = pane.active_item().map(|i| i.item_id()) {
                        pane.close_other_items(id, window, cx)
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &CloseAllItems, window, cx| {
                this.on_active_pane(window, cx, Pane::close_all_items)
            }))
            .on_action(cx.listener(|this, _: &ActivateNextItem, window, cx| {
                this.on_active_pane(window, cx, |p, w, cx| p.activate_relative(1, w, cx))
            }))
            .on_action(cx.listener(|this, _: &ActivatePreviousItem, window, cx| {
                this.on_active_pane(window, cx, |p, w, cx| p.activate_relative(-1, w, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleZoom, window, cx| {
                this.on_active_pane(window, cx, Pane::toggle_zoom)
            }))
            .on_action(cx.listener(|this, _: &terminal::ToggleZoom, window, cx| {
                this.on_active_pane(window, cx, Pane::toggle_zoom)
            }))
            .on_action(cx.listener(|this, _: &ActivateNextPane, window, cx| {
                this.activate_relative_pane(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivatePreviousPane, window, cx| {
                this.activate_relative_pane(-1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivatePaneLeft, window, cx| {
                this.activate_pane_in_direction(SplitDirection::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivatePaneRight, window, cx| {
                this.activate_pane_in_direction(SplitDirection::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivatePaneUp, window, cx| {
                this.activate_pane_in_direction(SplitDirection::Up, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivatePaneDown, window, cx| {
                this.activate_pane_in_direction(SplitDirection::Down, window, cx)
            }))
            .size_full()
            .font(ui_font)
            .text_color(colors.text)
            .bg(colors.background)
            .child(self.title_bar.clone())
            .child(self.render_body(cx))
            .child(self.status_bar.clone())
            .child(self.toast_layer.clone())
            .child(self.modal_layer.clone());

        client_side_decorations(content, window, cx)
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};

    use super::*;
    use crate::tests::init_test;

    fn workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(|_| FleetModel::offline());
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        (workspace, cx)
    }

    /// The tab titles of a pane, and which is active.
    fn tabs(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> (Vec<String>, usize) {
        pane.read_with(cx, |p, cx| {
            (
                p.items().map(|i| i.tab_content_text(cx).to_string()).collect(),
                p.active_index(),
            )
        })
    }

    fn active_pane(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<Pane> {
        w.read_with(cx, |w, _| w.active_pane().clone())
    }

    fn focused_in(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| pane.read(cx).has_focus(window, cx))
    }

    #[gpui::test]
    async fn tabs_open_cycle_and_close(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        let pane = active_pane(&w, cx);
        assert_eq!(tabs(&pane, cx), (vec!["Fleet".into()], 0));

        cx.simulate_keystrokes("ctrl-shift-c ctrl-shift-i ctrl-t");
        assert_eq!(
            tabs(&pane, cx),
            (
                vec!["Fleet".into(), "Comms".into(), "Inbox".into(), "Map".into()],
                3
            )
        );
        assert!(focused_in(&pane, cx), "an opened tab takes the keyboard");
        // Opening one that is open activates it.
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(tabs(&pane, cx).1, 1);

        cx.simulate_keystrokes("ctrl-tab ctrl-tab ctrl-tab");
        assert_eq!(tabs(&pane, cx).1, 0, "wraps round");
        cx.simulate_keystrokes("ctrl-shift-tab ctrl-pageup");
        assert_eq!(tabs(&pane, cx).1, 2);
        let fleet_tab = pane.read_with(cx, |p, _| p.items().next().unwrap().to_any());
        let activations = || fleet_tab.clone().downcast::<Placeholder>().unwrap();
        let before = activations().read_with(cx, |p, _| p.activations);

        // Closing the active tab goes back to the one used before it.
        cx.simulate_keystrokes("ctrl-tab ctrl-tab");
        assert_eq!(tabs(&pane, cx).1, 0);
        cx.simulate_keystrokes("ctrl-shift-tab");
        cx.simulate_keystrokes("ctrl-w");
        assert_eq!(
            tabs(&pane, cx),
            (vec!["Fleet".into(), "Comms".into(), "Inbox".into()], 0)
        );
        assert!(activations().read_with(cx, |p, _| p.activations) > before);

        // A middle click on a tab closes it.
        cx.run_until_parked();
        let tab = cx.debug_bounds("TAB-1").expect("the Comms tab is drawn");
        cx.simulate_click(tab.center(), Modifiers::none());
        assert_eq!(tabs(&pane, cx).1, 1, "a click activates");
        cx.simulate_mouse_down(tab.center(), MouseButton::Middle, Modifiers::none());
        cx.simulate_mouse_up(tab.center(), MouseButton::Middle, Modifiers::none());
        assert_eq!(tabs(&pane, cx).0, ["Fleet", "Inbox"]);

        // The last pane stays when its tabs are gone.
        cx.dispatch_action(CloseAllItems);
        assert_eq!(tabs(&pane, cx).0.len(), 0);
        assert_eq!(w.read_with(cx, |w, _| w.panes().len()), 1);
        // An item can close itself.
        cx.simulate_keystrokes("ctrl-shift-c");
        let item = pane.read_with(cx, |p, _| p.active_item().unwrap().to_any());
        item.downcast::<Placeholder>().unwrap().update(cx, |_, cx| {
            cx.emit(crate::item::ItemEvent::UpdateTab);
            cx.emit(crate::item::ItemEvent::CloseItem);
        });
        assert_eq!(tabs(&pane, cx).0.len(), 0);
    }

    #[gpui::test]
    async fn splits_move_the_keyboard_between_panes(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        let first = active_pane(&w, cx);
        cx.simulate_keystrokes("ctrl-\\");
        let right = active_pane(&w, cx);
        assert_ne!(first, right);
        assert_eq!(tabs(&right, cx).0, ["Fleet"], "the tab splits into a copy");
        assert!(focused_in(&right, cx));
        cx.simulate_keystrokes("ctrl-alt-\\");
        let below = active_pane(&w, cx);
        assert_eq!(
            w.read_with(cx, |w, _| w.panes()),
            [first.clone(), right.clone(), below.clone()]
        );

        // Next and previous go round in order.
        cx.dispatch_action(ActivateNextPane);
        assert_eq!(active_pane(&w, cx), first);
        assert!(focused_in(&first, cx));
        cx.dispatch_action(ActivatePreviousPane);
        assert_eq!(active_pane(&w, cx), below);

        // By direction, from where the panes were drawn.
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-alt-up");
        assert_eq!(active_pane(&w, cx), right);
        cx.simulate_keystrokes("ctrl-alt-left");
        assert_eq!(active_pane(&w, cx), first);
        cx.simulate_keystrokes("ctrl-alt-right ctrl-alt-down");
        assert_eq!(active_pane(&w, cx), below);
        cx.simulate_keystrokes("ctrl-alt-down");
        assert_eq!(active_pane(&w, cx), below, "nothing below");

        // A split pane whose last tab closes goes away.
        cx.simulate_keystrokes("ctrl-w");
        assert_eq!(w.read_with(cx, |w, _| w.panes()), [first.clone(), right.clone()]);
        assert!(w.read_with(cx, |w, cx| {
            let active = w.active_pane().clone();
            cx.windows().len() == 1 && w.panes().contains(&active)
        }));
    }

    #[gpui::test]
    async fn shift_escape_zooms_the_active_pane(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        let zoomed = |cx: &mut VisualTestContext| w.read_with(cx, |w, _| w.zoomed_pane());
        // From the Fleet panel the zoom goes to the active pane.
        cx.simulate_keystrokes("shift-escape");
        let pane = active_pane(&w, cx);
        assert_eq!(zoomed(cx), Some(pane.clone()));
        assert!(pane.read_with(cx, |p, _| p.is_zoomed()));
        assert!(focused_in(&pane, cx));
        cx.simulate_keystrokes("shift-escape");
        assert_eq!(zoomed(cx), None);
        assert!(!pane.read_with(cx, |p, _| p.is_zoomed()));

        // Toggling a dock steps out of the zoom, so the dock shows.
        cx.simulate_keystrokes("shift-escape ctrl-j");
        assert_eq!(zoomed(cx), None);
        // So does splitting.
        cx.simulate_keystrokes("ctrl-j shift-escape ctrl-\\");
        assert_eq!(zoomed(cx), None);
        assert_eq!(w.read_with(cx, |w, _| w.panes().len()), 2);
    }

    #[gpui::test]
    async fn docks_resize_within_the_window(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        let size = |position, cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| w.dock(position).read(cx).active_size(cx).unwrap())
        };
        assert_eq!(
            size(DockPosition::Left, cx),
            px(260.),
            "the Fleet panel's setting"
        );
        assert_eq!(size(DockPosition::Bottom, cx), px(160.));

        w.update(cx, |w, cx| w.resize_dock(DockPosition::Left, px(300.), cx));
        assert_eq!(size(DockPosition::Left, cx), px(300.));
        w.update(cx, |w, cx| w.resize_dock(DockPosition::Right, px(10.), cx));
        assert_eq!(size(DockPosition::Right, cx), MIN_DOCK_SIZE);
        w.update(cx, |w, cx| w.resize_dock(DockPosition::Bottom, px(100_000.), cx));
        let height = w.read_with(cx, |w, _| w.bounds.size.height);
        assert!(
            size(DockPosition::Bottom, cx) <= height - MIN_CENTER_SIZE,
            "{height:?}"
        );

        // Dragging the left dock's edge.
        cx.run_until_parked();
        let handle = cx
            .debug_bounds("Left-dock-resize-handle")
            .expect("the handle is drawn");
        let from = handle.center();
        let to = point(from.x + px(40.), from.y);
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            point(from.x + px(10.), from.y),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        let dragged = size(DockPosition::Left, cx);
        assert!((dragged - px(340.)).abs() <= px(4.), "{dragged:?}");
        // The size stays with the panel when the dock closes and opens.
        cx.simulate_keystrokes("ctrl-b ctrl-b");
        assert_eq!(size(DockPosition::Left, cx), dragged);
        // Moving the Fleet to the right takes its size along.
        cx.update(|_, cx| settings::reload(r#"{ "fleet_panel": { "dock": "right" } }"#, cx));
        cx.run_until_parked();
        assert_eq!(size(DockPosition::Right, cx), dragged);
        assert_eq!(size(DockPosition::Left, cx), MIN_DOCK_SIZE);
    }

    #[gpui::test]
    async fn the_switcher_opens_as_a_modal_and_closes(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        let open =
            |cx: &mut VisualTestContext| w.read_with(cx, |w, cx| w.modal_layer().read(cx).has_active_modal());
        let fleet_focus = w.read_with(cx, |w, cx| {
            w.dock(DockPosition::Left)
                .read(cx)
                .active_panel()
                .unwrap()
                .focus_handle(cx)
        });

        // Escape closes it, and the keyboard goes back.
        cx.simulate_keystrokes("ctrl-p");
        assert!(open(cx));
        cx.simulate_keystrokes("escape");
        assert!(!open(cx));
        assert!(cx.update(|window, _| fleet_focus.is_focused(window)));

        // Toggling it again closes it (its key moves the selection up
        // inside it, as in Zed's pickers on Linux).
        cx.simulate_keystrokes("ctrl-p");
        cx.dispatch_action(ToggleSwitcher);
        assert!(!open(cx));

        // A click outside closes it.
        cx.simulate_keystrokes("ctrl-p");
        cx.simulate_click(point(px(4.), px(600.)), Modifiers::none());
        assert!(!open(cx));

        // Picking an agent selects it in the Fleet.
        let fleet = w.read_with(cx, |w, _| w.fleet().clone());
        let (target, name) = fleet.read_with(cx, |f, _| {
            let a = f
                .store
                .agents
                .values()
                .find(|a| Some(&a.id) != f.store.selected.as_ref())
                .unwrap();
            (a.id.clone(), a.name.clone())
        });
        cx.simulate_keystrokes("ctrl-p");
        cx.simulate_input(&name);
        cx.simulate_keystrokes("enter");
        assert!(!open(cx));
        fleet.read_with(cx, |f, _| assert_eq!(f.store.selected.as_ref(), Some(&target)));
    }

    #[gpui::test]
    async fn notices_show_as_toasts(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        let fleet = w.read_with(cx, |w, _| w.fleet().clone());
        fleet.update(cx, |f, cx| f.show("opened settings.json".into(), false, cx));
        let texts = |cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| {
                w.toast_layer()
                    .read(cx)
                    .texts()
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(texts(cx), ["opened settings.json"]);
        cx.executor()
            .advance_clock(crate::toast_layer::FLASH_FOR + std::time::Duration::from_millis(1));
        cx.run_until_parked();
        assert!(texts(cx).is_empty());
    }
}
