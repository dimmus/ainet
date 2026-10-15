//! Comms as a tab: [`CommsItem`], opened by `workspace::OpenComms`
//! (`ctrl-shift-c`) or `--view comms`. What agents say to each other and
//! to you, as the TUI's Comms stream: direct messages, replies and the
//! rigs' chatrooms interleaved oldest first (`Store::entries`), each with
//! its sender's critter, who to whom in the agents' colours, how long ago,
//! the whole body, and whether it is held or not read yet. A message with
//! replies opens its thread beneath it, indented with guides.
//!
//! The stream is a `gpui::list` (rows of any height, only those on show
//! drawn) that sticks to its end as a chat does, unless you scrolled up.
//! The compose box at the bottom answers the selected entry or messages
//! `@Name`, as the TUI's input line does in Comms (`command::comms`).
//! The queue the TUI shows beside the stream is the bottom dock's.

use std::collections::HashSet;
use std::ops::Range;

use ainet_core::{Agent, Message};
use ainet_model::describe::age;
use ainet_model::{Command, Entry, Store, command};
use gpui::{
    ClickEvent, Entity, EventEmitter, FocusHandle, Focusable, FollowMode, FontWeight, HighlightStyle, Hsla,
    ListAlignment, ListState, MouseButton, Stateful, StyledText, Subscription, list,
};
use ui::{Chip, Disclosure, IndentGuideColors, Indicator, Tooltip, WithScrollbar, prelude::*};

use crate::actions::{Complete, Compose, Reply};
use crate::commands;
use crate::critter::{self, Critter};
use crate::fleet::{FleetModel, Tick};
use crate::item::{Item, ItemEvent};
use crate::text_input::{InputEvent, TextInput};
use crate::themes;

/// How many lines the compose box shows before it scrolls.
const COMPOSE_LINES: usize = 6;
/// How far each level of a thread is indented.
const INDENT: Pixels = px(20.);
/// The sender's critter.
const AVATAR: Pixels = px(20.);
/// The stream's padding left and right.
const GUTTER: Pixels = px(12.);

/// A row of the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    /// A message, in its place in the stream.
    Message(u64),
    /// A chat post.
    Chat(u64),
    /// A reply in the open thread of message `root`, `depth` levels in.
    Reply { root: u64, id: u64, depth: usize },
}

/// The stream's rows: the order of [`Store::entries`] (messages and chat
/// interleaved, oldest first), with the replies of each open thread under
/// its message.
pub fn rows(store: &Store, open: &HashSet<u64>) -> Vec<Row> {
    let mut entries: Vec<(i64, Row)> = store
        .messages
        .iter()
        .map(|m| (m.created, Row::Message(m.id)))
        .chain(store.chat.iter().map(|c| (c.created, Row::Chat(c.id))))
        .collect();
    // Stable, as `entries` sorts, so posts made at the same moment keep
    // the same order in both front ends.
    entries.sort_by_key(|(created, _)| *created);
    let mut rows = Vec::with_capacity(entries.len());
    for (_, row) in entries {
        rows.push(row);
        if let Row::Message(root) = row
            && open.contains(&root)
        {
            rows.extend(store.replies(root).into_iter().map(|(depth, m)| Row::Reply {
                root,
                id: m.id,
                depth,
            }));
        }
    }
    rows
}

/// How the list changes from `old` rows to `new`: the range of old rows
/// replaced and how many new rows take their place, keeping the rows both
/// start and end with, so the scroll position holds.
pub fn splice(old: &[Row], new: &[Row]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let start = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some((start..old.len() - end, new.len() - start - end))
}

/// What just happened to the stream, for [`sticks`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moved {
    /// Entries came or went, or a thread opened or closed.
    Changed,
    /// You sent something.
    Sent,
    /// A row was selected; `last` if it is the last one.
    Selected { last: bool },
}

/// Whether the stream sticks to its end after `moved`, as a chat does.
/// While it is stuck (at the bottom), what arrives keeps it there; once
/// you scroll up (gpui's list notices the wheel, and scrolling back down)
/// or select a row above the last, it stays where you are, and it comes
/// back down when you send something or select the last row.
pub fn sticks(stuck: bool, moved: Moved) -> bool {
    match moved {
        Moved::Changed => stuck,
        Moved::Sent => true,
        Moved::Selected { last } => last,
    }
}

/// Where the `@name`s are in `text`, as byte ranges with the `@`: an `@`
/// not inside a word, then letters, digits and `.-_@` (a seat address
/// has an `@`), without trailing punctuation.
pub fn mentions(text: &str) -> Vec<Range<usize>> {
    let name_char = |c: char| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@');
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find('@') {
        let start = at + found;
        let rest = &text[start + 1..];
        let len = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
        let name = rest[..len].trim_end_matches(['.', '-', '_', '@']);
        let alone = text[..start].chars().next_back().is_none_or(|c| !name_char(c));
        if alone && name.starts_with(char::is_alphanumeric) {
            out.push(start..start + 1 + name.len());
        }
        at = start + 1 + len;
    }
    out
}

/// A part of a message's body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Text(String),
    /// Between ``` fences: drawn in the buffer font.
    Code(String),
}

