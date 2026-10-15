//! A pane: a strip of tabs over the active item, after Zed's
//! `workspace::pane`. Tabs are Zed's `ui::Tab` in a `ui::TabBar`: click to
//! activate, middle-click or the close button (shown on hover) to close,
//! right-click for a menu. Closing a tab activates the one used before it,
//! as Zed does by default. What a pane cannot decide alone (splitting,
//! zooming, going away when its last tab closes) it asks the workspace with
//! a [`PaneEvent`].

use gpui::{
    Action, AnyElement, ClickEvent, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    ScrollHandle, Subscription, WeakEntity, Window,
};
use ui::{
    ContextMenu, IconButtonShape, PopoverMenu, Tab, TabBar, TabCloseSide, TabPosition, Tooltip, prelude::*,
    right_click_menu,
};

use crate::actions::{
    ActivateNextItem, ActivatePreviousItem, CloseActiveItem, CloseAllItems, CloseOtherItems, SplitDown,
    SplitRight, ToggleZoom,
};
use crate::item::{ItemEvent, ItemHandle, TabContentParams};
use crate::pane_group::SplitDirection;

/// What a pane asks of the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneEvent {
    /// Split me, the new pane on this side.
    Split(SplitDirection),
    /// My last tab closed: take me out of the group.
    Remove,
    /// I took the keyboard.
    Focus,
    /// Make me fill the window.
    ZoomIn,
    /// Put me back.
    ZoomOut,
    /// My active tab changed.
    ActivateItem,
}

