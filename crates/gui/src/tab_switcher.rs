//! The tab switcher (`ctrl-tab`), as Zed's: the active pane's tabs, the
//! latest used first, with the one used before this one selected. Holding
//! `ctrl`, each `tab` moves down (`ctrl-shift-tab` up), and letting go
//! switches; typing filters, and enter switches too.

use std::sync::Arc;

use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    DismissEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Modifiers, ModifiersChangedEvent,
    Subscription, Task, WeakEntity,
};
use ui::{ListItem, ListItemSpacing, prelude::*};

use crate::item::{ItemHandle, TabContentParams};
use crate::modal_layer::ModalView;
use crate::pane::Pane;
use crate::picker::{Picker, PickerDelegate, match_strings};

/// The modal: the picker, and the modifiers held when it opened.
pub struct TabSwitcher {
    picker: Entity<Picker<TabSwitcherDelegate>>,
    /// Held when it opened by key; letting go of them switches.
    init_modifiers: Option<Modifiers>,
    _subscription: Subscription,
}

impl TabSwitcher {
    pub fn new(pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = TabSwitcherDelegate::new(pane, cx);
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx).width(rems(28.)));
        let subscription = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent));
        let modifiers = window.modifiers();
        TabSwitcher {
            picker,
            init_modifiers: modifiers.modified().then_some(modifiers),
            _subscription: subscription,
        }
    }

    /// Moves the selection `delta` on (its key again, while open).
    pub fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.picker
            .update(cx, |picker, cx| picker.select_relative(delta, window, cx));
    }

    #[cfg(test)]
    pub fn picker(&self) -> &Entity<Picker<TabSwitcherDelegate>> {
        &self.picker
    }

    /// Letting go of the keys it opened with switches to the selected tab.
    fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(init) = self.init_modifiers else {
            return;
        };
        if !event.modified() || !init.is_subset_of(event) {
            self.init_modifiers = None;
            window.dispatch_action(Box::new(menu::Confirm), cx);
        }
    }
}

impl EventEmitter<DismissEvent> for TabSwitcher {}

impl ModalView for TabSwitcher {
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.picker
            .update(cx, |picker, cx| picker.on_before_dismiss(window, cx))
    }
}

impl Focusable for TabSwitcher {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for TabSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("TabSwitcher")
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .child(self.picker.clone())
    }
}

pub struct TabSwitcherDelegate {
    pane: WeakEntity<Pane>,
    /// The tabs, the latest used first; candidate ids index this.
    items: Vec<Box<dyn ItemHandle>>,
    candidates: Arc<Vec<StringMatchCandidate>>,
    matches: Vec<StringMatch>,
    selected: usize,
    /// Matched once: the next selection is the tab used before.
    started: bool,
}

impl TabSwitcherDelegate {
    fn new(pane: &Entity<Pane>, cx: &App) -> Self {
        let items: Vec<Box<dyn ItemHandle>> = pane
            .read(cx)
            .items_by_recency()
            .into_iter()
            .map(|i| i.boxed_clone())
            .collect();
        let candidates = items
            .iter()
            .enumerate()
            .map(|(ix, i)| StringMatchCandidate::new(ix, &i.tab_content_text(cx)))
            .collect();
        TabSwitcherDelegate {
            pane: pane.downgrade(),
            items,
            candidates: Arc::new(candidates),
            matches: Vec::new(),
            selected: 0,
            started: false,
        }
    }

    /// The selected tab's item (tests).
    #[cfg(test)]
    pub fn selected_item(&self) -> Option<EntityId> {
        let m = self.matches.get(self.selected)?;
        Some(self.items[m.candidate_id].item_id())
    }
}

