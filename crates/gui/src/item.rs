//! What a pane shows as tabs, after Zed's `workspace::item`: an [`Item`] is
//! a view with a tab (an icon or critter slot, a label, a tooltip) that can
//! be closed, maybe split into a second pane, and is told when its tab
//! becomes active or stops being active. Terminals, Comms, the Inbox, the
//! Map and reports are items; [`Placeholder`] stands in for views still to
//! come (Review) and is the first tab.

use gpui::{
    AnyElement, AnyView, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Subscription,
    Window,
};
use ui::prelude::*;

use crate::args::View;

/// How a tab is drawn: whether it is the pane's active tab, and whether its
/// pane has the keyboard (Zed draws the others' labels muted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabContentParams {
    pub selected: bool,
    pub deemphasized: bool,
}

impl TabContentParams {
    /// The label colour Zed uses for a tab in this state.
    pub fn text_color(&self) -> Color {
        if self.deemphasized {
            if self.selected {
                Color::Muted
            } else {
                Color::Hidden
            }
        } else if self.selected {
            Color::Default
        } else {
            Color::Muted
        }
    }
}

/// What an item tells its pane.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemEvent {
    /// Close my tab.
    CloseItem,
    /// My tab's label, icon or tooltip changed.
    UpdateTab,
}

pub trait Item: Focusable + Render + EventEmitter<ItemEvent> + Sized {
    /// The tab's words.
    fn tab_content_text(&self, cx: &App) -> SharedString;

    /// The tab's label; by default [`Self::tab_content_text`] in the tab's
    /// colour.
    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> AnyElement {
        Label::new(self.tab_content_text(cx))
            .color(params.text_color())
            .into_any_element()
    }

    /// What goes before the label: an icon, or an agent's critter.
    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        None
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        None
    }

    /// Whether splitting the pane copies this item into the new pane.
    fn can_split(&self) -> bool {
        false
    }

    /// The copy a split puts in the new pane, if [`Self::can_split`].
    fn clone_on_split(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Entity<Self>> {
        None
    }

    /// Its tab became the active one.
    fn activated(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    /// Its tab stopped being the active one.
    fn deactivated(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    /// Its tab was closed.
    fn on_removed(&mut self, _: &mut Window, _: &mut Context<Self>) {}
}

/// What hears an item's events: its id and the event.
pub type ItemEventHandler = Box<dyn Fn(EntityId, ItemEvent, &mut Window, &mut App)>;

/// An item of any type, as a pane holds it.
pub trait ItemHandle: 'static {
    fn item_id(&self) -> EntityId;
    fn tab_content(&self, params: TabContentParams, window: &Window, cx: &App) -> AnyElement;
    #[cfg_attr(not(test), allow(dead_code))]
    fn tab_content_text(&self, cx: &App) -> SharedString;
    fn tab_icon(&self, window: &Window, cx: &App) -> Option<AnyElement>;
    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString>;
    fn can_split(&self, cx: &App) -> bool;
    fn clone_on_split(&self, window: &mut Window, cx: &mut App) -> Option<Box<dyn ItemHandle>>;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    fn on_removed(&self, window: &mut Window, cx: &mut App);
    fn to_any(&self) -> AnyView;
    fn boxed_clone(&self) -> Box<dyn ItemHandle>;
    /// Calls `f` with every event the item emits.
    fn subscribe(&self, window: &mut Window, cx: &mut App, f: ItemEventHandler) -> Subscription;
}

impl<T: Item> ItemHandle for Entity<T> {
    fn item_id(&self) -> EntityId {
        self.entity_id()
    }

    fn tab_content(&self, params: TabContentParams, window: &Window, cx: &App) -> AnyElement {
        self.read(cx).tab_content(params, window, cx)
    }

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.read(cx).tab_content_text(cx)
    }

    fn tab_icon(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        self.read(cx).tab_icon(window, cx)
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).tab_tooltip_text(cx)
    }

    fn can_split(&self, cx: &App) -> bool {
        self.read(cx).can_split()
    }

    fn clone_on_split(&self, window: &mut Window, cx: &mut App) -> Option<Box<dyn ItemHandle>> {
        self.update(cx, |item, cx| item.clone_on_split(window, cx))
            .map(|e| Box::new(e) as Box<dyn ItemHandle>)
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |item, cx| {
            if active {
                item.activated(window, cx)
            } else {
                item.deactivated(window, cx)
            }
        })
    }

    fn on_removed(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |item, cx| item.on_removed(window, cx))
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn boxed_clone(&self) -> Box<dyn ItemHandle> {
        Box::new(self.clone())
    }

    fn subscribe(&self, window: &mut Window, cx: &mut App, f: ItemEventHandler) -> Subscription {
        window.subscribe(self, cx, move |item, event: &ItemEvent, window, cx| {
            f(item.entity_id(), *event, window, cx)
        })
    }
}

/// Where a view of phase 4 will be: a tab with its name, and a word on
/// what it will show. Splits into a copy, as Zed's editors do.
pub struct Placeholder {
    view: View,
    focus: FocusHandle,
    /// Times activated, for tests of the hooks.
    pub activations: usize,
}

impl Placeholder {
    pub fn new(view: View, cx: &mut Context<Self>) -> Self {
        Placeholder {
            view,
            focus: cx.focus_handle(),
            activations: 0,
        }
    }

    fn icon(&self) -> IconName {
        match self.view {
            View::Comms => IconName::Chat,
            View::Inbox => IconName::Bell,
            View::Map => IconName::Blocks,
            View::Review => IconName::Diff,
            View::Fleet | View::Terminals { .. } | View::Named(_) => IconName::Terminal,
        }
    }

    fn blurb(&self) -> &'static str {
        match self.view {
            View::Comms => "Messages and chat between agents will stream here",
            View::Inbox => "Requests waiting for you will be answered here",
            View::Map => "The fleet's map will be drawn here",
            View::Review => "Changes to review will be listed here",
            View::Fleet | View::Terminals { .. } | View::Named(_) => {
                "Terminals, Comms and the Inbox open here as tabs"
            }
        }
    }
}

impl EventEmitter<ItemEvent> for Placeholder {}

impl Focusable for Placeholder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Item for Placeholder {
    fn tab_content_text(&self, _: &App) -> SharedString {
        self.view.title().into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        Some(
            Icon::new(self.icon())
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some(format!("{} (coming in a later phase)", self.view.title()).into())
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Self>> {
        let view = self.view.clone();
        Some(cx.new(|cx| Placeholder::new(view, cx)))
    }

    fn activated(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.activations += 1;
    }
}

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("placeholder")
            .track_focus(&self.focus)
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .bg(cx.theme().colors().editor_background)
            .child(Label::new(self.blurb()).color(Color::Muted))
            .child(
                Label::new("Select an agent in the Fleet")
                    .size(LabelSize::Small)
                    .color(Color::Placeholder),
            )
    }
}
