//! Adding an agent: the spawn modal (`fleet::Spawn`, `s` in the Fleet), a
//! loose agent of a runtime in a folder with a role, as `/spawn` sends;
//! and the grow modal (`fleet::Grow`), a new seat in a rig (pod, member,
//! runtime, role), as `/grow` sends.

use ainet_core::Runtime;
use ainet_model::command;
use gpui::{DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription};
use ui::prelude::*;
use ui_input::InputField;

use crate::fleet::FleetModel;
use crate::form;
use crate::modal_layer::ModalView;

/// The form for `/spawn`.
pub struct SpawnModal {
    fleet: Entity<FleetModel>,
    runtime: Runtime,
    cwd: Entity<InputField>,
    role: Entity<InputField>,
    _subscriptions: Vec<Subscription>,
}

impl SpawnModal {
    pub fn new(fleet: Entity<FleetModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let here = std::env::current_dir().unwrap_or_default();
        let cwd = form::field(
            "Working directory",
            &format!("Where it works (default: {})", here.display()),
            window,
            cx,
        );
        let role = form::field("Role", "What it is for, e.g. code reviewer", window, cx);
        let subscriptions = form::chain(&[cwd.clone(), role.clone()], window, cx, |this, _, cx| {
            this.spawn(cx)
        });
        SpawnModal {
            fleet,
            runtime: Runtime::ClaudeCode,
            cwd,
            role,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_runtime(&mut self, runtime: Runtime, cx: &mut Context<Self>) {
        self.runtime = runtime;
        cx.notify();
    }

    /// Spawns the agent, and closes.
    pub fn spawn(&mut self, cx: &mut Context<Self>) {
        let cwd = form::text(&self.cwd, cx);
        let cmd = command::spawn(
            self.runtime,
            Some(cwd.as_str()).filter(|c| !c.is_empty()),
            &form::text(&self.role, cx),
        );
        self.fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for SpawnModal {}

impl ModalView for SpawnModal {
    /// The keyboard goes back from where the form took it, its first field.
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        window.focus(&self.cwd.focus_handle(cx), cx);
        true
    }
}

impl Focusable for SpawnModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.cwd.focus_handle(cx)
    }
}

impl Render for SpawnModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let body = v_flex()
            .gap_2()
            .child(form::runtime_choice(
                Some(self.runtime),
                None,
                move |runtime, _, cx| {
                    if let Some(runtime) = runtime {
                        let _ = this.update(cx, |this, cx| this.set_runtime(runtime, cx));
                    }
                },
            ))
            .child(form::folder_field("spawn-browse", &self.cwd))
            .child(self.role.clone());
        let button = Button::new("spawn", "Spawn")
            .style(ButtonStyle::Filled)
            .on_click(cx.listener(|this, _, _, cx| this.spawn(cx)));
        let hint = Label::new("enter: next field, then spawn")
            .size(LabelSize::XSmall)
            .color(Color::Muted);
        form::frame(
            "spawn-modal",
            "Spawn an Agent",
            Some("A loose agent, outside any rig.".into()),
            body,
            (Some(hint.into_any_element()), button.into_any_element()),
            vec![self.cwd.clone(), self.role.clone()],
            cx,
        )
    }
}

/// The form for `/grow`: a seat added to a rig.
pub struct GrowModal {
    fleet: Entity<FleetModel>,
    rig: String,
    /// `None`: the rig's own default.
    runtime: Option<Runtime>,
    pod: Entity<InputField>,
    member: Entity<InputField>,
    role: Entity<InputField>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl GrowModal {
    pub fn new(fleet: Entity<FleetModel>, rig: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pod = form::field("Pod", "e.g. dev", window, cx);
        let member = form::field("Member", "e.g. perf", window, cx);
        let role = form::field("Role", "e.g. PERFORMANCE ENGINEER", window, cx);
        let fields = [pod.clone(), member.clone(), role.clone()];
        let subscriptions = form::chain(&fields, window, cx, |this, _, cx| this.grow(cx));
        GrowModal {
            fleet,
            rig,
            runtime: None,
            pod,
            member,
            role,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_runtime(&mut self, runtime: Option<Runtime>, cx: &mut Context<Self>) {
        self.runtime = runtime;
        cx.notify();
    }

    /// Adds the seat and closes; without a pod and a member it says so
    /// and stays.
    pub fn grow(&mut self, cx: &mut Context<Self>) {
        let (pod, member) = (form::text(&self.pod, cx), form::text(&self.member, cx));
        if pod.is_empty() || member.is_empty() {
            self.error = Some("A seat needs a pod and a member.".into());
            cx.notify();
            return;
        }
        let cmd = command::grow(
            &self.rig,
            &pod,
            &member,
            self.runtime,
            &form::text(&self.role, cx),
        );
        self.fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
        cx.emit(DismissEvent);
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }
}

impl EventEmitter<DismissEvent> for GrowModal {}

impl ModalView for GrowModal {
    /// The keyboard goes back from where the form took it, its first field.
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        window.focus(&self.pod.focus_handle(cx), cx);
        true
    }
}

impl Focusable for GrowModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.pod.focus_handle(cx)
    }
}

