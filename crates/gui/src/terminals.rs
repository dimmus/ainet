//! The Terminals grid: every live agent's terminal at once, in rows and
//! columns, as one tab ([`TerminalsItem`]), as the TUI's Terminals view
//! (`^G`). Each tile is a [`TerminalItem`] the grid owns, with its header,
//! live screen and typing: a click gives a tile the keyboard (and ainetd
//! holds the agent's messages, as in a terminal tab). The grid tells its
//! tiles when they show and hide, as a pane tells its tabs, so the fleet
//! streams exactly the screens on show and none while the tab is hidden.
//!
//! Columns are the TUI's: `terminal::grid_cols` from the grid's room in
//! terminal cells, or as many as asked with `+` and `-` (`=` lets the
//! shape pick again); so is the rig filter (`ctrl-r`: all, each rig, all).
//! A tile can fill the grid (`terminal::ToggleZoom`) and go back.

use std::collections::HashSet;

use ainet_model::terminal::{fewer_cols, grid_cols, grid_step, more_cols, next_rig};
use gpui::{
    ClickEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyContext, Pixels, Size, Subscription, canvas,
};
use ui::{Tooltip, prelude::*};

use crate::actions::{
    ActivatePaneDown, ActivatePaneLeft, ActivatePaneRight, ActivatePaneUp, AutoColumns, CycleRig,
    FewerColumns, FocusGrid, MoreColumns, terminal,
};
use crate::fleet::FleetModel;
use crate::item::{Item, ItemEvent};
use crate::persistence::SavedItem;
use crate::terminal_element::Metrics;
use crate::terminal_item::TerminalItem;

/// One agent's terminal in the grid.
struct Tile {
    agent: String,
    item: Entity<TerminalItem>,
    _focus: [Subscription; 2],
}

pub struct TerminalsItem {
    fleet: Entity<FleetModel>,
    focus: FocusHandle,
    /// Only this rig's terminals; `None`: every rig's and the loose ones.
    rig: Option<String>,
    /// Columns asked for; `None` picks them from the grid's shape.
    cols: Option<usize>,
    /// The tiles, in the Store's (the TUI's) order.
    tiles: Vec<Tile>,
    /// The agent whose tile fills the grid, if one does.
    zoomed: Option<String>,
    /// The tile that last had the keyboard: where it goes back to from
    /// the grid.
    current: Option<String>,
    /// Whether the tab is on show.
    active: bool,
    /// The tiles told they are on show.
    showing: HashSet<String>,
    /// The room for the tiles, as last drawn.
    room: Option<Size<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalsItem {
    /// The grid of `rig`'s live terminals (every one's for `None`).
    pub fn new(
        fleet: Entity<FleetModel>,
        rig: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Agents start and stop: tiles come and go.
        let subscriptions = vec![cx.observe_in(&fleet, window, |this: &mut Self, _, window, cx| {
            this.sync_tiles(window, cx)
        })];
        let mut this = TerminalsItem {
            fleet,
            focus: cx.focus_handle(),
            rig,
            cols: None,
            tiles: Vec::new(),
            zoomed: None,
            current: None,
            active: false,
            showing: HashSet::new(),
            room: None,
            _subscriptions: subscriptions,
        };
        this.sync_tiles(window, cx);
        this
    }

    /// The rig shown, or `None` for all.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rig(&self) -> Option<&str> {
        self.rig.as_deref()
    }