impl PickerDelegate for TabSwitcherDelegate {
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
        "Switch to a tab…".into()
    }

    fn no_matches_text(&self, _: &App) -> SharedString {
        if self.items.is_empty() {
            "No tabs open".into()
        } else {
            "No matching tabs".into()
        }
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let found = match_strings(self.candidates.clone(), query, cx);
        cx.spawn(async move |picker, cx| {
            let matches = found.await;
            let _ = picker.update(cx, |picker, _| {
                let this = &mut picker.delegate;
                // At first, as Zed: the tab used before this one.
                this.selected = if this.started {
                    0
                } else {
                    usize::from(matches.len() > 1)
                };
                this.started = true;
                this.matches = matches;
            });
        })
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let id: Option<EntityId> = self
            .matches
            .get(self.selected)
            .map(|m| self.items[m.candidate_id].item_id());
        if let (Some(id), Some(pane)) = (id, self.pane.upgrade()) {
            pane.update(cx, |pane, cx| {
                if let Some(ix) = pane.index_of(id) {
                    pane.activate_item(ix, true, window, cx);
                }
            });
        }
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let m = self.matches.get(ix)?;
        let item = self.items.get(m.candidate_id)?;
        let params = TabContentParams {
            selected: true,
            deemphasized: false,
        };
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot::<AnyElement>(item.tab_icon(window, cx))
                .child(item.tab_content(params, window, cx)),
        )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::fleet::FleetModel;
    use crate::tests::{init_test, modal};
    use crate::workspace::Workspace;

    fn titles(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> (Vec<String>, String) {
        pane.read_with(cx, |p, cx| {
            (
                p.items_by_recency()
                    .iter()
                    .map(|i| i.tab_content_text(cx).to_string())
                    .collect(),
                p.active_item().unwrap().tab_content_text(cx).to_string(),
            )
        })
    }

    #[gpui::test]
    async fn ctrl_tab_goes_back_to_the_tab_used_before(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(|_| FleetModel::offline());
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-shift-c ctrl-shift-i ctrl-t");
        let pane = w.read_with(cx, |w, _| w.active_pane().clone());
        assert_eq!(
            titles(&pane, cx),
            (
                vec!["Map".into(), "Inbox".into(), "Comms".into(), "Fleet".into()],
                "Map".into()
            )
        );

        // Held ctrl, tab: the tab before is selected; letting go switches.
        cx.simulate_modifiers_change(Modifiers::control());
        cx.simulate_keystrokes("ctrl-tab");
        let switcher = modal::<TabSwitcher>(&w, cx).expect("the switcher");
        let selected = |cx: &mut VisualTestContext| {
            switcher.read_with(cx, |s, cx| s.picker().read(cx).delegate.selected_item())
        };
        let id_of = |title: &str, cx: &mut VisualTestContext| {
            pane.read_with(cx, |p, cx| {
                p.items()
                    .find(|i| i.tab_content_text(cx).as_ref() == title)
                    .unwrap()
                    .item_id()
            })
        };
        assert_eq!(selected(cx), Some(id_of("Inbox", cx)));
        // Tab again, still holding ctrl: on down the list.
        cx.simulate_keystrokes("ctrl-tab");
        assert_eq!(selected(cx), Some(id_of("Comms", cx)));
        cx.simulate_keystrokes("ctrl-shift-tab");
        assert_eq!(selected(cx), Some(id_of("Inbox", cx)));
        cx.simulate_modifiers_change(Modifiers::none());
        assert!(modal::<TabSwitcher>(&w, cx).is_none());
        assert_eq!(titles(&pane, cx).1, "Inbox");

        // So ctrl-tab twice over goes back and forth.
        cx.simulate_modifiers_change(Modifiers::control());
        cx.simulate_keystrokes("ctrl-tab");
        cx.simulate_modifiers_change(Modifiers::none());
        assert_eq!(titles(&pane, cx).1, "Map");

        // Opened without a modifier held (the palette): typing filters
        // and enter switches.
        cx.dispatch_action(crate::actions::tab_switcher::Toggle);
        cx.simulate_input("flee");
        cx.simulate_keystrokes("enter");
        assert!(modal::<TabSwitcher>(&w, cx).is_none());
        assert_eq!(titles(&pane, cx).1, "Fleet");
    }
}
