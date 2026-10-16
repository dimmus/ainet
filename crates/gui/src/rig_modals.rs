//! Restoring a rig and taking one down (`fleet::Restore`, `fleet::Down`),
//! and the rig picker that leads to them (and to growing one) when no
//! rig is selected, or switches rig (`fleet::SwitchRig`, the title bar's
//! rig switcher). Restoring picks a snapshot (`Request::SnapshotList`,
//! the latest by default) and the seats to start fresh; taking down asks
//! first, with a snapshot taken unless you say not to. Each sends what
//! `/restore` and `/down` send, through `ainet-model`'s command layer.

use std::collections::BTreeSet;
use std::sync::Arc;

use ainet_core::{Request, SnapshotInfo};
use ainet_model::describe::age;
use ainet_model::{Listing, Row, Store, command};
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Task, WeakEntity};
use ui::{Checkbox, HighlightedLabel, ListItem, ListItemSpacing, TintColor, ToggleState, prelude::*};

use crate::fleet::FleetModel;
use crate::form;
use crate::modal_layer::ModalView;
use crate::picker::{Picker, PickerDelegate, match_strings};
use crate::seat_modals::GrowModal;
use crate::workspace::Workspace;

/// What follows picking a rig.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigAction {
    Restore,
    Down,
    Grow,
    /// Selects the rig's first live agent.
    Select,
}

impl RigAction {
    fn placeholder(self) -> &'static str {
        match self {
            RigAction::Restore => "Restore which rig?",
            RigAction::Down => "Take which rig down?",
            RigAction::Grow => "Add a seat to which rig?",
            RigAction::Select => "Switch to which rig?",
        }
    }

    /// Opens the modal that follows for `rig`, in place of what is open.
    pub fn open(
        self,
        fleet: Entity<FleetModel>,
        rig: String,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        match self {
            RigAction::Restore => {
                workspace.toggle_modal(window, cx, |window, cx| RestoreModal::new(fleet, rig, window, cx))
            }
            RigAction::Down => workspace.toggle_modal(window, cx, |_, cx| DownModal::new(fleet, rig, cx)),
            RigAction::Grow => {
                workspace.toggle_modal(window, cx, |window, cx| GrowModal::new(fleet, rig, window, cx))
            }
            RigAction::Select => fleet.update(cx, |fleet, cx| match first_agent(&fleet.store, &rig) {
                Some(id) => fleet.select(&id, cx),
                None => fleet.show(format!("no live agents in rig {rig}"), false, cx),
            }),
        }
    }
}

/// The first live agent of `rig`, in the Fleet's order: where switching
/// to the rig puts the selection.
pub fn first_agent(store: &Store, rig: &str) -> Option<String> {
    store.rows.iter().find_map(|row| match row {
        Row::Agent { id, .. } => store
            .agents
            .get(id)
            .filter(|a| a.rig.as_deref() == Some(rig) && a.state.is_live())
            .map(|a| a.id.clone()),
        _ => None,
    })
}

/// The rigs ainet knows, to pick one.
pub struct RigPicker {
    fleet: Entity<FleetModel>,
    workspace: WeakEntity<Workspace>,
    then: RigAction,
    /// Rig names, by name; candidate ids index this.
    rigs: Vec<String>,
    candidates: Arc<Vec<StringMatchCandidate>>,
    matches: Vec<StringMatch>,
    selected: usize,
}

impl RigPicker {
    pub fn new(
        fleet: Entity<FleetModel>,
        workspace: WeakEntity<Workspace>,
        then: RigAction,
        cx: &App,
    ) -> Self {
        let rigs: Vec<String> = fleet.read(cx).store.rigs.keys().cloned().collect();
        let candidates = rigs
            .iter()
            .enumerate()
            .map(|(ix, r)| StringMatchCandidate::new(ix, r))
            .collect();
        RigPicker {
            fleet,
            workspace,
            then,
            rigs,
            candidates: Arc::new(candidates),
            matches: Vec::new(),
            selected: 0,
        }
    }
}