    /// The columns asked for, if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fixed_cols(&self) -> Option<usize> {
        self.cols
    }

    pub fn set_fixed_cols(&mut self, cols: Option<usize>, cx: &mut Context<Self>) {
        self.cols = cols.map(|c| c.max(1));
        cx.notify();
    }

    /// The agents tiled, in order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn agents(&self) -> Vec<String> {
        self.tiles.iter().map(|t| t.agent.clone()).collect()
    }

    /// The tile of `agent`, if it is in the grid.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tile(&self, agent: &str) -> Option<Entity<TerminalItem>> {
        self.tiles
            .iter()
            .find(|t| t.agent == agent)
            .map(|t| t.item.clone())
    }

    /// The agent whose tile fills the grid.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn zoomed(&self) -> Option<&str> {
        self.zoomed.as_deref()
    }

    /// Shows `rig`'s terminals (every one's for `None`).
    pub fn set_rig(&mut self, rig: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.rig != rig {
            self.rig = rig;
            self.sync_tiles(window, cx);
            cx.emit(ItemEvent::UpdateTab);
            cx.notify();
        }
    }

    /// `ctrl-r`, as the TUI's `^R`: all, then each rig, then all again.
    fn cycle_rig(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rigs = &self.fleet.read(cx).store.rigs;
        let next = next_rig(rigs.keys().map(String::as_str), self.rig.as_deref());
        self.set_rig(next, window, cx);
    }

    /// The columns the tiles are laid out in: as asked, else the count
    /// that keeps them closest to a terminal's shape in the room there is.
    pub fn columns(&self, window: &Window, cx: &App) -> usize {
        let cell = Metrics::new(window, cx).cell;
        let room = self.room.unwrap_or_else(|| window.viewport_size());
        grid_cols(
            self.tiles.len(),
            f64::from(room.width / cell.width),
            f64::from(room.height / cell.height),
            self.cols,
        )
    }

    fn more_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.columns(window, cx);
        self.set_fixed_cols(Some(more_cols(now, self.tiles.len())), cx);
    }

    fn fewer_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.columns(window, cx);
        self.set_fixed_cols(Some(fewer_cols(now)), cx);
    }

    /// Brings the tiles in line with the live terminals of the rig shown:
    /// new agents get a tile, gone ones lose theirs (as a closed tab), and
    /// the order is the Store's.
    fn sync_tiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.fleet.read(cx).store.terminal_ids(self.rig.as_deref());
        if self.tiles.iter().map(|t| &t.agent).eq(ids.iter()) {
            return;
        }
        let mut old = std::mem::take(&mut self.tiles);
        let mut tiles = Vec::with_capacity(ids.len());
        for id in &ids {
            match old.iter().position(|t| &t.agent == id) {
                Some(ix) => tiles.push(old.remove(ix)),
                None => tiles.push(self.new_tile(id, window, cx)),
            }
        }
        self.tiles = tiles;
        for tile in old {
            let handle = tile.item.focus_handle(cx);
            if handle.is_focused(window) || handle.contains_focused(window, cx) {
                window.focus(&self.focus, cx);
            }
            if self.zoomed.as_ref() == Some(&tile.agent) {
                self.zoomed = None;
            }
            if self.current.as_ref() == Some(&tile.agent) {
                self.current = None;
            }
            self.showing.remove(&tile.agent);
            tile.item.update(cx, |item, cx| item.on_removed(window, cx));
        }
        self.refresh_shown(window, cx);
        cx.notify();
    }

    fn new_tile(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) -> Tile {
        let (fleet, id) = (self.fleet.clone(), agent.to_string());
        let item = cx.new(|cx| TerminalItem::new(fleet, id, window, cx));
        let handle = item.focus_handle(cx);
        let id = agent.to_string();
        // The focused tile's border lights; it is where the keys from the
        // grid go back to.
        let focus = [
            cx.on_focus_in(&handle, window, move |this, _, cx| {
                this.current = Some(id.clone());
                cx.notify();
            }),
            cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()),
        ];
        Tile {
            agent: agent.to_string(),
            item,
            _focus: focus,
        }
    }

    /// Tells the tiles whether they are on show: all of them while the
    /// tab is (only the zoomed one, if one is), none while it is hidden.
    fn refresh_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for tile in &self.tiles {
            let on = self.active && self.zoomed.as_ref().is_none_or(|z| *z == tile.agent);
            if on == self.showing.contains(&tile.agent) {
                continue;
            }
            if on {
                self.showing.insert(tile.agent.clone());
            } else {
                self.showing.remove(&tile.agent);
            }
            tile.item.update(cx, |item, cx| {
                if on {
                    item.activated(window, cx)
                } else {
                    item.deactivated(window, cx)
                }
            });
        }
    }

    /// The tile with the keyboard (its menu counts), if one has it.
    fn focused_tile(&self, window: &Window, cx: &App) -> Option<usize> {
        self.tiles.iter().position(|t| {
            let handle = t.item.focus_handle(cx);
            handle.is_focused(window) || handle.contains_focused(window, cx)
        })
    }

    /// Gives tile `ix` the keyboard.
    fn focus_tile(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tile) = self.tiles.get(ix) {
            let handle = tile.item.focus_handle(cx);
            window.focus(&handle, cx);
            cx.notify();
        }
    }

    /// Gives `agent`'s tile the keyboard, if it has one on show.
    pub fn focus_agent(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.tiles.iter().position(|t| t.agent == agent) else {
            return false;
        };
        if self.zoomed.as_ref().is_some_and(|z| z != agent) {
            self.zoomed = Some(agent.to_string());
            self.refresh_shown(window, cx);
        }
        self.focus_tile(ix, window, cx);
        true
    }

    /// From the grid itself: back to the tile last used (the first if
    /// none was).
    fn enter_tiles(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let at = match (&self.zoomed, &self.current) {
            (Some(agent), _) | (None, Some(agent)) => self.tiles.iter().position(|t| &t.agent == agent),
            (None, None) => None,
        };
        match at.or((!self.tiles.is_empty()).then_some(0)) {
            Some(ix) => {
                self.focus_tile(ix, window, cx);
                true
            }
            None => false,
        }
    }

    /// `ctrl-alt-<arrow>`: from a tile, the tile that way (a zoomed grid
    /// zooms to it); past the grid's edge, the pane that way, as the
    /// workspace moves between panes. From the grid itself, the tile last
    /// used.
    fn step(&mut self, dx: isize, dy: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.focused_tile(window, cx) else {
            if !self.enter_tiles(window, cx) {
                cx.propagate();
            }
            return;
        };
        let cols = self.columns(window, cx);
        let Some(to) = grid_step(self.tiles.len(), cols, at, dx, dy) else {
            cx.propagate();
            return;
        };
        if self.zoomed.is_some() {
            self.zoomed = Some(self.tiles[to].agent.clone());
            self.refresh_shown(window, cx);
        }
        self.focus_tile(to, window, cx);
    }

    /// Fills the grid with `agent`'s tile, or puts the tiles back.
    pub fn toggle_zoom(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.zoomed.as_deref() == Some(agent) {
            self.zoomed = None;
        } else if let Some(ix) = self.tiles.iter().position(|t| t.agent == agent) {
            let had_keys = self.focused_tile(window, cx).is_some();
            self.zoomed = Some(agent.to_string());
            if had_keys {
                self.focus_tile(ix, window, cx);
            }
        } else {
            return;
        }
        self.refresh_shown(window, cx);
        cx.notify();
    }

    /// `terminal::ToggleZoom` (`shift-escape` in a tile, its menu's Zoom):
    /// that tile fills the grid, or the tiles come back. From the grid
    /// itself with nothing zoomed, the pane zooms instead.
    fn zoom_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.focused_tile(window, cx) {
            Some(ix) => Some(self.tiles[ix].agent.clone()),
            None if self.focus.is_focused(window) => self.zoomed.clone(),
            None => self.zoomed.clone().or_else(|| self.current.clone()),
        };
        match target {
            Some(agent) => self.toggle_zoom(&agent, window, cx),
            None => cx.propagate(),
        }
    }

    fn render_header(&self, cols: usize, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let count = self.tiles.len();
        let rig = match &self.rig {
            Some(rig) => format!("rig {rig}"),
            None => "all rigs".to_string(),
        };
        let zoomed = self.zoomed.as_ref().map(|id| {
            let store = &self.fleet.read(cx).store;
            store.agents.get(id).map_or(id.clone(), |a| a.name.clone())
        });
        let focus = &self.focus;
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(
                Icon::new(IconName::Terminal)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new("Terminals"))
            .child(
                div().debug_selector(|| "TERMINALS-RIG".into()).child(
                    Button::new("terminals-rig", rig)
                        .label_size(LabelSize::Small)
                        .tooltip(Tooltip::for_action_title_in("Next Rig", &CycleRig, focus))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.cycle_rig(window, cx))),
                ),
            )
            .child(
                Label::new(format!("{count} terminal{}", plural(count)))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .when_some(zoomed, |this, name| {
                this.child(
                    Label::new(format!("{name} fills the grid"))
                        .size(LabelSize::Small)
                        .color(Color::Accent),
                )
                .child(
                    IconButton::new("terminals-unzoom", IconName::Minimize)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Show Every Tile"))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if let Some(agent) = this.zoomed.clone() {
                                this.toggle_zoom(&agent, window, cx);
                            }
                        })),
                )
            })
            .child(
                Label::new(format!("{cols} column{}", plural(cols)))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                IconButton::new("terminals-fewer", IconName::Dash)
                    .icon_size(IconSize::Small)
                    .disabled(cols <= 1)
                    .tooltip(Tooltip::for_action_title_in(
                        "Fewer Columns",
                        &FewerColumns,
                        focus,
                    ))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.fewer_columns(window, cx))),
            )
            .child(
                IconButton::new("terminals-more", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .disabled(cols >= count)
                    .tooltip(Tooltip::for_action_title_in("More Columns", &MoreColumns, focus))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.more_columns(window, cx))),
            )
            .child(
                Button::new("terminals-auto", "Auto")
                    .label_size(LabelSize::Small)
                    .toggle_state(self.cols.is_none())
                    .tooltip(Tooltip::for_action_title_in(
                        "Columns by the Grid's Shape",
                        &AutoColumns,
                        focus,
                    ))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_fixed_cols(None, cx))),
            )
            .when(self.focus.is_focused(window) && count > 0, |this| {
                this.child(
                    Label::new("enter types into a tile")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
    }

    /// The tiles on show in rows of `cols`, the last row's wider; or why
    /// there are none.
    fn render_tiles(&self, cols: usize, window: &Window, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let shown: Vec<(usize, &Tile)> = self
            .tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| self.zoomed.as_ref().is_none_or(|z| *z == t.agent))
            .collect();
        if shown.is_empty() {
            let text = match &self.rig {
                Some(rig) => format!("No live terminals in rig {rig}"),
                None => "No live terminals".to_string(),
            };
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .child(Label::new(text).color(Color::Muted))
                .child(
                    Label::new("Agents' terminals show here as they start")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
                .into_any_element();
        }
        let grid_focused = self.focus.is_focused(window);
        let cols = if self.zoomed.is_some() { 1 } else { cols.max(1) };
        let tile = |(ix, tile): &(usize, &Tile)| {
            let handle = tile.item.focus_handle(cx);
            let border = if handle.is_focused(window) || handle.contains_focused(window, cx) {
                colors.border_focused
            } else if grid_focused && self.current.as_ref() == Some(&tile.agent) {
                colors.border_selected
            } else {
                colors.border_variant
            };
            let ix = *ix;
            div()
                .debug_selector(move || format!("TERMINALS-TILE-{ix}"))
                .flex_1()
                .min_w_0()
                .h_full()
                .rounded_sm()
                .border_1()
                .border_color(border)
                .overflow_hidden()
                // One pane holds them all, so a tile that redraws paints
                // the others again (GPUI redraws a view's whole subtree);
                // their rows come from their caches, nothing is shaped.
                .child(tile.item.clone())
        };
        v_flex()
            .size_full()
            .p_1()
            .gap_1()
            .children(shown.chunks(cols).map(|row| {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .gap_1()
                    .children(row.iter().map(tile))
            }))
            .into_any_element()
    }
}

impl EventEmitter<ItemEvent> for TerminalsItem {}

impl Focusable for TerminalsItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Item for TerminalsItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        match &self.rig {
            Some(rig) => format!("Terminals · {rig}").into(),
            None => "Terminals".into(),
        }
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        Some(
            Icon::new(IconName::Terminal)
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        let n = self.tiles.len();
        Some(format!("{n} live terminal{}", if n == 1 { "" } else { "s" }).into())
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        Some(SavedItem::Terminals {
            rig: self.rig.clone(),
            cols: self.cols,
        })
    }

    fn activated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = true;
        self.refresh_shown(window, cx);
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = false;
        self.refresh_shown(window, cx);
    }

    /// Closed: its tiles go as closed tabs do, and it stops following the
    /// fleet.
    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = false;
        self._subscriptions.clear();
        for tile in self.tiles.drain(..) {
            tile.item.update(cx, |item, cx| item.on_removed(window, cx));
        }
        self.showing.clear();
    }
}

