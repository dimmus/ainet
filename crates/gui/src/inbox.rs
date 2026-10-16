//! The Inbox as a tab: [`InboxItem`], opened by `workspace::OpenInbox`,
//! the waiting badge in the title and status bars, a toast's "Review", or
//! `ctrl-y`/`ctrl-a`/`ctrl-n` from anywhere else. One card per item of the
//! Store's inbox, drawn like the tool-call approval cards of Zed's agent
//! panel: the agent's critter and name, a chip of the kind, the summary,
//! the whole command, plan or edit in a scrolling block, and while it
//! waits on you Allow, Always and Deny with their keys. A question offers
//! its options as choice cards and a field for your own answer.
//!
//! Requests waiting on you come first, oldest first (the one the TUI's
//! keys go to); everything answered or only for your information is
//! history, newest first, folded away until you open it. The answers are
//! the TUI's: `Store::answer` for permissions and plans,
//! `command::answer_question` for questions.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::f32::consts::TAU;
use std::time::Duration;

use ainet_core::{self as core, Decision, InboxKind};
use ainet_model::command;
use ainet_model::describe::age;
use gpui::{
    Action, Animation, AnimationExt, ClickEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, KeyDownEvent, MouseButton, ScrollHandle, Subscription,
};
use ui::{Chip, ChoiceCard, Disclosure, KeyBinding, ListItem, ListItemSpacing, Tooltip, prelude::*};

use crate::actions::{Allow, Always, Answer, Deny, OpenTerminal};
use crate::critter::{self, Critter};
use crate::fleet::{FleetModel, Tick};
use crate::item::{Item, ItemEvent, TabContentParams};
use crate::persistence::SavedItem;
use crate::text_input::{InputEvent, TextInput};
use crate::themes;

/// How long the waiting badge glows when a request asks for you, and how
/// many times it pulses meanwhile: the TUI's glowing Inbox tab.
pub const PULSE: Duration = Duration::from_millis(1800);
const PULSES: f32 = 3.;

/// How tall a card's detail grows before it scrolls.
const DETAIL_HEIGHT: f32 = 18.;

/// The glow of the waiting badge `delta` (0..=1) of the way through
/// [`PULSE`]: from nothing up to full and back, [`PULSES`] times.
pub fn glow(delta: f32) -> f32 {
    (1. - (delta.clamp(0., 1.) * PULSES * TAU).cos()) / 2.
}

/// The cards in the order shown: indexes into the inbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Order {
    /// Waiting on you, oldest first.
    pub waiting: Vec<usize>,
    /// The rest, newest first.
    pub history: Vec<usize>,
}

pub fn order(inbox: &[core::InboxItem]) -> Order {
    let (waiting, mut history): (Vec<usize>, Vec<usize>) = (0..inbox.len()).partition(|&i| inbox[i].waiting);
    history.reverse();
    Order { waiting, history }
}

/// How a line of an item's detail is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    /// An edit's added, removed and hunk lines.
    Added,
    Removed,
    Hunk,
    /// A command, or code in a plan: the buffer font.
    Code,
    /// A plan's heading or list item (the marker dropped).
    Heading,
    Bullet,
    Text,
}

/// The detail of an item as styled lines: a permission's command or edit
/// coloured as the TUI's (`+`, `-`, `@`), a plan as light markdown
/// (headings, lists, fenced code), anything else as text.
pub fn detail_lines(kind: InboxKind, detail: &str) -> Vec<(LineStyle, String)> {
    let mut fenced = false;
    let mut lines = Vec::new();
    for line in detail.lines() {
        let styled = match kind {
            InboxKind::Permission => match line.chars().next() {
                Some('+') => LineStyle::Added,
                Some('-') => LineStyle::Removed,
                Some('@') => LineStyle::Hunk,
                _ => LineStyle::Code,
            },
            InboxKind::Plan => {
                let trimmed = line.trim_start();
                if trimmed.starts_with("```") {
                    fenced = !fenced;
                    continue;
                }
                if fenced {
                    LineStyle::Code
                } else if trimmed.starts_with('#') {
                    lines.push((
                        LineStyle::Heading,
                        trimmed.trim_start_matches('#').trim().to_string(),
                    ));
                    continue;
                } else if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| trimmed.strip_prefix(m)) {
                    lines.push((LineStyle::Bullet, rest.to_string()));
                    continue;
                } else {
                    LineStyle::Text
                }
            }
            _ => LineStyle::Text,
        };
        lines.push((styled, line.to_string()));
    }
    lines
}

/// Picks option `option` of a question: one more of several (or one fewer,
/// picked again), or the only one.
pub fn pick(picked: &mut BTreeSet<usize>, option: usize, multi: bool) {
    if multi {
        if !picked.remove(&option) {
            picked.insert(option);
        }
    } else {
        picked.clear();
        picked.insert(option);
    }
}

/// The colours of a kind's chip, as the TUI's: (label, background).
fn kind_colors(kind: InboxKind, cx: &App) -> (Color, Hsla) {
    let status = cx.theme().status();
    match kind {
        InboxKind::Permission => (Color::Warning, status.warning_background),
        InboxKind::Plan => (Color::Info, status.info_background),
        InboxKind::Question => (Color::Hint, status.hint_background),
        InboxKind::Finished => (Color::Muted, cx.theme().colors().element_background),
        InboxKind::Failed => (Color::Error, status.error_background),
    }
}

/// The chip of an item's kind, faded once it no longer waits.
fn kind_chip(item: &core::InboxItem, cx: &App) -> Chip {
    let (label, bg) = kind_colors(item.kind, cx);
    let bg = if item.waiting || item.kind == InboxKind::Finished {
        bg
    } else {
        bg.opacity(0.5)
    };
    Chip::new(item.kind.label().to_uppercase())
        .label_size(LabelSize::XSmall)
        .label_color(label)
        .bg_color(bg)
        .border_color(bg)
}

/// "N waiting" with a bell, glowing [`PULSES`] times each time a request
/// asks for you (`alerts` counts them, so each one starts the glow anew);
/// a click opens the Inbox.
pub fn waiting_badge(
    id: &'static str,
    waiting: usize,
    alerts: usize,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let glow_color = cx.theme().status().warning_background;
    let badge = h_flex()
        .id(id)
        .gap_1()
        .px_1()
        .rounded_sm()
        .cursor_pointer()
        .hover(|el| el.bg(cx.theme().colors().ghost_element_hover))
        .tooltip(|_, cx| Tooltip::for_action("Open the Inbox", &crate::actions::OpenInbox, cx))
        .on_click(on_click)
        .child(
            Icon::new(IconName::BellDot)
                .size(IconSize::Small)
                .color(Color::Warning),
        )
        .child(
            Label::new(format!("{waiting} waiting"))
                .size(LabelSize::Small)
                .color(Color::Warning),
        );
    if alerts == 0 {
        return badge.into_any_element();
    }
    badge
        .with_animation(
            ElementId::NamedInteger(format!("{id}-glow").into(), alerts as u64),
            Animation::new(PULSE),
            move |el, delta| el.bg(glow_color.opacity(glow(delta))),
        )
        .into_any_element()
}

