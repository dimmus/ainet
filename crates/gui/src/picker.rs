//! A fuzzy-filtered list in a modal, after Zed's `picker` crate: a query
//! [`TextInput`] over a list of matches with their matched characters
//! highlighted; up and down (`menu::SelectNext`/`SelectPrevious`) move the
//! selection, enter (`menu::Confirm`) picks, escape (`menu::Cancel`) or a
//! click outside closes. What is listed and what picking does is up to a
//! [`PickerDelegate`], as in Zed; [`match_strings`] runs Zed's own fuzzy
//! matcher off the UI thread.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    Task, UniformListScrollHandle, Window, uniform_list,
};
use ui::{Divider, prelude::*};

use crate::modal_layer::ModalView;
use crate::text_input::{InputEvent, TextInput};

/// The most matches a picker lists.
pub const MAX_MATCHES: usize = 100;

pub trait PickerDelegate: Sized + 'static {
    type ListItem: IntoElement;

    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Picker<Self>>);
    fn placeholder_text(&self, cx: &App) -> SharedString;
    fn no_matches_text(&self, _: &App) -> SharedString {
        "No matches".into()
    }
    /// Recomputes the matches for `query`; the picker redraws when the
    /// task is done.
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()>;
    /// Enter: picks the selected match (`secondary` with a modifier).
    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>);
    /// The picker is closing: without a pick, or after one (then after
    /// `confirm`).
    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>);
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem>;
    /// A strip under the list (buttons, hints), as Zed's pickers have.
    fn render_footer(&self, _: &mut Window, _: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        None
    }
}

pub struct Picker<D: PickerDelegate> {
    pub delegate: D,
    query: Entity<TextInput>,
    scroll_handle: UniformListScrollHandle,
    pending_update: Option<Task<()>>,
    width: Rems,
    /// Whether the delegate has heard it is closing.
    closed: bool,
}

impl<D: PickerDelegate> EventEmitter<DismissEvent> for Picker<D> {}

impl<D: PickerDelegate> ModalView for Picker<D> {
    /// However it closes (a click outside too), the delegate hears it.
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close(window, cx);
        true
    }
}

impl<D: PickerDelegate> Picker<D> {
    /// A picker listing the delegate's matches, all of one height.
    pub fn uniform_list(delegate: D, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = delegate.placeholder_text(cx);
        let query = cx.new(|cx| TextInput::new(window, cx).with_placeholder(placeholder));
        cx.subscribe_in(&query, window, |this, input, event: &InputEvent, window, cx| {
            if *event == InputEvent::Edited {
                let text = input.read(cx).text().to_string();
                this.update_matches(text, window, cx);
            }
        })
        .detach();
        let mut picker = Picker {
            delegate,
            query,
            scroll_handle: UniformListScrollHandle::new(),
            pending_update: None,
            width: rems(34.),
            closed: false,
        };
        picker.update_matches(String::new(), window, cx);
        picker
    }

    /// As wide as `width` (34 rem by default).
    pub fn width(mut self, width: Rems) -> Self {
        self.width = width;
        self
    }

    /// What is typed in the query field.
    pub fn query(&self, cx: &App) -> String {
        self.query.read(cx).text().to_string()
    }

