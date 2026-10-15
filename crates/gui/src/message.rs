//! Writing to one agent (`m` in the Fleet, or its menu): a modal with the
//! agent's critter and a text field. Enter sends what the TUI's `@Name
//! text` line sends (`ainet-model`'s `command::message_to`), always to this
//! agent: delivered into its terminal, through any typing guard, and kept
//! in its mailbox.
//! Shift-enter starts a new line; escape gives up.

use ainet_model::command;
use gpui::{DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription};
use ui::prelude::*;

use crate::critter::Critter;
use crate::fleet::FleetModel;
use crate::modal_layer::ModalView;
use crate::text_input::{InputEvent, TextInput};

pub struct MessageModal {
    fleet: Entity<FleetModel>,
    agent: String,
    input: Entity<TextInput>,
    _subscription: Subscription,
}

impl MessageModal {
    /// A message to agent `agent` (its id).
    pub fn new(
        fleet: Entity<FleetModel>,
        agent: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let label = fleet
            .read(cx)
            .store
            .agents
            .get(&agent)
            .map(|a| a.label())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .with_placeholder(format!("Message {label}…"))
                .with_multiline(Some(8))
        });
        // Escape needs nothing here: it goes on as `menu::Cancel`, which
        // closes the modal.
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if *event == InputEvent::Submitted {
                this.send(cx);
            }
        });
        MessageModal {
            fleet,
            agent,
            input,
            _subscription: subscription,
        }
    }

    /// Sends the text, if there is any, and closes.
    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let agent = self.agent.clone();
        self.fleet.update(cx, |fleet, cx| {
            // To this agent whatever the text says: a leading `@word` is
            // the command line's address, not the modal's.
            let sent = match fleet.store.agents.get(&agent) {
                Some(a) => command::message_to(a, text),
                None => ainet_model::Command::Error("no such agent".into()),
            };
            fleet.run(sent, cx);
        });
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for MessageModal {}

impl ModalView for MessageModal {}

impl Focusable for MessageModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for MessageModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fleet = self.fleet.read(cx);
        let title = fleet.store.agents.get(&self.agent).map(|a| {
            h_flex()
                .gap_2()
                .child(Critter::agent(a, fleet.tick(), cx).size(px(32.)))
                .child(
                    v_flex()
                        .child(Label::new(format!("Message {}", a.name)))
                        .child(Label::new(a.label()).size(LabelSize::Small).color(Color::Muted)),
                )
        });
        v_flex()
            .w(rems(34.))
            .elevation_3(cx)
            .overflow_hidden()
            .child(
                v_flex().gap_2().p_3().children(title).child(
                    div()
                        .px_2()
                        .py_1p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(cx.theme().colors().border_focused)
                        .bg(cx.theme().colors().editor_background)
                        .child(self.input.clone()),
                ),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(
                        Label::new("enter to send · shift-enter for a new line · escape to cancel")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
    }
}