impl PickerDelegate for RigPicker {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = ix;
    }

    fn placeholder_text(&self, _: &App) -> SharedString {
        self.then.placeholder().into()
    }

    fn no_matches_text(&self, _: &App) -> SharedString {
        if self.rigs.is_empty() {
            "No rigs yet: fleet: up brings one up".into()
        } else {
            "No matching rigs".into()
        }
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let found = match_strings(self.candidates.clone(), query, cx);
        cx.spawn(async move |picker, cx| {
            let matches = found.await;
            let _ = picker.update(cx, |picker, _| {
                picker.delegate.matches = matches;
                picker.delegate.selected = 0;
            });
        })
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(rig) = self
            .matches
            .get(self.selected)
            .and_then(|m| self.rigs.get(m.candidate_id))
            .cloned()
        else {
            return;
        };
        let (workspace, fleet, then) = (self.workspace.clone(), self.fleet.clone(), self.then);
        // Switching rig is all there is to it; the others' forms take the
        // picker's place.
        if then == RigAction::Select {
            cx.emit(DismissEvent);
        }
        window.defer(cx, move |window, cx| {
            let _ = workspace.update(cx, |workspace, cx| then.open(fleet, rig, workspace, window, cx));
        });
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let m = self.matches.get(ix)?;
        let rig = self
            .fleet
            .read(cx)
            .store
            .rigs
            .get(self.rigs.get(m.candidate_id)?)?;
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(HighlightedLabel::new(m.string.clone(), m.positions.clone()))
                .end_slot(
                    Label::new(format!("{} · {} seats", rig.status.label(), rig.seats.len()))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        )
    }
}

/// The form for `/restore`: which snapshot, and which seats start anew.
pub struct RestoreModal {
    fleet: Entity<FleetModel>,
    rig: String,
    /// Oldest first, once ainetd has listed them.
    snapshots: Option<Result<Vec<SnapshotInfo>, String>>,
    /// The snapshot to restore; `None`, the latest.
    picked: Option<String>,
    /// The seats (logical ids) to start fresh instead of resuming.
    fresh: BTreeSet<String>,
    focus: FocusHandle,
}

impl RestoreModal {
    pub fn new(fleet: Entity<FleetModel>, rig: String, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        fleet.update(cx, |fleet, cx| {
            fleet.list(
                Request::SnapshotList { rig: rig.clone() },
                move |listing, cx| {
                    let snapshots = match listing {
                        Ok(Listing::Snapshots(snapshots)) => Ok(snapshots),
                        Ok(_) => Err("ainetd answered with something else".into()),
                        Err(e) => Err(e),
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.snapshots = Some(snapshots);
                        cx.notify();
                    });
                },
                cx,
            )
        });
        RestoreModal {
            fleet,
            rig,
            snapshots: None,
            picked: None,
            fresh: BTreeSet::new(),
            focus: cx.focus_handle(),
        }
    }

    /// The snapshots to choose from, newest first: `None` is the latest.
    fn choices(&self) -> Vec<Option<&SnapshotInfo>> {
        let listed = self.snapshots.iter().flatten().flat_map(|s| s.iter().rev());
        std::iter::once(None).chain(listed.map(Some)).collect()
    }

    /// Picks the snapshot `delta` rows on from the picked one.
    fn pick_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let choices = self.choices();
        let at = choices
            .iter()
            .position(|c| c.map(|s| &s.id) == self.picked.as_ref())
            .unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, choices.len() as isize - 1) as usize;
        self.picked = choices[next].map(|s| s.id.clone());
        cx.notify();
    }

    /// Picks a snapshot by id (`None`: the latest).
    pub fn pick(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.picked = id;
        cx.notify();
    }

    /// Starts seat `logical` fresh, or not.
    pub fn toggle_fresh(&mut self, logical: &str, cx: &mut Context<Self>) {
        if !self.fresh.remove(logical) {
            self.fresh.insert(logical.to_string());
        }
        cx.notify();
    }

    /// Restores the rig, and closes.
    pub fn restore(&mut self, cx: &mut Context<Self>) {
        let cmd = command::restore(
            &self.rig,
            self.picked.clone(),
            self.fresh.iter().cloned().collect(),
        );
        self.fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
        cx.emit(DismissEvent);
    }

    /// The snapshots listed, oldest first (tests).
    #[cfg(test)]
    pub fn snapshots(&self) -> Option<&Result<Vec<SnapshotInfo>, String>> {
        self.snapshots.as_ref()
    }

    fn render_snapshots(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows =
            self.choices().into_iter().enumerate().map(|(ix, snapshot)| {
                let id = snapshot.map(|s| s.id.clone());
                let (title, meta) = match snapshot {
                    None => ("The latest snapshot".to_string(), None),
                    Some(s) => (
                        s.label.clone().unwrap_or_else(|| s.id.clone()),
                        Some(format!("{} ago · {} seats", age(s.created), s.seats)),
                    ),
                };
                ListItem::new(("snapshot", ix))
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(id == self.picked)
                    .start_slot(Icon::new(IconName::Check).size(IconSize::Small).color(
                        if id == self.picked {
                            Color::Accent
                        } else {
                            Color::Hidden
                        },
                    ))
                    .child(Label::new(title))
                    .end_slot::<Label>(meta.map(|m| Label::new(m).size(LabelSize::Small).color(Color::Muted)))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(id.clone(), cx)))
            });
        let note = match &self.snapshots {
            None => Some(("Asking ainetd for the snapshots…".to_string(), Color::Muted)),
            Some(Err(why)) => Some((why.clone(), Color::Error)),
            Some(Ok(_)) => None,
        };
        v_flex()
            .child(Label::new("Snapshot").size(LabelSize::Small))
            .children(rows)
            .children(note.map(|(text, color)| Label::new(text).size(LabelSize::Small).color(color)))
    }

    fn render_seats(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let seats: Vec<(String, Option<String>)> = self
            .fleet
            .read(cx)
            .store
            .rigs
            .get(&self.rig)
            .map(|r| {
                r.seats
                    .iter()
                    .map(|s| (s.logical.clone(), s.role.clone()))
                    .collect()
            })
            .unwrap_or_default();
        v_flex()
            .gap_1()
            .child(Label::new("Start fresh instead of resuming").size(LabelSize::Small))
            .children(seats.into_iter().map(|(logical, role)| {
                let checked = ToggleState::from(self.fresh.contains(&logical));
                let label = match role {
                    Some(role) => format!("{logical} · {role}"),
                    None => logical.clone(),
                };
                Checkbox::new(SharedString::from(format!("fresh-{logical}")), checked)
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_fresh(&logical, cx)))
            }))
    }
}

