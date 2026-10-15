//! What the rig and agent modals share: Zed's `InputField`s (over our
//! `TextInput`) where enter moves on to the next field and sends from the
//! last, tab and shift-tab go round the fields (context `Form`), a
//! Browse… button that asks the system for a folder, the runtime choice,
//! and the frame: Zed's `Modal` with a headline, a section and a footer of
//! buttons.

use std::rc::Rc;

use ainet_core::Runtime;
use gpui::{Entity, FocusHandle, Focusable, PathPromptOptions, Subscription};
use ui::{Modal, ModalFooter, ModalHeader, Section, TintColor, prelude::*};
use ui_input::InputField;

use crate::actions::{NextField, PreviousField};
use crate::text_input::erased::ErasedInput;
use crate::text_input::{InputEvent, TextInput};

/// How wide the forms are, as Zed's pickers.
pub const WIDTH: Rems = Rems(34.);

/// A labelled text field.
pub fn field(label: &str, placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<InputField> {
    let label = SharedString::from(label.to_string());
    cx.new(|cx| InputField::new(window, cx, placeholder).label(label))
}

/// The text input inside a field.
pub fn input(field: &Entity<InputField>, cx: &App) -> Entity<TextInput> {
    field
        .read(cx)
        .editor()
        .as_any()
        .downcast_ref::<ErasedInput>()
        .expect("fields are TextInputs (text_input::erased::init)")
        .0
        .clone()
}

/// What a field says, trimmed.
pub fn text(field: &Entity<InputField>, cx: &App) -> String {
    field.read(cx).text(cx).trim().to_string()
}

/// Puts `text` in a field.
pub fn set_text(field: &Entity<InputField>, text: &str, cx: &mut App) {
    input(field, cx).update(cx, |input, cx| {
        input.set_text(text, cx);
        input.move_to_end(cx);
    });
}

/// Enter in each field moves to the next one; in the last it calls
/// `submit`.
pub fn chain<V: 'static>(
    fields: &[Entity<InputField>],
    window: &mut Window,
    cx: &mut Context<V>,
    submit: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Vec<Subscription> {
    let submit = Rc::new(submit);
    let handles: Vec<FocusHandle> = fields.iter().map(|f| f.focus_handle(cx)).collect();
    fields
        .iter()
        .enumerate()
        .map(|(ix, field)| {
            let (submit, next) = (submit.clone(), handles.get(ix + 1).cloned());
            let input = input(field, cx);
            cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| {
                if *event == InputEvent::Submitted {
                    match &next {
                        Some(next) => window.focus(next, cx),
                        None => submit(this, window, cx),
                    }
                }
            })
        })
        .collect()
}

/// Moves the keyboard `delta` fields on from the one that has it,
/// round the form.
pub fn cycle(fields: &[Entity<InputField>], delta: isize, window: &mut Window, cx: &mut App) {
    let handles: Vec<FocusHandle> = fields.iter().map(|f| f.focus_handle(cx)).collect();
    if handles.is_empty() {
        return;
    }
    let at = handles
        .iter()
        .position(|h| h.contains_focused(window, cx))
        .unwrap_or(0) as isize;
    let next = (at + delta).rem_euclid(handles.len() as isize) as usize;
    window.focus(&handles[next], cx);
}

/// Asks the system for a folder; the one picked goes in `field`.
pub fn browse(field: &Entity<InputField>, window: &mut Window, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose".into()),
    });
    let field = field.clone();
    window
        .spawn(cx, async move |cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(dir) = paths.first()
            {
                let dir = dir.display().to_string();
                let _ = cx.update(|_, cx| set_text(&field, &dir, cx));
            }
        })
        .detach();
}

/// A field with a Browse… button beside it, for a folder.
pub fn folder_field(id: &'static str, field: &Entity<InputField>) -> impl IntoElement {
    let target = field.clone();
    h_flex()
        .w_full()
        .gap_1()
        .items_end()
        .child(div().flex_1().child(field.clone()))
        .child(
            Button::new(id, "Browse…")
                .style(ButtonStyle::Outlined)
                .on_click(move |_, window, cx| browse(&target, window, cx)),
        )
}

/// The runtimes a seat can have, with OpenRig's names.
pub const RUNTIMES: [Runtime; 3] = [Runtime::ClaudeCode, Runtime::Codex, Runtime::Shell];

/// A row of buttons to pick a runtime; with `default`, a first one for
/// "the rig's own" (`None`).
pub fn runtime_choice(
    picked: Option<Runtime>,
    default: Option<&'static str>,
    on_pick: impl Fn(Option<Runtime>, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_pick = Rc::new(on_pick);
    let choices = default
        .map(|label| (None, label))
        .into_iter()
        .chain(RUNTIMES.iter().map(|r| (Some(*r), r.spec_name())));
    v_flex()
        .gap_1()
        .child(Label::new("Runtime").size(LabelSize::Small))
        .child(h_flex().gap_1().children(choices.map(|(runtime, label)| {
            let on_pick = on_pick.clone();
            Button::new(SharedString::from(format!("runtime-{label}")), label)
                .style(ButtonStyle::Subtle)
                .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                .toggle_state(runtime == picked)
                .on_click(move |_, window, cx| on_pick(runtime, window, cx))
        })))
}

/// The frame of a form: Zed's modal with a headline and description, the
/// fields in a section, and a footer (a hint at the start, buttons at the
/// end). Tab and shift-tab go round `fields`.
pub fn frame(
    id: &'static str,
    headline: impl Into<SharedString>,
    description: Option<SharedString>,
    body: impl IntoElement,
    footer: (Option<AnyElement>, AnyElement),
    fields: Vec<Entity<InputField>>,
    cx: &App,
) -> Div {
    let backward = fields.clone();
    let header = ModalHeader::new().headline(headline);
    let header = match description {
        Some(description) => header.description(description),
        None => header,
    };
    v_flex()
        .key_context("Form")
        .w(WIDTH)
        .elevation_3(cx)
        .overflow_hidden()
        .on_action(move |_: &NextField, window, cx| cycle(&fields, 1, window, cx))
        .on_action(move |_: &PreviousField, window, cx| cycle(&backward, -1, window, cx))
        .child(
            Modal::new(id, None)
                .header(header)
                .section(Section::new().child(body))
                .footer(
                    ModalFooter::new()
                        .start_slot::<AnyElement>(footer.0)
                        .end_slot(footer.1),
                ),
        )
}

/// A form's error line.
pub fn error(why: Option<&SharedString>) -> Option<impl IntoElement> {
    why.map(|why| Label::new(why.clone()).size(LabelSize::Small).color(Color::Error))
}

/// Seat ids typed in a field: separated by commas or spaces.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seat_lists_split_on_commas_and_spaces() {
        assert_eq!(
            words(" dev.impl, dev.review  orch.lead,"),
            ["dev.impl", "dev.review", "orch.lead"]
        );
        assert!(words(" , ").is_empty());
    }
}