/// A body as text and fenced code, blank lines at either end of a part
/// dropped.
pub fn blocks(body: &str) -> Vec<Block> {
    fn text(lines: &mut Vec<&str>, out: &mut Vec<Block>) {
        let joined = lines.join("\n");
        let trimmed = joined.trim_matches('\n');
        if !trimmed.trim().is_empty() {
            out.push(Block::Text(trimmed.trim_end().to_string()));
        }
        lines.clear();
    }
    let (mut out, mut lines, mut code, mut fenced) = (Vec::new(), Vec::new(), Vec::new(), false);
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            if fenced {
                out.push(Block::Code(code.join("\n")));
                code.clear();
            } else {
                text(&mut lines, &mut out);
            }
            fenced = !fenced;
        } else if fenced {
            code.push(line);
        } else {
            lines.push(line);
        }
    }
    // An unclosed fence is code to the end.
    if fenced {
        out.push(Block::Code(code.join("\n")));
    }
    text(&mut lines, &mut out);
    out
}

/// Whether `who` (an id or a label) is you.
pub(crate) fn is_you(who: &str) -> bool {
    who == "operator" || who == "you (operator)"
}

/// The agent who posted as `sender` (a seat address or name), if known.
pub(crate) fn poster<'a>(store: &'a Store, sender: &str) -> Option<&'a Agent> {
    store
        .agents
        .values()
        .find(|a| a.label() == sender)
        .or_else(|| store.find_agent(sender))
}

/// The colour of an agent's name: its own, or the accent for you.
pub(crate) fn hue(id: &str, cx: &App) -> Hsla {
    if is_you(id) {
        Color::Accent.color(cx)
    } else {
        themes::agent_color(id, cx).cursor
    }
}

pub struct CommsItem {
    fleet: Entity<FleetModel>,
    /// The stream's: the list keys go to it.
    focus_handle: FocusHandle,
    list: ListState,
    rows: Vec<Row>,
    selected: Option<Row>,
    /// The messages whose thread is open.
    open: HashSet<u64>,
    compose: Entity<TextInput>,
    animation: critter::Animation,
    _subscriptions: Vec<Subscription>,
}