    /// Puts `text` in the query field and lists its matches.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A new text is matched once it is reported edited.
        let same = self.query.read(cx).text() == text;
        self.query.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.move_to_end(cx);
        });
        if same {
            self.update_matches(text.to_string(), window, cx);
        }
    }

    /// Lists the matches again, for when what the delegate lists changed
    /// (items that arrived later).
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.update_matches(query, window, cx);
    }

    fn update_matches(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let update = self.delegate.update_matches(query, window, cx);
        self.pending_update = Some(cx.spawn_in(window, async move |this, cx| {
            update.await;
            let _ = this.update(cx, |this, cx| this.matches_updated(cx));
        }));
    }

    fn matches_updated(&mut self, cx: &mut Context<Self>) {
        self.pending_update = None;
        let ix = self.delegate.selected_index();
        if self.delegate.match_count() > 0 {
            self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.delegate.match_count() == 0 {
            return;
        }
        let ix = ix.min(self.delegate.match_count() - 1);
        self.delegate.set_selected_index(ix, window, cx);
        self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Moves the selection `delta` down, wrapping round as Zed's does.
    pub fn select_relative(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.delegate.match_count() as isize;
        if n == 0 {
            return;
        }
        let ix = (self.delegate.selected_index() as isize + delta).rem_euclid(n);
        self.select(ix as usize, window, cx);
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Matches still coming: the pick waits for them.
        if let Some(update) = self.pending_update.take() {
            cx.spawn_in(window, async move |this, cx| {
                update.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.matches_updated(cx);
                    this.delegate.confirm(secondary, window, cx)
                });
            })
            .detach();
        } else {
            self.delegate.confirm(secondary, window, cx);
        }
    }

    /// Closes without a pick.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        cx.emit(DismissEvent);
    }

    /// Tells the delegate, once, that the picker is closing (after a pick
    /// too).
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.closed, true) {
            self.delegate.dismissed(window, cx);
        }
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.delegate.match_count();
        if count == 0 {
            return div()
                .px_2p5()
                .py_2()
                .child(Label::new(self.delegate.no_matches_text(cx)).color(Color::Muted))
                .into_any_element();
        }
        uniform_list(
            "picker-matches",
            count,
            cx.processor(|picker, range: std::ops::Range<usize>, window, cx| {
                let selected = picker.delegate.selected_index();
                range
                    .filter_map(|ix| {
                        let item = picker.delegate.render_match(ix, ix == selected, window, cx)?;
                        Some(
                            div()
                                .id(("picker-match", ix))
                                .on_click(cx.listener(move |picker, event: &ClickEvent, window, cx| {
                                    picker.select(ix, window, cx);
                                    picker.confirm(event.modifiers().secondary(), window, cx);
                                }))
                                .child(item),
                        )
                    })
                    .collect()
            }),
        )
        .py_1()
        .max_h(rems(24.))
        .track_scroll(&self.scroll_handle)
        .into_any_element()
    }
}

