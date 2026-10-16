//! Bringing a rig up (`fleet::Up`), in two steps as Zed's pickers lead to
//! forms: a [`SpecPicker`] over the spec library (`Request::Specs`), where
//! a spec file's path or any name can be typed too, then the [`UpModal`]:
//! where the seats work (Browse… asks the system for a folder), a new name
//! for the rig, and seats to start fresh. Plan shows what would launch
//! (`up --plan`) in the form; Launch sends what `/up` sends.

use std::sync::Arc;

use ainet_core::Request;
use ainet_core::protocol::SpecEntry;
use ainet_model::{Command, Listing, command};
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Task, WeakEntity};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use ui_input::InputField;

use crate::fleet::FleetModel;
use crate::form;
use crate::modal_layer::ModalView;
use crate::picker::{Picker, PickerDelegate, match_strings};
use crate::workspace::Workspace;

/// The spec library, to pick what to bring up.
pub struct SpecPicker {
    fleet: Entity<FleetModel>,
    workspace: WeakEntity<Workspace>,
    /// The library once ainetd has listed it, or why it could not.
    specs: Option<Result<Vec<SpecEntry>, String>>,
    candidates: Arc<Vec<StringMatchCandidate>>,
    matches: Vec<StringMatch>,
    selected: usize,
    /// What is typed, offered as a source of its own (a spec file, or a
    /// name the library does not list), after the matches.
    typed: Option<String>,
}

impl SpecPicker {
    /// A picker that asks ainetd for the library and lists it once it
    /// comes.
    pub fn open(
        fleet: Entity<FleetModel>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Picker<Self> {
        let picker = cx.entity().downgrade();
        let window_handle = window.window_handle();
        fleet.update(cx, |fleet, cx| {
            fleet.list(
                Request::Specs,
                move |listing, cx| {
                    // Later: the fleet is busy handing this over.
                    cx.defer(move |cx| {
                        let _ = window_handle.update(cx, |_, window, cx| {
                            let _ = picker.update(cx, |picker, cx| {
                                picker.delegate.listed(listing);
                                picker.refresh(window, cx);
                            });
                        });
                    });
                },
                cx,
            )
        });
        let delegate = SpecPicker {
            fleet,
            workspace,
            specs: None,
            candidates: Arc::default(),
            matches: Vec::new(),
            selected: 0,
            typed: None,
        };
        Picker::uniform_list(delegate, window, cx)
    }

    fn listed(&mut self, listing: Result<Listing, String>) {
        let specs = match listing {
            Ok(Listing::Specs(specs)) => Ok(specs),
            Ok(_) => Err("ainetd answered with something else".into()),
            Err(e) => Err(e),
        };
        if let Ok(specs) = &specs {
            self.candidates = Arc::new(
                specs
                    .iter()
                    .enumerate()
                    .map(|(ix, s)| StringMatchCandidate::new(ix, &s.name))
                    .collect(),
            );
        }
        self.specs = Some(specs);
    }

    fn spec(&self, ix: usize) -> Option<&SpecEntry> {
        let m = self.matches.get(ix)?;
        self.specs.as_ref()?.as_ref().ok()?.get(m.candidate_id)
    }
}

impl PickerDelegate for SpecPicker {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len() + usize::from(self.typed.is_some())
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = ix;
    }

    fn placeholder_text(&self, _: &App) -> SharedString {
        "Bring up a library spec, or type a spec file's path…".into()
    }

    fn no_matches_text(&self, _: &App) -> SharedString {
        match &self.specs {
            None => "Asking ainetd for the spec library…".into(),
            Some(Err(why)) => why.clone().into(),
            Some(Ok(_)) => "The library is empty: type a spec file's path".into(),
        }
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let found = match_strings(self.candidates.clone(), query.trim().to_string(), cx);
        cx.spawn(async move |picker, cx| {
            let matches = found.await;
            let _ = picker.update(cx, |picker, cx| {
                let this = &mut picker.delegate;
                let query = query.trim();
                let listed = matches.iter().any(|m| m.string == query);
                this.typed = (!query.is_empty() && !listed).then(|| query.to_string());
                this.matches = matches;
                this.selected = 0;
                cx.notify();
            });
        })
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let (source, summary) = match self.spec(self.selected) {
            Some(spec) => (spec.name.clone(), spec.summary.clone()),
            None => match &self.typed {
                Some(typed) => (typed.clone(), None),
                None => return,
            },
        };
        // The form takes the picker's place (and where the keyboard goes
        // back to), once the picker is done.
        let (workspace, fleet) = (self.workspace.clone(), self.fleet.clone());
        window.defer(cx, move |window, cx| {
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.toggle_modal(window, cx, |window, cx| {
                    UpModal::new(fleet, source, summary, window, cx)
                })
            });
        });
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let item = ListItem::new(ix)
            .inset(true)
            .spacing(ListItemSpacing::Sparse)
            .toggle_state(selected);
        let Some(spec) = self.spec(ix) else {
            let typed = self.typed.as_deref()?;
            return Some(
                item.start_slot(Icon::new(IconName::File).color(Color::Muted))
                    .child(Label::new(format!("Bring up “{typed}”")))
                    .end_slot(
                        Label::new("a spec file, library name or rig")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            );
        };
        let m = &self.matches[ix];
        let summary = spec.summary.as_deref().and_then(|s| s.lines().next());
        Some(
            item.child(
                v_flex()
                    .child(HighlightedLabel::new(m.string.clone(), m.positions.clone()))
                    .children(summary.map(|s| {
                        Label::new(s.to_string())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate()
                    })),
            )
            .end_slot(
                Label::new(format!("{} seats · {}", spec.seats, spec.source))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            ),
        )
    }
}