/// A question's field for your own answer.
struct AnswerField {
    input: Entity<TextInput>,
    _subscription: Subscription,
}

pub struct InboxItem {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
    /// The card the keys answer: an inbox item's id.
    selected: Option<u64>,
    /// Whether the selected card was waiting on you when last seen, to
    /// move on to the next once it is answered.
    selected_waiting: bool,
    history_open: bool,
    /// History cards opened to show their detail.
    opened: HashSet<u64>,
    /// The options picked per waiting question, by index.
    picked: HashMap<u64, BTreeSet<usize>>,
    answers: HashMap<u64, AnswerField>,
    scroll: ScrollHandle,
    /// The scrolling list's children as last drawn: the card each one is.
    children: Vec<Option<u64>>,
    animation: critter::Animation,
    _subscriptions: Vec<Subscription>,
}

impl InboxItem {
    pub fn new(fleet: Entity<FleetModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_in(&fleet, window, |this, _, window, cx| {
                this.keep_focus(window, cx);
                this.sync(cx);
                cx.emit(ItemEvent::UpdateTab);
                cx.notify();
            }),
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.notify();
                }
            }),
        ];
        let mut inbox = InboxItem {
            fleet,
            focus_handle: cx.focus_handle(),
            selected: None,
            selected_waiting: false,
            history_open: false,
            opened: HashSet::new(),
            picked: HashMap::new(),
            answers: HashMap::new(),
            scroll: ScrollHandle::new(),
            children: Vec::new(),
            animation: critter::Animation::default(),
            _subscriptions: subscriptions,
        };
        inbox.sync(cx);
        inbox
    }

    /// The selected card's inbox item id.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn history_open(&self) -> bool {
        self.history_open
    }

    /// The options picked for question `id`, by index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn picked(&self, id: u64) -> Vec<usize> {
        self.picked
            .get(&id)
            .map(|p| p.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The field for your own answer to question `id`, once drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn answer_input(&self, id: u64) -> Option<Entity<TextInput>> {
        self.answers.get(&id).map(|a| a.input.clone())
    }

    /// The ids of the cards shown, top to bottom.
    pub fn shown(&self, cx: &App) -> Vec<u64> {
        let inbox = &self.fleet.read(cx).store.inbox;
        let order = order(inbox);
        let history = if self.history_open {
            order.history
        } else {
            Vec::new()
        };
        order
            .waiting
            .into_iter()
            .chain(history)
            .map(|i| inbox[i].id)
            .collect()
    }

    fn item(&self, id: u64, cx: &App) -> Option<core::InboxItem> {
        self.fleet
            .read(cx)
            .store
            .inbox
            .iter()
            .find(|i| i.id == id)
            .cloned()
    }

    /// An answer field about to go (its question no longer waits) hands
    /// the keyboard back to the cards, or it would go nowhere.
    fn keep_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        let going = self.answers.iter().any(|(id, field)| {
            !store.inbox.iter().any(|i| i.id == *id && i.waiting)
                && field.input.focus_handle(cx).contains_focused(window, cx)
        });
        if going {
            window.focus(&self.focus_handle, cx);
        }
    }

    /// Keeps the selection on a card that shows, moving on to the next
    /// request once the selected one is answered, and forgets what was
    /// picked or typed for questions no longer waiting.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        let waiting: HashSet<u64> = store.inbox.iter().filter(|i| i.waiting).map(|i| i.id).collect();
        self.picked.retain(|id, _| waiting.contains(id));
        self.answers.retain(|id, _| waiting.contains(id));
        let first_waiting = store.inbox.iter().find(|i| i.waiting).map(|i| i.id);
        let selected = self
            .selected
            .and_then(|id| store.inbox.iter().find(|i| i.id == id));
        let keep = match selected {
            None => false,
            Some(item) if item.waiting => true,
            // Answered: on to the next request, if there is one.
            Some(_) if self.selected_waiting && first_waiting.is_some() => false,
            Some(_) => self.history_open,
        };
        if !keep {
            self.selected = first_waiting.or_else(|| {
                self.history_open
                    .then(|| order(&store.inbox).history.first().map(|&i| store.inbox[i].id))
                    .flatten()
            });
        }
        self.selected_waiting = self.selected.is_some_and(|id| waiting.contains(&id));
    }

    /// Selects card `id` (showing the history if it is there) and scrolls
    /// it into view.
    pub fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id, cx) else { return };
        if !item.waiting {
            self.history_open = true;
        }
        self.selected = Some(id);
        self.selected_waiting = item.waiting;
        self.scroll_to_selected();
        cx.notify();
    }

    fn scroll_to_selected(&self) {
        if let Some(ix) = self
            .children
            .iter()
            .position(|c| *c == self.selected && c.is_some())
        {
            self.scroll.scroll_to_item(ix);
        }
    }

    /// Moves the selection `delta` cards down (up if negative).
    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        if shown.is_empty() {
            return;
        }
        let at = self.selected.and_then(|id| shown.iter().position(|s| *s == id));
        let next = match at {
            Some(at) => (at as isize + delta).clamp(0, shown.len() as isize - 1) as usize,
            None => 0,
        };
        self.select(shown[next], cx);
    }

    fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        let edge = if last { shown.last() } else { shown.first() };
        if let Some(&id) = edge {
            self.select(id, cx);
        }
    }

    fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.history_open = !self.history_open;
        self.sync(cx);
        cx.notify();
    }

    /// Answers card `id` with `decision`, as the TUI's `^Y`/`^A`/`^N`:
    /// says why when it cannot be answered so (a question wants its
    /// answer; history is not waiting).
    fn answer(&mut self, id: u64, decision: Decision, cx: &mut Context<Self>) {
        let Some(item) = self.item(id, cx) else { return };
        self.fleet
            .update(cx, |fleet, cx| match fleet.store.answer(&item, decision) {
                Ok((req, ok)) => fleet.request(req, ok, cx),
                // Answering what no longer waits is a mistake; a question
                // just wants its answer.
                Err(why) => fleet.show(why, !item.waiting, cx),
            });
    }

    /// Answers the selected card, or says nothing waits.
    fn answer_selected(&mut self, decision: Decision, cx: &mut Context<Self>) {
        match self.selected {
            Some(id) => self.answer(id, decision, cx),
            None => self.fleet.update(cx, |fleet, cx| {
                fleet.show("nothing is waiting on you".into(), false, cx)
            }),
        }
    }

    /// Sends question `id` your own words if you typed some, else the
    /// options picked.
    fn submit(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id, cx) else { return };
        if !item.waiting {
            let why = "that one is not waiting on you";
            self.fleet
                .update(cx, |fleet, cx| fleet.show(why.into(), true, cx));
            return;
        }
        let typed = self
            .answers
            .get(&id)
            .map(|a| a.input.read(cx).text().trim().to_string())
            .unwrap_or_default();
        let sent = if typed.is_empty() {
            let picked: Vec<usize> = self.picked(id);
            command::answer_choices(&item, &picked)
        } else {
            command::answer_question(&item, typed)
        };
        self.fleet.update(cx, |fleet, cx| fleet.run(sent, cx));
    }

    /// Enter: answers a waiting question, opens or folds a history card,
    /// and on a request says which keys answer it.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected.and_then(|id| self.item(id, cx)) else {
            return;
        };
        if !item.waiting {
            self.toggle_opened(item.id, cx);
        } else if item.kind == InboxKind::Question {
            self.submit(item.id, cx);
        } else {
            let how = "allow (ctrl-y), always allow (ctrl-a) or deny (ctrl-n)";
            self.fleet
                .update(cx, |fleet, cx| fleet.show(how.into(), false, cx));
        }
    }

    fn toggle_opened(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.opened.remove(&id) {
            self.opened.insert(id);
        }
        cx.notify();
    }

    /// Picks option `option` of question `id`.
    fn pick(&mut self, id: u64, option: usize, cx: &mut Context<Self>) {
        let Some(item) = self
            .item(id, cx)
            .filter(|i| i.waiting && option < i.options.len())
        else {
            return;
        };
        pick(self.picked.entry(id).or_default(), option, item.multi_select);
        cx.notify();
    }

    /// Digits pick a question's options while the Inbox (not a field)
    /// has the keyboard, as the TUI's `1-9`.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if !self.focus_handle.is_focused(window) || keystroke.modifiers.modified() {
            return;
        }
        let Some(digit) = keystroke
            .key
            .parse::<usize>()
            .ok()
            .filter(|d| (1..=9).contains(d))
        else {
            return;
        };
        if let Some(id) = self.selected {
            self.pick(id, digit - 1, cx);
            cx.stop_propagation();
        }
    }

    /// The field for your own answer to question `id`.
    fn answer_field(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(field) = self.answers.get(&id) {
            return field.input.clone();
        }
        let input = cx.new(|cx| TextInput::new(window, cx).with_placeholder("Or type your own answer…"));
        let subscription = cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
            if *event == InputEvent::Submitted {
                this.submit(id, cx);
            }
        });
        self.answers.insert(
            id,
            AnswerField {
                input: input.clone(),
                _subscription: subscription,
            },
        );
        input
    }

    fn render_empty(&self, cx: &App) -> Div {
        let fleet = self.fleet.read(cx);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Critter::new("inbox-zero", fleet.tick(), false, cx)
                    .size(px(64.))
                    .animate(&self.animation),
            )
            .child(Label::new("Inbox zero. Nothing needs you.").color(Color::Muted))
    }

    /// A card: who asks and what, the detail, and the answers while it
    /// waits on you.
    fn render_card(
        &mut self,
        item: &core::InboxItem,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = item.id;
        let focused = self.focus_handle.contains_focused(window, cx);
        let colors = cx.theme().colors();
        let border = if selected {
            if focused {
                colors.border_focused
            } else {
                colors.border_selected
            }
        } else {
            colors.border.opacity(0.8)
        };
        // Zed's tool cards: a faint header, the editor's background.
        let header_bg = colors
            .element_background
            .blend(colors.editor_foreground.opacity(0.025));
        let (rule, background) = (colors.border.opacity(0.8), colors.editor_background);
        let fleet = self.fleet.read(cx);
        let agent = fleet.store.agents.get(&item.agent);
        let critter = agent.map(|a| {
            Critter::agent(a, fleet.tick(), cx)
                .size(px(20.))
                .animate(&self.animation)
        });
        let name = agent
            .map(|a| a.name.clone())
            .unwrap_or_else(|| item.agent.chars().take(8).collect());
        let hue = themes::agent_color(&item.agent, cx).cursor;
        let created = item.created;
        let history = !item.waiting;

        let header = h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .bg(header_bg)
            .children(critter)
            .child(
                Label::new(name)
                    .weight(FontWeight::SEMIBOLD)
                    .color(Color::Custom(hue)),
            )
            .child(kind_chip(item, cx))
            .child(div().flex_1())
            .child(
                Label::new(format!("{} ago", age(created)))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .when(history, |el| {
                el.child(
                    Disclosure::new(("inbox-card-fold", id), true)
                        .opened_icon(IconName::ChevronUp)
                        .closed_icon(IconName::ChevronDown)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_opened(id, cx))),
                )
            });

        let summary = div().px_2().py_1p5().child(Label::new(item.summary.clone()));
        let lines = detail_lines(item.kind, &item.detail);
        let shows_detail = !lines.is_empty() && item.detail.trim() != item.summary.trim();
        let detail = shows_detail.then(|| {
            v_flex()
                .id(("inbox-detail", id))
                .max_h(rems(DETAIL_HEIGHT))
                .overflow_y_scroll()
                .px_2()
                .py_1p5()
                .gap_0p5()
                .border_t_1()
                .border_color(rule)
                .children(
                    lines
                        .into_iter()
                        .map(|(style, text)| render_line(style, text, cx)),
                )
        });

        let footer = if item.waiting && item.kind == InboxKind::Question {
            Some(self.render_question(item, selected, window, cx))
        } else if item.waiting {
            Some(self.render_answers(item, selected, cx))
        } else if item.kind == InboxKind::Question {
            Some(
                div()
                    .px_2()
                    .py_1p5()
                    .border_t_1()
                    .border_color(rule)
                    .child(
                        Label::new("A message for you: answer it from Comms")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        v_flex()
            .id(("inbox-card", id))
            .debug_selector(move || format!("INBOX-CARD-{id}"))
            .w_full()
            // In the scrolling column a card keeps its height: let to
            // shrink, cards that do not all fit were squeezed until
            // their detail and buttons were cut off.
            .flex_none()
            .rounded_md()
            .border_1()
            .when(item.kind == InboxKind::Failed, |el| el.border_dashed())
            .border_color(border)
            .bg(background)
            .overflow_hidden()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(header)
            .child(summary)
            .children(detail)
            .children(footer)
            .into_any_element()
    }

    /// Allow, Always and Deny, with their keys on the selected card, and a
    /// way to the agent's own terminal.
    fn render_answers(&self, item: &core::InboxItem, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let focus = self.focus_handle.clone();
        let button = |label: &'static str,
                      icon: IconName,
                      color: Color,
                      decision: Decision,
                      action: &dyn Action,
                      cx: &mut Context<Self>| {
            let button = Button::new(SharedString::from(format!("inbox-{label}-{id}")), label)
                .start_icon(Icon::new(icon).size(IconSize::XSmall).color(color))
                .label_size(LabelSize::Small)
                .when(selected, |el| {
                    el.key_binding(KeyBinding::for_action_in(action, &focus, cx).size(rems_from_px(12_f32)))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select(id, cx);
                    this.answer(id, decision, cx);
                }));
            // A button cannot be named for tests itself; its box can.
            div()
                .debug_selector(move || format!("INBOX-{}-{id}", label.to_uppercase()))
                .child(button)
        };
        let agent = item.agent.clone();
        h_flex()
            .p_1()
            .gap_0p5()
            .border_t_1()
            .border_color(cx.theme().colors().border.opacity(0.8))
            .child(button(
                "Allow",
                IconName::Check,
                Color::Success,
                Decision::Allow,
                &Allow,
                cx,
            ))
            .child(button(
                "Always",
                IconName::CheckDouble,
                Color::Success,
                Decision::Always,
                &Always,
                cx,
            ))
            .child(button(
                "Deny",
                IconName::Close,
                Color::Error,
                Decision::Deny,
                &Deny,
                cx,
            ))
            .child(div().flex_1())
            .child(open_terminal_button(id, agent, self.fleet.clone()))
            .into_any_element()
    }

    /// A waiting question: its options to pick, a field for your own
    /// words, and the buttons to answer or leave it to the terminal.
    fn render_question(
        &mut self,
        item: &core::InboxItem,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = item.id;
        let picked = self.picked.get(&id).cloned().unwrap_or_default();
        let input = self.answer_field(id, window, cx);
        let focus = self.focus_handle.clone();
        let colors = cx.theme().colors();
        let (rule, border, background) = (
            colors.border.opacity(0.8),
            colors.border,
            colors.editor_background,
        );
        let options: Vec<ChoiceCard> = item
            .options
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let label = format!("{}. {option}", i + 1);
                let chosen = picked.contains(&i);
                let card = if item.multi_select {
                    ChoiceCard::checkbox(
                        SharedString::from(format!("inbox-option-{id}-{i}")),
                        label,
                        chosen,
                    )
                } else {
                    ChoiceCard::radio(
                        SharedString::from(format!("inbox-option-{id}-{i}")),
                        label,
                        chosen,
                    )
                };
                card.on_click(cx.listener(move |this, _, _, cx| {
                    this.select(id, cx);
                    this.pick(id, i, cx);
                }))
            })
            .collect();
        let how = match (item.options.is_empty(), item.multi_select) {
            (true, _) => "type your answer, then enter",
            (false, true) => "pick any (1–9), or type your own answer",
            (false, false) => "pick one (1–9), or type your own answer",
        };
        let agent = item.agent.clone();
        v_flex()
            .p_2()
            .gap_1p5()
            .border_t_1()
            .border_color(rule)
            .child(v_flex().gap_1().children(options))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(border)
                    .bg(background)
                    .child(input),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .child(
                        Button::new(("inbox-answer", id), "Answer")
                            .start_icon(
                                Icon::new(IconName::Send)
                                    .size(IconSize::XSmall)
                                    .color(Color::Accent),
                            )
                            .label_size(LabelSize::Small)
                            .when(selected, |el| {
                                el.key_binding(
                                    KeyBinding::for_action_in(&Answer, &focus, cx).size(rems_from_px(12_f32)),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(id, cx);
                                this.submit(id, cx);
                            })),
                    )
                    .child(
                        Button::new(("inbox-leave", id), "Leave to Terminal")
                            .start_icon(
                                Icon::new(IconName::Close)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .label_size(LabelSize::Small)
                            .when(selected, |el| {
                                el.key_binding(
                                    KeyBinding::for_action_in(&Deny, &focus, cx).size(rems_from_px(12_f32)),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(id, cx);
                                this.answer(id, Decision::Deny, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(Label::new(how).size(LabelSize::XSmall).color(Color::Muted))
                    .child(open_terminal_button(id, agent, self.fleet.clone())),
            )
            .into_any_element()
    }

    /// An answered item, folded to one line.
    fn render_row(&self, item: &core::InboxItem, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let store = &self.fleet.read(cx).store;
        let name = store
            .agents
            .get(&item.agent)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| item.agent.chars().take(8).collect());
        let hue = themes::agent_color(&item.agent, cx).cursor;
        let (mark, color) = if item.kind == InboxKind::Failed {
            (IconName::XCircle, Color::Error)
        } else {
            (IconName::Check, Color::Muted)
        };
        div()
            .debug_selector(move || format!("INBOX-CARD-{id}"))
            .flex_none()
            .child(
                ListItem::new(("inbox-row", id))
                    .spacing(ListItemSpacing::Sparse)
                    .rounded()
                    .toggle_state(selected)
                    .start_slot(Icon::new(mark).size(IconSize::Small).color(color))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(id, cx);
                        this.toggle_opened(id, cx);
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(kind_chip(item, cx))
                            .child(Label::new(name).size(LabelSize::Small).color(Color::Custom(hue)))
                            .child(
                                Label::new(item.summary.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted)
                                    .truncate(),
                            )
                            .child(div().flex_1())
                            .child(
                                Label::new(age(item.created))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_history_header(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let open = self.history_open;
        h_flex()
            .id("inbox-history")
            .flex_none()
            .gap_1()
            .pt_1()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx)))
            .child(
                Disclosure::new("inbox-history-disclosure", open)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx))),
            )
            .child(Label::new("History").size(LabelSize::Small).color(Color::Muted))
            .child(
                Label::new(count.to_string())
                    .size(LabelSize::Small)
                    .color(Color::Disabled),
            )
            .into_any_element()
    }
}