impl CommsItem {
    pub fn new(fleet: Entity<FleetModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let compose = cx.new(|cx| TextInput::new(window, cx).with_multiline(Some(COMPOSE_LINES)));
        let subscriptions = vec![
            cx.observe(&fleet, |this, _, cx| {
                this.sync(cx);
                cx.notify();
            }),
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&compose, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Submitted => this.send(window, cx),
                    // Escape hands the keyboard back to the stream.
                    InputEvent::Cancelled => window.focus(&this.focus_handle, cx),
                    InputEvent::Edited | InputEvent::Blurred => cx.notify(),
                }
            }),
        ];
        let list = ListState::new(0, ListAlignment::Top, px(1000.));
        list.set_follow_mode(FollowMode::Tail);
        let mut comms = CommsItem {
            fleet,
            focus_handle: cx.focus_handle(),
            list,
            rows: Vec::new(),
            selected: None,
            open: HashSet::new(),
            compose,
            animation: critter::Animation::default(),
            _subscriptions: subscriptions,
        };
        comms.sync(cx);
        // As the TUI's: the newest entry is selected, so a line answers it.
        let last = comms.rows.last().copied();
        comms.set_selected(last, cx);
        comms
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<Row> {
        self.selected
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn compose(&self) -> &Entity<TextInput> {
        &self.compose
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn list(&self) -> &ListState {
        &self.list
    }

    /// The entry a row shows.
    fn entry(&self, row: Row, cx: &App) -> Option<Entry> {
        let store = &self.fleet.read(cx).store;
        match row {
            Row::Message(id) | Row::Reply { id, .. } => store
                .messages
                .iter()
                .find(|m| m.id == id)
                .cloned()
                .map(Entry::Message),
            Row::Chat(id) => store.chat.iter().find(|c| c.id == id).cloned().map(Entry::Chat),
        }
    }

    /// Rebuilds the rows from the Store, telling the list what changed,
    /// and keeps the selection on a row that is still there.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        self.open.retain(|id| store.messages.iter().any(|m| m.id == *id));
        let rows = rows(store, &self.open);
        if let Some((range, count)) = splice(&self.rows, &rows) {
            self.list.splice(range, count);
            self.rows = rows;
            self.stick(Moved::Changed);
        }
        let selected = self.selected.and_then(|row| {
            if self.rows.contains(&row) {
                return Some(row);
            }
            // A reply whose thread closed: the reply in its own place.
            let Row::Reply { id, .. } = row else { return None };
            self.rows.iter().find(|r| **r == Row::Message(id)).copied()
        });
        if selected != self.selected {
            self.set_selected(selected, cx);
        }
    }

    /// Selects `row`; the compose box says what a line will do.
    fn set_selected(&mut self, row: Option<Row>, cx: &mut Context<Self>) {
        self.selected = row;
        let placeholder = match row.and_then(|r| self.entry(r, cx)) {
            Some(Entry::Message(m)) => format!("Reply to #{} from {}…", m.id, m.from_label),
            Some(Entry::Chat(c)) => format!("Post in #{} ({})…", c.topic, c.rig),
            None => "@name and a message, or select one to answer…".to_string(),
        };
        self.compose
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        cx.notify();
    }

    /// Makes the list follow its end, or not, as [`sticks`] says.
    fn stick(&self, moved: Moved) {
        let stuck = self.list.is_following_tail();
        match (stuck, sticks(stuck, moved)) {
            (false, true) => self.list.set_follow_mode(FollowMode::Tail),
            (true, false) => self.list.pause_following_tail(),
            _ => {}
        }
    }

    /// Selects row `ix` and scrolls it into view.
    fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&row) = self.rows.get(ix) else { return };
        self.set_selected(Some(row), cx);
        let last = ix + 1 == self.rows.len();
        self.stick(Moved::Selected { last });
        let viewport = self.list.viewport_bounds();
        let shown = self
            .list
            .bounds_for_item(ix)
            .is_some_and(|b| b.top() >= viewport.top() && b.bottom() <= viewport.bottom());
        if !last && !shown {
            self.list.scroll_to_reveal_item(ix);
        }
    }

    /// Moves the selection `delta` rows down (up if negative); with none,
    /// from the last row, as the TUI's.
    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        let at = self.selected.and_then(|s| self.rows.iter().position(|r| *r == s));
        let next = (at.unwrap_or(last) as isize + delta).clamp(0, last as isize) as usize;
        self.select_index(next, cx);
    }

    fn select_edge(&mut self, end: bool, cx: &mut Context<Self>) {
        if !self.rows.is_empty() {
            self.select_index(if end { self.rows.len() - 1 } else { 0 }, cx);
        }
    }

    /// Opens or closes the thread of message `id`.
    fn toggle_thread(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.open.remove(&id) {
            self.open.insert(id);
        }
        self.sync(cx);
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_open(&self, id: u64) -> bool {
        self.open.contains(&id)
    }

    fn has_replies(&self, id: u64, cx: &App) -> bool {
        self.fleet
            .read(cx)
            .store
            .messages
            .iter()
            .any(|m| m.reply_to == Some(id) && m.id != id)
    }

    /// Enter on the stream: opens or closes the selected message's
    /// thread, or, with none to open, answers it.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Some(Row::Message(id)) if self.has_replies(id, cx) => self.toggle_thread(id, cx),
            _ => self.reply(window, cx),
        }
    }

    /// Gives the compose box the keyboard.
    pub fn focus_compose(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.compose.focus_handle(cx), cx);
    }

    /// `comms::Compose`: a new message rather than an answer, so the
    /// selection goes and the box starts with the `@` of a name.
    pub fn new_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_selected(None, cx);
        self.compose.update(cx, |input, cx| {
            if input.is_empty() {
                input.set_text("@", cx);
            }
        });
        self.focus_compose(window, cx);
    }

    /// `comms::Reply`: answers the selected entry (the newest if none is).
    fn reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            self.select_edge(true, cx);
        }
        self.focus_compose(window, cx);
    }

    /// Tab: completes the `@name` being typed, as the TUI's.
    fn complete(&mut self, cx: &mut Context<Self>) {
        let text = self.compose.read(cx).text().to_string();
        if let Some(done) = command::complete(&self.fleet.read(cx).store, &text) {
            self.compose.update(cx, |input, cx| input.set_text(&done, cx));
        }
    }

    /// Sends what was typed, as the TUI's input line does in Comms: a
    /// `/command`, an `@Name` message, or an answer to the selected entry.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.compose.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let selected = self.selected.and_then(|row| self.entry(row, cx));
        let store = &self.fleet.read(cx).store;
        let cmd = match text.strip_prefix('/') {
            Some(cmd) => commands::parse(store, cmd),
            None => command::comms(store, selected.as_ref(), &text),
        };
        // A mistake stays in the box to be put right.
        let done = !matches!(cmd, Command::Error(_));
        let sent = matches!(cmd, Command::Request(..));
        self.run(cmd, window, cx);
        if done {
            self.compose.update(cx, |input, cx| input.set_text("", cx));
        }
        if sent {
            self.stick(Moved::Sent);
        }
    }

    /// Carries out a command typed in the box; the console's own (views,
    /// answers, quitting) go to the app's actions.
    fn run(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        commands::run(&self.fleet, cmd, window, cx);
    }

    /// A sender's avatar: the agent's critter, or you.
    fn avatar(&self, who: &str, agent: Option<&Agent>, cx: &App) -> AnyElement {
        let tick = self.fleet.read(cx).tick();
        match agent {
            Some(agent) => Critter::agent(agent, tick, cx)
                .size(AVATAR)
                .animate(&self.animation)
                .into_any_element(),
            None if is_you(who) => div()
                .flex()
                .flex_none()
                .size(AVATAR)
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(cx.theme().colors().element_background)
                .child(
                    Icon::new(IconName::Person)
                        .size(IconSize::Small)
                        .color(Color::Accent),
                )
                .into_any_element(),
            // Someone who has left the fleet: still, as a ghost would be.
            None => Critter::new(who, 0, false, cx).size(AVATAR).into_any_element(),
        }
    }

    /// A body: text with its `@name`s in their agents' colours, and code
    /// in the buffer font.
    fn render_body(&self, body: &str, window: &Window, cx: &App) -> Div {
        let store = &self.fleet.read(cx).store;
        let colors = cx.theme().colors();
        v_flex()
            .gap_1()
            .text_ui(cx)
            .children(blocks(body).into_iter().map(|block| {
                match block {
                    Block::Text(text) => {
                        let highlights: Vec<(Range<usize>, HighlightStyle)> = mentions(&text)
                            .into_iter()
                            .map(|range| {
                                let color = match store.find_agent(&text[range.start + 1..range.end]) {
                                    Some(a) => hue(&a.id, cx),
                                    None => Color::Accent.color(cx),
                                };
                                let style = HighlightStyle {
                                    color: Some(color),
                                    font_weight: Some(FontWeight::SEMIBOLD),
                                    ..HighlightStyle::default()
                                };
                                (range, style)
                            })
                            .collect();
                        let style = window.text_style();
                        div()
                            .child(StyledText::new(text).with_default_highlights(&style, highlights))
                            .into_any_element()
                    }
                    Block::Code(code) => div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(colors.element_background)
                        .font_buffer(cx)
                        .text_ui_sm(cx)
                        .child(code)
                        .into_any_element(),
                }
            }))
    }

    /// A row's frame: hover and selection, the click that selects it, and
    /// `depth` indent guides down its left.
    fn render_frame(
        &self,
        ix: usize,
        row: Row,
        depth: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selected = self.selected == Some(row);
        let focused = self.focus_handle.is_focused(window);
        let colors = cx.theme().colors();
        let guide = IndentGuideColors::panel(cx).default;
        let id = match row {
            Row::Message(id) => SharedString::from(format!("comms-message-{id}")),
            Row::Chat(id) => SharedString::from(format!("comms-chat-{id}")),
            Row::Reply { root, id, .. } => SharedString::from(format!("comms-reply-{root}-{id}")),
        };
        let selector = id.to_uppercase();
        h_flex()
            .id(id)
            .debug_selector(move || selector.clone())
            .relative()
            .items_start()
            .w_full()
            .gap_2()
            .pl(GUTTER + INDENT * depth as f32)
            .pr(GUTTER)
            .py_1p5()
            .border_l_2()
            .border_color(if selected && focused {
                colors.border_focused
            } else {
                gpui::transparent_black()
            })
            .when(selected, |el| el.bg(colors.ghost_element_selected))
            .when(!selected, |el| el.hover(|el| el.bg(colors.ghost_element_hover)))
            .children((0..depth).map(|level| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(GUTTER + INDENT * level as f32 + AVATAR / 2.)
                    .w_px()
                    .bg(guide)
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.select_index(ix, cx);
            }))
    }

    /// A message: who to whom, its number and what it answers, whether it
    /// is held or unread, when, and its thread's disclosure.
    fn render_message(
        &self,
        ix: usize,
        row: Row,
        m: &Message,
        depth: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let sender = store.agents.get(&m.from);
        let avatar = self.avatar(&m.from, sender, cx);
        let replies = if depth == 0 { store.replies(m.id).len() } else { 0 };
        let id = m.id;
        let name = |label: &str, who: &str| {
            Label::new(label.to_string())
                .size(LabelSize::Small)
                .weight(FontWeight::SEMIBOLD)
                .color(Color::Custom(hue(who, cx)))
        };
        let state = if m.held {
            let warning = cx.theme().status().warning_background;
            Some(
                Chip::new("held")
                    .label_size(LabelSize::XSmall)
                    .label_color(Color::Warning)
                    .bg_color(warning)
                    .border_color(warning)
                    .tooltip(Tooltip::text(
                        "Held until the recipient's typing guard lets it through",
                    ))
                    .into_any_element(),
            )
        } else if m.read.is_none() && !is_you(&m.to) {
            let why = if m.delivered.is_none() {
                "Not delivered yet"
            } else {
                "Delivered, not read yet"
            };
            Some(
                div()
                    .id(("comms-unread", id))
                    .tooltip(Tooltip::text(why))
                    .child(Indicator::dot().color(Color::Info))
                    .into_any_element(),
            )
        } else {
            None
        };
        let header = h_flex()
            .gap_1()
            .min_w_0()
            .child(name(&m.from_label, &m.from))
            .child(Label::new("→").size(LabelSize::Small).color(Color::Muted))
            .child(name(&m.to_label, &m.to))
            .child(
                Label::new(format!("#{id}"))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .children(m.reply_to.map(|r| {
                Label::new(format!("↳ #{r}"))
                    .size(LabelSize::XSmall)
                    .color(Color::Disabled)
            }))
            .children(state)
            .child(div().flex_1())
            .when(replies > 0, |el| {
                let open = self.open.contains(&id);
                el.child(
                    h_flex()
                        .id(("comms-thread", id))
                        .gap_0p5()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_thread(id, cx)))
                        .child(
                            Disclosure::new(("comms-thread-disclosure", id), open)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_thread(id, cx))),
                        )
                        .child(
                            Label::new(if replies == 1 {
                                "1 reply".to_string()
                            } else {
                                format!("{replies} replies")
                            })
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                        ),
                )
            })
            .child(
                Label::new(format!("{} ago", age(m.created)))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );
        self.render_frame(ix, row, depth, window, cx)
            .child(avatar)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(header)
                    .child(self.render_body(&m.body, window, cx)),
            )
            .into_any_element()
    }

    /// A chat post: who, in which rig's topic, when.
    fn render_chat(
        &self,
        ix: usize,
        row: Row,
        c: &ainet_core::ChatMessage,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let agent = poster(store, &c.sender);
        let who = agent.map(|a| a.id.clone()).unwrap_or_else(|| c.sender.clone());
        let avatar = self.avatar(&who, agent, cx);
        let name_color = if agent.is_some() || is_you(&who) {
            Color::Custom(hue(&who, cx))
        } else {
            Color::Default
        };
        let info = cx.theme().status().info_background;
        let header = h_flex()
            .gap_1p5()
            .min_w_0()
            .child(
                Label::new(c.sender.clone())
                    .size(LabelSize::Small)
                    .weight(FontWeight::SEMIBOLD)
                    .color(name_color),
            )
            .child(
                Chip::new(format!("#{} {}", c.topic, c.rig))
                    .label_size(LabelSize::XSmall)
                    .label_color(Color::Info)
                    .bg_color(info)
                    .border_color(info),
            )
            .child(div().flex_1())
            .child(
                Label::new(format!("{} ago", age(c.created)))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );
        self.render_frame(ix, row, 0, window, cx)
            .child(avatar)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(header)
                    .child(self.render_body(&c.body, window, cx)),
            )
            .into_any_element()
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(&row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let depth = match row {
            Row::Reply { depth, .. } => depth,
            _ => 0,
        };
        match self.entry(row, cx) {
            Some(Entry::Message(m)) => self.render_message(ix, row, &m, depth, window, cx),
            Some(Entry::Chat(c)) => self.render_chat(ix, row, &c, window, cx),
            None => div().into_any_element(),
        }
    }

    /// What the stream shows before anyone said anything: the TUI's words.
    fn render_empty(&self, cx: &App) -> Div {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .child(
                Critter::new("comms-quiet", self.fleet.read(cx).tick(), false, cx)
                    .size(px(64.))
                    .animate(&self.animation),
            )
            .child(Label::new("Nobody has said anything yet.").color(Color::Muted))
            .child(
                Label::new("Agents talk with the ainet tools send_message, ask_agent and chat_send;")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Label::new("you can join in with @seat text, /broadcast and /chat.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    /// The compose box: what a line will do, the field, and send.
    fn render_compose(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let compose_focus = self.compose.focus_handle(cx);
        let focused = compose_focus.is_focused(window);
        let target = self.selected.and_then(|row| self.entry(row, cx));
        let (icon, what) = match &target {
            Some(Entry::Message(m)) => (
                IconName::ReplyArrowRight,
                format!("Answering #{} from {}", m.id, m.from_label),
            ),
            Some(Entry::Chat(c)) => (IconName::Hash, format!("Posting in #{} ({})", c.topic, c.rig)),
            None => (IconName::AtSign, "A new message: start with @name".to_string()),
        };
        let empty = self.compose.read(cx).is_empty();
        v_flex()
            .flex_none()
            .gap_1()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(colors.border_variant)
            .bg(colors.panel_background)
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(what).size(LabelSize::XSmall).color(Color::Muted))
                    .when(target.is_some(), |el| {
                        el.child(
                            IconButton::new("comms-clear-target", IconName::Close)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Write a new message instead"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_selected(None, cx);
                                    this.focus_compose(window, cx);
                                })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .key_context("CommsCompose")
                    .items_end()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if focused {
                        colors.border_focused
                    } else {
                        colors.border
                    })
                    .bg(colors.editor_background)
                    .on_action(cx.listener(|this, _: &Complete, _, cx| this.complete(cx)))
                    .child(div().flex_1().min_w_0().child(self.compose.clone()))
                    .child(
                        IconButton::new("comms-send", IconName::Send)
                            .icon_size(IconSize::Small)
                            .icon_color(if empty { Color::Muted } else { Color::Accent })
                            .disabled(empty)
                            .tooltip(Tooltip::text("Send (enter; shift-enter for a new line)"))
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
            .child(
                Label::new(
                    "@name messages an agent (tab completes it) · /chat and /broadcast post to the rig",
                )
                .size(LabelSize::XSmall)
                .color(Color::Disabled),
            )
    }
}

impl EventEmitter<ItemEvent> for CommsItem {}

impl Focusable for CommsItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for CommsItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        "Comms".into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        Some(
            Icon::new(IconName::Chat)
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some("Messages and chat between agents, and with you".into())
    }
}

impl Render for CommsItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let (background, rule) = (colors.editor_background, colors.border_variant);
        let store = &self.fleet.read(cx).store;
        let counts = format!(
            "{} messages · {} chat posts",
            store.messages.len(),
            store.chat.len()
        );
        let stream = if self.rows.is_empty() {
            self.render_empty(cx).into_any_element()
        } else {
            div()
                .size_full()
                .child(
                    list(
                        self.list.clone(),
                        cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
                    )
                    .size_full()
                    .py_1(),
                )
                .vertical_scrollbar_for(&self.list, window, cx)
                .into_any_element()
        };

        v_flex()
            .id("comms")
            .size_full()
            .bg(background)
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.select_relative(1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.select_relative(-1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectFirst, _, cx| this.select_edge(false, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectLast, _, cx| this.select_edge(true, cx)))
            .on_action(cx.listener(|this, _: &Compose, window, cx| this.new_message(window, cx)))
            .on_action(cx.listener(|this, _: &Reply, window, cx| this.reply(window, cx)))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(rule)
                    .child(
                        Icon::new(IconName::Chat)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new("Comms").weight(FontWeight::SEMIBOLD))
                    .child(Label::new(counts).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(
                div()
                    .id("comms-stream")
                    .key_context("Comms")
                    .track_focus(&self.focus_handle)
                    .flex_1()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
                    )
                    .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.confirm(window, cx)))
                    .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.set_selected(None, cx)))
                    .child(stream),
            )
            .child(self.render_compose(window, cx))
    }
}

