//! An agent's conversation as a tab: [`ConversationItem`], OpenRig's
//! communication view, opened by `fleet::OpenConversation` (`ctrl-shift-o`,
//! `c` in the Fleet, the agent's menu, the terminal's strip) and laid side
//! by side for a rig by `workspace::TileConversations`. It reads like
//! Zed's agent panel: your prompts as blocks, the agent's words with a dot,
//! its tool calls as `Bash(cargo test)` with the first lines they printed,
//! and the messages ainetd typed into its terminal as framed blocks;
//! `ainet …` and `rig …` commands stand out. A prompt box under it
//! messages the agent as the Message modal does.
//!
//! ainetd reads the conversation from the session's transcript
//! (`Request::Conversation`). The tab asks for what came after the last
//! entry it has (or from a tool call still waiting for its result) when
//! the agent moves on (its state, its tool, a message to or from it), and
//! every few seconds while it shows. A daemon from before the request
//! sends the transcript as text, shown as it comes with a notice; a Codex
//! seat gets a notice.

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use ainet_core::conversation::EntryKind;
use ainet_core::{AgentState, ConversationEntry};
use ainet_model::describe::age;
use ainet_model::{Command, command};
use gpui::{
    Action, ClickEvent, Entity, EventEmitter, FocusHandle, Focusable, FollowMode, FontWeight, HighlightStyle,
    ListAlignment, ListState, MouseButton, StyledText, Subscription, Task, list,
};
use ui::{Indicator, Tooltip, WithScrollbar, prelude::*};

use crate::actions::OpenTerminal;
use crate::comms::{self, Block, Moved, sticks};
use crate::critter::{Animation, Critter};
use crate::fleet::{FleetModel, Tick};
use crate::item::{Item, ItemEvent, TabContentParams};
use crate::panels::state_color;
use crate::persistence::SavedItem;
use crate::text_input::{InputEvent, TextInput};

/// How many entries are asked for at first.
pub const TAIL: usize = 200;

/// How often a conversation on show asks whether there is more, between
/// the agent's own moves (a reply is written without a hook firing).
pub const POLL_EVERY: Duration = Duration::from_secs(3);

/// How many lines the prompt box shows before it scrolls.
const PROMPT_LINES: usize = 6;

/// The words a command starts with after `ainet ` or `rig `: the console's
/// and OpenRig's subcommands. "the rig lead" in prose is no command.
const COMMAND_WORDS: [&str; 43] = [
    "adopt",
    "ask",
    "broadcast",
    "capture",
    "chat",
    "chatroom",
    "close",
    "create",
    "critter",
    "discover",
    "down",
    "expand",
    "export",
    "grow",
    "gui",
    "hook",
    "import",
    "inbox",
    "launch",
    "list",
    "ls",
    "msg",
    "ps",
    "queue",
    "release",
    "remove",
    "reply",
    "restore",
    "screen",
    "seat",
    "send",
    "show",
    "shrink",
    "shutdown",
    "snapshot",
    "spawn",
    "specs",
    "status",
    "terminal",
    "transcript",
    "up",
    "validate",
    "whoami",
];

/// Where the `ainet …` and `rig …` commands in `text` are, as byte ranges:
/// one starting a line or after a space, quote, bracket or `$`, running
/// to its closing backtick when it opened with one, else to the end of
/// the line.
pub fn commands(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let mut at = 0;
        while at < line.len() {
            let rest = &line[at..];
            let Some(found) = ["ainet ", "rig "]
                .iter()
                .filter_map(|w| rest.find(w).map(|i| (i, w.len())))
                .min()
            else {
                break;
            };
            let (start, word) = (at + found.0, found.1);
            let before = line[..start].chars().next_back();
            let alone = before.is_none_or(|c| c.is_whitespace() || "`\"'($⎿>".contains(c));
            let sub: String = line[start + word..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if alone && COMMAND_WORDS.contains(&sub.as_str()) {
                let end = if before == Some('`') {
                    line[start..].find('`').map_or(line.len(), |i| start + i)
                } else {
                    line.trim_end().len()
                };
                out.push(line_start + start..line_start + end);
                at = end.max(start + word);
            } else {
                at = start + word;
            }
        }
        line_start += line.len() + 1;
    }
    out
}

/// A tool call's header: `Bash(cargo test)`, its main argument's first
/// line (marked when it goes on), or the tool alone.
pub fn tool_title(entry: &ConversationEntry) -> String {
    let tool = if entry.tool.is_empty() {
        "Tool"
    } else {
        &entry.tool
    };
    let mut lines = entry.code.lines();
    match lines.next().filter(|l| !l.trim().is_empty()) {
        Some(first) if lines.next().is_some() => format!("{tool}({first} …)"),
        Some(first) => format!("{tool}({first})"),
        None => tool.to_string(),
    }
}

/// How the agent answers a message it got: the command ainetd suggests.
pub fn reply_hint(entry: &ConversationEntry) -> Option<String> {
    match (&entry.room, entry.message) {
        (Some(room), _) => Some(format!("ainet chat send in {room}")),
        (None, Some(id)) => Some(format!("ainet msg reply {id} \"…\"")),
        (None, None) => None,
    }
}

/// Where to ask from next: before the first tool call still waiting for
/// its result since your last prompt (so it comes back with it), else
/// after the last entry; `None` asks for the whole tail.
pub fn resume_after(entries: &[ConversationEntry]) -> Option<u64> {
    let last = entries.last()?;
    let turn = entries
        .iter()
        .rposition(|e| matches!(e.kind, EntryKind::You | EntryKind::Inbound))
        .unwrap_or(0);
    match entries[turn..].iter().find(|e| e.is_running()) {
        Some(running) => Some(running.id.saturating_sub(1)),
        None => Some(last.id),
    }
}

