//! Docks and the panels in them, after Zed's `workspace::dock`: a dock on
//! the left, right or bottom holds one or more [`Panel`]s and shows one of
//! them (each has its button in the status bar, as Zed's), can be opened
//! and closed, and is resized by dragging its inner edge (a double click
//! puts the default size back). Sizes are kept per panel, start from the
//! panel's default (the Fleet panel's from `fleet_panel.width` in
//! settings), and are saved with the rest of the workspace
//! (`persistence`).

use gpui::{
    Action, AnyView, App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseUpEvent,
    Pixels, StyleRefinement, Window, deferred, px,
};
use ui::prelude::*;

use crate::actions::{ToggleBottomDock, ToggleLeftDock, ToggleRightDock};

/// The grab area along a dock's inner edge, as Zed's.
pub const RESIZE_HANDLE_SIZE: Pixels = px(6.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

/// What a dock's edge drags, for the workspace to resize it.
#[derive(Clone)]
pub struct DraggedDock(pub DockPosition);

impl Render for DraggedDock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub trait Panel: Focusable + Render + Sized {
    /// A stable name, for tests and saved workspaces.
    fn persistent_name() -> &'static str;

    /// Its size along the dock's axis until resized.
    fn default_size(&self, cx: &App) -> Pixels;

    /// Its button in the status bar.
    fn icon(&self) -> IconName;

    /// The button's tooltip.
    fn icon_tooltip(&self) -> &'static str;

    /// What its button does when the panel is not the one showing: shows
    /// it and gives it the keyboard (Zed's `ToggleFocus`).
    fn toggle_action(&self) -> Box<dyn Action>;

    /// A count on its button while another panel shows, as Zed's.
    fn icon_label(&self, _: &App) -> Option<usize> {
        None
    }

    /// Told when its dock opens or closes with it showing.
    fn set_active(&mut self, _active: bool, _: &mut Window, _: &mut Context<Self>) {}
}

/// A panel of any type, as a dock holds it.
pub trait PanelHandle: 'static {
    fn persistent_name(&self) -> &'static str;
    fn default_size(&self, cx: &App) -> Pixels;
    fn icon(&self, cx: &App) -> IconName;
    fn icon_tooltip(&self, cx: &App) -> &'static str;
    fn toggle_action(&self, cx: &App) -> Box<dyn Action>;
    fn icon_label(&self, cx: &App) -> Option<usize>;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    fn to_any(&self) -> AnyView;
}

impl<T: Panel> PanelHandle for Entity<T> {
    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
    }

    fn default_size(&self, cx: &App) -> Pixels {
        self.read(cx).default_size(cx)
    }

    fn icon(&self, cx: &App) -> IconName {
        self.read(cx).icon()
    }

    fn icon_tooltip(&self, cx: &App) -> &'static str {
        self.read(cx).icon_tooltip()
    }

    fn toggle_action(&self, cx: &App) -> Box<dyn Action> {
        self.read(cx).toggle_action()
    }

    fn icon_label(&self, cx: &App) -> Option<usize> {
        self.read(cx).icon_label(cx)
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |panel, cx| panel.set_active(active, window, cx))
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }
}

struct PanelEntry {
    panel: Box<dyn PanelHandle>,
    /// Set once dragged; until then the panel's default.
    size: Option<Pixels>,
}

/// What a dock holds, apart from where it is: moved whole between the left
/// and right docks when the Fleet panel changes sides.
pub struct DockContents {
    panels: Vec<PanelEntry>,
    active: usize,
    open: bool,
}

pub struct Dock {
    position: DockPosition,
    contents: DockContents,
    focus_handle: FocusHandle,
}

impl Dock {
    pub fn new(position: DockPosition, cx: &mut Context<Self>) -> Self {
        Dock {
            position,
            contents: DockContents {
                panels: Vec::new(),
                active: 0,
                open: false,
            },
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn add_panel<T: Panel>(&mut self, panel: Entity<T>, cx: &mut Context<Self>) {
        self.contents.panels.push(PanelEntry {
            panel: Box::new(panel),
            size: None,
        });
        cx.notify();
    }

    pub fn position(&self) -> DockPosition {
        self.position
    }

    /// The action that opens and closes this dock, for key hints.
    pub fn toggle_action(&self) -> Box<dyn Action> {
        match self.position {
            DockPosition::Left => Box::new(ToggleLeftDock),
            DockPosition::Right => Box::new(ToggleRightDock),
            DockPosition::Bottom => Box::new(ToggleBottomDock),
        }
    }

    pub fn is_open(&self) -> bool {
        self.contents.open && !self.contents.panels.is_empty()
    }

    /// Opens or closes the dock, telling the panel showing.
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.contents.open == open {
            return;
        }
        self.contents.open = open;
        if let Some(entry) = self.contents.panels.get(self.contents.active) {
            entry.panel.set_active(open, window, cx);
        }
        cx.notify();
    }

    /// Its panels, in order.
    pub fn panels(&self) -> impl Iterator<Item = &dyn PanelHandle> {
        self.contents.panels.iter().map(|e| e.panel.as_ref())
    }

    /// Where the panel named `name` is among its panels.
    pub fn panel_index(&self, name: &str) -> Option<usize> {
        self.panels().position(|p| p.persistent_name() == name)
    }

    pub fn active_index(&self) -> usize {
        self.contents.active
    }

    /// Shows panel `ix` in place of the one showing, telling both while
    /// the dock is open.
    pub fn activate_panel(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.contents.active || ix >= self.contents.panels.len() {
            return;
        }
        if self.contents.open {
            self.contents.panels[self.contents.active]
                .panel
                .set_active(false, window, cx);
        }
        self.contents.active = ix;
        if self.contents.open {
            self.contents.panels[ix].panel.set_active(true, window, cx);
        }
        cx.notify();
    }

    pub fn active_panel(&self) -> Option<&dyn PanelHandle> {
        self.contents
            .panels
            .get(self.contents.active)
            .map(|e| e.panel.as_ref())
    }

    /// The name of the panel showing, if any.
    pub fn active_panel_name(&self) -> Option<&'static str> {
        self.active_panel().map(|p| p.persistent_name())
    }