#[cfg(test)]
mod pure_tests {
    use ainet_model::{Demo, Msg};

    use super::*;

    fn demo() -> Store {
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        store
    }

    #[test]
    fn rows_are_the_entries_in_order_with_open_threads_under_their_message() {
        let store = demo();
        let closed = rows(&store, &HashSet::new());
        let entries: Vec<Row> = store
            .entries()
            .iter()
            .map(|e| match e {
                Entry::Message(m) => Row::Message(m.id),
                Entry::Chat(c) => Row::Chat(c.id),
            })
            .collect();
        assert_eq!(closed, entries);
        // Messages and chat are interleaved, not one after the other.
        let first_chat = closed.iter().position(|r| matches!(r, Row::Chat(_))).unwrap();
        let last_message = closed.iter().rposition(|r| matches!(r, Row::Message(_))).unwrap();
        assert!(first_chat < last_message, "{closed:?}");

        // The demo's question has an answer: open, it shows under it.
        let (question, answer) = store
            .messages
            .iter()
            .find_map(|m| m.reply_to.map(|r| (r, m.id)))
            .unwrap();
        let open = rows(&store, &HashSet::from([question]));
        let at = open.iter().position(|r| *r == Row::Message(question)).unwrap();
        assert_eq!(
            open[at + 1],
            Row::Reply {
                root: question,
                id: answer,
                depth: 1
            }
        );
        assert_eq!(open.len(), closed.len() + 1);
    }