impl EventEmitter<DismissEvent> for RestoreModal {}
impl ModalView for RestoreModal {}

impl Focusable for RestoreModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RestoreModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = v_flex()
            .gap_3()
            .child(self.render_snapshots(cx))
            .child(self.render_seats(cx));
        let buttons = Button::new("restore", "Restore")
            .style(ButtonStyle::Filled)
            .key_binding(ui::KeyBinding::for_action_in(&menu::Confirm, &self.focus, cx))
            .on_click(cx.listener(|this, _, _, cx| this.restore(cx)));
        form::frame(
            "restore-modal",
            format!("Restore {}", self.rig),
            Some("Its seats come back as the snapshot left them.".into()),
            body,
            (None, buttons.into_any_element()),
            Vec::new(),
            cx,
        )
        .track_focus(&self.focus)
        .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.restore(cx)))
        .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.pick_relative(1, cx)))
        .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.pick_relative(-1, cx)))
    }
}

/// Asks before `/down`, with the snapshot it takes first.
pub struct DownModal {
    fleet: Entity<FleetModel>,
    rig: String,
    snapshot: bool,
    focus: FocusHandle,
}

impl DownModal {
    pub fn new(fleet: Entity<FleetModel>, rig: String, cx: &mut Context<Self>) -> Self {
        DownModal {
            fleet,
            rig,
            snapshot: true,
            focus: cx.focus_handle(),
        }
    }

    /// Takes a snapshot first, or not.
    pub fn toggle_snapshot(&mut self, cx: &mut Context<Self>) {
        self.snapshot = !self.snapshot;
        cx.notify();
    }