/// Where the plan stands.
enum Plan {
    None,
    Asking,
    Shown(Result<String, String>),
}

/// The form for `/up`: the spec picked, then where, as what, and which
/// seats start anew.
pub struct UpModal {
    fleet: Entity<FleetModel>,
    source: String,
    summary: Option<String>,
    cwd: Entity<InputField>,
    name: Entity<InputField>,
    fresh: Entity<InputField>,
    plan: Plan,
    _subscriptions: Vec<Subscription>,
}

impl UpModal {
    pub fn new(
        fleet: Entity<FleetModel>,
        source: String,
        summary: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file = std::path::Path::new(&source).exists();
        let here = std::env::current_dir().unwrap_or_default();
        let cwd_hint = if file {
            "the spec's own".to_string()
        } else {
            here.display().to_string()
        };
        let cwd = form::field(
            "Working directory",
            &format!("Where the seats work (default: {cwd_hint})"),
            window,
            cx,
        );
        let name = form::field("Rig name", "As the spec names it", window, cx);
        let fresh = form::field(
            "Fresh seats",
            "Seats to start anew, e.g. dev.impl, dev.review",
            window,
            cx,
        );
        let fields = [cwd.clone(), name.clone(), fresh.clone()];
        let subscriptions = form::chain(&fields, window, cx, |this, _, cx| this.launch(cx));
        UpModal {
            fleet,
            source,
            summary,
            cwd,
            name,
            fresh,
            plan: Plan::None,
            _subscriptions: subscriptions,
        }
    }

    /// What `/up` (or with `plan`, `/plan`) sends for the form as it is.
    pub fn command(&self, plan: bool, cx: &App) -> Command {
        let cwd = form::text(&self.cwd, cx);
        let name = form::text(&self.name, cx);
        command::up(
            &self.source,
            Some(cwd.as_str()).filter(|c| !c.is_empty()),
            Some(name.as_str()),
            form::words(&form::text(&self.fresh, cx)),
            plan,
        )
    }

    /// Asks ainetd what would launch, and shows it here.
    pub fn plan(&mut self, cx: &mut Context<Self>) {
        let Command::Request(req, _) = self.command(true, cx) else {
            return;
        };
        let this = cx.entity().downgrade();
        self.fleet.update(cx, |fleet, cx| {
            fleet.ask(
                *req,
                move |reply, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.plan = Plan::Shown(reply);
                        cx.notify();
                    });
                },
                cx,
            )
        });
        self.plan = Plan::Asking;
        cx.notify();
    }

    /// Brings the rig up, and closes.
    pub fn launch(&mut self, cx: &mut Context<Self>) {
        let cmd = self.command(false, cx);
        self.fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
        cx.emit(DismissEvent);
    }

    /// The plan as shown, or why there is none (tests).
    #[cfg(test)]
    pub fn plan_text(&self) -> Option<&Result<String, String>> {
        match &self.plan {
            Plan::Shown(reply) => Some(reply),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn fields(&self) -> [&Entity<InputField>; 3] {
        [&self.cwd, &self.name, &self.fresh]
    }

    fn render_plan(&self, cx: &App) -> Option<AnyElement> {
        let colors = cx.theme().colors();
        let text = match &self.plan {
            Plan::None => return None,
            Plan::Asking => {
                return Some(
                    Label::new("Asking ainetd for the plan…")
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .into_any_element(),
                );
            }
            Plan::Shown(Err(why)) => {
                return Some(
                    Label::new(why.clone())
                        .size(LabelSize::Small)
                        .color(Color::Error)
                        .into_any_element(),
                );
            }
            Plan::Shown(Ok(text)) => text.clone(),
        };
        Some(
            div()
                .id("up-plan")
                .max_h(rems(16.))
                .overflow_scroll()
                .p_2()
                .rounded_sm()
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.editor_background)
                .font_buffer(cx)
                .text_ui_sm(cx)
                .whitespace_nowrap()
                .children(text.lines().map(|line| div().child(line.to_string())))
                .into_any_element(),
        )
    }
}

impl EventEmitter<DismissEvent> for UpModal {}

impl ModalView for UpModal {
    /// The modal layer gives the keyboard back from where the form took
    /// it, the first field; it may be in another.
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        window.focus(&self.cwd.focus_handle(cx), cx);
        true
    }
}

impl Focusable for UpModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.cwd.focus_handle(cx)
    }
}