    #[test]
    fn splices_keep_the_rows_on_either_side() {
        let (a, b, c, d) = (Row::Message(1), Row::Chat(1), Row::Message(2), Row::Chat(2));
        assert_eq!(splice(&[a, b], &[a, b]), None);
        assert_eq!(splice(&[a, b], &[a, b, c]), Some((2..2, 1)), "arrived");
        assert_eq!(splice(&[a, b, c], &[b, c, d]), Some((0..3, 3)));
        assert_eq!(splice(&[a, b, c], &[b, c]), Some((0..1, 0)), "the oldest went");
        let reply = Row::Reply {
            root: 1,
            id: 2,
            depth: 1,
        };
        assert_eq!(
            splice(&[a, b], &[a, reply, b]),
            Some((1..1, 1)),
            "a thread opened"
        );
        assert_eq!(splice(&[a, reply, b], &[a, b]), Some((1..2, 0)));
        assert_eq!(splice(&[], &[a]), Some((0..0, 1)));
    }

    #[test]
    fn the_stream_sticks_to_its_end_like_a_chat() {
        // At the bottom, arrivals keep it there; scrolled up, they do not.
        assert!(sticks(true, Moved::Changed));
        assert!(!sticks(false, Moved::Changed));
        // Sending brings it down, wherever it was.
        assert!(sticks(false, Moved::Sent));
        assert!(sticks(true, Moved::Sent));
        // Reading back lets go; the last row sticks again.
        assert!(!sticks(true, Moved::Selected { last: false }));
        assert!(sticks(false, Moved::Selected { last: true }));
    }