/// A button opening the asking agent's terminal: you can always answer
/// there instead.
fn open_terminal_button(id: u64, agent: String, fleet: Entity<FleetModel>) -> IconButton {
    IconButton::new(("inbox-terminal", id), IconName::Terminal)
        .icon_size(IconSize::Small)
        .icon_color(Color::Muted)
        .tooltip(Tooltip::text("Answer in the agent's terminal"))
        .on_click(move |_, window, cx| {
            fleet.update(cx, |fleet, cx| fleet.select(&agent, cx));
            window.dispatch_action(OpenTerminal.boxed_clone(), cx);
        })
}

/// One line of an item's detail.
fn render_line(style: LineStyle, text: String, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let status = cx.theme().status();
    let code = |color: Hsla| {
        div()
            .font_buffer(cx)
            .text_ui_sm(cx)
            .text_color(color)
            .child(text.clone())
            .into_any_element()
    };
    match style {
        LineStyle::Added => code(status.created),
        LineStyle::Removed => code(status.deleted),
        LineStyle::Hunk => code(status.info),
        LineStyle::Code => code(colors.text),
        LineStyle::Heading => Label::new(text).weight(FontWeight::SEMIBOLD).into_any_element(),
        LineStyle::Bullet => h_flex()
            .items_start()
            .gap_1p5()
            .child(Label::new("•").color(Color::Muted))
            .child(Label::new(text))
            .into_any_element(),
        LineStyle::Text if text.trim().is_empty() => div().h_2().into_any_element(),
        LineStyle::Text => Label::new(text).into_any_element(),
    }
}