impl Render for TerminalsItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `grid` only while the grid itself (not a tile) has the keyboard:
        // its bare keys must reach the agent while you type in a tile.
        let mut context = KeyContext::new_with_defaults();
        context.add("Terminals");
        if self.focus.is_focused(window) {
            context.add("grid");
        }
        let cols = self.columns(window, cx);
        let this = cx.entity();
        let measure = canvas(
            move |bounds, _, cx| {
                this.update(cx, |this, cx| {
                    if this.room != Some(bounds.size) {
                        this.room = Some(bounds.size);
                        cx.notify();
                    }
                })
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        v_flex()
            .id("terminals")
            .key_context(context)
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .on_action(cx.listener(|this, _: &MoreColumns, window, cx| this.more_columns(window, cx)))
            .on_action(cx.listener(|this, _: &FewerColumns, window, cx| this.fewer_columns(window, cx)))
            .on_action(cx.listener(|this, _: &AutoColumns, _, cx| this.set_fixed_cols(None, cx)))
            .on_action(cx.listener(|this, _: &CycleRig, window, cx| this.cycle_rig(window, cx)))
            .on_action(cx.listener(|this, _: &FocusGrid, window, cx| {
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                if !this.enter_tiles(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ActivatePaneLeft, window, cx| this.step(-1, 0, window, cx)))
            .on_action(cx.listener(|this, _: &ActivatePaneRight, window, cx| this.step(1, 0, window, cx)))
            .on_action(cx.listener(|this, _: &ActivatePaneUp, window, cx| this.step(0, -1, window, cx)))
            .on_action(cx.listener(|this, _: &ActivatePaneDown, window, cx| this.step(0, 1, window, cx)))
            .on_action(cx.listener(|this, _: &terminal::ToggleZoom, window, cx| this.zoom_action(window, cx)))
            .child(self.render_header(cols, window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(measure)
                    .child(self.render_tiles(cols, window, cx)),
            )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::{AgentState, Event, Request};
    use ainet_model::Msg;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, size};

    use super::*;
    use crate::args::View;
    use crate::pane_group::SplitDirection;
    use crate::tests::{demo_workspace, init_test};
    use crate::workspace::Workspace;

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    /// The active pane's active tab, as the grid.
    fn active_grid(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<TerminalsItem>> {
        w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<TerminalsItem>().ok())
        })
    }

    /// The demo in a window with the grid opened by `ctrl-g`.
    fn open_grid(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, Entity<TerminalsItem>, &mut VisualTestContext) {
        let (w, cx) = demo_workspace(cx);
        cx.simulate_keystrokes("ctrl-g");
        cx.run_until_parked();
        let grid = active_grid(&w, cx).expect("ctrl-g opens the grid");
        (w, grid, cx)
    }

    fn take_sent(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> Vec<Request> {
        fleet.update(cx, |f, _| {
            std::mem::take(&mut f.sent).into_iter().map(|(r, _)| r).collect()
        })
    }

    /// Lets the keyboard settle, so ainetd hears where it is.
    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(crate::fleet::INTERACT_SETTLE);
        cx.run_until_parked();
    }

    fn interacts(sent: &[Request]) -> Vec<(String, bool)> {
        sent.iter()
            .filter_map(|r| match r {
                Request::Interact { agent, on } => Some((agent.clone(), *on)),
                _ => None,
            })
            .collect()
    }

    fn inputs(sent: &[Request]) -> Vec<(String, String)> {
        sent.iter()
            .filter_map(|r| match r {
                Request::Input { agent, data } => Some((agent.clone(), data.clone())),
                _ => None,
            })
            .collect()
    }

    /// Which of `agents` the fleet keeps fresh for a view on show.
    fn shown(fleet: &Entity<FleetModel>, agents: &[String], cx: &mut VisualTestContext) -> Vec<String> {
        fleet.read_with(cx, |f, _| {
            agents.iter().filter(|a| f.is_shown(a)).cloned().collect()
        })
    }

    fn has_keys(grid: &Entity<TerminalsItem>, agent: &str, cx: &mut VisualTestContext) -> bool {
        let tile = grid.read_with(cx, |g, _| g.tile(agent).expect("a tile"));
        cx.update(|window, cx| tile.focus_handle(cx).is_focused(window))
    }

    fn grid_has_keys(grid: &Entity<TerminalsItem>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| grid.focus_handle(cx).is_focused(window))
    }

    fn click_tile(ix: usize, cx: &mut VisualTestContext) {
        cx.run_until_parked();
        let selector: &'static str = format!("TERMINALS-TILE-{ix}").leak();
        let tile = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("tile {ix} is drawn"));
        cx.simulate_click(tile.center(), Modifiers::none());
    }

    #[gpui::test]
    async fn ctrl_g_tiles_every_live_terminal_in_the_tuis_order(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let all = fleet.read_with(cx, |f, _| f.store.terminal_ids(None));
        assert_eq!(all.len(), 7, "the demo's seven agents");
        assert_eq!(grid.read_with(cx, |g, _| g.agents()), all);
        assert!(grid_has_keys(&grid, cx), "the grid itself has the keyboard");
        let tab = w.read_with(cx, |w, cx| {
            let pane = w.active_pane().read(cx);
            pane.active_item().unwrap().tab_content_text(cx).to_string()
        });
        assert_eq!(tab, "Terminals");
        assert_eq!(shown(&fleet, &all, cx), all, "every tile is kept fresh");

        // Rows of `columns` tiles, in order, left to right.
        let cols = cx.update(|window, cx| grid.read(cx).columns(window, cx));
        assert!((2..7).contains(&cols), "{cols}");
        let bounds: Vec<_> = (0..all.len())
            .map(|ix| {
                let selector: &'static str = format!("TERMINALS-TILE-{ix}").leak();
                cx.debug_bounds(selector).expect("every tile is drawn")
            })
            .collect();
        assert_eq!(bounds[0].top(), bounds[cols - 1].top());
        assert!(bounds[0].left() < bounds[1].left());
        assert!(bounds[cols].top() > bounds[0].top(), "a second row");
        // Every tile painted its agent's screen.
        for agent in &all {
            let tile = grid.read_with(cx, |g, _| g.tile(agent).unwrap());
            assert!(tile.read_with(cx, |t, _| t.laid_out_size().is_some()), "{agent}");
        }

        // Again: the same tab, not a second.
        cx.simulate_keystrokes("ctrl-g");
        assert_eq!(active_grid(&w, cx), Some(grid));
        let tabs = w.read_with(cx, |w, cx| w.active_pane().read(cx).items().count());
        assert_eq!(tabs, 2, "the Fleet stand-in and the grid");
    }

    #[gpui::test]
    async fn ctrl_r_shows_each_rig_in_turn(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let all = fleet.read_with(cx, |f, _| f.store.terminal_ids(None));
        let of = |rig: &str, cx: &mut VisualTestContext| {
            fleet.read_with(cx, |f, _| f.store.terminal_ids(Some(rig)))
        };
        let (docs, infra) = (of("docs", cx), of("infra", cx));
        let tab = |cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| {
                w.active_pane()
                    .read(cx)
                    .active_item()
                    .unwrap()
                    .tab_content_text(cx)
                    .to_string()
            })
        };

        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(
            grid.read_with(cx, |g, _| g.rig().map(str::to_string)),
            Some("docs".into())
        );
        assert_eq!(grid.read_with(cx, |g, _| g.agents()), docs);
        assert_eq!(tab(cx), "Terminals · docs");
        assert_eq!(shown(&fleet, &all, cx), docs, "the others stop");
        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(grid.read_with(cx, |g, _| g.agents()), infra);
        assert_eq!(shown(&fleet, &all, cx), infra);
        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(
            grid.read_with(cx, |g, _| g.rig().map(str::to_string)),
            None,
            "round to all"
        );
        assert_eq!(grid.read_with(cx, |g, _| g.agents()), all);
        assert_eq!(tab(cx), "Terminals");

        // The rig button does the same.
        cx.run_until_parked();
        let button = cx.debug_bounds("TERMINALS-RIG").expect("the rig button is drawn");
        cx.simulate_click(button.center(), Modifiers::none());
        assert_eq!(
            grid.read_with(cx, |g, _| g.rig().map(str::to_string)),
            Some("docs".into())
        );

        // In a tile, ctrl-r is the agent's.
        click_tile(0, cx);
        settle(cx);
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("ctrl-r");
        assert_eq!(
            inputs(&take_sent(&fleet, cx)),
            [(docs[0].clone(), "\x12".to_string())]
        );
        assert_eq!(
            grid.read_with(cx, |g, _| g.rig().map(str::to_string)),
            Some("docs".into())
        );
    }

    #[gpui::test]
    async fn plus_minus_and_equals_set_the_columns(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let auto = cx.update(|window, cx| grid.read(cx).columns(window, cx));
        let fixed = |cx: &mut VisualTestContext| grid.read_with(cx, |g, _| g.fixed_cols());

        cx.simulate_keystrokes("+");
        assert_eq!(fixed(cx), Some(auto + 1));
        cx.simulate_keystrokes("- - -");
        assert_eq!(fixed(cx), Some((auto + 1).saturating_sub(3).max(1)));
        for _ in 0..10 {
            cx.simulate_keystrokes("-");
        }
        assert_eq!(fixed(cx), Some(1), "at least one");
        cx.run_until_parked();
        let (a, b) = (
            cx.debug_bounds("TERMINALS-TILE-0").unwrap(),
            cx.debug_bounds("TERMINALS-TILE-1").unwrap(),
        );
        assert!(b.top() > a.top(), "one column: one tile per row");
        for _ in 0..10 {
            cx.simulate_keystrokes("+");
        }
        assert_eq!(fixed(cx), Some(7), "at most one per tile");
        cx.simulate_keystrokes("=");
        assert_eq!(fixed(cx), None);
        cx.dispatch_action(MoreColumns);
        assert_eq!(fixed(cx), Some(auto + 1), "the action, as from the palette");
        cx.dispatch_action(AutoColumns);

        // Typed into a tile, they are the agent's.
        click_tile(2, cx);
        settle(cx);
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("+ - =");
        let agent = grid.read_with(cx, |g, _| g.agents()[2].clone());
        let typed: Vec<String> = inputs(&take_sent(&fleet, cx))
            .into_iter()
            .map(|(a, data)| {
                assert_eq!(a, agent);
                data
            })
            .collect();
        assert_eq!(typed, ["+", "-", "="]);
        assert_eq!(fixed(cx), None);
    }

    #[gpui::test]
    async fn a_click_gives_a_tile_the_keys_and_ainetd_holds_its_messages(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let agents = grid.read_with(cx, |g, _| g.agents());
        settle(cx);
        take_sent(&fleet, cx);

        click_tile(1, cx);
        settle(cx);
        assert!(has_keys(&grid, &agents[1], cx));
        assert_eq!(interacts(&take_sent(&fleet, cx)), [(agents[1].clone(), true)]);
        assert_eq!(
            fleet.read_with(cx, |f, _| f.store.selected.clone()),
            Some(agents[1].clone())
        );
        cx.simulate_keystrokes("x escape");
        assert_eq!(
            inputs(&take_sent(&fleet, cx)),
            [
                (agents[1].clone(), "x".into()),
                (agents[1].clone(), "\x1b".into())
            ]
        );

        // ctrl-alt-arrows go from tile to tile; ainetd hears it once the
        // keyboard settles.
        cx.simulate_keystrokes("ctrl-alt-right");
        settle(cx);
        assert!(has_keys(&grid, &agents[2], cx));
        let mut told = interacts(&take_sent(&fleet, cx));
        told.sort();
        assert_eq!(told, [(agents[1].clone(), false), (agents[2].clone(), true)]);
        cx.simulate_keystrokes("ctrl-alt-left ctrl-alt-left");
        assert!(has_keys(&grid, &agents[0], cx));
        let cols = cx.update(|window, cx| grid.read(cx).columns(window, cx));
        cx.simulate_keystrokes("ctrl-alt-down");
        assert!(has_keys(&grid, &agents[cols], cx));
        settle(cx);
        take_sent(&fleet, cx);

        // ctrl-alt-g: back to the grid, the agent's messages flow again;
        // enter types into the tile last used.
        cx.simulate_keystrokes("ctrl-alt-g");
        settle(cx);
        assert!(grid_has_keys(&grid, cx));
        assert_eq!(interacts(&take_sent(&fleet, cx)), [(agents[cols].clone(), false)]);
        cx.simulate_keystrokes("enter");
        assert!(has_keys(&grid, &agents[cols], cx));

        // Past the grid's edge, the pane that way.
        cx.simulate_keystrokes("ctrl-alt-g");
        w.update_in(cx, |w, window, cx| {
            let pane = w.active_pane().clone();
            w.split_pane(&pane, SplitDirection::Right, window, cx);
        });
        let right = w.read_with(cx, |w, _| w.active_pane().clone());
        cx.simulate_keystrokes("ctrl-alt-left");
        settle(cx);
        assert_eq!(active_grid(&w, cx), Some(grid.clone()));
        cx.simulate_keystrokes("enter");
        let at = agents.iter().position(|a| has_keys(&grid, a, cx)).unwrap();
        // Narrower now: the columns as drawn beside the new pane.
        let cols = cx.update(|window, cx| grid.read(cx).columns(window, cx));
        let in_row = (agents.len() - at / cols * cols).min(cols);
        for _ in at % cols + 1..in_row {
            cx.simulate_keystrokes("ctrl-alt-right");
        }
        assert_ne!(w.read_with(cx, |w, _| w.active_pane().clone()), right);
        cx.simulate_keystrokes("ctrl-alt-right");
        cx.run_until_parked();
        assert_eq!(
            w.read_with(cx, |w, _| w.active_pane().clone()),
            right,
            "out of the grid"
        );
    }

    #[gpui::test]
    async fn a_new_screen_shapes_only_its_tiles_rows(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let tiles: Vec<Entity<TerminalItem>> =
            grid.read_with(cx, |g, _| g.agents().iter().filter_map(|a| g.tile(a)).collect());
        let shaped = |cx: &mut VisualTestContext| {
            tiles
                .iter()
                .map(|t| t.read_with(cx, |t, _| t.row_cache_counts().1))
                .collect::<Vec<_>>()
        };
        let agent = grid.read_with(cx, |g, _| g.agents()[0].clone());
        let before = shaped(cx);
        let screen = Msg::Term {
            id: agent,
            ansi: "one\r\ntwo".into(),
            size: (24, 80),
            rows: vec!["one".into(), "two".into()],
        };
        fleet.update(cx, |f, cx| f.receive(vec![screen], cx));
        cx.run_until_parked();
        let after = shaped(cx);
        assert!(after[0] > before[0], "its rows were shaped");
        assert_eq!(after[1..], before[1..], "no other tile shaped a row");
    }

    #[gpui::test]
    async fn a_tile_zooms_inside_the_grid(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let agents = grid.read_with(cx, |g, _| g.agents());
        click_tile(0, cx);
        cx.simulate_keystrokes("shift-escape");
        cx.run_until_parked();
        assert_eq!(
            grid.read_with(cx, |g, _| g.zoomed().map(str::to_string)),
            Some(agents[0].clone())
        );
        assert!(
            w.read_with(cx, |w, _| w.zoomed_pane().is_none()),
            "the grid's own zoom"
        );
        assert_eq!(
            shown(&fleet, &agents, cx),
            [agents[0].clone()],
            "only it is kept fresh"
        );
        assert!(has_keys(&grid, &agents[0], cx));
        assert!(cx.debug_bounds("TERMINALS-TILE-0").is_some());
        assert!(cx.debug_bounds("TERMINALS-TILE-2").is_none());

        // Moving on zooms the next tile.
        cx.simulate_keystrokes("ctrl-alt-right");
        assert_eq!(
            grid.read_with(cx, |g, _| g.zoomed().map(str::to_string)),
            Some(agents[1].clone())
        );
        assert_eq!(shown(&fleet, &agents, cx), [agents[1].clone()]);

        // Back: every tile again.
        cx.simulate_keystrokes("shift-escape");
        cx.run_until_parked();
        assert_eq!(grid.read_with(cx, |g, _| g.zoomed().map(str::to_string)), None);
        assert_eq!(shown(&fleet, &agents, cx), agents);
        // The tile's menu zooms it too.
        cx.dispatch_action(terminal::ToggleZoom);
        assert_eq!(
            grid.read_with(cx, |g, _| g.zoomed().map(str::to_string)),
            Some(agents[1].clone())
        );
    }

    #[gpui::test]
    async fn a_hidden_or_closed_grid_stops_fetching(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let agents = grid.read_with(cx, |g, _| g.agents());
        assert_eq!(shown(&fleet, &agents, cx), agents);

        // Another tab over it: nothing of the grid's is fetched.
        cx.simulate_keystrokes("ctrl-shift-i");
        cx.run_until_parked();
        assert!(shown(&fleet, &agents, cx).is_empty());
        cx.executor().advance_clock(crate::fleet::TICK * 3);
        cx.run_until_parked();
        fleet.update(cx, |f, _| f.fetched.clear());
        for agent in &agents {
            let output = Msg::Event(Box::new(Event::Output { agent: agent.clone() }));
            fleet.update(cx, |f, cx| f.receive(vec![output], cx));
        }
        cx.executor()
            .advance_clock(ainet_model::terminal::FETCH_EVERY * 3);
        cx.run_until_parked();
        assert!(fleet.read_with(cx, |f, _| f.fetched.is_empty()));

        // Back on show, then closed.
        cx.simulate_keystrokes("ctrl-g");
        cx.run_until_parked();
        assert_eq!(shown(&fleet, &agents, cx), agents);
        cx.simulate_keystrokes("ctrl-w");
        cx.run_until_parked();
        assert!(active_grid(&w, cx).is_none());
        assert!(shown(&fleet, &agents, cx).is_empty());
        assert!(grid.read_with(cx, |g, _| g.agents().is_empty()));
    }

    #[gpui::test]
    async fn tiles_come_and_go_as_agents_start_and_stop(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let before = grid.read_with(cx, |g, _| g.agents());
        let mut new = fleet.read_with(cx, |f, _| f.store.agents[&before[0]].clone());
        new.id = "aaa-new".into();
        new.seat = Some("dev.zz@docs".into());
        let arrive = |agent: ainet_core::Agent, cx: &mut VisualTestContext| {
            let event = Event::Agent { agent };
            fleet.update(cx, |f, cx| f.receive(vec![Msg::Event(Box::new(event))], cx));
            cx.run_until_parked();
        };
        arrive(new.clone(), cx);
        let now = grid.read_with(cx, |g, _| g.agents());
        assert_eq!(
            now,
            fleet.read_with(cx, |f, _| f.store.terminal_ids(None)),
            "in order"
        );
        assert_eq!(now.len(), before.len() + 1);
        assert!(
            fleet.read_with(cx, |f, _| f.is_shown("aaa-new")),
            "on show at once"
        );

        // It types, then goes: its tile goes and the keyboard stays in the
        // grid.
        let ix = now.iter().position(|a| a == "aaa-new").unwrap();
        click_tile(ix, cx);
        assert!(has_keys(&grid, "aaa-new", cx));
        new.state = AgentState::Gone;
        arrive(new, cx);
        assert_eq!(grid.read_with(cx, |g, _| g.agents()), before);
        assert!(!fleet.read_with(cx, |f, _| f.is_shown("aaa-new")));
        assert!(grid_has_keys(&grid, cx));
    }

    #[gpui::test]
    async fn the_latest_view_of_an_agent_sizes_its_pane(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let fleet = fleet(&w, cx);
        let agent = grid.read_with(cx, |g, _| g.agents()[0].clone());
        let tile = grid.read_with(cx, |g, _| g.tile(&agent).unwrap());
        let resize = |cx: &mut VisualTestContext| {
            cx.executor()
                .advance_clock(crate::terminal_item::RESIZE_AFTER * 2);
            cx.run_until_parked();
        };
        resize(cx);
        let tile_size = tile.read_with(cx, |t, _| t.laid_out_size().unwrap());
        assert_eq!(fleet.read_with(cx, |f, _| f.terminal_size(&agent)), tile_size);

        // Its own tab, beside the grid: shown last, its size.
        let tab = w.update_in(cx, |w, window, cx| {
            let pane = w.active_pane().clone();
            let right = w.split_pane(&pane, SplitDirection::Right, window, cx);
            let (fleet, agent) = (w.fleet().clone(), agent.clone());
            let tab = cx.new(|cx| TerminalItem::new(fleet, agent, window, cx));
            right.update(cx, |p, cx| {
                p.add_item(Box::new(tab.clone()), true, true, window, cx)
            });
            tab
        });
        resize(cx);
        let (tile_size, tab_size) = (
            tile.read_with(cx, |t, _| t.laid_out_size().unwrap()),
            tab.read_with(cx, |t, _| t.laid_out_size().unwrap()),
        );
        assert_ne!(tile_size, tab_size);
        assert_eq!(fleet.read_with(cx, |f, _| f.terminal_size(&agent)), tab_size);

        // A click in the tile: the tile's. The window resized, both lay
        // out anew and the tile keeps it: no flip-flop.
        click_tile(0, cx);
        resize(cx);
        assert_eq!(fleet.read_with(cx, |f, _| f.terminal_size(&agent)), tile_size);
        take_sent(&fleet, cx);
        cx.simulate_resize(size(px(1500.), px(900.)));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        resize(cx);
        let tile_size = tile.read_with(cx, |t, _| t.laid_out_size().unwrap());
        let resizes: Vec<(u16, u16)> = take_sent(&fleet, cx)
            .iter()
            .filter_map(|r| match r {
                Request::Resize { agent: a, rows, cols } if *a == agent => Some((*rows, *cols)),
                _ => None,
            })
            .collect();
        assert_eq!(resizes, [tile_size]);
    }

    #[gpui::test]
    async fn opening_a_tiled_agent_types_into_its_tile(cx: &mut TestAppContext) {
        let (w, grid, cx) = open_grid(cx);
        let agent = grid.read_with(cx, |g, _| g.agents()[5].clone());
        let fleet = fleet(&w, cx);
        fleet.update(cx, |f, cx| f.select(&agent, cx));
        cx.dispatch_action(crate::actions::OpenTerminal);
        assert!(has_keys(&grid, &agent, cx));
        let tabs = w.read_with(cx, |w, cx| w.active_pane().read(cx).items().count());
        assert_eq!(tabs, 2, "no tab of its own");
    }

    #[gpui::test]
    async fn views_and_commands_open_the_grid(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(
                fleet,
                Some(View::Terminals {
                    rig: Some("infra".into()),
                }),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let grid = active_grid(&w, cx).expect("--view rig:infra opens the grid");
        let infra = fleet(&w, cx).read_with(cx, |f, _| f.store.terminal_ids(Some("infra")));
        assert_eq!(grid.read_with(cx, |g, _| g.agents()), infra);
        // Hidden until its pane shows it: here it is the first tab.
        assert_eq!(shown(&fleet(&w, cx), &infra, cx), infra);

        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, Some(View::Named("nowhere".into())), window, cx)
        });
        cx.run_until_parked();
        let grid = active_grid(&w, cx).expect("a rig's name opens the grid");
        assert!(grid.read_with(cx, |g, _| g.agents().is_empty()));
        assert_eq!(
            grid.read_with(cx, |g, _| g.rig().map(str::to_string)),
            Some("nowhere".into())
        );
    }
}