/// Puts `new`, the entries after `after`, in place of those `entries` had
/// after it; returns the range replaced and how many took its place, for
/// the list to splice.
pub fn merge(
    entries: &mut Vec<ConversationEntry>,
    after: u64,
    new: Vec<ConversationEntry>,
) -> (Range<usize>, usize) {
    let start = entries.iter().position(|e| e.id > after).unwrap_or(entries.len());
    let count = new.len();
    let range = start..entries.len();
    entries.splice(range.clone(), new);
    (range, count)
}

/// What the tab watches of its agent: when any of it changes, there is
/// likely more to read.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    state: AgentState,
    since: i64,
    tool: Option<String>,
    transcript: Option<PathBuf>,
    /// The newest message to or from it.
    message: Option<u64>,
    /// A new snapshot (a reconnect) may bring anything.
    snapshots: u64,
}

/// Where the conversation stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    Loading,
    Ready,
    Failed(String),
}

pub struct ConversationItem {
    fleet: Entity<FleetModel>,
    agent: String,
    /// The thread's: the keyboard rests here when not in the prompt box.
    focus_handle: FocusHandle,
    prompt: Entity<TextInput>,
    list: ListState,
    entries: Vec<ConversationEntry>,
    model: Option<String>,
    cwd: Option<String>,
    permission_mode: Option<String>,
    notice: Option<String>,
    /// The transcript as text, from a daemon too old for entries.
    plain: Option<String>,
    loaded: Loaded,
    /// A request is on its way; `again` if something moved meanwhile.
    asking: bool,
    again: bool,
    /// Its tab is on show; `stale` if something moved while it was not.
    shown: bool,
    stale: bool,
    seen: Option<Seen>,
    poll: Option<Task<()>>,
    animation: Animation,
    _subscriptions: Vec<Subscription>,
}