    /// The showing panel's size along the dock's axis.
    pub fn active_size(&self, cx: &App) -> Option<Pixels> {
        let entry = self.contents.panels.get(self.contents.active)?;
        Some(entry.size.unwrap_or_else(|| entry.panel.default_size(cx)))
    }

    /// Resizes the showing panel.
    pub fn resize_active(&mut self, size: Option<Pixels>, cx: &mut Context<Self>) {
        if let Some(entry) = self.contents.panels.get_mut(self.contents.active) {
            entry.size = size.map(|s| s.max(RESIZE_HANDLE_SIZE).round());
            cx.notify();
        }
    }

    /// Each panel's size if it was dragged to one, by name.
    pub fn panel_sizes(&self) -> impl Iterator<Item = (&'static str, Option<Pixels>)> + '_ {
        self.contents
            .panels
            .iter()
            .map(|e| (e.panel.persistent_name(), e.size))
    }

    /// Sizes the panel named `name` (a saved size), if it is here.
    pub fn set_panel_size(&mut self, name: &str, size: Pixels, cx: &mut Context<Self>) {
        if let Some(entry) = self
            .contents
            .panels
            .iter_mut()
            .find(|e| e.panel.persistent_name() == name)
        {
            entry.size = Some(size.max(RESIZE_HANDLE_SIZE).round());
            cx.notify();
        }
    }

    /// Hands over what the dock holds, leaving it empty and closed.
    pub fn take_contents(&mut self, cx: &mut Context<Self>) -> DockContents {
        cx.notify();
        std::mem::replace(
            &mut self.contents,
            DockContents {
                panels: Vec::new(),
                active: 0,
                open: false,
            },
        )
    }

    pub fn set_contents(&mut self, contents: DockContents, cx: &mut Context<Self>) {
        self.contents = contents;
        cx.notify();
    }

    fn resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.position;
        let handle = div()
            .id("resize-handle")
            .debug_selector(move || format!("{position:?}-dock-resize-handle"))
            .on_drag(DraggedDock(position), |dock, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| dock.clone())
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|dock, e: &MouseUpEvent, _, cx| {
                    if e.click_count == 2 {
                        dock.resize_active(None, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .occlude();
        deferred(match position {
            DockPosition::Left => handle
                .absolute()
                .right(-RESIZE_HANDLE_SIZE / 2.)
                .top(px(0.))
                .h_full()
                .w(RESIZE_HANDLE_SIZE)
                .cursor_col_resize(),
            DockPosition::Bottom => handle
                .absolute()
                .top(-RESIZE_HANDLE_SIZE / 2.)
                .left(px(0.))
                .w_full()
                .h(RESIZE_HANDLE_SIZE)
                .cursor_row_resize(),
            DockPosition::Right => handle
                .absolute()
                .top(px(0.))
                .left(-RESIZE_HANDLE_SIZE / 2.)
                .h_full()
                .w(RESIZE_HANDLE_SIZE)
                .cursor_col_resize(),
        })
    }
}

impl Focusable for Dock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Dock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (Some(entry), true) = (self.contents.panels.get(self.contents.active), self.contents.open) else {
            return div()
                .id("dock")
                .track_focus(&self.focus_handle)
                .into_any_element();
        };
        let size = entry.size.unwrap_or_else(|| entry.panel.default_size(cx));
        let view = entry.panel.to_any();
        let position = self.position;
        div()
            .id("dock")
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_none()
            .bg(cx.theme().colors().panel_background)
            .border_color(cx.theme().colors().border)
            .map(|el| match position {
                DockPosition::Left => el.w(size).h_full().border_r_1(),
                DockPosition::Right => el.w(size).h_full().border_l_1(),
                DockPosition::Bottom => el.h(size).w_full().border_t_1(),
            })
            // Cached: a panel draws again only when it changed.
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .child(view.cached(StyleRefinement::default().size_full())),
            )
            .child(self.resize_handle(cx))
            .into_any_element()
    }
}