impl Render for GrowModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(self.pod.clone()))
                    .child(div().flex_1().child(self.member.clone())),
            )
            .child(form::runtime_choice(
                self.runtime,
                Some("rig default"),
                move |runtime, _, cx| {
                    let _ = this.update(cx, |this, cx| this.set_runtime(runtime, cx));
                },
            ))
            .child(self.role.clone())
            .children(form::error(self.error.as_ref()));
        let button = Button::new("grow", "Add Seat")
            .style(ButtonStyle::Filled)
            .on_click(cx.listener(|this, _, _, cx| this.grow(cx)));
        let hint = Label::new("enter: next field, then add")
            .size(LabelSize::XSmall)
            .color(Color::Muted);
        form::frame(
            "grow-modal",
            format!("Add a Seat to {}", self.rig),
            Some("Its agent starts once the seat is added.".into()),
            body,
            (Some(hint.into_any_element()), button.into_any_element()),
            vec![self.pod.clone(), self.member.clone(), self.role.clone()],
            cx,
        )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use std::path::Path;

    use ainet_core::Request;
    use gpui::TestAppContext;

    use super::*;
    use crate::actions::Grow;
    use crate::tests::{demo_workspace, modal, sent};

    #[gpui::test]
    async fn spawn_sends_the_runtime_folder_and_role(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        // `s` in the Fleet, which has the keyboard.
        cx.simulate_keystrokes("s");
        let form = modal::<SpawnModal>(&w, cx).expect("the spawn form");
        form.update(cx, |f, cx| f.set_runtime(Runtime::Codex, cx));
        cx.simulate_input("/tmp");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("code reviewer");
        cx.simulate_keystrokes("enter");
        assert!(modal::<SpawnModal>(&w, cx).is_none());
        match sent(&w, cx).last() {
            Some(Request::Spawn(spec)) => {
                assert_eq!(spec.runtime, Runtime::Codex);
                assert_eq!(spec.cwd, Path::new("/tmp"));
                assert_eq!(spec.role.as_deref(), Some("CODE REVIEWER"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[gpui::test]
    async fn grow_adds_a_seat_to_the_selected_rig(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        cx.dispatch_action(Grow);
        let form = modal::<GrowModal>(&w, cx).expect("the selected agent's rig");
        let before = sent(&w, cx).len();
        cx.simulate_keystrokes("enter enter enter");
        assert!(
            form.read_with(cx, |f, _| f.error().is_some()),
            "a pod and member are needed"
        );
        assert_eq!(sent(&w, cx).len(), before);

        cx.simulate_keystrokes("tab");
        cx.simulate_input("dev");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("perf");
        form.update(cx, |f, cx| f.set_runtime(Some(Runtime::Codex), cx));
        cx.simulate_keystrokes("enter");
        cx.simulate_input("PERFORMANCE ENGINEER");
        cx.simulate_keystrokes("enter");
        assert!(modal::<GrowModal>(&w, cx).is_none());
        match sent(&w, cx).last() {
            Some(Request::Grow { rig, seats, edges }) => {
                assert_eq!(rig, "docs");
                assert!(edges.is_empty());
                assert_eq!((seats[0].pod.as_str(), seats[0].member.as_str()), ("dev", "perf"));
                assert_eq!(seats[0].runtime, Some(Runtime::Codex));
                assert_eq!(seats[0].role.as_deref(), Some("PERFORMANCE ENGINEER"));
            }
            other => panic!("{other:?}"),
        }
    }
}