impl ConversationItem {
    /// The conversation of agent `agent` (its id); it asks for it at once.
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
        let prompt = cx.new(|cx| {
            TextInput::new(window, cx)
                .with_placeholder(format!("Message {label}…"))
                .with_multiline(Some(PROMPT_LINES))
        });
        let subscriptions = vec![
            cx.observe(&fleet, |this, _, cx| {
                this.follow(cx);
                cx.emit(ItemEvent::UpdateTab);
                cx.notify();
            }),
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &prompt,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Submitted => this.send(cx),
                    // Escape hands the keyboard back to the thread.
                    InputEvent::Cancelled => window.focus(&this.focus_handle, cx),
                    InputEvent::Edited | InputEvent::Blurred => cx.notify(),
                },
            ),
        ];
        let list = ListState::new(0, ListAlignment::Top, px(1000.));
        list.set_follow_mode(FollowMode::Tail);
        let mut item = ConversationItem {
            fleet,
            agent,
            focus_handle: cx.focus_handle(),
            prompt,
            list,
            entries: Vec::new(),
            model: None,
            cwd: None,
            permission_mode: None,
            notice: None,
            plain: None,
            loaded: Loaded::Loading,
            asking: false,
            again: false,
            shown: false,
            stale: false,
            seen: None,
            poll: None,
            animation: Animation::default(),
            _subscriptions: subscriptions,
        };
        item.seen = item.seen(cx);
        item.refresh(cx);
        item
    }

    /// The agent whose conversation this is.
    pub fn agent_id(&self) -> &str {
        &self.agent
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn entries(&self) -> &[ConversationEntry] {
        &self.entries
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn plain(&self) -> Option<&str> {
        self.plain.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn loaded(&self) -> &Loaded {
        &self.loaded
    }

    pub fn prompt(&self) -> &Entity<TextInput> {
        &self.prompt
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn list(&self) -> &ListState {
        &self.list
    }

    /// Gives the prompt box the keyboard.
    pub fn focus_prompt(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.prompt.focus_handle(cx), cx);
    }

    fn seen(&self, cx: &App) -> Option<Seen> {
        let fleet = self.fleet.read(cx);
        let store = &fleet.store;
        let agent = store.agents.get(&self.agent)?;
        let message = store
            .messages
            .iter()
            .rev()
            .find(|m| m.to == self.agent || m.from == self.agent)
            .map(|m| m.id);
        Some(Seen {
            state: agent.state,
            since: agent.state_since,
            tool: agent.last_tool.clone(),
            transcript: agent.transcript.clone(),
            message,
            snapshots: fleet.snapshots(),
        })
    }

    /// The fleet moved: if the agent did, there is likely more to read,
    /// now if the tab shows, else once it does. A new transcript (a new
    /// session in the seat) is read from its start.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let seen = self.seen(cx);
        if seen == self.seen {
            return;
        }
        let new_session = seen.as_ref().map(|s| &s.transcript) != self.seen.as_ref().map(|s| &s.transcript);
        self.seen = seen;
        if new_session {
            let old = self.entries.len();
            self.entries.clear();
            self.list.splice(0..old, 0);
        }
        if self.shown {
            self.refresh(cx);
        } else {
            self.stale = true;
        }
    }

    /// Asks for what came since the last entry (everything, at first).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.asking {
            self.again = true;
            return;
        }
        self.asking = true;
        self.stale = false;
        let after = if self.plain.is_some() {
            None
        } else {
            resume_after(&self.entries)
        };
        let this = cx.weak_entity();
        let agent = self.agent.clone();
        self.fleet.update(cx, |fleet, cx| {
            fleet.conversation(
                &agent,
                after,
                TAIL,
                move |reply, cx| {
                    // Closed meanwhile: nobody to tell.
                    let _ = this.update(cx, |item, cx| item.receive(after, reply, cx));
                },
                cx,
            )
        });
    }

    fn receive(
        &mut self,
        after: Option<u64>,
        reply: Result<ainet_core::Conversation, String>,
        cx: &mut Context<Self>,
    ) {
        self.asking = false;
        match reply {
            Ok(c) => {
                match after.filter(|_| c.plain.is_none()) {
                    Some(after) => {
                        let (range, count) = merge(&mut self.entries, after, c.entries);
                        if !range.is_empty() || count > 0 {
                            self.list.splice(range, count);
                        }
                    }
                    None => {
                        let old = self.entries.len();
                        self.entries = c.entries;
                        self.list.splice(0..old, self.entries.len());
                    }
                }
                // Facts only a part of the transcript may say: kept.
                self.model = c.model.or(self.model.take());
                self.cwd = c.cwd.or(self.cwd.take());
                self.permission_mode = c.permission_mode.or(self.permission_mode.take());
                self.notice = c.notice;
                self.plain = c.plain;
                self.loaded = Loaded::Ready;
                self.stick(Moved::Changed);
            }
            Err(why) => self.loaded = Loaded::Failed(why),
        }
        if std::mem::take(&mut self.again) {
            self.refresh(cx);
        }
        cx.emit(ItemEvent::UpdateTab);
        cx.notify();
    }

    /// Makes the thread follow its end, or not, as Comms does.
    fn stick(&self, moved: Moved) {
        let stuck = self.list.is_following_tail();
        match (stuck, sticks(stuck, moved)) {
            (false, true) => self.list.set_follow_mode(FollowMode::Tail),
            (true, false) => self.list.pause_following_tail(),
            _ => {}
        }
    }

    /// Sends what was typed to the agent, as the Message modal does
    /// (`command::message_to`): delivered into its terminal.
    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let agent = self.agent.clone();
        self.fleet.update(cx, |fleet, cx| {
            // To this agent whatever the text says: a leading `@word` is
            // the command line's address, not the box's.
            let sent = match fleet.store.agents.get(&agent) {
                Some(a) => command::message_to(a, text),
                None => Command::Error("no such agent".into()),
            };
            fleet.run(sent, cx);
        });
        self.prompt.update(cx, |input, cx| input.set_text("", cx));
        self.stick(Moved::Sent);
        cx.notify();
    }

    /// While on show, asks every [`POLL_EVERY`] for what came.
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_EVERY).await;
                let Ok(()) = this.update(cx, |item, cx| {
                    if !item.asking {
                        item.refresh(cx);
                    }
                }) else {
                    return;
                };
            }
        }));
    }

    /// Text with its `ainet …` and `rig …` commands in the warning colour,
    /// in the buffer font for code.
    fn styled(&self, text: &str, code: bool, window: &Window, cx: &App) -> StyledText {
        let color = cx.theme().status().warning;
        let mut style = window.text_style();
        if code {
            style.font_family = theme::theme_settings(cx).buffer_font(cx).family.clone();
        }
        let highlights = commands(text).into_iter().map(|range| {
            let style = HighlightStyle {
                color: Some(color),
                font_weight: Some(FontWeight::MEDIUM),
                ..HighlightStyle::default()
            };
            (range, style)
        });
        StyledText::new(text.to_string()).with_default_highlights(&style, highlights)
    }

    /// Words of a prompt, a reply or a message: paragraphs, and fenced
    /// code in the buffer font.
    fn render_text(&self, id: u64, text: &str, window: &Window, cx: &App) -> Div {
        let colors = cx.theme().colors();
        let has_command = !commands(text).is_empty();
        v_flex()
            .gap_1()
            .text_ui(cx)
            .when(has_command, |el| {
                el.debug_selector(move || format!("CONVERSATION-COMMAND-{id}"))
            })
            .children(comms::blocks(text).into_iter().map(|block| {
                match block {
                    Block::Text(text) => div()
                        .child(self.styled(&text, false, window, cx))
                        .into_any_element(),
                    Block::Code(code) => div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(colors.element_background)
                        .font_buffer(cx)
                        .text_ui_sm(cx)
                        .child(self.styled(&code, true, window, cx))
                        .into_any_element(),
                }
            }))
    }

    fn when(&self, entry: &ConversationEntry) -> Option<Label> {
        entry.at.map(|at| {
            Label::new(format!("{} ago", age(at)))
                .size(LabelSize::XSmall)
                .color(Color::Muted)
        })
    }

    /// Your prompt, on a lighter block as Zed's agent panel has it.
    fn render_you(&self, e: &ConversationEntry, window: &Window, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let id = e.id;
        v_flex()
            .debug_selector(move || format!("CONVERSATION-YOU-{id}"))
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.element_background)
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(IconName::Person)
                            .size(IconSize::XSmall)
                            .color(Color::Accent),
                    )
                    .child(
                        Label::new("You")
                            .size(LabelSize::XSmall)
                            .weight(FontWeight::SEMIBOLD)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1())
                    .children(self.when(e)),
            )
            .child(self.render_text(id, &e.text, window, cx))
            .into_any_element()
    }

    /// The agent's words, after a dot.
    fn render_reply(&self, e: &ConversationEntry, window: &Window, cx: &App) -> AnyElement {
        let id = e.id;
        h_flex()
            .debug_selector(move || format!("CONVERSATION-REPLY-{id}"))
            .items_start()
            .gap_2()
            .child(div().pt(px(7.)).child(Indicator::dot().color(Color::Muted)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_text(id, &e.text, window, cx)),
            )
            .into_any_element()
    }

    /// A tool call: `Bash(cargo test)` after a dot that says how it went,
    /// what it was for, and the first lines of its result.
    fn render_tool(&self, e: &ConversationEntry, window: &Window, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let id = e.id;
        let (dot, status) = match (e.done, e.failed) {
            (false, _) => (Color::Accent, Some("running")),
            (true, true) => (Color::Error, Some("failed")),
            (true, false) => (Color::Success, None),
        };
        let title = tool_title(e);
        let name_len = title.find('(').unwrap_or(title.len());
        let mut highlights = vec![(
            0..name_len,
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..HighlightStyle::default()
            },
        )];
        let command = if name_len < title.len() {
            commands(&title[name_len + 1..])
                .into_iter()
                .map(|r| r.start + name_len + 1..r.end + name_len + 1)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let warning = cx.theme().status().warning;
        highlights.extend(command.iter().map(|r| {
            (
                r.clone(),
                HighlightStyle {
                    color: Some(warning),
                    ..HighlightStyle::default()
                },
            )
        }));
        let mut style = window.text_style();
        style.font_family = theme::theme_settings(cx).buffer_font(cx).family.clone();
        let header = h_flex()
            .items_start()
            .gap_2()
            .child(div().pt(px(6.)).child(Indicator::dot().color(dot)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_buffer(cx)
                    .text_ui_sm(cx)
                    .when(!command.is_empty(), |el| {
                        el.debug_selector(move || format!("CONVERSATION-COMMAND-{id}"))
                    })
                    .child(StyledText::new(title).with_default_highlights(&style, highlights)),
            )
            .children(status.map(|s| Label::new(s).size(LabelSize::XSmall).color(dot)))
            .children(self.when(e));
        let output = (!e.output.is_empty()).then(|| {
            let mut lines = e.output.lines();
            let first = lines.next().unwrap_or("");
            let text = std::iter::once(format!("⎿  {first}"))
                .chain(lines.map(|l| format!("   {l}")))
                .collect::<Vec<_>>()
                .join("\n");
            v_flex()
                .debug_selector(move || format!("CONVERSATION-OUTPUT-{id}"))
                .ml(rems(0.4))
                .pl_3()
                .py_0p5()
                .gap_0p5()
                .border_l_1()
                .border_color(colors.border_variant)
                .child(
                    Label::new("Output excerpt")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted)
                        .buffer_font(cx),
                )
                .child(
                    div()
                        .font_buffer(cx)
                        .text_ui_xs(cx)
                        .text_color(if e.failed {
                            Color::Error.color(cx)
                        } else {
                            colors.text_muted
                        })
                        .child(text),
                )
        });
        v_flex()
            .debug_selector(move || format!("CONVERSATION-TOOL-{id}"))
            .gap_1()
            .child(header)
            .when(!e.text.is_empty(), |el| {
                el.child(
                    div().pl_4().child(
                        Label::new(e.text.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
                )
            })
            .children(output)
            .into_any_element()
    }

    /// A message ainetd typed into the terminal: framed, with who sent it
    /// to whom and when, its words, and how the agent answers it.
    fn render_inbound(&self, e: &ConversationEntry, window: &Window, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let store = &self.fleet.read(cx).store;
        let id = e.id;
        let title = match (&e.room, e.message, e.reply_to) {
            (Some(room), _, _) => format!("Mention in {room}"),
            (None, Some(n), Some(r)) => format!("Reply #{n} to #{r}"),
            (None, Some(n), None) => format!("Message #{n}"),
            (None, None, _) => "Message".to_string(),
        };
        let from_color = if comms::is_you(&e.from) || e.from.starts_with("you") {
            Color::Accent
        } else {
            match store.find_agent(&e.from) {
                Some(a) => Color::Custom(comms::hue(&a.id, cx)),
                None => Color::Default,
            }
        };
        let to = store
            .agents
            .get(&self.agent)
            .map(|a| a.seat.clone().unwrap_or_else(|| a.name.clone()))
            .unwrap_or_default();
        let field = |name: &'static str, value: Label| {
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(rems(3.))
                        .child(Label::new(name).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .child(value)
        };
        let rule = || div().h_px().w_full().bg(colors.border_variant);
        let hint = reply_hint(e);
        v_flex()
            .debug_selector(move || format!("CONVERSATION-INBOUND-{id}"))
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface_background)
            .overflow_hidden()
            .child(
                h_flex()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .bg(colors.element_background)
                    .child(
                        Icon::new(IconName::Envelope)
                            .size(IconSize::XSmall)
                            .color(Color::Accent),
                    )
                    .child(
                        Label::new(title)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child(div().flex_1())
                    .children(self.when(e)),
            )
            .child(
                v_flex()
                    .px_2()
                    .py_1()
                    .gap_0p5()
                    .child(field(
                        "From",
                        Label::new(e.from.clone())
                            .size(LabelSize::XSmall)
                            .color(from_color),
                    ))
                    .child(field("To", Label::new(to).size(LabelSize::XSmall)))
                    .children(e.at.map(|at| {
                        field(
                            "Sent",
                            Label::new(format!("{} ago", age(at))).size(LabelSize::XSmall),
                        )
                    })),
            )
            .child(rule())
            .child(
                div()
                    .px_2()
                    .py_1p5()
                    .child(self.render_text(id, &e.text, window, cx)),
            )
            .children(hint.map(|hint| {
                v_flex().child(rule()).child(
                    h_flex()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .child(Label::new("↩ Reply:").size(LabelSize::XSmall).color(Color::Muted))
                        .child(
                            div()
                                .font_buffer(cx)
                                .text_ui_xs(cx)
                                .text_color(cx.theme().status().warning)
                                .child(hint),
                        ),
                )
            }))
            .into_any_element()
    }

    fn render_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(e) = self.entries.get(ix) else {
            return div().into_any_element();
        };
        let body = match e.kind {
            EntryKind::You => self.render_you(e, window, cx),
            EntryKind::Reply => self.render_reply(e, window, cx),
            EntryKind::Tool => self.render_tool(e, window, cx),
            EntryKind::Inbound => self.render_inbound(e, window, cx),
            EntryKind::Other => Label::new("(something this version cannot show)")
                .size(LabelSize::XSmall)
                .color(Color::Disabled)
                .into_any_element(),
        };
        div().w_full().px_3().py_1p5().child(body).into_any_element()
    }

    /// Who: the critter, the runtime and the seat, its state; and the way
    /// to its terminal.
    fn render_header(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let fleet = self.fleet.read(cx);
        let agent = fleet.store.agents.get(&self.agent)?;
        let seat = agent.seat.clone().unwrap_or_else(|| agent.name.clone());
        let id = agent.id.clone();
        let fleet_entity = self.fleet.clone();
        let count = match self.entries.len() {
            0 => None,
            1 => Some("1 entry".to_string()),
            n => Some(format!("{n} entries")),
        };
        Some(
            h_flex()
                .flex_none()
                .gap_2()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(cx.theme().colors().border_variant)
                .bg(cx.theme().colors().tab_bar_background)
                .child(
                    Critter::agent(agent, fleet.tick(), cx)
                        .size(px(20.))
                        .animate(&self.animation),
                )
                .child(
                    Label::new(agent.runtime.label())
                        .size(LabelSize::Small)
                        .weight(FontWeight::SEMIBOLD),
                )
                .child(
                    Label::new(seat)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .buffer_font(cx),
                )
                .child(Indicator::dot().color(state_color(agent.state)))
                .child(
                    Label::new(agent.state.label())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(div().flex_1())
                .children(count.map(|c| Label::new(c).size(LabelSize::XSmall).color(Color::Muted)))
                .child(
                    IconButton::new("conversation-terminal", IconName::Terminal)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Open Terminal"))
                        .on_click(move |_, window, cx| {
                            fleet_entity.update(cx, |fleet, cx| fleet.select(&id, cx));
                            window.dispatch_action(OpenTerminal.boxed_clone(), cx);
                        }),
                )
                .child(
                    IconButton::new("conversation-refresh", IconName::RotateCw)
                        .icon_size(IconSize::Small)
                        .disabled(self.asking)
                        .tooltip(Tooltip::text("Read Again"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx))),
                ),
        )
    }

    /// The thread, or what stands in for it.
    fn render_thread(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let waiting = |text: String, color: Color| {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .p_4()
                .child(Label::new(text).color(color))
                .into_any_element()
        };
        if let Some(plain) = &self.plain {
            return div()
                .id("conversation-plain")
                .size_full()
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .font_buffer(cx)
                .text_ui_sm(cx)
                .debug_selector(|| "CONVERSATION-PLAIN".into())
                .child(plain.clone())
                .into_any_element();
        }
        if self.entries.is_empty() {
            return match &self.loaded {
                Loaded::Loading => waiting("Reading the transcript…".into(), Color::Muted),
                Loaded::Failed(why) => waiting(why.clone(), Color::Error),
                Loaded::Ready if self.notice.is_some() => div().into_any_element(),
                Loaded::Ready => waiting("Nothing said yet.".into(), Color::Muted),
            };
        }
        div()
            .size_full()
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix: usize, window, cx| this.render_entry(ix, window, cx)),
                )
                .size_full()
                .py_1(),
            )
            .vertical_scrollbar_for(&self.list, window, cx)
            .into_any_element()
    }

    /// The prompt box: `❯`, the field and send.
    fn render_prompt(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let focused = self.prompt.focus_handle(cx).is_focused(window);
        let empty = self.prompt.read(cx).is_empty();
        h_flex()
            .flex_none()
            .px_3()
            .pt_2()
            .pb_1()
            .bg(colors.panel_background)
            .border_t_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .flex_1()
                    .items_start()
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
                    .child(
                        Label::new("❯")
                            .color(if focused { Color::Accent } else { Color::Muted })
                            .buffer_font(cx),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "CONVERSATION-PROMPT".into())
                            .child(self.prompt.clone()),
                    )
                    .child(
                        IconButton::new("conversation-send", IconName::Send)
                            .icon_size(IconSize::Small)
                            .icon_color(if empty { Color::Muted } else { Color::Accent })
                            .disabled(empty)
                            .tooltip(Tooltip::text("Send (enter; shift-enter for a new line)"))
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    ),
            )
    }

    /// The session: permission mode, model and working directory, as the
    /// transcript says them, else as the fleet knows them.
    fn render_footer(&self, cx: &App) -> Option<Div> {
        let agent = self.fleet.read(cx).store.agents.get(&self.agent)?;
        let home = std::env::var("HOME").unwrap_or_default();
        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| agent.cwd.display().to_string());
        let cwd = match cwd.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => cwd,
        };
        let parts: Vec<String> = [
            self.permission_mode.clone(),
            self.model.clone().or_else(|| agent.model.clone()),
            Some(cwd),
        ]
        .into_iter()
        .flatten()
        .collect();
        Some(
            h_flex()
                .flex_none()
                .px_3()
                .pb_1p5()
                .bg(cx.theme().colors().panel_background)
                .debug_selector(|| "CONVERSATION-FOOTER".into())
                .child(
                    Label::new(parts.join(" · "))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted)
                        .truncate(),
                ),
        )
    }
}