impl<D: PickerDelegate> Focusable for Picker<D> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl<D: PickerDelegate> Render for Picker<D> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("Picker")
            .w(self.width)
            .elevation_3(cx)
            .overflow_hidden()
            .on_action(
                cx.listener(|this, _: &menu::SelectNext, window, cx| this.select_relative(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &menu::SelectPrevious, window, cx| {
                    this.select_relative(-1, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &menu::SelectFirst, window, cx| this.select(0, window, cx)))
            .on_action(
                cx.listener(|this, _: &menu::SelectLast, window, cx| this.select(usize::MAX, window, cx)),
            )
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.confirm(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &menu::SecondaryConfirm, window, cx| this.confirm(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| this.cancel(window, cx)))
            .child(
                h_flex()
                    .h_9()
                    .px_2p5()
                    .flex_none()
                    .overflow_hidden()
                    .child(div().flex_1().child(self.query.clone())),
            )
            .child(Divider::horizontal())
            .child(self.render_list(cx))
            .children(self.delegate.render_footer(window, cx))
    }
}

/// Matches `query` against `candidates` with Zed's fuzzy matcher (smart
/// case, shorter strings first on ties), best first, off the UI thread.
pub fn match_strings(
    candidates: Arc<Vec<StringMatchCandidate>>,
    query: String,
    cx: &App,
) -> Task<Vec<StringMatch>> {
    match_strings_up_to(candidates, query, MAX_MATCHES, cx)
}

/// [`match_strings`], keeping up to `limit` matches.
pub fn match_strings_up_to(
    candidates: Arc<Vec<StringMatchCandidate>>,
    query: String,
    limit: usize,
    cx: &App,
) -> Task<Vec<StringMatch>> {
    let executor = cx.background_executor().clone();
    cx.background_spawn(async move {
        let cancel = AtomicBool::new(false);
        fuzzy::match_strings(&candidates, &query, true, true, limit, &cancel, executor).await
    })
}

#[cfg(test)]
pub mod tests {
    use gpui::{TestAppContext, VisualTestContext};
    use ui::{HighlightedLabel, ListItem, ListItemSpacing};

    use super::*;
    use crate::tests::init_test;

    /// Picks a word from a list.
    pub struct Words {
        words: Arc<Vec<StringMatchCandidate>>,
        pub matches: Vec<StringMatch>,
        selected: usize,
        pub picked: Option<String>,
        pub dismissed: bool,
    }

    impl Words {
        pub fn new(words: &[&str]) -> Self {
            Words {
                words: Arc::new(
                    words
                        .iter()
                        .enumerate()
                        .map(|(i, w)| StringMatchCandidate::new(i, w))
                        .collect(),
                ),
                matches: Vec::new(),
                selected: 0,
                picked: None,
                dismissed: false,
            }
        }
    }

    impl PickerDelegate for Words {
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
            "Find a word…".into()
        }

        fn update_matches(
            &mut self,
            query: String,
            _: &mut Window,
            cx: &mut Context<Picker<Self>>,
        ) -> Task<()> {
            let found = match_strings(self.words.clone(), query, cx);
            cx.spawn(async move |picker, cx| {
                let matches = found.await;
                let _ = picker.update(cx, |picker, _| {
                    picker.delegate.matches = matches;
                    picker.delegate.selected = 0;
                });
            })
        }

        fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
            self.picked = self.matches.get(self.selected).map(|m| m.string.clone());
            cx.emit(DismissEvent);
        }

        fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {
            self.dismissed = true;
        }

        fn render_match(
            &self,
            ix: usize,
            selected: bool,
            _: &mut Window,
            _: &mut Context<Picker<Self>>,
        ) -> Option<ListItem> {
            let m = self.matches.get(ix)?;
            Some(
                ListItem::new(ix)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .child(HighlightedLabel::new(m.string.clone(), m.positions.clone())),
            )
        }
    }

    fn picker(cx: &mut TestAppContext) -> (Entity<Picker<Words>>, &mut VisualTestContext) {
        init_test(cx);
        let (picker, cx) = cx.add_window_view(|window, cx| {
            Picker::uniform_list(
                Words::new(&["lead", "ui-dev", "docs-writer", "load-test"]),
                window,
                cx,
            )
        });
        cx.update(|window, cx| window.focus(&picker.focus_handle(cx), cx));
        cx.run_until_parked();
        (picker, cx)
    }

    fn listed(picker: &Entity<Picker<Words>>, cx: &mut VisualTestContext) -> Vec<String> {
        picker.read_with(cx, |p, _| {
            p.delegate.matches.iter().map(|m| m.string.clone()).collect()
        })
    }

    #[gpui::test]
    async fn filters_as_you_type_and_highlights(cx: &mut TestAppContext) {
        let (picker, cx) = picker(cx);
        assert_eq!(listed(&picker, cx).len(), 4, "all of them before typing");
        cx.simulate_input("ld");
        assert_eq!(listed(&picker, cx), ["lead", "load-test"]);
        picker.read_with(cx, |p, _| {
            assert_eq!(p.delegate.matches[0].positions, [0, 3], "l and d of lead");
        });
        cx.simulate_input("x");
        assert!(listed(&picker, cx).is_empty());
        cx.simulate_keystrokes("backspace backspace backspace");
        assert_eq!(listed(&picker, cx).len(), 4);
    }

    #[gpui::test]
    async fn keys_move_the_selection_and_enter_picks(cx: &mut TestAppContext) {
        let (picker, cx) = picker(cx);
        let dismissed = std::rc::Rc::new(std::cell::Cell::new(false));
        cx.update(|window, cx| {
            let dismissed = dismissed.clone();
            window
                .subscribe(&picker, cx, move |_, _: &DismissEvent, _, _| dismissed.set(true))
                .detach();
        });
        cx.simulate_input("d");
        let matches = listed(&picker, cx);
        assert!(matches.len() >= 3, "{matches:?}");
        cx.simulate_keystrokes("down down");
        assert_eq!(picker.read_with(cx, |p, _| p.delegate.selected), 2);
        cx.simulate_keystrokes("up up up");
        assert_eq!(
            picker.read_with(cx, |p, _| p.delegate.selected),
            matches.len() - 1,
            "wraps round"
        );
        cx.simulate_keystrokes("enter");
        let picked = picker.read_with(cx, |p, _| p.delegate.picked.clone());
        assert_eq!(picked.as_ref(), matches.last());
        assert!(dismissed.get());
    }

    #[gpui::test]
    async fn escape_dismisses_without_a_pick(cx: &mut TestAppContext) {
        let (picker, cx) = picker(cx);
        cx.simulate_keystrokes("escape");
        picker.read_with(cx, |p, _| {
            assert!(p.delegate.dismissed && p.delegate.picked.is_none());
        });
    }
}
