//! The agent switcher (`ctrl-p`, as Zed's file finder): a picker over the
//! fleet's agents by name, seat, role and rig, in Fleet order. Picking one
//! selects it in the Fleet; phase 5 makes it open the agent's terminal.

use std::sync::Arc;

use ainet_model::Row;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{DismissEvent, Entity, Task};
use ui::{HighlightedLabel, Indicator, ListItem, ListItemSpacing, prelude::*};

use crate::fleet::FleetModel;
use crate::panels::state_color;
use crate::picker::{Picker, PickerDelegate, match_strings};

pub struct AgentSwitcher {
    fleet: Entity<FleetModel>,
    /// Agent ids, in Fleet order; candidate ids index this.
    ids: Vec<String>,
    candidates: Arc<Vec<StringMatchCandidate>>,
    matches: Vec<StringMatch>,
    selected: usize,
}

impl AgentSwitcher {
    pub fn new(fleet: Entity<FleetModel>, cx: &App) -> Self {
        let store = &fleet.read(cx).store;
        let mut ids = Vec::new();
        let mut candidates = Vec::new();
        for row in &store.rows {
            let Row::Agent { id, .. } = row else { continue };
            let Some(agent) = store.agents.get(id) else {
                continue;
            };
            let words: Vec<&str> = [
                Some(agent.name.as_str()),
                agent.seat.as_deref(),
                agent.role.as_deref(),
                agent.rig.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect();
            candidates.push(StringMatchCandidate::new(ids.len(), &words.join(" · ")));
            ids.push(id.clone());
        }
        AgentSwitcher {
            fleet,
            ids,
            candidates: Arc::new(candidates),
            matches: Vec::new(),
            selected: 0,
        }
    }
}

impl PickerDelegate for AgentSwitcher {
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
        "Find an agent by name, seat, role or rig…".into()
    }

    fn no_matches_text(&self, _: &App) -> SharedString {
        if self.ids.is_empty() {
            "No agents yet".into()
        } else {
            "No matching agents".into()
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

    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(id) = self
            .matches
            .get(self.selected)
            .and_then(|m| self.ids.get(m.candidate_id))
        {
            let id = id.clone();
            self.fleet.update(cx, |fleet, cx| fleet.select(&id, cx));
        }
        cx.emit(DismissEvent);
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
        let id = self.ids.get(m.candidate_id)?;
        let state = self.fleet.read(cx).store.agents.get(id)?.state;
        Some(
            ListItem::new(ix)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(Indicator::dot().color(state_color(state)))
                .child(HighlightedLabel::new(m.string.clone(), m.positions.clone()))
                .end_slot(
                    Label::new(state.label())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        )
    }
}
