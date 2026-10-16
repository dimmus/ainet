//! One modal at a time over the workspace, after Zed's
//! `workspace::modal_layer`: opening one takes the keyboard, and escape
//! (`menu::Cancel`), a click outside it or the modal's own `DismissEvent`
//! closes it and gives the keyboard back to where it was. Toggling the
//! modal that is open closes it, as Zed's pickers do on their key.

use gpui::{AnyView, DismissEvent, Entity, FocusHandle, ManagedView, MouseButton, Subscription};
use ui::prelude::*;

/// A view the modal layer can show.
pub trait ModalView: ManagedView {
    /// Asked before closing; false keeps it open.
    fn on_before_dismiss(&mut self, _: &mut Window, _: &mut Context<Self>) -> bool {
        true
    }
}

trait ModalViewHandle {
    fn on_before_dismiss(&self, window: &mut Window, cx: &mut App) -> bool;
    fn view(&self) -> AnyView;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
}

impl<V: ModalView> ModalViewHandle for Entity<V> {
    fn on_before_dismiss(&self, window: &mut Window, cx: &mut App) -> bool {
        self.update(cx, |modal, cx| modal.on_before_dismiss(window, cx))
    }

    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }
}

struct ActiveModal {
    modal: Box<dyn ModalViewHandle>,
    /// Where the keyboard goes back to.
    previous_focus: Option<FocusHandle>,
    _dismissed: Subscription,
}

#[derive(Default)]
pub struct ModalLayer {
    active: Option<ActiveModal>,
}

impl ModalLayer {
    pub fn new() -> Self {
        ModalLayer::default()
    }

    /// Opens a modal of type `V`, or closes it if one is open; another
    /// kind of modal that is open is closed first.
    pub fn toggle_modal<V, B>(&mut self, window: &mut Window, cx: &mut Context<Self>, build: B)
    where
        V: ModalView,
        B: FnOnce(&mut Window, &mut Context<V>) -> V,
    {
        let mut previous_focus = window.focused(cx);
        if let Some(active) = &self.active {
            let same = active.modal.view().downcast::<V>().is_ok();
            previous_focus = active.previous_focus.clone();
            if !self.hide_modal(window, cx) || same {
                return;
            }
        }
        let modal = cx.new(|cx| build(window, cx));
        let dismissed = cx.subscribe_in(&modal, window, |this, _, _: &DismissEvent, window, cx| {
            this.hide_modal(window, cx);
        });
        let focus = modal.read(cx).focus_handle(cx);
        self.active = Some(ActiveModal {
            modal: Box::new(modal),
            previous_focus,
            _dismissed: dismissed,
        });
        // Once it is drawn, as Zed does.
        cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    /// Closes the open modal unless it refuses; true if it closed.
    pub fn hide_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(active) = &self.active else {
            return false;
        };
        if !active.modal.on_before_dismiss(window, cx) {
            return false;
        }
        let active = self.active.take().expect("checked above");
        let modal_focus = active.modal.focus_handle(cx);
        if let Some(previous) = active.previous_focus
            && (modal_focus.contains_focused(window, cx) || window.focused(cx).is_none())
        {
            window.focus(&previous, cx);
        }
        cx.notify();
        true
    }

    /// The open modal, if it is a `V`.
    pub fn active_modal<V: ModalView>(&self) -> Option<Entity<V>> {
        self.active.as_ref()?.modal.view().downcast::<V>().ok()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_active_modal(&self) -> bool {
        self.active.is_some()
    }
}

impl Render for ModalLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(active) = &self.active else {
            return div().into_any_element();
        };
        div()
            .id("modal-layer")
            .absolute()
            .size_full()
            .inset_0()
            .occlude()
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                this.hide_modal(window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.hide_modal(window, cx);
                }),
            )
            .child(
                v_flex().h(px(0.)).top_20().items_center().child(
                    h_flex()
                        .occlude()
                        .child(active.modal.view())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                ),
            )
            .into_any_element()
    }
}