    /// Takes the rig down, and closes.
    pub fn down(&mut self, cx: &mut Context<Self>) {
        let cmd = command::down(&self.rig, self.snapshot);
        self.fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for DownModal {}
impl ModalView for DownModal {}

impl Focusable for DownModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DownModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self
            .fleet
            .read(cx)
            .store
            .rigs
            .get(&self.rig)
            .map_or(0, |r| r.seats.iter().filter(|s| s.agent.is_some()).count());
        let body = Checkbox::new("down-snapshot", ToggleState::from(self.snapshot))
            .label("Take a snapshot first, to restore it later")
            .on_click(cx.listener(|this, _, _, cx| this.toggle_snapshot(cx)));
        let buttons = Button::new("down", "Take Down")
            .style(ButtonStyle::Tinted(TintColor::Error))
            .key_binding(ui::KeyBinding::for_action_in(&menu::Confirm, &self.focus, cx))
            .on_click(cx.listener(|this, _, _, cx| this.down(cx)));
        form::frame(
            "down-modal",
            format!("Take {} Down?", self.rig),
            Some(match running {
                1 => "Its agent stops.".into(),
                n => format!("Its {n} agents stop.").into(),
            }),
            body,
            (None, buttons.into_any_element()),
            Vec::new(),
            cx,
        )
        .track_focus(&self.focus)
        .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.down(cx)))
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::Request;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::actions::{Down, Restore, SwitchRig};
    use crate::tests::{demo_workspace, modal, sent};
    use crate::title_bar::rig_label;

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    #[gpui::test]
    async fn restore_picks_a_rig_a_snapshot_and_fresh_seats(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        cx.dispatch_action(Restore);
        let picker = modal::<Picker<RigPicker>>(&w, cx).expect("a rig picker first");
        let rigs = picker.read_with(cx, |p, _| p.delegate.matches.len());
        assert_eq!(rigs, 2);
        cx.simulate_input("infra");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let form = modal::<RestoreModal>(&w, cx).expect("the restore form");
        assert!(matches!(
            sent(&w, cx).last(),
            Some(Request::SnapshotList { rig }) if rig == "infra"
        ));
        let snapshots = form.read_with(cx, |f, _| f.snapshots().cloned());
        let Some(Ok(snapshots)) = snapshots else {
            panic!("{snapshots:?}")
        };
        assert_eq!(snapshots.len(), 2);

        // The latest by default: down picks the newest listed, then the
        // one before it.
        cx.simulate_keystrokes("down down");
        let seat = fleet(&w, cx).read_with(cx, |f, _| f.store.rigs["infra"].seats[0].logical.clone());
        form.update(cx, |f, cx| f.toggle_fresh(&seat, cx));
        cx.simulate_keystrokes("enter");
        assert!(modal::<RestoreModal>(&w, cx).is_none());
        match sent(&w, cx).last() {
            Some(Request::Restore { rig, snapshot, fresh }) => {
                assert_eq!(rig, "infra");
                assert_eq!(snapshot.as_ref(), Some(&snapshots[0].id), "the older one");
                assert_eq!(fresh, &[seat]);
            }
            other => panic!("{other:?}"),
        }

        // Straight away: the latest, nobody fresh.
        cx.dispatch_action(Restore);
        cx.simulate_input("docs");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert!(matches!(
            sent(&w, cx).last(),
            Some(Request::Restore { rig, snapshot: None, fresh }) if rig == "docs" && fresh.is_empty()
        ));
    }

    #[gpui::test]
    async fn down_asks_first_with_a_snapshot_unless_told_not_to(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let rig = fleet(&w, cx).read_with(cx, |f, _| f.store.selected_agent().and_then(|a| a.rig.clone()));
        let rig = rig.expect("the demo selects an agent in a rig");
        cx.dispatch_action(Down);
        let form = modal::<DownModal>(&w, cx).expect("the selected agent's rig, asked about");
        let before = sent(&w, cx).len();
        cx.simulate_keystrokes("escape");
        assert!(modal::<DownModal>(&w, cx).is_none());
        assert_eq!(sent(&w, cx).len(), before, "nothing sent on escape");
        drop(form);

        cx.dispatch_action(Down);
        cx.simulate_keystrokes("enter");
        assert!(matches!(
            sent(&w, cx).last(),
            Some(Request::Down { rig: r, snapshot: true, delete: false }) if *r == rig
        ));

        cx.dispatch_action(Down);
        let form = modal::<DownModal>(&w, cx).unwrap();
        form.update(cx, |f, cx| f.toggle_snapshot(cx));
        cx.simulate_keystrokes("enter");
        assert!(matches!(
            sent(&w, cx).last(),
            Some(Request::Down { snapshot: false, .. })
        ));

        // With nobody selected and two rigs: which rig first.
        fleet(&w, cx).update(cx, |f, cx| {
            f.store.selected = None;
            cx.notify();
        });
        cx.dispatch_action(Down);
        assert!(modal::<Picker<RigPicker>>(&w, cx).is_some());
    }

    /// The title bar's rig switcher names the selected agent's rig, and
    /// picking another selects its first live agent.
    #[gpui::test]
    async fn the_rig_switcher_selects_the_rigs_first_agent(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let fleet = fleet(&w, cx);
        let label = |cx: &mut VisualTestContext| fleet.read_with(cx, |f, _| rig_label(&f.store));
        assert_eq!(label(cx), "docs");

        let switcher = cx
            .debug_bounds("TITLE-RIG-SWITCHER")
            .expect("the rig switcher is drawn");
        cx.simulate_click(switcher.center(), gpui::Modifiers::none());
        let picker = modal::<Picker<RigPicker>>(&w, cx).expect("a click picks a rig");
        assert_eq!(picker.read_with(cx, |p, _| p.delegate.matches.len()), 2);
        cx.simulate_input("infra");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(modal::<Picker<RigPicker>>(&w, cx).is_none());
        let (selected, first) = fleet.read_with(cx, |f, _| {
            (f.store.selected.clone(), first_agent(&f.store, "infra"))
        });
        assert!(first.is_some());
        assert_eq!(selected, first);
        assert_eq!(label(cx), "infra");

        // From the keyboard too, back to docs.
        cx.dispatch_action(SwitchRig);
        cx.simulate_input("docs");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(label(cx), "docs");
    }
}