pub struct Pane {
    focus_handle: FocusHandle,
    items: Vec<Box<dyn ItemHandle>>,
    active: usize,
    /// Item ids, the most recently active last: closing a tab goes back to
    /// the one before it.
    history: Vec<EntityId>,
    zoomed: bool,
    item_subscriptions: Vec<(EntityId, Subscription)>,
    tab_bar_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Pane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus_handle, window, |pane, window, cx| {
            // Focused itself (a click on an empty pane, or the workspace
            // handing it the keyboard): pass it on to the active tab.
            if pane.focus_handle.is_focused(window)
                && let Some(item) = pane.active_item()
            {
                window.focus(&item.focus_handle(cx), cx);
            }
            cx.emit(PaneEvent::Focus);
            cx.notify();
        });
        let focus_out = cx.on_focus_out(&focus_handle, window, |_, _, _, cx| cx.notify());
        Pane {
            focus_handle,
            items: Vec::new(),
            active: 0,
            history: Vec::new(),
            zoomed: false,
            item_subscriptions: Vec::new(),
            tab_bar_scroll: ScrollHandle::new(),
            _subscriptions: vec![focus_in, focus_out],
        }
    }

    pub fn items(&self) -> impl Iterator<Item = &dyn ItemHandle> {
        self.items.iter().map(|i| i.as_ref())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_item(&self) -> Option<&dyn ItemHandle> {
        self.items.get(self.active).map(|i| i.as_ref())
    }

    pub fn index_of(&self, id: EntityId) -> Option<usize> {
        self.items.iter().position(|i| i.item_id() == id)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    pub fn set_zoomed(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }

    /// Whether the keyboard is in this pane. Also true for a tab focused
    /// since the last frame, which the drawn focus tree does not know yet.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
            || self.items.iter().any(|i| i.focus_handle(cx).is_focused(window))
    }

    /// Adds a tab after the active one; activates (and focuses) it if
    /// asked.
    pub fn add_item(
        &mut self,
        item: Box<dyn ItemHandle>,
        activate: bool,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = item.item_id();
        if let Some(ix) = self.index_of(id) {
            if activate {
                self.activate_item(ix, focus, window, cx);
            }
            return;
        }
        let pane = cx.entity().downgrade();
        let subscription = item.subscribe(
            window,
            cx,
            Box::new(move |id, event, window, cx| {
                let _ = pane.update(cx, |pane, cx| match event {
                    ItemEvent::CloseItem => pane.close_item_by_id(id, window, cx),
                    ItemEvent::UpdateTab => cx.notify(),
                });
            }),
        );
        self.item_subscriptions.push((id, subscription));
        let ix = if self.items.is_empty() { 0 } else { self.active + 1 };
        self.items.insert(ix, item);
        if self.items.len() == 1 {
            self.active = 0;
            self.activated(window, cx);
        } else if ix <= self.active {
            self.active += 1;
        }
        if activate {
            self.activate_item(ix, focus, window, cx);
        }
        cx.notify();
    }

    /// Records the active item as the latest and tells it so.
    fn activated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.active) else {
            return;
        };
        let id = item.item_id();
        self.history.retain(|h| *h != id);
        self.history.push(id);
        item.set_active(true, window, cx);
        cx.emit(PaneEvent::ActivateItem);
    }

    /// Makes tab `ix` the active one, and gives it the keyboard if `focus`.
    pub fn activate_item(&mut self, ix: usize, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.items.len() {
            return;
        }
        if ix != self.active {
            if let Some(old) = self.items.get(self.active) {
                old.set_active(false, window, cx);
            }
            self.active = ix;
            self.activated(window, cx);
            self.tab_bar_scroll.scroll_to_item(ix);
        }
        if focus {
            let handle = self.items[ix].focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// Steps `delta` tabs on (back if negative), wrapping round.
    pub fn activate_relative(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let ix = (self.active as isize + delta).rem_euclid(n as isize) as usize;
        let focus = self.has_focus(window, cx);
        self.activate_item(ix, focus, window, cx);
    }

    /// Closes the tab of item `id`. The tab used before it becomes active;
    /// a pane left without tabs asks to be removed.
    pub fn close_item_by_id(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else {
            return;
        };
        let had_focus = self.has_focus(window, cx);
        let was_active = ix == self.active;
        let item = self.items.remove(ix);
        self.item_subscriptions.retain(|(i, _)| *i != id);
        self.history.retain(|h| *h != id);
        if was_active {
            item.set_active(false, window, cx);
        }
        item.on_removed(window, cx);
        if self.items.is_empty() {
            self.active = 0;
            if had_focus {
                window.focus(&self.focus_handle, cx);
            }
            cx.emit(PaneEvent::Remove);
        } else if was_active {
            let previous = self.history.last().and_then(|h| self.index_of(*h));
            self.active = previous.unwrap_or(ix.min(self.items.len() - 1));
            self.activated(window, cx);
            if had_focus {
                let handle = self.items[self.active].focus_handle(cx);
                window.focus(&handle, cx);
            }
        } else if ix < self.active {
            self.active -= 1;
        }
        cx.notify();
    }

    pub fn close_active_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_item().map(|i| i.item_id()) {
            self.close_item_by_id(id, window, cx);
        }
    }

    /// Closes every tab but `keep`.
    pub fn close_other_items(&mut self, keep: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let others: Vec<EntityId> = self
            .items
            .iter()
            .map(|i| i.item_id())
            .filter(|id| *id != keep)
            .collect();
        for id in others {
            self.close_item_by_id(id, window, cx);
        }
    }

    pub fn close_all_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let all: Vec<EntityId> = self.items.iter().map(|i| i.item_id()).collect();
        for id in all {
            self.close_item_by_id(id, window, cx);
        }
    }

    pub fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zoomed {
            cx.emit(PaneEvent::ZoomOut);
        } else if !self.items.is_empty() {
            if !self.has_focus(window, cx) {
                window.focus(&self.focus_handle, cx);
            }
            cx.emit(PaneEvent::ZoomIn);
        }
    }

    fn render_tab(
        &self,
        ix: usize,
        item: &dyn ItemHandle,
        focused: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_active = ix == self.active;
        let id = item.item_id();
        let params = TabContentParams {
            selected: is_active,
            deemphasized: !focused,
        };
        let position = if ix == 0 {
            TabPosition::First
        } else if ix + 1 == self.items.len() {
            TabPosition::Last
        } else {
            TabPosition::Middle(ix.cmp(&self.active))
        };
        let close_tooltip = {
            let focus = self.focus_handle.clone();
            // The key only closes the active tab, so only it shows the key.
            move |_: &mut Window, cx: &mut App| {
                if is_active {
                    Tooltip::for_action_in("Close Tab", &CloseActiveItem, &focus, cx)
                } else {
                    Tooltip::simple("Close Tab", cx)
                }
            }
        };
        let tab = Tab::new(ix)
            .position(position)
            .close_side(TabCloseSide::End)
            .toggle_state(is_active)
            .on_click(
                cx.listener(move |pane, _: &ClickEvent, window, cx| pane.activate_item(ix, true, window, cx)),
            )
            .on_aux_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
                if event.is_middle_click() {
                    pane.close_item_by_id(id, window, cx);
                    cx.stop_propagation();
                }
            }))
            .end_slot(
                IconButton::new("close tab", IconName::Close)
                    .shape(IconButtonShape::Square)
                    .icon_color(Color::Muted)
                    .size(ButtonSize::None)
                    .icon_size(IconSize::Small)
                    .visible_on_hover("")
                    .tooltip(close_tooltip)
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.close_item_by_id(id, window, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                h_flex()
                    .id(("pane-tab-content", ix))
                    .gap_1()
                    .children(item.tab_icon(window, cx))
                    .child(item.tab_content(params, window, cx))
                    .when_some(item.tab_tooltip_text(cx), |el, text| {
                        el.tooltip(Tooltip::text(text))
                    }),
            );
        let pane = cx.entity().downgrade();
        let focus = self.focus_handle.clone();
        right_click_menu(("tab-menu", ix))
            .trigger(move |_, _, _| tab)
            .menu(move |window, cx| tab_menu(pane.clone(), id, focus.clone(), window, cx))
            .into_any_element()
    }

    /// Splits with item `id` active, so the copy is of that one.
    fn split_with(
        &mut self,
        id: EntityId,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.index_of(id) {
            self.activate_item(ix, true, window, cx);
        }
        cx.emit(PaneEvent::Split(direction));
    }

    fn render_tab_bar(&self, focused: bool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<AnyElement> = self
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| self.render_tab(ix, item.as_ref(), focused, window, cx))
            .collect();
        let focus = self.focus_handle.clone();
        let split_menu = PopoverMenu::new("split-menu")
            .trigger_with_tooltip(
                IconButton::new("split", IconName::Split).icon_size(IconSize::Small),
                Tooltip::text("Split Pane"),
            )
            .anchor(gpui::Anchor::TopRight)
            .menu(move |window, cx| {
                let focus = focus.clone();
                Some(ContextMenu::build(window, cx, move |menu, _, _| {
                    menu.context(focus)
                        .action("Split Right", SplitRight.boxed_clone())
                        .action("Split Down", SplitDown.boxed_clone())
                }))
            });
        let zoomed = self.zoomed;
        let focus = self.focus_handle.clone();
        let zoom = IconButton::new(
            "toggle-zoom",
            if zoomed {
                IconName::Minimize
            } else {
                IconName::Maximize
            },
        )
        .icon_size(IconSize::Small)
        .toggle_state(zoomed)
        .on_click(cx.listener(|pane, _, window, cx| pane.toggle_zoom(window, cx)))
        .tooltip(move |_, cx| {
            Tooltip::for_action_in(
                if zoomed { "Zoom Out" } else { "Zoom In" },
                &ToggleZoom,
                &focus,
                cx,
            )
        });
        TabBar::new("tab_bar")
            .track_scroll(&self.tab_bar_scroll)
            .children(tabs)
            .end_child(split_menu)
            .end_child(zoom)
    }
}