    #[test]
    fn mentions_are_names_after_an_at() {
        let text = "Standup: @rev.review take it, @orch-lead@docs. mail a@b.c @ @-x (@dev.ui)";
        let found: Vec<&str> = mentions(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["@rev.review", "@orch-lead@docs", "@dev.ui"]);
        assert!(mentions("").is_empty());
        let text = "héllo @Möss!";
        let found: Vec<&str> = mentions(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["@Möss"]);
    }

    #[test]
    fn bodies_split_into_text_and_code() {
        let body = "Try this:\n\n```sh\ncargo test\n  --all\n```\nThen tell me.\n";
        assert_eq!(
            blocks(body),
            [
                Block::Text("Try this:".into()),
                Block::Code("cargo test\n  --all".into()),
                Block::Text("Then tell me.".into()),
            ]
        );
        assert_eq!(blocks("one\ntwo"), [Block::Text("one\ntwo".into())]);
        assert_eq!(blocks("```\nopen"), [Block::Code("open".into())]);
        assert!(blocks("  \n").is_empty());
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::{ChatMessage, Event, Request};
    use ainet_model::Msg;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    /// The demo fleet in an active window, with Comms open.
    fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<CommsItem>, &mut VisualTestContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-shift-c");
        cx.run_until_parked();
        let comms = w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<CommsItem>().ok())
                .expect("Comms is the active tab")
        });
        (w, comms, cx)
    }

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    /// The requests sent since the last call.
    fn take_sent(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> Vec<(Request, String)> {
        fleet.update(cx, |f, _| std::mem::take(&mut f.sent))
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

    fn compose_text(comms: &Entity<CommsItem>, cx: &mut VisualTestContext) -> String {
        comms.read_with(cx, |c, cx| c.compose().read(cx).text().to_string())
    }

    /// A row's debug selector, which the test context wants for good.
    fn selector(row: Row) -> &'static str {
        let name = match row {
            Row::Message(id) => format!("COMMS-MESSAGE-{id}"),
            Row::Chat(id) => format!("COMMS-CHAT-{id}"),
            Row::Reply { root, id, .. } => format!("COMMS-REPLY-{root}-{id}"),
        };
        Box::leak(name.into_boxed_str())
    }

    fn focused(handle: &FocusHandle, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, _| handle.is_focused(window))
    }

    /// What the TUI's input line sends for `line` in Comms with `entry`
    /// selected.
    fn tui_sends(store: &Store, entry: Option<&Entry>, line: &str) -> (Request, String) {
        match command::comms(store, entry, line) {
            Command::Request(req, ok) => (*req, ok),
            other => panic!("not a request: {other:?}"),
        }
    }

    #[gpui::test]
    async fn the_stream_is_the_stores_entries_and_follows_new_ones(cx: &mut TestAppContext) {
        let (w, comms, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let expected = fleet.read_with(cx, |f, _| rows(&f.store, &HashSet::new()));
        assert!(!expected.is_empty());
        comms.read_with(cx, |c, _| {
            assert_eq!(c.rows(), expected);
            assert_eq!(c.list().item_count(), expected.len());
            assert_eq!(c.selected(), expected.last().copied(), "the newest, as the TUI's");
        });
        let compose = comms.read_with(cx, |c, cx| c.compose().focus_handle(cx));
        assert!(focused(&compose, cx), "opening Comms is to write");
        assert!(comms.read_with(cx, |c, _| c.list().is_following_tail()));

        // A post arrives: it is the last row, and the stream follows.
        let post = ChatMessage {
            id: 900,
            rig: "docs".into(),
            topic: "general".into(),
            sender: "orch-lead@docs".into(),
            body: "@dev.ui ship it".into(),
            created: i64::MAX / 2,
        };
        fleet.update(cx, |f, cx| {
            f.receive(vec![Msg::Event(Box::new(Event::Chat { message: post }))], cx)
        });
        cx.run_until_parked();
        comms.read_with(cx, |c, _| {
            assert_eq!(c.rows().last(), Some(&Row::Chat(900)));
            assert_eq!(c.list().item_count(), expected.len() + 1);
            assert!(c.list().is_following_tail());
        });
        assert!(cx.debug_bounds("COMMS-CHAT-900").is_some(), "drawn at the bottom");
    }

    #[gpui::test]
    async fn lines_answer_the_selection_or_message_an_agent(cx: &mut TestAppContext) {
        let (w, comms, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        take_sent(&fleet, cx);
        let (message, chat) = fleet.read_with(cx, |f, _| {
            let entries = f.store.entries();
            let message = entries
                .iter()
                .find(|e| matches!(e, Entry::Message(_)))
                .cloned()
                .unwrap();
            let chat = entries
                .iter()
                .find(|e| matches!(e, Entry::Chat(_)))
                .cloned()
                .unwrap();
            (message, chat)
        });

        // A message selected: a threaded reply, as the TUI sends it.
        let Entry::Message(m) = &message else {
            unreachable!()
        };
        let ix = comms.read_with(cx, |c, _| {
            c.rows().iter().position(|r| *r == Row::Message(m.id)).unwrap()
        });
        comms.update(cx, |c, cx| c.select_index(ix, cx));
        cx.simulate_input("on it");
        cx.simulate_keystrokes("enter");
        let expected = fleet.read_with(cx, |f, _| tui_sends(&f.store, Some(&message), "on it"));
        assert_eq!(
            format!("{:?}", take_sent(&fleet, cx)),
            format!("{:?}", [expected])
        );
        assert_eq!(compose_text(&comms, cx), "", "sent, the box empties");

        // A chat post selected: a post in its topic.
        let Entry::Chat(c) = &chat else { unreachable!() };
        let ix = comms.read_with(cx, |cm, _| {
            cm.rows().iter().position(|r| *r == Row::Chat(c.id)).unwrap()
        });
        comms.update(cx, |cm, cx| cm.select_index(ix, cx));
        cx.simulate_input("same here");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("and here");
        cx.simulate_keystrokes("enter");
        let expected = fleet.read_with(cx, |f, _| tui_sends(&f.store, Some(&chat), "same here\nand here"));
        assert_eq!(
            format!("{:?}", take_sent(&fleet, cx)),
            format!("{:?}", [expected])
        );

        // `@Name` messages that agent, whatever is selected.
        cx.simulate_input("@orch-lead ship it");
        cx.simulate_keystrokes("enter");
        let expected = fleet.read_with(cx, |f, _| tui_sends(&f.store, None, "@orch-lead ship it"));
        assert_eq!(
            format!("{:?}", take_sent(&fleet, cx)),
            format!("{:?}", [expected])
        );

        // Nothing selected: what to do, and the words stay.
        let stream = comms.read_with(cx, |c, cx| c.focus_handle(cx));
        cx.update(|window, cx| window.focus(&stream, cx));
        cx.simulate_keystrokes("escape");
        assert_eq!(comms.read_with(cx, |c, _| c.selected()), None);
        comms.update_in(cx, |c, window, cx| c.focus_compose(window, cx));
        cx.simulate_input("hello");
        cx.simulate_keystrokes("enter");
        assert!(take_sent(&fleet, cx).is_empty());
        assert_eq!(
            toasts(&w, cx).last().map(String::as_str),
            Some("select a message to answer, or start with @Name")
        );
        assert_eq!(compose_text(&comms, cx), "hello");

        // The console's /chat posts to the selected agent's rig.
        cx.simulate_keystrokes("ctrl-a backspace");
        cx.simulate_input("/chat standup in five");
        cx.simulate_keystrokes("enter");
        let sent = take_sent(&fleet, cx);
        assert!(
            matches!(&sent[..], [(Request::ChatSend { body, .. }, _)] if body == "standup in five"),
            "{sent:?}"
        );
    }

    #[gpui::test]
    async fn tab_completes_a_name_and_the_keys_move_through_the_stream(cx: &mut TestAppContext) {
        let (w, comms, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        cx.simulate_input("@orch-l");
        cx.simulate_keystrokes("tab");
        let expected = fleet.read_with(cx, |f, _| command::complete(&f.store, "@orch-l").unwrap());
        assert_eq!(compose_text(&comms, cx), expected);
        assert_eq!(expected, "@orch-lead@docs ");
        // Nothing to complete: tab changes nothing.
        cx.simulate_input("hi");
        cx.simulate_keystrokes("tab");
        assert_eq!(compose_text(&comms, cx), "@orch-lead@docs hi");

        // With words in the box, up and down are the box's.
        let n = comms.read_with(cx, |c, _| c.rows().len());
        let selected = |cx: &mut VisualTestContext| {
            comms.read_with(cx, |c, _| {
                c.selected().and_then(|s| c.rows().iter().position(|r| *r == s))
            })
        };
        assert_eq!(selected(cx), Some(n - 1));
        cx.simulate_keystrokes("up");
        assert_eq!(selected(cx), Some(n - 1));
        // Empty, they select entries.
        cx.simulate_keystrokes("ctrl-a backspace up up");
        assert_eq!(selected(cx), Some(n - 3));
        cx.simulate_keystrokes("down");
        assert_eq!(selected(cx), Some(n - 2));

        // From the stream too; c writes a new message, r answers.
        let stream = comms.read_with(cx, |c, cx| c.focus_handle(cx));
        cx.update(|window, cx| window.focus(&stream, cx));
        cx.simulate_keystrokes("down");
        assert_eq!(selected(cx), Some(n - 1));
        assert!(comms.read_with(cx, |c, _| c.list().is_following_tail()));
        cx.simulate_keystrokes("home");
        assert_eq!(selected(cx), Some(0));
        cx.simulate_keystrokes("c");
        let compose = comms.read_with(cx, |c, cx| c.compose().focus_handle(cx));
        assert!(focused(&compose, cx));
        assert_eq!(selected(cx), None);
        assert_eq!(compose_text(&comms, cx), "@");
        cx.simulate_keystrokes("escape");
        assert!(focused(&stream, cx));
        cx.simulate_keystrokes("r");
        assert!(focused(&compose, cx));
        assert_eq!(
            selected(cx),
            Some(n - 1),
            "r with nothing selected answers the newest"
        );
    }

    #[gpui::test]
    async fn reading_back_holds_the_stream_until_you_send(cx: &mut TestAppContext) {
        let (w, comms, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let post = |id: u64| ChatMessage {
            id,
            rig: "docs".into(),
            topic: "general".into(),
            sender: "orch-lead@docs".into(),
            body: format!("post {id}\nwith a second line"),
            created: ainet_core::now_ms() + id as i64,
        };
        let arrive = |ids: Range<u64>, cx: &mut VisualTestContext| {
            let msgs = ids
                .map(|id| Msg::Event(Box::new(Event::Chat { message: post(id) })))
                .collect();
            fleet.update(cx, |f, cx| f.receive(msgs, cx));
            cx.run_until_parked();
        };
        let following = |cx: &mut VisualTestContext| comms.read_with(cx, |c, _| c.list().is_following_tail());
        arrive(1000..1060, cx);
        assert!(following(cx));
        assert!(cx.debug_bounds("COMMS-CHAT-1059").is_some(), "at the end");

        // Up to the first entry: the stream stays there as posts arrive.
        let stream = comms.read_with(cx, |c, cx| c.focus_handle(cx));
        cx.update(|window, cx| window.focus(&stream, cx));
        cx.simulate_keystrokes("home");
        cx.run_until_parked();
        let first = comms.read_with(cx, |c, _| c.rows()[0]);
        assert!(!following(cx));
        arrive(1060..1061, cx);
        assert!(!following(cx));
        assert!(cx.debug_bounds(selector(first)).is_some(), "still where you read");
        assert!(cx.debug_bounds("COMMS-CHAT-1060").is_none());

        // Sending takes it back to the end.
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("seen");
        cx.simulate_keystrokes("enter");
        assert_eq!(take_sent(&fleet, cx).len(), 1);
        cx.run_until_parked();
        assert!(following(cx));
        assert!(cx.debug_bounds("COMMS-CHAT-1060").is_some());
    }

    #[gpui::test]
    async fn threads_open_under_their_message(cx: &mut TestAppContext) {
        let (w, comms, cx) = demo(cx);
        let fleet = fleet(&w, cx);
        let (question, answer) = fleet.read_with(cx, |f, _| {
            f.store
                .messages
                .iter()
                .find_map(|m| m.reply_to.map(|r| (r, m.id)))
                .unwrap()
        });
        let reply = Row::Reply {
            root: question,
            id: answer,
            depth: 1,
        };
        let n = comms.read_with(cx, |c, _| c.rows().len());

        // Enter on the message opens its thread; again closes it.
        let ix = comms.read_with(cx, |c, _| {
            c.rows()
                .iter()
                .position(|r| *r == Row::Message(question))
                .unwrap()
        });
        let stream = comms.read_with(cx, |c, cx| c.focus_handle(cx));
        cx.update(|window, cx| window.focus(&stream, cx));
        comms.update(cx, |c, cx| c.select_index(ix, cx));
        cx.simulate_keystrokes("enter");
        comms.read_with(cx, |c, _| {
            assert!(c.is_open(question));
            assert_eq!(c.rows()[ix + 1], reply);
            assert_eq!(c.list().item_count(), n + 1);
        });
        cx.run_until_parked();
        let guide = cx.debug_bounds(selector(reply)).expect("the reply is drawn");
        let root = cx
            .debug_bounds(selector(Row::Message(question)))
            .expect("its message is drawn");
        assert!(guide.top() >= root.bottom() - px(1.), "under its message");

        // A reply selected answers that reply; closing the thread puts
        // the selection on the reply's own place in the stream.
        comms.update(cx, |c, cx| c.select_index(ix + 1, cx));
        take_sent(&fleet, cx);
        comms.update_in(cx, |c, window, cx| c.focus_compose(window, cx));
        cx.simulate_input("thanks");
        cx.simulate_keystrokes("enter");
        let sent = take_sent(&fleet, cx);
        assert!(
            matches!(&sent[..], [(Request::Message { reply_to: Some(r), .. }, _)] if *r == answer),
            "{sent:?}"
        );
        comms.update(cx, |c, cx| c.toggle_thread(question, cx));
        comms.read_with(cx, |c, _| {
            assert!(!c.is_open(question));
            assert_eq!(c.selected(), Some(Row::Message(answer)));
        });
    }

    #[gpui::test]
    async fn a_click_selects_and_gives_the_stream_the_keys(cx: &mut TestAppContext) {
        let (_, comms, cx) = demo(cx);
        // The row above the newest (which is selected already).
        let row = comms.read_with(cx, |c, _| c.rows()[c.rows().len() - 2]);
        let bounds = cx.debug_bounds(selector(row)).expect("the row is drawn");
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert_eq!(comms.read_with(cx, |c, _| c.selected()), Some(row));
        let stream = comms.read_with(cx, |c, cx| c.focus_handle(cx));
        assert!(focused(&stream, cx));
    }
}
