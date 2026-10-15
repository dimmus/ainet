//! The window's root view, laid out like Zed's workspace: title bar, the
//! left, right and bottom [`Dock`]s around the center's panes (tabs and
//! splits), the status bar, and over them the modal and toast layers,
//! inside client-side decorations. The Fleet panel docks left (or right,
//! by setting) with the Agent panel opposite; the Queue, Chatroom and
//! Events panels share the bottom dock. Replies that are long text (a
//! transcript, a spec) open as report tabs.
//!
//! It handles what spans panes and docks: toggling and resizing docks,
//! splitting, moving between and zooming panes, opening tabs, modals, and
//! the pane actions when the keyboard is outside the panes. It is saved
//! as it is and restored at the next start (`persist`).

mod persist;

use ainet_model::{AgentCommand, command, terminal};
use gpui::{
    Bounds, DragMoveEvent, Entity, FocusHandle, Focusable, Pixels, PromptLevel, Subscription, WeakEntity,
    canvas,
};
use ui::prelude::*;

use crate::actions::panel::{agent_panel, chatroom_panel, events_panel, fleet_panel, queue_panel};
use crate::actions::{
    ActivateNextItem, ActivateNextPane, ActivatePaneDown, ActivatePaneLeft, ActivatePaneRight,
    ActivatePaneUp, ActivatePreviousItem, ActivatePreviousPane, Allow, Always, Capture, CloseActiveItem,
    CloseAgent, CloseAllItems, CloseOtherItems, CloseWindow, Compose, Continue, Deny, MessageAgent,
    NextWaiting, OpenComms, OpenInbox, OpenKeymap, OpenMap, OpenSettings, OpenTerminal, Operator, Relaunch,
    ReleaseSeat, RemoveSeat, SelectNext, SelectPrev, ShowTranscript, SplitDown, SplitRight, TileAll,
    ToggleBottomDock, ToggleCritterSize, ToggleLeftDock, ToggleRightDock, ToggleSwitcher, ToggleTypingGuard,
    ToggleZoom, terminal as terminal_actions,
};
use crate::actions::{
    Down, Grow, Restore, Spawn, SwitchRig, Up, command_palette, tab_switcher, theme_selector,
};
use crate::agent_panel::AgentPanel;
use crate::args::View;
use crate::chatroom::ChatroomPanel;
use crate::command_palette::CommandPalette;
use crate::comms::CommsItem;
use crate::decorations::client_side_decorations;
use crate::dock::{Dock, DockPosition, DraggedDock, Panel, RESIZE_HANDLE_SIZE};
use crate::events_panel::EventsPanel;
use crate::fleet::{FleetModel, ReportWanted};
use crate::fleet_panel::FleetPanel;
use crate::inbox::InboxItem;
use crate::item::{ItemHandle, Placeholder};
use crate::map::MapItem;
use crate::message::MessageModal;
use crate::modal_layer::{ModalLayer, ModalView};
use crate::pane::{Pane, PaneEvent};
use crate::pane_group::{PaneGroup, SplitDirection};
use crate::picker::Picker;
use crate::queue_panel::QueuePanel;
use crate::report::ReportItem;
use crate::rig_modals::{RigAction, RigPicker};
use crate::seat_modals::SpawnModal;
use crate::settings::{self, DockSide};
use crate::status_bar::StatusBar;
use crate::switcher::AgentSwitcher;
use crate::tab_switcher::TabSwitcher;
use crate::terminal_element::Metrics as TerminalMetrics;
use crate::terminal_item::TerminalItem;
use crate::theme_selector::ThemeSelector;
use crate::title_bar::TitleBar;
use crate::toast_layer::ToastLayer;
use crate::up_modal::SpecPicker;
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
    /// Where it is saved, once `persist` is called.
    persist: Option<persist::Persist>,
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
        let this = cx.entity().downgrade();
        let title_bar = cx.new(|cx| TitleBar::new(fleet.clone(), this.clone(), cx));
        let modal_layer = cx.new(|_| ModalLayer::new());
        let toast_layer = cx.new(|cx| ToastLayer::new(&fleet, cx).with_workspace(this.clone()));

        let fleet_panel = cx.new(|cx| FleetPanel::new(fleet.clone(), cx));
        let agent_panel = cx.new(|cx| AgentPanel::new(fleet.clone(), cx));
        let queue_panel = cx.new(|cx| QueuePanel::new(fleet.clone(), this.clone(), cx));
        let chatroom_panel = cx.new(|cx| ChatroomPanel::new(fleet.clone(), window, cx));
        let events_panel = cx.new(|cx| EventsPanel::new(fleet.clone(), cx));
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
        bottom_dock.update(cx, |d, cx| {
            d.add_panel(queue_panel, cx);
            d.add_panel(chatroom_panel, cx);
            d.add_panel(events_panel, cx);
        });
        for dock in [&left_dock, &right_dock, &bottom_dock] {
            dock.update(cx, |d, cx| d.set_open(true, window, cx));
        }
        let docks = [left_dock.clone(), bottom_dock.clone(), right_dock.clone()];
        let status_bar = cx.new(|cx| StatusBar::new(fleet.clone(), this, docks, cx));

        let first = cx.new(|cx| Pane::new(window, cx));
        let mut subscriptions = vec![
            cx.subscribe_in(&first, window, Self::handle_pane_event),
            cx.observe_global_in::<settings::AppSettings>(window, |this, window, cx| {
                this.follow_settings(window, cx)
            }),
            // Long text to read opens as a tab.
            cx.subscribe_in(&fleet, window, |this, _, wanted: &ReportWanted, window, cx| {
                this.open_report(wanted.request.clone(), wanted.title.clone(), window, cx);
            }),
        ];
        let first_item: Box<dyn ItemHandle> = match view.unwrap_or(View::Fleet) {
            View::Inbox => Box::new(cx.new(|cx| InboxItem::new(fleet.clone(), window, cx))),
            View::Comms => Box::new(cx.new(|cx| CommsItem::new(fleet.clone(), window, cx))),
            View::Map => Box::new(cx.new(|cx| MapItem::new(fleet.clone(), cx))),
            view => Box::new(cx.new(|cx| Placeholder::new(view, cx))),
        };
        first.update(cx, |pane, cx| pane.add_item(first_item, true, false, window, cx));
        subscriptions.push(cx.observe(&first, |_, _, cx| cx.notify()));

        // The Fleet has the keyboard until a terminal or tab takes it.
        window.focus(&fleet_panel.focus_handle(cx), cx);
        // The window manager's close goes the way of `CloseWindow`.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = this.update(cx, |this, cx| this.save_now(cx));
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
            persist: None,
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
            bottom: self.bottom_dock.read(cx).is_open(),
        }
    }

    /// The bottom dock's panel of type `T`, if it has one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn bottom_panel<T: Panel>(&self, cx: &App) -> Option<Entity<T>> {
        self.bottom_dock
            .read(cx)
            .panels()
            .find_map(|p| p.to_any().downcast::<T>().ok())
    }

    /// A panel's `ToggleFocus`, as Zed's: shows the panel named `name` (its
    /// dock opens, it becomes the one showing) and gives it the keyboard;
    /// if it has the keyboard already, gives it back to the center.
    pub fn toggle_panel_focus(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let docks = [
            self.left_dock.clone(),
            self.right_dock.clone(),
            self.bottom_dock.clone(),
        ];
        for dock in docks {
            let Some(ix) = dock.read(cx).panel_index(name) else {
                continue;
            };
            let showing = dock.read(cx).is_open() && dock.read(cx).active_index() == ix;
            let focus = dock.read(cx).panels().nth(ix).map(|p| p.focus_handle(cx));
            if showing && focus.as_ref().is_some_and(|f| f.contains_focused(window, cx)) {
                let pane = self.active_pane.focus_handle(cx);
                window.focus(&pane, cx);
            } else {
                self.zoom_out(cx);
                dock.update(cx, |d, cx| {
                    d.activate_panel(ix, window, cx);
                    d.set_open(true, window, cx);
                });
                if let Some(focus) = focus {
                    window.focus(&focus, cx);
                }
            }
            cx.notify();
            return;
        }
    }

    /// Opens a report of `request`'s reply, titled `title`, as a tab of
    /// the active pane, and asks for it; a report of the same title open
    /// anywhere is asked again and activated instead.
    pub fn open_report(
        &mut self,
        request: ainet_core::Request,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ReportItem> {
        for pane in self.panes() {
            let found = pane.read(cx).items().position(|item| {
                item.to_any()
                    .downcast::<ReportItem>()
                    .is_ok_and(|r| r.read(cx).title().as_ref() == title)
            });
            if let Some(ix) = found {
                self.active_pane = pane.clone();
                pane.update(cx, |p, cx| p.activate_item(ix, true, window, cx));
                let report = pane
                    .read(cx)
                    .active_item()
                    .and_then(|i| i.to_any().downcast::<ReportItem>().ok())
                    .expect("just activated");
                report.update(cx, |r, cx| r.refresh(cx));
                return report;
            }
        }
        let fleet = self.fleet.clone();
        let report = cx.new(|cx| ReportItem::new(fleet, request, title, window, cx));
        self.active_pane.update(cx, |p, cx| {
            p.add_item(Box::new(report.clone()), true, true, window, cx)
        });
        report.update(cx, |r, cx| r.refresh(cx));
        cx.notify();
        report
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

    /// Opens the Map and gives it the keyboard: activates its tab where
    /// one is open (in any pane), else adds one to the active pane.
    pub fn open_map(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<MapItem> {
        for pane in self.panes() {
            let at = pane
                .read(cx)
                .items()
                .position(|i| i.to_any().downcast::<MapItem>().is_ok());
            if let Some(ix) = at {
                self.active_pane = pane.clone();
                pane.update(cx, |p, cx| p.activate_item(ix, true, window, cx));
                return pane
                    .read(cx)
                    .active_item()
                    .and_then(|i| i.to_any().downcast::<MapItem>().ok())
                    .expect("just activated");
            }
        }
        let map = cx.new(|cx| MapItem::new(self.fleet.clone(), cx));
        self.active_pane.update(cx, |p, cx| {
            p.add_item(Box::new(map.clone()), true, true, window, cx)
        });
        cx.notify();
        map
    }

    /// Opens the Inbox and gives it the keyboard: activates its tab where
    /// one is open (in any pane), else adds one to the active pane. With
    /// `item`, that card is selected.
    pub fn open_inbox(
        &mut self,
        item: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InboxItem> {
        let mut found = None;
        for pane in self.panes() {
            let at = pane
                .read(cx)
                .items()
                .position(|i| i.to_any().downcast::<InboxItem>().is_ok());
            if let Some(ix) = at {
                pane.update(cx, |p, cx| p.activate_item(ix, true, window, cx));
                let inbox = pane
                    .read(cx)
                    .active_item()
                    .and_then(|i| i.to_any().downcast::<InboxItem>().ok());
                self.active_pane = pane.clone();
                found = inbox;
                break;
            }
        }
        let inbox = match found {
            Some(inbox) => inbox,
            None => {
                let inbox = cx.new(|cx| InboxItem::new(self.fleet.clone(), window, cx));
                self.active_pane.update(cx, |p, cx| {
                    p.add_item(Box::new(inbox.clone()), true, true, window, cx)
                });
                inbox
            }
        };
        if let Some(item) = item {
            inbox.update(cx, |inbox, cx| inbox.select(item, cx));
        }
        let focus = inbox.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
        inbox
    }

    /// Opens Comms with its compose box at the keyboard: activates its
    /// tab where one is open (in any pane), else adds one to the active
    /// pane.
    pub fn open_comms(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<CommsItem> {
        let mut found = None;
        for pane in self.panes() {
            let at = pane
                .read(cx)
                .items()
                .position(|i| i.to_any().downcast::<CommsItem>().is_ok());
            if let Some(ix) = at {
                pane.update(cx, |p, cx| p.activate_item(ix, false, window, cx));
                found = pane
                    .read(cx)
                    .active_item()
                    .and_then(|i| i.to_any().downcast::<CommsItem>().ok());
                self.active_pane = pane.clone();
                break;
            }
        }
        let comms = match found {
            Some(comms) => comms,
            None => {
                let comms = cx.new(|cx| CommsItem::new(self.fleet.clone(), window, cx));
                self.active_pane.update(cx, |p, cx| {
                    p.add_item(Box::new(comms.clone()), true, false, window, cx)
                });
                comms
            }
        };
        comms.update(cx, |comms, cx| comms.focus_compose(window, cx));
        cx.notify();
        comms
    }

    /// `ctrl-y`, `ctrl-a` or `ctrl-n` outside the Inbox: as the TUI's, it
    /// opens the Inbox on the oldest request waiting (the selected
    /// agent's first), so you read before you answer; the Inbox's own
    /// keys then answer it.
    fn answer_from_outside(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        match store.next_waiting().map(|ix| store.inbox[ix].id) {
            Some(id) => {
                self.open_inbox(Some(id), window, cx);
            }
            None => self.fleet.update(cx, |fleet, cx| {
                fleet.show("nothing is waiting on you".into(), false, cx)
            }),
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
                // The tab itself takes the keyboard: focusing the pane
                // would leave it on the pane when it had it already.
                self.active_pane = pane.clone();
                pane.update(cx, |p, cx| p.activate_item(ix, true, window, cx));
                return;
            }
        }
        let (fleet, id) = (self.fleet.clone(), id.to_string());
        let item = cx.new(|cx| TerminalItem::new(fleet, id, window, cx));
        self.active_pane
            .update(cx, |p, cx| p.add_item(Box::new(item), true, true, window, cx));
    }

    /// Lays out every live terminal of the selected agent's rig (all of
    /// them if it has none) as a grid of panes, as the TUI's Terminals
    /// view: the first in the active pane, the rest split off it in rows
    /// of terminal-shaped tiles. Panes left from tiling before, holding
    /// only these terminals, make way.
    pub fn tile_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        let rig = store.selected_agent().and_then(|a| a.rig.clone());
        self.tile_rig(rig.as_deref(), window, cx);
    }

    /// Lays out every live terminal of `rig` (all of them for `None`) as
    /// [`Self::tile_all`] does: `/view rig:<name>`.
    pub fn tile_rig(&mut self, rig: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_out(cx);
        let rig = rig.map(str::to_string);
        let store = &self.fleet.read(cx).store;
        let ids = store.terminal_ids(rig.as_deref());
        let Some(first) = ids.first().cloned() else {
            let text = match rig {
                Some(rig) => format!("no live terminals in rig {rig}"),
                None => "no live terminals".to_string(),
            };
            self.fleet.update(cx, |fleet, cx| fleet.show(text, false, cx));
            return;
        };
        let terminal_of = |item: &dyn ItemHandle, cx: &App| {
            item.to_any()
                .downcast::<TerminalItem>()
                .ok()
                .map(|t| t.read(cx).agent_id().to_string())
        };
        for pane in self.panes() {
            let only_these = pane
                .read(cx)
                .items()
                .all(|item| terminal_of(item, cx).is_some_and(|agent| ids.contains(&agent)));
            if pane != self.active_pane && only_these {
                pane.update(cx, |p, cx| p.close_all_items(window, cx));
                self.center.remove(&pane);
            }
        }

        // Shaped by the room the center has, in terminal cells.
        let (width, height) = match self.center.bounding_box_for_pane(&self.active_pane) {
            Some(b) => (b.size.width, b.size.height),
            None => (self.bounds.size.width, self.bounds.size.height),
        };
        let cell = TerminalMetrics::new(window, cx).cell;
        let cols = terminal::grid_cols(
            ids.len(),
            f64::from(width / cell.width),
            f64::from(height / cell.height),
            None,
        );
        let rows: Vec<&[String]> = ids.chunks(cols).collect();

        let anchor = self.active_pane.clone();
        let existing = anchor
            .read(cx)
            .items()
            .position(|item| terminal_of(item, cx).as_deref() == Some(&first));
        match existing {
            Some(ix) => anchor.update(cx, |p, cx| p.activate_item(ix, false, window, cx)),
            None => {
                let item = self.new_terminal(&first, window, cx);
                anchor.update(cx, |p, cx| p.add_item(item, true, false, window, cx));
            }
        }
        // The rows first, one below the other, then each row's tiles.
        let mut row_starts = vec![anchor.clone()];
        for row in &rows[1..] {
            let above = row_starts.last().cloned().unwrap_or_else(|| anchor.clone());
            let item = self.new_terminal(&row[0], window, cx);
            row_starts.push(self.split_with(&above, SplitDirection::Down, item, window, cx));
        }
        for (row, start) in rows.iter().zip(row_starts) {
            let mut last = start;
            for id in &row[1..] {
                let item = self.new_terminal(id, window, cx);
                last = self.split_with(&last, SplitDirection::Right, item, window, cx);
            }
        }
        self.active_pane = anchor.clone();
        anchor.update(cx, |p, cx| p.activate_item(p.active_index(), true, window, cx));
        cx.notify();
    }

    fn new_terminal(&self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Box<dyn ItemHandle> {
        let (fleet, id) = (self.fleet.clone(), id.to_string());
        Box::new(cx.new(|cx| TerminalItem::new(fleet, id, window, cx)))
    }

    /// Splits `pane` with a new pane holding `item`.
    fn split_with(
        &mut self,
        pane: &Entity<Pane>,
        direction: SplitDirection,
        item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let new = self.add_pane(window, cx);
        new.update(cx, |p, cx| p.add_item(item, true, false, window, cx));
        self.center.split(pane, &new, direction);
        new
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

    /// `ctrl-o`, as the TUI's: the operator's terminal, to type into; or,
    /// with no operator running, starts one.
    fn operator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let operator = self.fleet.read(cx).store.operator().map(|a| a.id.clone());
        match operator {
            Some(id) => self.open_terminal(&id, window, cx),
            None => self
                .fleet
                .update(cx, |fleet, cx| fleet.run(command::operator_up(), cx)),
        }
    }

    /// Selects the next agent with a request waiting on you.
    fn next_waiting(&mut self, cx: &mut Context<Self>) {
        self.fleet.update(cx, |fleet, cx| {
            match fleet.store.next_waiting_agent().map(str::to_string) {
                Some(id) => fleet.select(&id, cx),
                None => fleet.show("nothing is waiting on you".into(), false, cx),
            }
        });
    }

    /// Takes the selected agent's seat out of its rig once you say so.
    fn remove_seat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.fleet.read(cx).store.selected_agent().cloned() else {
            return;
        };
        let Some(seat) = agent.seat.clone() else {
            return self.agent_command(AgentCommand::Remove, cx);
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Remove {seat} from its rig?"),
            Some("Its agent stops and the seat is gone from the rig."),
            &["Remove", "Cancel"],
            cx,
        );
        let fleet = self.fleet.clone();
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(0) {
                fleet.update(cx, |fleet, cx| {
                    let remove = command::for_agent(&fleet.store, &agent.id, AgentCommand::Remove);
                    fleet.run(remove, cx);
                });
            }
        })
        .detach();
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
        let (fleet, this) = (self.fleet.clone(), cx.entity().downgrade());
        self.toggle_modal(window, cx, move |window, cx| {
            let switcher = AgentSwitcher::new(fleet, this, cx);
            Picker::uniform_list(switcher, window, cx)
        });
    }

    /// The command palette, over the actions where the keyboard is.
    fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fleet = self.fleet.clone();
        let focus = window.focused(cx).unwrap_or_else(|| self.focus.clone());
        self.toggle_modal(window, cx, move |window, cx| {
            let palette = CommandPalette::new(fleet, focus, window, cx);
            Picker::uniform_list(palette, window, cx).width(rems(38.))
        });
    }

    /// The theme selector, previewing as it moves.
    fn toggle_theme_selector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fleet = self.fleet.clone();
        self.toggle_modal(window, cx, move |window, cx| {
            let selector = ThemeSelector::new(fleet, cx);
            Picker::uniform_list(selector, window, cx)
        });
    }

    /// The tab switcher over the active pane's tabs; its key again, while
    /// it is open, moves down it.
    fn toggle_tab_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(switcher) = self.modal_layer.read(cx).active_modal::<TabSwitcher>() {
            switcher.update(cx, |switcher, cx| switcher.cycle(1, window, cx));
            return;
        }
        let pane = self.active_pane.clone();
        self.toggle_modal(window, cx, move |window, cx| TabSwitcher::new(&pane, window, cx));
    }

    /// `fleet::Up`: the spec picker, then the form.
    fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (fleet, this) = (self.fleet.clone(), cx.entity().downgrade());
        self.toggle_modal(window, cx, move |window, cx| {
            SpecPicker::open(fleet, this, window, cx)
        });
    }

    /// `fleet::Spawn`: the spawn form.
    fn spawn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fleet = self.fleet.clone();
        self.toggle_modal(window, cx, move |window, cx| SpawnModal::new(fleet, window, cx));
    }

    /// Restoring, taking down or growing a rig: the selected agent's rig
    /// (or the only one) for taking down and growing, as the TUI's
    /// commands; else, and always to restore or switch rig, a rig picker
    /// first.
    pub fn rig_action(&mut self, then: RigAction, window: &mut Window, cx: &mut Context<Self>) {
        let rig = match then {
            RigAction::Restore | RigAction::Select => None,
            RigAction::Down | RigAction::Grow => self.fleet.read(cx).store.rig_for(None, None),
        };
        let fleet = self.fleet.clone();
        match rig {
            Some(rig) => then.open(fleet, rig, self, window, cx),
            None => {
                let this = cx.entity().downgrade();
                self.toggle_modal(window, cx, move |window, cx| {
                    let picker = RigPicker::new(fleet, this, then, cx);
                    Picker::uniform_list(picker, window, cx)
                });
            }
        }
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
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.save_now(cx);
                quit::close_window(window, cx)
            }))
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
            .on_action(cx.listener(|this, _: &fleet_panel::ToggleFocus, window, cx| {
                this.toggle_panel_focus(FleetPanel::persistent_name(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &agent_panel::ToggleFocus, window, cx| {
                this.toggle_panel_focus(AgentPanel::persistent_name(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &queue_panel::ToggleFocus, window, cx| {
                this.toggle_panel_focus(QueuePanel::persistent_name(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &chatroom_panel::ToggleFocus, window, cx| {
                this.toggle_panel_focus(ChatroomPanel::persistent_name(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &events_panel::ToggleFocus, window, cx| {
                this.toggle_panel_focus(EventsPanel::persistent_name(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.select(-1, cx)))
            .on_action(|_: &ToggleCritterSize, _, cx| {
                let mut next = settings::get(cx).clone();
                next.critters.size = next.critters.size.toggled();
                settings::set(next, cx);
            })
            .on_action(cx.listener(|this, _: &ToggleSwitcher, window, cx| this.toggle_switcher(window, cx)))
            .on_action(cx.listener(|this, _: &command_palette::Toggle, window, cx| {
                this.toggle_command_palette(window, cx)
            }))
            .on_action(cx.listener(|this, _: &theme_selector::Toggle, window, cx| {
                this.toggle_theme_selector(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &tab_switcher::Toggle, window, cx| {
                    this.toggle_tab_switcher(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Up, window, cx| this.up(window, cx)))
            .on_action(cx.listener(|this, _: &Spawn, window, cx| this.spawn(window, cx)))
            .on_action(
                cx.listener(|this, _: &Restore, window, cx| this.rig_action(RigAction::Restore, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Down, window, cx| this.rig_action(RigAction::Down, window, cx)))
            .on_action(cx.listener(|this, _: &Grow, window, cx| this.rig_action(RigAction::Grow, window, cx)))
            .on_action(
                cx.listener(|this, _: &SwitchRig, window, cx| this.rig_action(RigAction::Select, window, cx)),
            )
            // The selected agent's actions, from the Fleet's keys and menu.
            .on_action(cx.listener(|this, _: &TileAll, window, cx| this.tile_all(window, cx)))
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
                cx.listener(|this, _: &ReleaseSeat, _, cx| this.agent_command(AgentCommand::Release, cx)),
            )
            .on_action(cx.listener(|this, _: &RemoveSeat, window, cx| this.remove_seat(window, cx)))
            .on_action(cx.listener(|this, _: &Operator, window, cx| this.operator(window, cx)))
            .on_action(cx.listener(|this, _: &NextWaiting, _, cx| this.next_waiting(cx)))
            .on_action(cx.listener(|this, _: &OpenComms, window, cx| {
                this.open_comms(window, cx);
            }))
            // Outside Comms, a new message opens it to write one.
            .on_action(cx.listener(|this, _: &Compose, window, cx| {
                let comms = this.open_comms(window, cx);
                comms.update(cx, |comms, cx| comms.new_message(window, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenInbox, window, cx| {
                this.open_inbox(None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Allow, window, cx| this.answer_from_outside(window, cx)))
            .on_action(cx.listener(|this, _: &Always, window, cx| this.answer_from_outside(window, cx)))
            .on_action(cx.listener(|this, _: &Deny, window, cx| this.answer_from_outside(window, cx)))
            .on_action(cx.listener(|this, _: &OpenMap, window, cx| {
                this.open_map(window, cx);
            }))
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
            .on_action(cx.listener(|this, _: &terminal_actions::ToggleZoom, window, cx| {
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

        cx.simulate_keystrokes("ctrl-pagedown ctrl-pagedown ctrl-pagedown");
        assert_eq!(tabs(&pane, cx).1, 0, "wraps round");
        cx.simulate_keystrokes("ctrl-shift-tab ctrl-pageup");
        assert_eq!(tabs(&pane, cx).1, 2);
        let fleet_tab = pane.read_with(cx, |p, _| p.items().next().unwrap().to_any());
        let activations = || fleet_tab.clone().downcast::<Placeholder>().unwrap();
        let before = activations().read_with(cx, |p, _| p.activations);

        // Closing the active tab goes back to the one used before it.
        cx.simulate_keystrokes("ctrl-pagedown ctrl-pagedown");
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
        cx.simulate_keystrokes("ctrl-t");
        let item = pane.read_with(cx, |p, _| p.active_item().unwrap().to_any());
        item.downcast::<MapItem>().unwrap().update(cx, |_, cx| {
            cx.emit(crate::item::ItemEvent::UpdateTab);
            cx.emit(crate::item::ItemEvent::CloseItem);
        });
        assert_eq!(tabs(&pane, cx).0.len(), 0);
    }

    /// Right-clicks tab `tab` (its number in its pane) and clicks the
    /// menu's `entry`.
    fn pick_from_tab_menu(tab: usize, entry: &str, cx: &mut VisualTestContext) {
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds(format!("TAB-{tab}").leak())
            .unwrap_or_else(|| panic!("tab {tab} is drawn"));
        cx.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::none());
        cx.run_until_parked();
        let entry = cx
            .debug_bounds(format!("MENU_ITEM-{entry}").leak())
            .unwrap_or_else(|| panic!("{entry} is in the menu"));
        cx.simulate_click(entry.center(), Modifiers::none());
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn a_tabs_menu_acts_on_that_tab(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        cx.update(|window, _| window.activate_window());
        let first = active_pane(&w, cx);
        cx.simulate_keystrokes("ctrl-shift-c ctrl-shift-i ctrl-t");
        assert_eq!(
            tabs(&first, cx),
            (
                vec!["Fleet".into(), "Comms".into(), "Inbox".into(), "Map".into()],
                3
            )
        );
        // Tabs are numbered per pane, so a pane made by splitting is
        // closed again before tabs are picked by number.
        let close = |pane: Entity<Pane>, cx: &mut VisualTestContext| {
            cx.update(|window, cx| pane.update(cx, |p, cx| p.close_all_items(window, cx)));
            assert_eq!(w.read_with(cx, |w, _| w.panes()), std::slice::from_ref(&first));
        };

        // Split Right and Split Down from the Fleet tab, which is not the
        // active one: the copy is of it.
        pick_from_tab_menu(0, "Split Right", cx);
        let right = active_pane(&w, cx);
        assert_ne!(right, first);
        assert_eq!(tabs(&right, cx).0, ["Fleet"]);
        assert_eq!(tabs(&first, cx).1, 0, "the Fleet tab was activated to split");
        close(right, cx);
        pick_from_tab_menu(0, "Split Down", cx);
        let below = active_pane(&w, cx);
        assert_eq!(tabs(&below, cx).0, ["Fleet"]);
        assert_eq!(w.read_with(cx, |w, _| w.panes()), [first.clone(), below.clone()]);
        close(below, cx);

        // Close Others from Inbox.
        pick_from_tab_menu(2, "Close Others", cx);
        assert_eq!(tabs(&first, cx).0, ["Inbox"]);

        // Close from a tab that is not the active one.
        cx.simulate_keystrokes("ctrl-t");
        assert_eq!(tabs(&first, cx), (vec!["Inbox".into(), "Map".into()], 1));
        pick_from_tab_menu(0, "Close", cx);
        assert_eq!(tabs(&first, cx), (vec!["Map".into()], 0));

        // Close All; the last pane stays, empty.
        cx.simulate_keystrokes("ctrl-shift-c");
        pick_from_tab_menu(1, "Close All", cx);
        assert!(tabs(&first, cx).0.is_empty());
        assert_eq!(w.read_with(cx, |w, _| w.panes()), [first]);
    }

    #[gpui::test]
    async fn the_status_bars_buttons_show_and_close_their_panels(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        cx.update(|window, _| window.activate_window());
        let click = |name: &'static str, cx: &mut VisualTestContext| {
            cx.run_until_parked();
            let bounds = cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is drawn"));
            cx.simulate_click(bounds.center(), Modifiers::none());
            cx.run_until_parked();
        };
        let bottom = |cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| {
                let dock = w.dock(DockPosition::Bottom).read(cx);
                (dock.is_open(), dock.active_panel_name())
            })
        };
        assert_eq!(bottom(cx), (true, Some("QueuePanel")));

        // Another panel's button shows that panel, with the keyboard.
        click("DOCK-BUTTON-EventsPanel", cx);
        assert_eq!(bottom(cx), (true, Some("EventsPanel")));
        let events = w.read_with(cx, |w, cx| w.bottom_panel::<EventsPanel>(cx).unwrap());
        assert!(cx.update(|window, cx| events.focus_handle(cx).contains_focused(window, cx)));
        // The lit one closes its dock; then it opens it on its panel.
        click("DOCK-BUTTON-EventsPanel", cx);
        assert_eq!(bottom(cx), (false, Some("EventsPanel")));
        click("DOCK-BUTTON-EventsPanel", cx);
        assert_eq!(bottom(cx), (true, Some("EventsPanel")));
        click("DOCK-BUTTON-QueuePanel", cx);
        assert_eq!(bottom(cx), (true, Some("QueuePanel")));

        // The side docks' buttons, at either end of the bar.
        let docks = |cx: &mut VisualTestContext| w.read_with(cx, |w, cx| w.docks(cx));
        click("DOCK-BUTTON-FleetPanel", cx);
        assert!(!docks(cx).fleet);
        click("DOCK-BUTTON-FleetPanel", cx);
        assert!(docks(cx).fleet);
        let fleet = w.read_with(cx, |w, cx| w.fleet_panel().focus_handle(cx));
        assert!(cx.update(|window, _| fleet.is_focused(window)));
        click("DOCK-BUTTON-AgentPanel", cx);
        assert!(!docks(cx).agent);
        click("DOCK-BUTTON-AgentPanel", cx);
        let after = docks(cx);
        assert!(after.agent && after.fleet && after.bottom, "{after:?}");
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
        // The secondary pick only selects it.
        cx.simulate_keystrokes("ctrl-p");
        cx.simulate_input(&name);
        cx.simulate_keystrokes("ctrl-enter");
        assert!(!open(cx));
        fleet.read_with(cx, |f, _| assert_eq!(f.store.selected.as_ref(), Some(&target)));
        let terminals = |cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| {
                w.panes()
                    .iter()
                    .flat_map(|p| {
                        p.read(cx)
                            .items()
                            .filter_map(|i| i.to_any().downcast::<TerminalItem>().ok())
                            .map(|t| t.read(cx).agent_id().to_string())
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
        };
        assert!(terminals(cx).is_empty());
        assert!(
            cx.update(|window, _| fleet_focus.is_focused(window)),
            "the keyboard is back"
        );

        // Picking one opens its terminal, which takes the keyboard.
        let (other, other_name) = fleet.read_with(cx, |f, _| {
            let a = f
                .store
                .agents
                .values()
                .find(|a| a.id != target && a.state.is_live())
                .unwrap();
            (a.id.clone(), a.name.clone())
        });
        cx.simulate_keystrokes("ctrl-p");
        cx.simulate_input(&other_name);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!open(cx));
        assert_eq!(terminals(cx), std::slice::from_ref(&other));
        let pane = active_pane(&w, cx);
        let terminal = pane.read_with(cx, |p, _| p.active_item().unwrap().to_any());
        let terminal = terminal.downcast::<TerminalItem>().unwrap();
        assert!(cx.update(|window, cx| Focusable::focus_handle(&terminal, cx).contains_focused(window, cx)));
        fleet.read_with(cx, |f, _| assert_eq!(f.store.selected.as_ref(), Some(&other)));
    }

    #[gpui::test]
    async fn the_bottom_dock_holds_three_panels_shown_by_their_actions(cx: &mut TestAppContext) {
        let (w, cx) = workspace(cx);
        let bottom = |cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| {
                let dock = w.dock(DockPosition::Bottom).read(cx);
                (dock.is_open(), dock.active_panel_name())
            })
        };
        assert_eq!(bottom(cx), (true, Some("QueuePanel")));
        let names: Vec<&str> = w.read_with(cx, |w, cx| {
            w.dock(DockPosition::Bottom)
                .read(cx)
                .panels()
                .map(|p| p.persistent_name())
                .collect()
        });
        assert_eq!(names, ["QueuePanel", "ChatroomPanel", "EventsPanel"]);

        // Its action shows a panel and gives it the keyboard; again, the
        // keyboard goes back to the center and the panel stays.
        let events = w.read_with(cx, |w, cx| w.bottom_panel::<EventsPanel>(cx).unwrap());
        cx.dispatch_action(events_panel::ToggleFocus);
        assert_eq!(bottom(cx), (true, Some("EventsPanel")));
        assert!(cx.update(|window, cx| events.focus_handle(cx).contains_focused(window, cx)));
        cx.dispatch_action(events_panel::ToggleFocus);
        let pane = active_pane(&w, cx);
        assert!(focused_in(&pane, cx));
        assert_eq!(bottom(cx), (true, Some("EventsPanel")));

        // A closed dock opens on the panel asked for.
        cx.simulate_keystrokes("ctrl-j");
        assert_eq!(bottom(cx), (false, Some("EventsPanel")));
        assert!(!w.read_with(cx, |w, cx| w.docks(cx).bottom));
        cx.dispatch_action(queue_panel::ToggleFocus);
        assert_eq!(bottom(cx), (true, Some("QueuePanel")));

        // The side panels have theirs too.
        cx.simulate_keystrokes("ctrl-b");
        assert!(!w.read_with(cx, |w, cx| w.docks(cx).fleet));
        cx.dispatch_action(fleet_panel::ToggleFocus);
        assert!(w.read_with(cx, |w, cx| w.docks(cx).fleet));
        let fleet_focus = w.read_with(cx, |w, cx| w.fleet_panel().focus_handle(cx));
        assert!(cx.update(|window, _| fleet_focus.is_focused(window)));
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