impl EventEmitter<ItemEvent> for InboxItem {}

impl Focusable for InboxItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for InboxItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        "Inbox".into()
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        Some(SavedItem::Inbox)
    }

    /// "Inbox", and how many wait on you.
    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> AnyElement {
        let waiting = self.fleet.read(cx).tally().waiting;
        h_flex()
            .gap_1p5()
            .child(Label::new("Inbox").color(params.text_color()))
            .when(waiting > 0, |el| {
                el.child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(cx.theme().status().warning_background)
                        .child(
                            Label::new(waiting.to_string())
                                .size(LabelSize::XSmall)
                                .color(Color::Warning),
                        ),
                )
            })
            .into_any_element()
    }

    fn tab_icon(&self, _: &Window, cx: &App) -> Option<AnyElement> {
        let waiting = self.fleet.read(cx).tally().waiting > 0;
        let (icon, color) = if waiting {
            (IconName::BellDot, Color::Warning)
        } else {
            (IconName::Bell, Color::Muted)
        };
        Some(
            Icon::new(icon)
                .size(IconSize::Small)
                .color(color)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some("Requests and questions waiting on you".into())
    }
}

impl Render for InboxItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inbox = self.fleet.read(cx).store.inbox.clone();
        let order = order(&inbox);
        let mut children: Vec<Option<u64>> = Vec::new();
        let mut cards: Vec<AnyElement> = Vec::new();
        for &i in &order.waiting {
            let item = &inbox[i];
            let selected = self.selected == Some(item.id);
            cards.push(self.render_card(item, selected, window, cx));
            children.push(Some(item.id));
        }
        if order.waiting.is_empty() && !order.history.is_empty() {
            cards.push(
                Label::new("Nothing is waiting on you.")
                    .color(Color::Muted)
                    .into_any_element(),
            );
            children.push(None);
        }
        if !order.history.is_empty() {
            cards.push(self.render_history_header(order.history.len(), cx));
            children.push(None);
            if self.history_open {
                for &i in &order.history {
                    let item = &inbox[i];
                    let selected = self.selected == Some(item.id);
                    cards.push(if self.opened.contains(&item.id) {
                        self.render_card(item, selected, window, cx)
                    } else {
                        self.render_row(item, selected, cx)
                    });
                    children.push(Some(item.id));
                }
            }
        }
        self.children = children;
        let waiting = order.waiting.len();
        let colors = cx.theme().colors();
        let (background, rule) = (colors.editor_background, colors.border_variant);

        v_flex()
            .id("inbox")
            .key_context("Inbox")
            .track_focus(&self.focus_handle)
            // A click anywhere in it gives it the keyboard, so its keys
            // answer the card clicked (a field keeps the focus it took).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus_handle.contains_focused(window, cx) {
                        window.focus(&this.focus_handle, cx);
                    }
                }),
            )
            .size_full()
            .bg(background)
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.select_relative(1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.select_relative(-1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectFirst, _, cx| this.select_edge(false, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectLast, _, cx| this.select_edge(true, cx)))
            // Escape in an answer field gives the keyboard back to the cards.
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .on_action(cx.listener(|this, _: &Allow, _, cx| this.answer_selected(Decision::Allow, cx)))
            .on_action(cx.listener(|this, _: &Always, _, cx| this.answer_selected(Decision::Always, cx)))
            .on_action(cx.listener(|this, _: &Deny, _, cx| this.answer_selected(Decision::Deny, cx)))
            .on_action(cx.listener(|this, _: &Answer, _, cx| this.confirm(cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(rule)
                    .child(
                        Icon::new(IconName::Bell)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new("Inbox").weight(FontWeight::SEMIBOLD))
                    .child(
                        Label::new(if waiting > 0 {
                            format!("{waiting} waiting on you")
                        } else {
                            "nothing waiting".to_string()
                        })
                        .size(LabelSize::Small)
                        .color(if waiting > 0 {
                            Color::Warning
                        } else {
                            Color::Muted
                        }),
                    ),
            )
            .map(|el| {
                if inbox.is_empty() {
                    el.child(self.render_empty(cx))
                } else {
                    el.child(
                        v_flex()
                            .id("inbox-cards")
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .flex_1()
                            .min_h_0()
                            .p_3()
                            .gap_3()
                            .children(cards),
                    )
                }
            })
    }
}

#[cfg(test)]
mod pure_tests {
    use super::*;

    fn item(id: u64, kind: InboxKind, waiting: bool) -> core::InboxItem {
        core::InboxItem {
            id,
            agent: "a".into(),
            kind,
            summary: format!("item {id}"),
            detail: String::new(),
            created: id as i64,
            waiting,
            options: vec![],
            multi_select: false,
        }
    }

    #[test]
    fn waiting_comes_first_oldest_first_then_history_newest_first() {
        let inbox = [
            item(1, InboxKind::Finished, false),
            item(2, InboxKind::Permission, true),
            item(3, InboxKind::Question, false),
            item(4, InboxKind::Plan, true),
        ];
        assert_eq!(
            order(&inbox),
            Order {
                waiting: vec![1, 3],
                history: vec![2, 0],
            }
        );
        assert_eq!(order(&[]), Order::default());
    }

    #[test]
    fn details_are_styled_by_kind() {
        let edit = "src/a.rs\n@@ -1 +1 @@\n-old\n+new";
        let styles: Vec<LineStyle> = detail_lines(InboxKind::Permission, edit)
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        use LineStyle::*;
        assert_eq!(styles, [Code, Hunk, Removed, Added]);

        let plan = "# Plan\n\n1. Read\n- add tests\n* ship\n```\nlet x = 1;\n```\nDone";
        assert_eq!(
            detail_lines(InboxKind::Plan, plan),
            [
                (Heading, "Plan".to_string()),
                (Text, String::new()),
                (Text, "1. Read".into()),
                (Bullet, "add tests".into()),
                (Bullet, "ship".into()),
                (Code, "let x = 1;".into()),
                (Text, "Done".into()),
            ]
        );
        // A finished message keeps its dashes as text.
        assert_eq!(detail_lines(InboxKind::Finished, "- done")[0].0, Text);
    }

    #[test]
    fn picks_one_or_several() {
        let mut one = BTreeSet::new();
        pick(&mut one, 0, false);
        pick(&mut one, 2, false);
        assert_eq!(one.iter().copied().collect::<Vec<_>>(), [2]);
        pick(&mut one, 2, false);
        assert_eq!(one.len(), 1, "picking it again keeps it");

        let mut many = BTreeSet::new();
        pick(&mut many, 0, true);
        pick(&mut many, 2, true);
        pick(&mut many, 0, true);
        assert_eq!(many.iter().copied().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn the_badge_pulses_and_settles() {
        assert!(glow(0.) < 1e-6);
        assert!(glow(1.) < 1e-4, "dark again at the end");
        let peak = 1. / (2. * PULSES);
        assert!((glow(peak) - 1.).abs() < 1e-4);
        let peaks = (0..=600)
            .map(|i| glow(i as f32 / 600.))
            .collect::<Vec<_>>()
            .windows(3)
            .filter(|w| w[1] > w[0] && w[1] >= w[2] && w[1] > 0.9)
            .count();
        assert_eq!(peaks, PULSES as usize);
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::{Event, Request};
    use ainet_model::Msg;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    /// The demo fleet in an active window, the Fleet panel at the keys.
    fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (w, cx)
    }

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    /// The requests sent since the last call.
    fn take_sent(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> Vec<Request> {
        fleet.update(cx, |f, _| {
            std::mem::take(&mut f.sent).into_iter().map(|(r, _)| r).collect()
        })
    }

    fn answers(sent: &[Request]) -> Vec<(u64, Decision)> {
        sent.iter()
            .filter_map(|r| match r {
                Request::Answer { item, decision } => Some((*item, *decision)),
                _ => None,
            })
            .collect()
    }

    fn questions_answered(sent: &[Request]) -> Vec<(u64, String)> {
        sent.iter()
            .filter_map(|r| match r {
                Request::AnswerQuestion { item, answer } => Some((*item, answer.clone())),
                _ => None,
            })
            .collect()
    }

    /// The Inbox, if it is the active tab.
    fn inbox(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<InboxItem>> {
        w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<InboxItem>().ok())
        })
    }

    fn toasts(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
        w.read_with(cx, |w, cx| {
            w.toast_layer()
                .read(cx)
                .texts()
                .into_iter()
                .map(String::from)
                .collect()
        })
    }

    /// The agent in seat `seat`.
    fn seat(fleet: &Entity<FleetModel>, seat: &str, cx: &mut VisualTestContext) -> String {
        fleet.read_with(cx, |f, _| {
            f.store
                .agents
                .values()
                .find(|a| a.seat.as_deref() == Some(seat))
                .map(|a| a.id.clone())
                .unwrap()
        })
    }

    fn arrives(fleet: &Entity<FleetModel>, item: core::InboxItem, cx: &mut VisualTestContext) {
        fleet.update(cx, |f, cx| {
            f.receive(vec![Msg::Event(Box::new(Event::Inbox { item }))], cx)
        });
        cx.run_until_parked();
    }

    fn resolved(fleet: &Entity<FleetModel>, id: u64, cx: &mut VisualTestContext) {
        fleet.update(cx, |f, cx| {
            f.receive(vec![Msg::Event(Box::new(Event::InboxResolved { id }))], cx)
        });
        cx.run_until_parked();
    }

    fn asking(
        id: u64,
        agent: &str,
        kind: InboxKind,
        options: &[&str],
        multi_select: bool,
    ) -> core::InboxItem {
        core::InboxItem {
            id,
            agent: agent.into(),
            kind,
            summary: format!("asks {id}"),
            detail: format!("the whole of {id}"),
            created: 0,
            waiting: true,
            options: options.iter().map(|o| o.to_string()).collect(),
            multi_select,
        }
    }

    #[gpui::test]
    async fn the_keys_open_the_inbox_first_then_answer(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let (waiting, name) = fleet.read_with(cx, |f, _| {
            let ix = f.store.next_waiting().expect("the demo has a request waiting");
            let item = &f.store.inbox[ix];
            (item.id, f.store.agents[&item.agent].name.clone())
        });
        take_sent(&fleet, cx);

        // Outside the Inbox, ctrl-y only opens it on the request.
        assert!(inbox(&w, cx).is_none());
        cx.simulate_keystrokes("ctrl-y");
        let inbox = inbox(&w, cx).expect("the Inbox opened as a tab");
        assert!(
            answers(&take_sent(&fleet, cx)).is_empty(),
            "read before you allow"
        );
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(waiting));
        let focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        assert!(
            cx.update(|window, _| focus.is_focused(window)),
            "the Inbox has the keys"
        );

        // Inside, it answers; the demo resolves the request.
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(answers(&take_sent(&fleet, cx)), [(waiting, Decision::Allow)]);
        cx.run_until_parked();
        let item = fleet.read_with(cx, |f, _| f.store.inbox.iter().find(|i| i.id == waiting).cloned());
        assert!(!item.expect("still in the inbox").waiting, "resolved");
        assert!(toasts(&w, cx).contains(&format!("allowed {name}'s permission")));
        inbox.read_with(cx, |i, cx| {
            assert!(!i.shown(cx).contains(&waiting), "folded into the history");
            assert_eq!(i.selected(), None, "nothing else waits");
        });

        // Nothing waits now, in the Inbox or out of it.
        cx.simulate_keystrokes("ctrl-n");
        assert!(answers(&take_sent(&fleet, cx)).is_empty());
        assert_eq!(
            toasts(&w, cx).last().map(String::as_str),
            Some("nothing is waiting on you")
        );
        let panel = w.read_with(cx, |w, cx| w.fleet_panel().focus_handle(cx));
        cx.update(|window, cx| window.focus(&panel, cx));
        cx.simulate_keystrokes("ctrl-a");
        assert!(answers(&take_sent(&fleet, cx)).is_empty());
        assert!(!cx.update(|window, _| focus.is_focused(window)));
    }

    #[gpui::test]
    async fn the_selected_card_is_answered_and_the_next_one_follows(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let first = fleet.read_with(cx, |f, _| f.store.inbox[f.store.next_waiting().unwrap()].id);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        arrives(&fleet, asking(900, &lead, InboxKind::Plan, &[], false), cx);
        cx.dispatch_action(crate::actions::OpenInbox);
        cx.run_until_parked();
        let inbox = inbox(&w, cx).unwrap();
        inbox.read_with(cx, |i, cx| {
            assert_eq!(i.shown(cx), [first, 900], "waiting, oldest first");
            assert_eq!(i.selected(), Some(first));
        });
        take_sent(&fleet, cx);

        // Down, then deny the plan; up, always allow the first.
        cx.simulate_keystrokes("down");
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(900));
        cx.simulate_keystrokes("ctrl-n up ctrl-a");
        assert_eq!(
            answers(&take_sent(&fleet, cx)),
            [(900, Decision::Deny), (first, Decision::Always)]
        );
        cx.run_until_parked();
        // The demo resolved its own request: the plan is next.
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(900));

        // A click on a card selects it.
        resolved(&fleet, 900, cx);
        arrives(&fleet, asking(901, &lead, InboxKind::Permission, &[], false), cx);
        arrives(&fleet, asking(902, &lead, InboxKind::Permission, &[], false), cx);
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(901));
        let card = cx.debug_bounds("INBOX-CARD-902").expect("the card is drawn");
        cx.simulate_click(card.center(), Modifiers::none());
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(902));
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(answers(&take_sent(&fleet, cx)), [(902, Decision::Allow)]);
    }

    #[gpui::test]
    async fn the_buttons_answer_their_own_card(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let first = fleet.read_with(cx, |f, _| f.store.inbox[f.store.next_waiting().unwrap()].id);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        arrives(&fleet, asking(901, &lead, InboxKind::Permission, &[], false), cx);
        arrives(&fleet, asking(902, &lead, InboxKind::Plan, &[], false), cx);
        cx.dispatch_action(crate::actions::OpenInbox);
        cx.run_until_parked();
        let inbox = inbox(&w, cx).unwrap();
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(first));
        take_sent(&fleet, cx);
        let click = |button: String, cx: &mut VisualTestContext| {
            let bounds = cx
                .debug_bounds(button.clone().leak())
                .unwrap_or_else(|| panic!("{button} is drawn"));
            cx.simulate_click(bounds.center(), Modifiers::none());
        };

        // A button on a card that is not selected answers that card, not
        // the selected one, and selects it.
        click("INBOX-DENY-902".into(), cx);
        assert_eq!(answers(&take_sent(&fleet, cx)), [(902, Decision::Deny)]);
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(902));
        click("INBOX-ALWAYS-901".into(), cx);
        assert_eq!(answers(&take_sent(&fleet, cx)), [(901, Decision::Always)]);
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(901));

        // Allow on the demo's own request: the demo resolves it, and its
        // buttons go with the wait.
        click(format!("INBOX-ALLOW-{first}"), cx);
        assert_eq!(answers(&take_sent(&fleet, cx)), [(first, Decision::Allow)]);
        cx.run_until_parked();
        assert!(cx.debug_bounds(format!("INBOX-ALLOW-{first}").leak()).is_none());
        // A click gives the Inbox the keys, as a click on the card does.
        let focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        assert!(cx.update(|window, cx| focus.contains_focused(window, cx)));
    }

    #[gpui::test]
    async fn cards_keep_their_buttons_when_they_do_not_all_fit(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        // More cards, each with a detail block, than a short window holds.
        let ids: Vec<u64> = (910..918).collect();
        for &id in &ids {
            let mut item = asking(id, &lead, InboxKind::Permission, &[], false);
            item.detail = "go test ./...\n".repeat(6);
            arrives(&fleet, item, cx);
        }
        cx.simulate_resize(gpui::size(px(1000.), px(500.)));
        cx.dispatch_action(crate::actions::OpenInbox);
        cx.run_until_parked();

        // Scrolled into view one by one, every card holds its buttons:
        // squeezed, they were cut off below the card's edge.
        let inbox = inbox(&w, cx).unwrap();
        for &id in &ids {
            inbox.update(cx, |i, cx| i.select(id, cx));
            cx.run_until_parked();
            let card = cx
                .debug_bounds(format!("INBOX-CARD-{id}").leak())
                .expect("the card is drawn");
            let allow = cx
                .debug_bounds(format!("INBOX-ALLOW-{id}").leak())
                .expect("its Allow button is drawn");
            assert!(
                allow.bottom() <= card.bottom() && allow.top() >= card.top(),
                "card {id}: the button {allow:?} lies outside the card {card:?}"
            );
        }
    }

    #[gpui::test]
    async fn questions_take_the_options_picked_or_your_words(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        arrives(
            &fleet,
            asking(2000, &lead, InboxKind::Question, &["a", "b", "c"], true),
            cx,
        );
        arrives(
            &fleet,
            asking(2001, &lead, InboxKind::Question, &["x", "y"], false),
            cx,
        );
        let inbox = w.update_in(cx, |w, window, cx| w.open_inbox(Some(2000), window, cx));
        cx.run_until_parked();
        take_sent(&fleet, cx);

        // Several: digits pick and unpick; enter sends their numbers.
        cx.simulate_keystrokes("1 3 2 2");
        assert_eq!(inbox.read_with(cx, |i, _| i.picked(2000)), [0, 2]);
        cx.simulate_keystrokes("enter");
        assert_eq!(
            questions_answered(&take_sent(&fleet, cx)),
            [(2000, "1 3".to_string())]
        );
        // ctrl-y does not answer a question.
        cx.simulate_keystrokes("ctrl-y");
        assert!(answers(&take_sent(&fleet, cx)).is_empty());
        assert_eq!(
            toasts(&w, cx).last().map(String::as_str),
            Some("type an option's number (or your answer) and Enter")
        );

        // One: a new pick replaces the old; nothing picked says so.
        inbox.update(cx, |i, cx| i.select(2001, cx));
        cx.simulate_keystrokes("enter");
        assert!(questions_answered(&take_sent(&fleet, cx)).is_empty());
        assert_eq!(
            toasts(&w, cx).last().map(String::as_str),
            Some("pick an option or type your answer")
        );
        cx.simulate_keystrokes("1 2");
        assert_eq!(inbox.read_with(cx, |i, _| i.picked(2001)), [1]);

        // Your own words win over the picks; digits typed stay words.
        cx.run_until_parked();
        let field = inbox.read_with(cx, |i, _| i.answer_input(2001)).expect("drawn");
        cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
        cx.simulate_input("cover WSL2 1");
        assert_eq!(inbox.read_with(cx, |i, _| i.picked(2001)), [1]);
        cx.simulate_keystrokes("enter");
        assert_eq!(
            questions_answered(&take_sent(&fleet, cx)),
            [(2001, "cover WSL2 1".to_string())]
        );
        // Escape gives the keys back to the cards; ctrl-n leaves the
        // question to the terminal.
        cx.simulate_keystrokes("escape");
        let focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        assert!(cx.update(|window, _| focus.is_focused(window)));
        cx.simulate_keystrokes("ctrl-n");
        assert_eq!(answers(&take_sent(&fleet, cx)), [(2001, Decision::Deny)]);

        // Once answered, its picks and field are forgotten.
        resolved(&fleet, 2001, cx);
        inbox.read_with(cx, |i, _| {
            assert!(i.picked(2001).is_empty());
            assert!(i.answer_input(2001).is_none());
        });

        // Answered from its field, the keyboard goes back to the cards
        // when the field goes.
        let field = inbox.read_with(cx, |i, _| i.answer_input(2000)).expect("drawn");
        cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
        cx.simulate_input("both");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            questions_answered(&take_sent(&fleet, cx)),
            [(2000, "both".to_string())]
        );
        resolved(&fleet, 2000, cx);
        assert!(inbox.read_with(cx, |i, _| i.answer_input(2000)).is_none());
        assert!(
            cx.update(|window, _| focus.is_focused(window)),
            "the Inbox has the keys"
        );
    }

    #[gpui::test]
    async fn a_click_on_a_card_gives_the_inbox_the_keys(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        arrives(&fleet, asking(901, &lead, InboxKind::Permission, &[], false), cx);
        arrives(&fleet, asking(902, &lead, InboxKind::Permission, &[], false), cx);
        let inbox = w.update_in(cx, |w, window, cx| w.open_inbox(Some(901), window, cx));
        cx.run_until_parked();
        // The keyboard is elsewhere when the card is clicked.
        let panel = w.read_with(cx, |w, cx| w.fleet_panel().focus_handle(cx));
        cx.update(|window, cx| window.focus(&panel, cx));
        cx.run_until_parked();
        take_sent(&fleet, cx);
        let card = cx.debug_bounds("INBOX-CARD-902").expect("the card is drawn");
        cx.simulate_click(card.center(), Modifiers::none());
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(902));
        let focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        assert!(cx.update(|window, _| focus.is_focused(window)));
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(answers(&take_sent(&fleet, cx)), [(902, Decision::Allow)]);
    }

    #[gpui::test]
    async fn the_next_waiting_key_selects_who_asks(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let lead = seat(&fleet, "orch-lead@docs", cx);
        let ui = seat(&fleet, "dev-ui@docs", cx);
        fleet.update(cx, |f, _| {
            for i in f.store.inbox.iter_mut() {
                i.waiting = false;
            }
        });
        cx.simulate_keystrokes("ctrl-shift-y");
        assert_eq!(
            toasts(&w, cx).last().map(String::as_str),
            Some("nothing is waiting on you")
        );
        arrives(&fleet, asking(901, &lead, InboxKind::Permission, &[], false), cx);
        arrives(&fleet, asking(902, &ui, InboxKind::Plan, &[], false), cx);
        fleet.update(cx, |f, cx| f.select(&ui, cx));
        let selected = |cx: &mut VisualTestContext| fleet.read_with(cx, |f, _| f.store.selected.clone());
        cx.simulate_keystrokes("ctrl-shift-y");
        assert_eq!(selected(cx), Some(lead.clone()));
        cx.simulate_keystrokes("ctrl-shift-y");
        assert_eq!(selected(cx), Some(ui));
    }

    #[gpui::test]
    async fn history_folds_away_and_opens(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, Some(crate::args::View::Inbox), window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let fleet = fleet(&w, cx);
        let inbox = inbox(&w, cx).expect("--view inbox opens the Inbox");
        let (waiting, history) = fleet.read_with(cx, |f, _| {
            let order = order(&f.store.inbox);
            let ids = |v: Vec<usize>| v.into_iter().map(|i| f.store.inbox[i].id).collect::<Vec<_>>();
            (ids(order.waiting), ids(order.history))
        });
        assert!(!history.is_empty());
        inbox.read_with(cx, |i, cx| {
            assert!(!i.history_open());
            assert_eq!(i.shown(cx), waiting);
        });
        inbox.update(cx, |i, cx| i.toggle_history(cx));
        inbox.read_with(cx, |i, cx| {
            assert_eq!(i.shown(cx), [waiting.clone(), history.clone()].concat());
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("INBOX-CARD-1").is_some(),
            "history rows are drawn"
        );

        // Enter opens a history card's detail and folds it again.
        inbox.update(cx, |i, cx| i.select(history[0], cx));
        let focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.simulate_keystrokes("enter");
        assert!(inbox.read_with(cx, |i, _| i.opened.contains(&history[0])));
        cx.simulate_keystrokes("enter");
        assert!(!inbox.read_with(cx, |i, _| i.opened.contains(&history[0])));
    }

    #[gpui::test]
    async fn a_request_for_you_toasts_and_glows(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        assert_eq!(
            fleet.read_with(cx, |f, _| f.alerts()),
            0,
            "the snapshot is not news"
        );
        let lead = seat(&fleet, "orch-lead@docs", cx);
        let name = fleet.read_with(cx, |f, _| f.store.agents[&lead].name.clone());
        arrives(&fleet, asking(950, &lead, InboxKind::Permission, &[], false), cx);
        assert_eq!(toasts(&w, cx).last(), Some(&format!("{name}: asks 950")));
        let notice = fleet.read_with(cx, |f, _| f.notices().last().cloned().unwrap());
        assert_eq!(
            notice.attention,
            Some(crate::fleet::Attention {
                agent: lead.clone(),
                item: 950
            })
        );
        assert_eq!(fleet.read_with(cx, |f, _| f.alerts()), 1);

        // What only informs you does not.
        let before = toasts(&w, cx).len();
        let mut done = asking(951, &lead, InboxKind::Finished, &[], false);
        done.waiting = false;
        arrives(&fleet, done, cx);
        assert_eq!(toasts(&w, cx).len(), before);
        assert_eq!(fleet.read_with(cx, |f, _| f.alerts()), 1);

        // The toast's Review opens the Inbox on it.
        w.update_in(cx, |w, window, cx| w.open_inbox(Some(950), window, cx));
        let inbox = inbox(&w, cx).unwrap();
        assert_eq!(inbox.read_with(cx, |i, _| i.selected()), Some(950));
    }
}