/// What a tab's menu entry does to its pane, for the tab's item.
type TabMenuRun = fn(&mut Pane, EntityId, &mut Window, &mut Context<Pane>);

/// A tab's right-click menu, as Zed's (the parts ainet has).
fn tab_menu(
    pane: WeakEntity<Pane>,
    id: EntityId,
    focus: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    let entries: [(&str, Box<dyn Action>, TabMenuRun); 5] = [
        ("Close", Box::new(CloseActiveItem), |p, id, w, cx| {
            p.close_item_by_id(id, w, cx)
        }),
        ("Close Others", Box::new(CloseOtherItems), |p, id, w, cx| {
            p.close_other_items(id, w, cx)
        }),
        ("Close All", Box::new(CloseAllItems), |p, _, w, cx| {
            p.close_all_items(w, cx)
        }),
        ("Split Right", Box::new(SplitRight), |p, id, w, cx| {
            p.split_with(id, SplitDirection::Right, w, cx)
        }),
        ("Split Down", Box::new(SplitDown), |p, id, w, cx| {
            p.split_with(id, SplitDirection::Down, w, cx)
        }),
    ];
    ContextMenu::build(window, cx, move |mut menu, _, _| {
        menu = menu.context(focus);
        for (i, (label, action, run)) in entries.into_iter().enumerate() {
            if i == 3 {
                menu = menu.separator();
            }
            let pane = pane.clone();
            menu = menu.entry(label, Some(action), move |window, cx| {
                let _ = pane.update(cx, |pane, cx| run(pane, id, window, cx));
            });
        }
        menu
    })
}

impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.has_focus(window, cx);
        let content = match self.active_item() {
            Some(item) => div().size_full().child(item.to_any()).into_any_element(),
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(cx.theme().colors().editor_background)
                .child(Label::new("No open tabs").color(Color::Placeholder))
                .into_any_element(),
        };
        v_flex()
            .key_context("Pane")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex_none()
            .overflow_hidden()
            .on_action(
                cx.listener(|_, _: &SplitRight, _, cx| cx.emit(PaneEvent::Split(SplitDirection::Right))),
            )
            .on_action(cx.listener(|_, _: &SplitDown, _, cx| cx.emit(PaneEvent::Split(SplitDirection::Down))))
            .on_action(
                cx.listener(|pane, _: &CloseActiveItem, window, cx| pane.close_active_item(window, cx)),
            )
            .on_action(cx.listener(|pane, _: &CloseOtherItems, window, cx| {
                if let Some(id) = pane.active_item().map(|i| i.item_id()) {
                    pane.close_other_items(id, window, cx)
                }
            }))
            .on_action(cx.listener(|pane, _: &CloseAllItems, window, cx| pane.close_all_items(window, cx)))
            .on_action(
                cx.listener(|pane, _: &ActivateNextItem, window, cx| pane.activate_relative(1, window, cx)),
            )
            .on_action(cx.listener(|pane, _: &ActivatePreviousItem, window, cx| {
                pane.activate_relative(-1, window, cx)
            }))
            .on_action(cx.listener(|pane, _: &ToggleZoom, window, cx| pane.toggle_zoom(window, cx)))
            .when(!self.items.is_empty(), |el| {
                el.child(self.render_tab_bar(focused, window, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .child(content),
            )
    }
}
