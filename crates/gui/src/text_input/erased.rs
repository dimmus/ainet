//! [`TextInput`] as Zed's `ui_input::ErasedEditor`, so Zed's `InputField`
//! (labels, icons, errors, masking) works in ainet's forms. Zed registers
//! its `editor` crate there; [`init`] registers ours instead.

use std::any::Any;
use std::sync::Arc;

use gpui::{AnyElement, App, AppContext as _, Entity, FocusHandle, Focusable as _, Subscription, Window};
use ui::IntoElement as _;
use ui_input::{ERASED_EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent};

use super::{InputEvent, TextInput};

/// Makes `InputField::new` build a [`TextInput`]. The factory is
/// process-wide and set once; later calls (tests open many apps) keep it.
pub fn init() {
    let _ = ERASED_EDITOR_FACTORY.set(|window, cx| {
        let input = cx.new(|cx| TextInput::new(window, cx));
        Arc::new(ErasedInput(input))
    });
}

/// A [`TextInput`] behind Zed's editor trait.
pub struct ErasedInput(pub Entity<TextInput>);

impl ErasedEditor for ErasedInput {
    fn text(&self, cx: &App) -> String {
        self.0.read(cx).text().to_string()
    }

    fn set_text(&self, text: &str, _: &mut Window, cx: &mut App) {
        self.0.update(cx, |input, cx| input.set_text(text, cx));
    }

    fn clear(&self, _: &mut Window, cx: &mut App) {
        self.0.update(cx, |input, cx| input.set_text("", cx));
    }

    fn set_placeholder_text(&self, text: &str, _: &mut Window, cx: &mut App) {
        let text = text.to_string();
        self.0.update(cx, |input, cx| input.set_placeholder(text, cx));
    }

    fn move_selection_to_end(&self, _: &mut Window, cx: &mut App) {
        self.0.update(cx, |input, cx| input.move_to_end(cx));
    }

    fn select_all(&self, _: &mut Window, cx: &mut App) {
        self.0.update(cx, |input, cx| input.select_all(cx));
    }

    fn set_masked(&self, masked: bool, _: &mut Window, cx: &mut App) {
        self.0.update(cx, |input, cx| input.set_masked(masked, cx));
    }

    fn set_read_only(&self, read_only: bool, cx: &mut App) {
        self.0.update(cx, |input, cx| input.set_read_only(read_only, cx));
    }

    fn set_multiline(&self, max_lines: Option<usize>, _: &mut Window, cx: &mut App) {
        self.0
            .update(cx, |input, cx| input.set_multiline(Some(max_lines), cx));
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.focus_handle(cx)
    }

    fn subscribe(
        &self,
        mut callback: Box<dyn FnMut(ErasedEditorEvent, &mut Window, &mut App) + 'static>,
        window: &mut Window,
        cx: &mut App,
    ) -> Subscription {
        window.subscribe(
            &self.0,
            cx,
            move |_, event: &InputEvent, window, cx| match event {
                InputEvent::Edited => callback(ErasedEditorEvent::BufferEdited, window, cx),
                InputEvent::Blurred => callback(ErasedEditorEvent::Blurred, window, cx),
                InputEvent::Submitted | InputEvent::Cancelled => {}
            },
        )
    }

    fn render(&self, _: &mut Window, _: &App) -> AnyElement {
        self.0.clone().into_any_element()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};
    use ui_input::InputField;

    use super::*;
    use crate::tests::init_test;

    #[gpui::test]
    async fn zeds_input_field_runs_on_our_text_input(cx: &mut TestAppContext) {
        init_test(cx);
        let (field, cx): (Entity<InputField>, &mut VisualTestContext) =
            cx.add_window_view(|window, cx| InputField::new(window, cx, "Seat name").label("Name"));
        let editor = field.read_with(cx, |f, _| f.editor().clone());
        let input = editor
            .as_any()
            .downcast_ref::<ErasedInput>()
            .expect("our editor")
            .0
            .clone();
        let edits = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|window, cx| {
            let edits = edits.clone();
            std::mem::forget(editor.subscribe(
                Box::new(move |e, _, _| {
                    if e == ErasedEditorEvent::BufferEdited {
                        edits.set(edits.get() + 1)
                    }
                }),
                window,
                cx,
            ));
            window.focus(&field.focus_handle(cx), cx);
        });
        cx.run_until_parked();
        cx.simulate_input("lead");
        assert_eq!(field.read_with(cx, |f, cx| f.text(cx)), "lead");
        assert_eq!(edits.get(), 4);
        input.read_with(cx, |i, _| assert_eq!(i.placeholder.as_ref(), "Seat name"));
        cx.update(|window, cx| editor.clear(window, cx));
        assert!(field.read_with(cx, |f, cx| f.is_empty(cx)));
    }
}
