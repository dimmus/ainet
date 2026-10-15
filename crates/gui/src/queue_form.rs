//! The queue's small forms, as modals: a note on an item, handing it off
//! to another seat, blocking it on something. Enter in the first field
//! goes to the second, enter in the last sends what `ainet queue note`,
//! `handoff` or `block` sends (`ainet-model`'s `command::queue`); escape
//! gives up. What is missing is said under the fields, and the form
//! stays open to put it right.

use ainet_model::{Command, QueueCommand, command};
use gpui::{DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription};
use ui::prelude::*;

use crate::fleet::FleetModel;
use crate::modal_layer::ModalView;
use crate::text_input::{InputEvent, TextInput};

/// Which form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueForm {
    Note,
    Handoff,
    Block,
}

impl QueueForm {
    fn title(self) -> &'static str {
        match self {
            QueueForm::Note => "Add a Note",
            QueueForm::Handoff => "Hand Off",
            QueueForm::Block => "Block On",
        }
    }

    /// The fields' placeholders: one, or two.
    fn fields(self) -> (&'static str, Option<&'static str>) {
        match self {
            QueueForm::Note => ("What has happened…", None),
            QueueForm::Handoff => ("To which seat (dev-ui@docs)…", Some("What they should do…")),
            QueueForm::Block => (
                "On what: q-N or external:<name>…",
                Some("What to do once unblocked…"),
            ),
        }
    }

    /// The command the fields make.
    fn command(self, first: String, second: String) -> QueueCommand {
        match self {
            QueueForm::Note => QueueCommand::Note(first),
            QueueForm::Handoff => QueueCommand::Handoff {
                to: first,
                body: second,
            },
            QueueForm::Block => QueueCommand::Block {
                on: first,
                continuation: second,
            },
        }
    }
}

pub struct QueueFormModal {
    fleet: Entity<FleetModel>,
    form: QueueForm,
    /// The queue item's id.
    item: String,
    first: Entity<TextInput>,
    second: Option<Entity<TextInput>>,
    /// Why the last try was not sent.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl QueueFormModal {
    pub fn new(
        fleet: Entity<FleetModel>,
        form: QueueForm,
        item: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (first_hint, second_hint) = form.fields();
        let first = cx.new(|cx| TextInput::new(window, cx).with_placeholder(first_hint));
        let second = second_hint.map(|hint| {
            cx.new(|cx| {
                TextInput::new(window, cx)
                    .with_placeholder(hint)
                    .with_multiline(Some(6))
            })
        });
        // Escape needs nothing here: it goes on as `menu::Cancel`, which
        // closes the modal.
        let mut subscriptions =
            vec![
                cx.subscribe_in(&first, window, |this, _, event: &InputEvent, window, cx| {
                    if *event == InputEvent::Submitted {
                        match &this.second {
                            Some(second) => window.focus(&second.focus_handle(cx), cx),
                            None => this.send(cx),
                        }
                    }
                }),
            ];
        if let Some(second) = &second {
            subscriptions.push(cx.subscribe(second, |this, _, event: &InputEvent, cx| {
                if *event == InputEvent::Submitted {
                    this.send(cx);
                }
            }));
        }
        QueueFormModal {
            fleet,
            form,
            item,
            first,
            second,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// Sends what the fields say and closes, or says what is missing.
    fn send(&mut self, cx: &mut Context<Self>) {
        let first = self.first.read(cx).text().to_string();
        let second = self
            .second
            .as_ref()
            .map(|s| s.read(cx).text().to_string())
            .unwrap_or_default();
        let cmd = command::queue(
            &self.fleet.read(cx).store,
            &self.item,
            self.form.command(first, second),
        );
        if let Command::Error(why) = cmd {
            self.error = Some(why.into());
            cx.notify();
            return;
        }
        self.fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
        cx.emit(DismissEvent);
    }

    fn field(input: &Entity<TextInput>, cx: &App) -> Div {
        div()
            .px_2()
            .py_1p5()
            .rounded_sm()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().editor_background)
            .child(input.clone())
    }
}

impl EventEmitter<DismissEvent> for QueueFormModal {}

impl ModalView for QueueFormModal {
    /// The modal layer gives the keyboard back from where the modal took
    /// it, the first field; it may be in the second.
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        window.focus(&self.first.focus_handle(cx), cx);
        true
    }
}

impl Focusable for QueueFormModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.first.focus_handle(cx)
    }
}

impl Render for QueueFormModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self
            .fleet
            .read(cx)
            .store
            .queue
            .get(&self.item)
            .map(|q| q.summary.clone())
            .unwrap_or_default();
        let hint = if self.second.is_some() {
            "enter for the next field, then to send · escape to cancel"
        } else {
            "enter to send · escape to cancel"
        };
        v_flex()
            .id("queue-form")
            .w(rems(34.))
            .elevation_3(cx)
            .overflow_hidden()
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .child(
                        v_flex()
                            .child(Label::new(format!("{} · {}", self.form.title(), self.item)))
                            .child(
                                Label::new(summary)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    )
                    .child(Self::field(&self.first, cx))
                    .children(self.second.as_ref().map(|second| Self::field(second, cx)))
                    .children(
                        self.error
                            .clone()
                            .map(|why| Label::new(why).size(LabelSize::Small).color(Color::Error)),
                    ),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted)),
            )
    }
}
