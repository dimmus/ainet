//! Docks and the panels in them, after Zed's `workspace::dock`: a dock on
//! the left, right or bottom shows one of its [`Panel`]s, can be opened and
//! closed, and is resized by dragging its inner edge (a double click puts
//! the default size back). Sizes are kept in memory for the session, per
//! panel, and start from the panel's default (the Fleet panel's from
//! `fleet_panel.width` in settings); phase 7 saves them with the rest of
//! the workspace.

use gpui::{
    Action, AnyView, App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseUpEvent,
    Pixels, Window, deferred, px,
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
    /// A stable name, for tests and (later) saved layouts.
    fn persistent_name() -> &'static str;

    /// Its size along the dock's axis until resized.
    fn default_size(&self, cx: &App) -> Pixels;

    /// Its button in the status bar.
    fn icon(&self) -> IconName;

    /// The button's tooltip.
    fn icon_tooltip(&self) -> &'static str;

    /// Told when its dock opens or closes with it showing.
    fn set_active(&mut self, _active: bool, _: &mut Window, _: &mut Context<Self>) {}
}

/// A panel of any type, as a dock holds it.
pub trait PanelHandle: 'static {
    fn persistent_name(&self) -> &'static str;
    fn default_size(&self, cx: &App) -> Pixels;
    fn icon(&self, cx: &App) -> IconName;
    fn icon_tooltip(&self, cx: &App) -> &'static str;
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
            .child(div().size_full().overflow_hidden().child(view))
            .child(self.resize_handle(cx))
            .into_any_element()
    }
}