impl EventEmitter<ItemEvent> for ConversationItem {}

impl Focusable for ConversationItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ConversationItem {
    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.fleet
            .read(cx)
            .store
            .agents
            .get(&self.agent)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Conversation".into())
            .into()
    }

    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> AnyElement {
        Label::new(self.tab_content_text(cx))
            .color(params.text_color())
            .into_any_element()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        let color = match self.loaded {
            Loaded::Failed(_) => Color::Error,
            _ => Color::Muted,
        };
        Some(
            Icon::new(IconName::Chat)
                .size(IconSize::Small)
                .color(color)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        let agent = self.fleet.read(cx).store.agents.get(&self.agent)?;
        Some(format!("Conversation with {}", agent.label()).into())
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        Some(SavedItem::Conversation {
            agent: self.agent.clone(),
        })
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Self>> {
        let (fleet, agent) = (self.fleet.clone(), self.agent.clone());
        Some(cx.new(|cx| ConversationItem::new(fleet, agent, window, cx)))
    }

    /// On show: what came while hidden, then a look every few seconds.
    fn activated(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.start_polling(cx);
        if self.stale {
            self.refresh(cx);
        }
    }

    fn deactivated(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.shown = false;
        self.poll = None;
    }

    fn on_removed(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.shown = false;
        self.poll = None;
    }
}

impl Render for ConversationItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let notice = self.notice.clone().map(|notice| {
            h_flex()
                .flex_none()
                .gap_2()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(colors.border_variant)
                .bg(colors.surface_background)
                .debug_selector(|| "CONVERSATION-NOTICE".into())
                .child(
                    Icon::new(IconName::Info)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(notice).size(LabelSize::Small).color(Color::Muted))
        });
        v_flex()
            .id("conversation")
            .key_context("Conversation")
            .size_full()
            .bg(colors.editor_background)
            .children(self.render_header(cx))
            .children(notice)
            .child(
                div()
                    .id("conversation-thread")
                    .track_focus(&self.focus_handle)
                    .flex_1()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
                    )
                    // Enter on the thread: to the prompt box, to write.
                    .on_action(
                        cx.listener(|this, _: &menu::Confirm, window, cx| this.focus_prompt(window, cx)),
                    )
                    .child(self.render_thread(window, cx)),
            )
            .child(self.render_prompt(window, cx))
            .children(self.render_footer(cx))
    }
}