impl Render for UpModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let asking = matches!(self.plan, Plan::Asking);
        let description = self
            .summary
            .clone()
            .unwrap_or_else(|| "A spec file, library spec or rig ainet knows".into());
        let body = v_flex()
            .gap_2()
            .child(form::folder_field("up-browse", &self.cwd))
            .child(self.name.clone())
            .child(self.fresh.clone())
            .children(self.render_plan(cx));
        let buttons = h_flex()
            .gap_1()
            .child(
                Button::new("up-plan-button", "Plan")
                    .style(ButtonStyle::Outlined)
                    .loading(asking)
                    .on_click(cx.listener(|this, _, _, cx| this.plan(cx))),
            )
            .child(
                Button::new("up-launch", "Launch")
                    .style(ButtonStyle::Filled)
                    .on_click(cx.listener(|this, _, _, cx| this.launch(cx))),
            );
        let hint = Label::new("enter: next field, then launch")
            .size(LabelSize::XSmall)
            .color(Color::Muted);
        form::frame(
            "up-modal",
            format!("Bring Up {}", self.source),
            Some(description.into()),
            body,
            (Some(hint.into_any_element()), buttons.into_any_element()),
            vec![self.cwd.clone(), self.name.clone(), self.fresh.clone()],
            cx,
        )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use std::path::{Path, PathBuf};

    use ainet_core::Request;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::actions::Up;
    use crate::tests::{demo_workspace, modal, sent};

    fn ups(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<ainet_core::protocol::UpSpec> {
        sent(w, cx)
            .into_iter()
            .filter_map(|r| match r {
                Request::Up(spec) => Some(spec),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    async fn a_library_spec_is_planned_then_launched_as_the_form_says(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        cx.dispatch_action(Up);
        let picker = modal::<Picker<SpecPicker>>(&w, cx).expect("the spec picker");
        assert!(matches!(sent(&w, cx).last(), Some(Request::Specs)));
        let listed = picker.read_with(cx, |p, _| {
            p.delegate
                .matches
                .iter()
                .map(|m| m.string.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(listed, ["docs", "infra"], "the demo's library");

        cx.simulate_input("docs");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(modal::<Picker<SpecPicker>>(&w, cx).is_none());
        let form = modal::<UpModal>(&w, cx).expect("the form follows");
        let [cwd, _, _] = form.read_with(cx, |f, _| f.fields().map(Clone::clone));
        assert!(cx.update(|window, cx| cwd.focus_handle(cx).contains_focused(window, cx)));

        // Browse… puts the folder picked in the field.
        cx.update(|window, cx| form::browse(&cwd, window, cx));
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.directories && !options.files);
            Some(vec![PathBuf::from("/srv/app")])
        });
        cx.run_until_parked();
        assert_eq!(cwd.read_with(cx, |f, cx| f.text(cx)), "/srv/app");

        // Enter moves on; tab goes round too.
        cx.simulate_keystrokes("enter");
        cx.simulate_input("team");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("dev.ui, dev.app");

        form.update(cx, |f, cx| f.plan(cx));
        cx.run_until_parked();
        let plan = ups(&w, cx).pop().expect("a plan asked for");
        assert!(plan.plan && plan.source == "docs");
        assert_eq!(plan.cwd.as_deref(), Some(Path::new("/srv/app")));
        assert_eq!(plan.name.as_deref(), Some("team"));
        assert_eq!(plan.fresh, ["dev.ui", "dev.app"]);
        let shown = form.read_with(cx, |f, _| f.plan_text().cloned());
        assert!(
            matches!(&shown, Some(Ok(text)) if text.starts_with("rig docs") && text.contains("/srv/app")),
            "{shown:?}"
        );
        assert!(modal::<UpModal>(&w, cx).is_some(), "the plan shows in the form");

        // Enter in the last field launches it.
        cx.simulate_keystrokes("enter");
        assert!(modal::<UpModal>(&w, cx).is_none());
        let launch = ups(&w, cx).pop().unwrap();
        assert!(!launch.plan);
        assert_eq!(
            (launch.source.as_str(), launch.name.as_deref(), launch.fresh.len()),
            ("docs", Some("team"), 2)
        );
    }

    #[gpui::test]
    async fn a_typed_source_is_offered_and_escape_gives_up(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        cx.dispatch_action(Up);
        let picker = modal::<Picker<SpecPicker>>(&w, cx).unwrap();
        cx.simulate_input("starter");
        picker.read_with(cx, |p, _| {
            assert!(p.delegate.matches.is_empty());
            assert_eq!(p.delegate.typed.as_deref(), Some("starter"));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let form = modal::<UpModal>(&w, cx).expect("the form for a typed name");
        // A name the demo does not know: the plan says so.
        form.update(cx, |f, cx| f.plan(cx));
        cx.run_until_parked();
        let shown = form.read_with(cx, |f, _| f.plan_text().cloned());
        assert_eq!(shown, Some(Err("no spec named starter in the demo".into())));
        let before = sent(&w, cx).len();
        cx.simulate_keystrokes("escape");
        assert!(modal::<UpModal>(&w, cx).is_none());
        assert_eq!(sent(&w, cx).len(), before, "nothing launched");
    }
}