#[cfg(test)]
mod pure_tests {
    use super::*;

    fn entry(id: u64, kind: EntryKind) -> ConversationEntry {
        ConversationEntry::new(id, kind, "")
    }

    #[test]
    fn commands_stand_out_in_prose_and_code() {
        let text = "Next `ainet queue update q-1 --note \"ok\"` then done.\nainet msg send lead hi\nthe rig lead said so";
        let found: Vec<&str> = commands(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(
            found,
            ["ainet queue update q-1 --note \"ok\"", "ainet msg send lead hi"]
        );
        let rig =
            "$ rig queue create --destination dev-app@factory\nAnswer with the ainet tool reply_message";
        let found: Vec<&str> = commands(rig).into_iter().map(|r| &rig[r]).collect();
        assert_eq!(found, ["rig queue create --destination dev-app@factory"]);
        assert!(commands("frigate ainetwork rig").is_empty());
    }

    #[test]
    fn tool_titles_and_reply_hints() {
        let mut call = entry(1, EntryKind::Tool);
        call.tool = "Bash".into();
        call.code = "cargo test".into();
        assert_eq!(tool_title(&call), "Bash(cargo test)");
        call.code = "cd api\ncargo test".into();
        assert_eq!(tool_title(&call), "Bash(cd api …)");
        call.code.clear();
        assert_eq!(tool_title(&call), "Bash");
        let mut m = entry(2, EntryKind::Inbound);
        assert_eq!(reply_hint(&m), None);
        m.message = Some(4);
        assert_eq!(reply_hint(&m).as_deref(), Some("ainet msg reply 4 \"…\""));
        m.room = Some("docs · general".into());
        assert_eq!(
            reply_hint(&m).as_deref(),
            Some("ainet chat send in docs · general")
        );
    }

    #[test]
    fn asking_again_starts_at_a_call_still_running() {
        assert_eq!(resume_after(&[]), None);
        let mut running = entry(5, EntryKind::Tool);
        let done = ConversationEntry {
            done: true,
            ..entry(7, EntryKind::Tool)
        };
        let mut entries = vec![
            entry(1, EntryKind::You),
            running.clone(),
            done,
            entry(9, EntryKind::Reply),
        ];
        assert_eq!(resume_after(&entries), Some(4));
        // A call left running before your last prompt never finishes.
        entries.push(entry(12, EntryKind::Inbound));
        assert_eq!(resume_after(&entries), Some(12));

        running.done = true;
        let mut entries = vec![
            entry(1, EntryKind::You),
            entry(5, EntryKind::Tool),
            entry(9, EntryKind::Reply),
        ];
        let (range, count) = merge(
            &mut entries,
            4,
            vec![running, entry(9, EntryKind::Reply), entry(11, EntryKind::Reply)],
        );
        assert_eq!((range, count), (1..3, 3));
        assert_eq!(entries.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 5, 9, 11]);
        assert!(entries[1].done);
        let (range, count) = merge(&mut entries, 11, vec![]);
        assert_eq!((range, count), (4..4, 0));
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::Request;
    use gpui::{TestAppContext, VisualTestContext, size};

    use super::*;
    use crate::actions::{OpenConversation, TileConversations};
    use crate::tests::{demo_workspace, init_test, sent};
    use crate::workspace::Workspace;

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    /// The id of the agent in `seat`.
    fn agent(w: &Entity<Workspace>, seat: &str, cx: &mut VisualTestContext) -> String {
        fleet(w, cx).read_with(cx, |f, _| {
            f.store
                .agents
                .values()
                .find(|a| a.seat.as_deref() == Some(seat) || a.name == seat)
                .map(|a| a.id.clone())
                .expect("the demo has it")
        })
    }

    /// Selects `seat`'s agent and opens its conversation, tall enough for
    /// the whole thread to be drawn.
    fn open(w: &Entity<Workspace>, seat: &str, cx: &mut VisualTestContext) -> Entity<ConversationItem> {
        cx.simulate_resize(size(px(1600.), px(2400.)));
        let id = agent(w, seat, cx);
        fleet(w, cx).update(cx, |f, cx| f.select(&id, cx));
        cx.dispatch_action(OpenConversation);
        cx.run_until_parked();
        conversations(w, cx)
            .into_iter()
            .find(|c| c.read_with(cx, |c, _| c.agent_id() == id))
            .expect("its conversation is open")
    }

    fn conversations(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Entity<ConversationItem>> {
        w.read_with(cx, |w, cx| {
            w.panes()
                .iter()
                .flat_map(|p| {
                    p.read(cx)
                        .items()
                        .filter_map(|i| i.to_any().downcast::<ConversationItem>().ok())
                        .collect::<Vec<_>>()
                })
                .collect()
        })
    }

    fn drawn(selector: String, cx: &mut VisualTestContext) -> bool {
        cx.debug_bounds(Box::leak(selector.into_boxed_str())).is_some()
    }

    fn asked(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Option<u64>> {
        sent(w, cx)
            .into_iter()
            .filter_map(|r| match r {
                Request::Conversation { after, .. } => Some(after),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    async fn the_demo_conversation_reads_as_a_thread(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let talk = open(&w, "dev-app@docs", cx);
        let (kinds, ids) = talk.read_with(cx, |c, _| {
            assert_eq!(c.loaded(), &Loaded::Ready);
            let kinds: Vec<EntryKind> = c.entries().iter().map(|e| e.kind).collect();
            let ids: Vec<u64> = c.entries().iter().map(|e| e.id).collect();
            (kinds, ids)
        });
        use EntryKind::*;
        assert_eq!(kinds, [You, Reply, Tool, Tool, Reply, Tool, Inbound, Tool]);
        // Asked once, for the tail; the prompt box has the keyboard.
        assert_eq!(asked(&w, cx), [None]);
        let prompt = talk.read_with(cx, |c, _| c.prompt().clone());
        cx.update(|window, cx| assert!(prompt.focus_handle(cx).is_focused(window)));

        // Every kind is drawn: the prompt, words, a tool call with its
        // output, the message it got, the commands standing out.
        let you = format!("CONVERSATION-YOU-{}", ids[0]);
        assert!(drawn(you, cx));
        assert!(drawn(format!("CONVERSATION-REPLY-{}", ids[1]), cx));
        assert!(drawn(format!("CONVERSATION-TOOL-{}", ids[2]), cx));
        assert!(drawn(format!("CONVERSATION-OUTPUT-{}", ids[2]), cx));
        assert!(drawn(format!("CONVERSATION-INBOUND-{}", ids[6]), cx));
        // `ainet queue show q-1`, and the reply naming `ainet queue update`.
        assert!(drawn(format!("CONVERSATION-COMMAND-{}", ids[2]), cx));
        assert!(drawn(format!("CONVERSATION-COMMAND-{}", ids[4]), cx));
        assert!(!drawn(format!("CONVERSATION-COMMAND-{}", ids[1]), cx));
        assert!(drawn("CONVERSATION-FOOTER".into(), cx));
        talk.read_with(cx, |c, cx| {
            assert_eq!(
                c.tab_content_text(cx),
                c.fleet.read(cx).store.agents[c.agent_id()].name.as_str()
            );
            assert_eq!(c.permission_mode.as_deref(), Some("acceptEdits"));
            assert_eq!(c.model.as_deref(), Some("Sonnet 5.5"));
        });

        // Asked again, one tab.
        cx.dispatch_action(OpenConversation);
        cx.run_until_parked();
        assert_eq!(conversations(&w, cx).len(), 1);
    }

    #[gpui::test]
    async fn the_prompt_box_messages_the_agent_and_the_thread_grows(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let talk = open(&w, "dev-app@docs", cx);
        let (id, last) = talk.read_with(cx, |c, _| {
            (c.agent_id().to_string(), c.entries().last().unwrap().id)
        });
        cx.simulate_input("Raise the timeout to 5s.");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("Then file a follow-up.");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        // What the Message modal sends: to this agent, through the guard.
        let body = "Raise the timeout to 5s.\nThen file a follow-up.";
        let messages: Vec<Request> = sent(&w, cx)
            .into_iter()
            .filter(|r| matches!(r, Request::Message { .. }))
            .collect();
        let store_agent = fleet(&w, cx).read_with(cx, |f, _| f.store.agents[&id].clone());
        let ainet_model::Command::Request(expected, _) = command::message_to(&store_agent, body.into())
        else {
            panic!("a request");
        };
        assert_eq!(format!("{messages:?}"), format!("{:?}", [*expected]));
        let prompt = talk.read_with(cx, |c, _| c.prompt().clone());
        assert!(prompt.read_with(cx, |p, _| p.is_empty()));
        // The message landed: the thread asked for what came after its
        // last entry, and shows it at the end, followed.
        assert_eq!(asked(&w, cx), [None, Some(last)]);
        talk.read_with(cx, |c, _| {
            let new = c.entries().last().unwrap();
            assert_eq!(
                (new.kind, new.from.as_str(), new.text.as_str()),
                (EntryKind::Inbound, "you (operator)", body)
            );
            assert!(c.list().is_following_tail());
        });
        // Escape gives the thread the keyboard; enter there, the box.
        cx.simulate_keystrokes("escape");
        let thread = talk.read_with(cx, |c, cx| c.focus_handle(cx));
        cx.update(|window, _| assert!(thread.is_focused(window)));
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| assert!(prompt.focus_handle(cx).is_focused(window)));
    }

    #[gpui::test]
    async fn a_hidden_conversation_catches_up_once_shown(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let talk = open(&w, "dev-app@docs", cx);
        let id = talk.read_with(cx, |c, _| c.agent_id().to_string());
        let before = talk.read_with(cx, |c, _| c.entries().len());
        // The Map covers it; a message to the agent asks nothing yet.
        w.update_in(cx, |w, window, cx| w.open_map(window, cx));
        cx.run_until_parked();
        let send = |cx: &mut VisualTestContext| {
            fleet(&w, cx).update(cx, |f, cx| {
                let a = f.store.agents[&id].clone();
                f.run(command::message_to(&a, "Status?".into()), cx)
            });
            cx.run_until_parked();
        };
        send(cx);
        assert_eq!(asked(&w, cx).len(), 1);
        assert_eq!(talk.read_with(cx, |c, _| c.entries().len()), before);
        // Shown again: it asks for what came, once.
        let id2 = id.clone();
        w.update_in(cx, |w, window, cx| w.open_conversation(&id2, window, cx));
        cx.run_until_parked();
        assert_eq!(asked(&w, cx).len(), 2);
        assert_eq!(talk.read_with(cx, |c, _| c.entries().len()), before + 1);
        // On show, a slow look every few seconds.
        cx.executor().advance_clock(POLL_EVERY);
        cx.run_until_parked();
        assert_eq!(asked(&w, cx).len(), 3);
        assert_eq!(talk.read_with(cx, |c, _| c.entries().len()), before + 1);
    }

    #[gpui::test]
    async fn codex_seats_and_old_daemons_get_a_notice(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let codex = open(&w, "ops-data@infra", cx);
        codex.read_with(cx, |c, _| {
            assert!(c.entries().is_empty());
            assert!(c.notice().unwrap().starts_with("Codex keeps"), "{:?}", c.notice());
        });
        assert!(drawn("CONVERSATION-NOTICE".into(), cx));
    }

    #[gpui::test]
    async fn an_old_daemon_gives_the_transcript_as_text(cx: &mut TestAppContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(|cx| FleetModel::demo_with(ainet_model::Demo::without_conversations(), cx));
            Workspace::new(fleet, None, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let talk = open(&w, "dev-app@docs", cx);
        let task =
            fleet(&w, cx).read_with(cx, |f, _| f.store.selected_agent().unwrap().task.clone().unwrap());
        talk.read_with(cx, |c, _| {
            assert!(c.entries().is_empty());
            assert_eq!(c.notice(), Some(ainet_model::link::OLD_DAEMON));
            assert!(c.plain().unwrap().contains(&task));
        });
        assert!(drawn("CONVERSATION-PLAIN".into(), cx));
        assert!(drawn("CONVERSATION-NOTICE".into(), cx));
        // Asked again, the whole text again: it has no entries to go on from.
        talk.update(cx, |c, cx| c.refresh(cx));
        cx.run_until_parked();
        assert_eq!(asked(&w, cx), [None, None]);
    }

    #[gpui::test]
    async fn keys_open_it_and_a_rig_lays_out_side_by_side(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        // `c` in the Fleet, which has the keyboard at start.
        cx.simulate_keystrokes("c");
        cx.run_until_parked();
        let selected = fleet(&w, cx).read_with(cx, |f, _| f.store.selected.clone().unwrap());
        let open = conversations(&w, cx);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].read_with(cx, |c, _| c.agent_id().to_string()), selected);
        // ctrl-shift-o from the thread, for another agent.
        let ui = agent(&w, "dev-ui@docs", cx);
        fleet(&w, cx).update(cx, |f, cx| f.select(&ui, cx));
        cx.simulate_keystrokes("escape ctrl-shift-o");
        cx.run_until_parked();
        assert_eq!(conversations(&w, cx).len(), 2);

        // The docs rig's five, side by side, each in a pane of its own;
        // the two open stay where they are.
        cx.dispatch_action(TileConversations);
        cx.run_until_parked();
        let (agents, panes) = w.read_with(cx, |w, cx| {
            let docs = w.fleet().read(cx).store.terminal_ids(Some("docs"));
            (docs, w.panes().len())
        });
        assert_eq!(agents.len(), 5);
        let open: Vec<String> = conversations(&w, cx)
            .iter()
            .map(|c| c.read_with(cx, |c, _| c.agent_id().to_string()))
            .collect();
        assert_eq!(open.len(), 5);
        assert!(agents.iter().all(|a| open.contains(a)));
        assert_eq!(panes, 4, "three new panes beside the one holding the first two");
        cx.dispatch_action(TileConversations);
        cx.run_until_parked();
        assert_eq!(conversations(&w, cx).len(), 5);
    }
}
