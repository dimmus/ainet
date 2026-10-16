//! The command line: OpenRig's `cmd ▸` line as a strip above the status
//! bar, there all the time (settings.json `"command_line"`). A line means
//! what it means in the TUI's input line, through the shared command
//! language: `/command` as in the palette and Comms (`commands::parse` and
//! `run`), `@Name text` to that agent (`command::message`), anything else
//! to the selected agent, or, while the Inbox shows a question waiting on
//! you, its answer (`command::answer_question`).
//!
//! `tab` completes `@names` and `/command` names, `up` and `down` recall
//! the lines typed before (the last 50, kept with the workspace), and
//! `ctrl-;` reaches it from anywhere but a terminal; escape gives the
//! keyboard back to where it was. What a line will do shows beside it
//! while you type; what was wrong with one shows there in red, and as a
//! toast, and the line stays to be put right.

use ainet_core::InboxItem as Question;
use ainet_model::{Command, Store, command};
use gpui::{Entity, FocusHandle, Focusable, Subscription, WeakEntity, WeakFocusHandle};
use ui::{KeyBinding, prelude::*};

use crate::actions::command_line::{Complete, NextLine, PreviousLine, ToggleFocus};
use crate::commands;
use crate::fleet::FleetModel;
use crate::persistence::COMMAND_HISTORY;
use crate::text_input::{InputEvent, TextInput};
use crate::workspace::Workspace;

/// What the strip says while the line is empty.
const HINT: &str = "/command · @Name message · text to the selected agent · Tab completes";

/// The lines typed, the latest last, and where `up` and `down` are in
/// them.
#[derive(Debug, Default)]
pub struct History {
    lines: Vec<String>,
    /// The line recalled, if one is.
    at: Option<usize>,
    /// What was being typed before the first `up`, for `down` past the
    /// newest line.
    draft: String,
}

impl History {
    /// The lines, the latest last.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Replaces the lines (a saved workspace's), keeping the latest.
    pub fn set_lines(&mut self, lines: Vec<String>) {
        let skip = lines.len().saturating_sub(COMMAND_HISTORY);
        self.lines = lines.into_iter().skip(skip).collect();
        self.reset();
    }

    /// Remembers a line run; the same line twice in a row once.
    pub fn push(&mut self, line: &str) {
        self.reset();
        if self.lines.last().map(String::as_str) != Some(line) {
            self.lines.push(line.to_string());
        }
        let excess = self.lines.len().saturating_sub(COMMAND_HISTORY);
        self.lines.drain(..excess);
    }

    /// `up`: the line before the one recalled (the newest at first, when
    /// `typed` is kept for coming back down). `None` at the oldest.
    pub fn previous(&mut self, typed: &str) -> Option<&str> {
        let at = match self.at {
            None if self.lines.is_empty() => return None,
            None => {
                self.draft = typed.to_string();
                self.lines.len() - 1
            }
            Some(0) => return None,
            Some(at) => at - 1,
        };
        self.at = Some(at);
        Some(&self.lines[at])
    }

    /// `down`: the line after the one recalled, or past the newest what
    /// was being typed. `None` when nothing is recalled.
    pub fn next(&mut self) -> Option<String> {
        let at = self.at?;
        if at + 1 < self.lines.len() {
            self.at = Some(at + 1);
            Some(self.lines[at + 1].clone())
        } else {
            self.at = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// The line recalled, if one is.
    pub fn recalled(&self) -> Option<&str> {
        self.at.map(|at| self.lines[at].as_str())
    }

    /// Forgets where `up` and `down` were: the line was run or edited.
    pub fn reset(&mut self) {
        self.at = None;
        self.draft.clear();
    }
}

/// What enter does with `line`: a `/command` (rig commands on
/// `view_rig`, the rig the active view shows, `/answer` on `question`),
/// an `@Name` message, the answer to `question` (the Inbox's, when it
/// shows one waiting), or a message to the selected agent. As the TUI's
/// input line.
pub fn command_for(
    store: &Store,
    question: Option<&Question>,
    view_rig: Option<&str>,
    line: &str,
) -> Command {
    if let Some(cmd) = line.strip_prefix('/') {
        return commands::parse_in(store, cmd, view_rig, question);
    }
    match question {
        Some(question) if !line.starts_with('@') => command::answer_question(question, line.to_string()),
        _ => command::message(store, line, store.selected.as_deref()),
    }
}

/// What `line` will do, said beside it as it is typed; `None` for the
/// hint (nothing typed).
pub fn preview(store: &Store, question: Option<&Question>, line: &str) -> Option<String> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let name = |id: &str| {
        store
            .agents
            .get(id)
            .map(|a| a.label())
            .unwrap_or_else(|| id.to_string())
    };
    if let Some(rest) = line.strip_prefix('/') {
        if let Some(c) = command::command_named(line) {
            return Some(format!("{} · {}", c.usage(), c.about));
        }
        if rest.contains(' ') {
            return None;
        }
        // Still typing the name: the commands it may be. Short aliases
        // (`/q`, `/y`) are not listed, so no match is not called wrong.
        let names: Vec<String> = command::commands_starting(rest)
            .map(|c| format!("/{}", c.name))
            .collect();
        return (!names.is_empty()).then(|| names.join("  "));
    }
    if let Some(rest) = line.strip_prefix('@') {
        let who = rest.split_whitespace().next().unwrap_or("");
        return Some(match store.find_agent(who) {
            Some(a) => format!("to {}", a.label()),
            None => format!("no agent called {who}"),
        });
    }
    if let Some(question) = question {
        return Some(format!("answers {}'s question", name(&question.agent)));
    }
    Some(match &store.selected {
        Some(id) => format!("to {}", name(id)),
        None => "select an agent, or start with @Name".to_string(),
    })
}

pub struct CommandLine {
    fleet: Entity<FleetModel>,
    workspace: WeakEntity<Workspace>,
    input: Entity<TextInput>,
    history: History,
    /// What was wrong with the last line run, until it is edited.
    error: Option<String>,
    /// Where the keyboard was before it came here, for escape.
    came_from: Option<WeakFocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl CommandLine {
    pub fn new(
        fleet: Entity<FleetModel>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(window, cx).with_placeholder("Type a command or a message"));
        let subscriptions = vec![
            // Who a line goes to follows the selection.
            cx.observe(&fleet, |_, _, cx| cx.notify()),
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Submitted => this.submit(window, cx),
                    InputEvent::Cancelled => this.give_back(window, cx),
                    InputEvent::Edited => {
                        // A recalled line, edited, is a new line.
                        if this.history.recalled() != Some(input.read(cx).text()) {
                            this.history.reset();
                        }
                        this.error = None;
                        cx.notify();
                    }
                    InputEvent::Blurred => cx.notify(),
                }
            }),
        ];
        CommandLine {
            fleet,
            workspace,
            input,
            history: History::default(),
            error: None,
            came_from: None,
            _subscriptions: subscriptions,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn input(&self) -> &Entity<TextInput> {
        &self.input
    }

    /// What was wrong with the last line run, while it shows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The lines typed, the latest last (saved with the workspace).
    pub fn history(&self) -> &[String] {
        self.history.lines()
    }

    pub fn set_history(&mut self, lines: Vec<String>) {
        self.history.set_lines(lines);
    }

    /// Whether the line has the keyboard.
    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.focus_handle(cx).is_focused(window)
    }

    /// `ctrl-;`: the keyboard to the line, remembering where it was; or,
    /// if the line has it, back there.
    pub fn toggle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_focused(window, cx) {
            return self.give_back(window, cx);
        }
        self.came_from = window.focused(cx).map(|f| f.downgrade());
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Escape: the keyboard back where it was before the line had it, or
    /// to the active pane if that is gone.
    fn give_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let back = self.came_from.take().and_then(|f| f.upgrade());
        match back {
            Some(focus) => window.focus(&focus, cx),
            None => {
                if let Some(workspace) = self.workspace.upgrade() {
                    let pane = workspace.read(cx).active_pane().focus_handle(cx);
                    window.focus(&pane, cx);
                }
            }
        }
        cx.notify();
    }

    /// The question the Inbox shows, if it is the active tab and the
    /// question waits on you.
    fn question(&self, cx: &App) -> Option<Question> {
        self.workspace.upgrade()?.read(cx).inbox_question(cx)
    }

    /// Enter: runs the line. Done, it clears; wrong, it stays, and what
    /// is wrong shows beside it (and as a toast).
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.input.read(cx).text().trim().to_string();
        if line.is_empty() {
            return;
        }
        self.history.push(&line);
        let question = self.question(cx);
        let view_rig = self.workspace.upgrade().and_then(|w| w.read(cx).view_rig(cx));
        let cmd = command_for(
            &self.fleet.read(cx).store,
            question.as_ref(),
            view_rig.as_deref(),
            &line,
        );
        let problem = match cmd {
            // `/close` with nobody selected, `/answer` with nothing asked.
            Command::Nothing => Some("nothing to act on".to_string()),
            cmd => commands::run(&self.fleet, cmd, window, cx),
        };
        match problem {
            Some(problem) => self.error = Some(problem),
            None => self.input.update(cx, |input, cx| input.set_text("", cx)),
        }
        cx.notify();
    }

    /// Tab: completes the `@name` or `/command` name being typed.
    fn complete(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_string();
        if let Some(done) = command::complete_line(&self.fleet.read(cx).store, &text) {
            self.input.update(cx, |input, cx| input.set_text(&done, cx));
        }
    }

    /// `up`: the line typed before.
    fn previous_line(&mut self, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).text().to_string();
        if let Some(line) = self.history.previous(&typed).map(str::to_string) {
            self.input.update(cx, |input, cx| input.set_text(&line, cx));
        }
    }

    /// `down`: the line typed after, or back to what was being typed.
    fn next_line(&mut self, cx: &mut Context<Self>) {
        if let Some(line) = self.history.next() {
            self.input.update(cx, |input, cx| input.set_text(&line, cx));
        }
    }
}

impl Focusable for CommandLine {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for CommandLine {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let focused = self.is_focused(window, cx);
        let text = self.input.read(cx).text().to_string();
        let question = self.question(cx);
        let (note, color) = match &self.error {
            Some(error) => (error.clone(), Color::Error),
            None => match preview(&self.fleet.read(cx).store, question.as_ref(), &text) {
                Some(preview) => (preview, Color::Muted),
                None => (HINT.to_string(), Color::Disabled),
            },
        };
        h_flex()
            .id("command-line")
            .key_context("CommandLine")
            .flex_none()
            .w_full()
            .gap_2()
            .px(DynamicSpacing::Base08.rems(cx))
            .py_0p5()
            .bg(colors.editor_background)
            .border_t_1()
            .border_color(if focused {
                colors.border_focused
            } else {
                colors.border
            })
            // A click into the line remembers where the keyboard was, as
            // `ctrl-;` does, so escape goes back there.
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                if !this.is_focused(window, cx) {
                    this.came_from = window.focused(cx).map(|f| f.downgrade());
                }
            }))
            .on_action(cx.listener(|this, _: &Complete, _, cx| this.complete(cx)))
            .on_action(cx.listener(|this, _: &PreviousLine, _, cx| this.previous_line(cx)))
            .on_action(cx.listener(|this, _: &NextLine, _, cx| this.next_line(cx)))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Label::new("cmd")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .buffer_font(cx),
                    )
                    .child(
                        Label::new("▸")
                            .size(LabelSize::Small)
                            .color(if focused { Color::Accent } else { Color::Muted })
                            .buffer_font(cx),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(rems(12.))
                    .debug_selector(|| "COMMAND-LINE-INPUT".into())
                    .child(self.input.clone()),
            )
            .child(
                h_flex()
                    .id("command-line-note")
                    .flex_shrink(1.)
                    .min_w_0()
                    .max_w(relative(0.6))
                    .gap_2()
                    .overflow_x_hidden()
                    .child(Label::new(note).size(LabelSize::Small).color(color).truncate())
                    .when(!focused && text.is_empty() && self.error.is_none(), |el| {
                        el.child(KeyBinding::for_action(&ToggleFocus, cx).size(rems_from_px(12_f32)))
                    }),
            )
    }
}

#[cfg(test)]
mod pure_tests {
    use ainet_core::InboxKind;
    use ainet_core::Request;
    use ainet_model::{Demo, Msg};

    use super::*;

    fn demo() -> Store {
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        store
    }

    #[test]
    fn history_recalls_up_and_down_and_keeps_what_was_typed() {
        let mut h = History::default();
        assert_eq!(h.previous("x"), None, "nothing typed yet");
        for line in ["/ps", "@lead hi", "@lead hi", "hello"] {
            h.push(line);
        }
        assert_eq!(
            h.lines(),
            ["/ps", "@lead hi", "hello"],
            "the same line twice once"
        );
        assert_eq!(h.next(), None, "nothing recalled");
        assert_eq!(h.previous("half"), Some("hello"));
        assert_eq!(h.previous("hello"), Some("@lead hi"));
        assert_eq!(h.previous("@lead hi"), Some("/ps"));
        assert_eq!(h.previous("/ps"), None, "the oldest");
        assert_eq!(h.recalled(), Some("/ps"));
        assert_eq!(h.next().as_deref(), Some("@lead hi"));
        assert_eq!(h.next().as_deref(), Some("hello"));
        assert_eq!(h.next().as_deref(), Some("half"), "back to what was typed");
        assert_eq!(h.next(), None);
        // A line run starts again from the newest.
        h.previous("");
        h.push("/specs");
        assert_eq!(h.previous(""), Some("/specs"));
    }

    #[test]
    fn history_keeps_the_last_fifty() {
        let mut h = History::default();
        for i in 0..60 {
            h.push(&format!("line {i}"));
        }
        assert_eq!(h.lines().len(), COMMAND_HISTORY);
        assert_eq!(h.lines()[0], "line 10");
        h.set_lines((0..70).map(|i| i.to_string()).collect());
        assert_eq!(h.lines().len(), COMMAND_HISTORY);
        assert_eq!(h.lines().last().map(String::as_str), Some("69"));
    }

    #[test]
    fn lines_mean_what_the_tuis_input_line_means() {
        let mut store = demo();
        let lead = store.find_agent("orch-lead").unwrap().id.clone();
        let other = store.find_agent("dev-app").unwrap().id.clone();
        store.selected = Some(other.clone());
        let to = |cmd: Command| match cmd {
            Command::Request(req, _) => match *req {
                Request::Message { to, body, .. } => (to, body),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        // Plain text: the selected agent; `@Name`: that one.
        assert_eq!(
            to(command_for(&store, None, None, "hi there")),
            (vec![other.clone()], "hi there".into())
        );
        assert_eq!(
            to(command_for(&store, None, None, "@orch-lead go")),
            (vec![lead.clone()], "go".into())
        );
        // A question the Inbox shows takes plain text, not `@Name`.
        let mut question = store.inbox[0].clone();
        question.kind = InboxKind::Question;
        match command_for(&store, Some(&question), None, "yes please") {
            Command::Request(req, _) => {
                assert!(
                    matches!(*req, Request::AnswerQuestion { item, ref answer } if item == question.id && answer == "yes please")
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            to(command_for(&store, Some(&question), None, "@orch-lead go")).0,
            vec![lead]
        );
        // `/commands` as the palette's.
        assert!(
            matches!(command_for(&store, None, None, "/ps"), Command::Request(req, _) if matches!(*req, Request::Ps))
        );
        assert!(matches!(
            command_for(&store, None, None, "/down"),
            Command::Request(..)
        ));
        assert!(matches!(command_for(&store, None, None, "/nope"), Command::Error(e) if e.contains("/help")));
        // Nobody selected: plain text has nowhere to go.
        store.selected = None;
        assert!(matches!(command_for(&store, None, None, "hi"), Command::Error(e) if e == "no such agent"));
    }

    #[test]
    fn previews_say_what_enter_will_do() {
        let mut store = demo();
        let lead = store.find_agent("orch-lead").unwrap().clone();
        store.selected = Some(lead.id.clone());
        assert_eq!(preview(&store, None, ""), None);
        assert_eq!(
            preview(&store, None, "hi").unwrap(),
            format!("to {}", lead.label())
        );
        assert_eq!(
            preview(&store, None, "@orch-l").unwrap(),
            format!("to {}", lead.label())
        );
        assert_eq!(preview(&store, None, "@zzz hi").unwrap(), "no agent called zzz");
        assert_eq!(
            preview(&store, None, "/grow dev").unwrap(),
            "/grow <pod> <member> [runtime] [ROLE] · add a seat to the rig"
        );
        assert_eq!(preview(&store, None, "/sp").unwrap(), "/spawn  /spec  /specs");
        assert_eq!(preview(&store, None, "/q"), Some("/queue  /quit".into()));
        assert_eq!(
            preview(&store, None, "/zz"),
            None,
            "an alias, maybe: not called wrong"
        );
        let mut question = store.inbox[0].clone();
        question.kind = InboxKind::Question;
        let asker = store.agents[&question.agent].label();
        assert_eq!(
            preview(&store, Some(&question), "yes").unwrap(),
            format!("answers {asker}'s question")
        );
        store.selected = None;
        assert_eq!(
            preview(&store, None, "hi").unwrap(),
            "select an agent, or start with @Name"
        );
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::{Event, InboxKind};
    use ainet_model::Msg;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::inbox::InboxItem;
    use crate::tests::demo_workspace;

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    fn line(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<CommandLine> {
        w.read_with(cx, |w, _| w.command_line().clone())
    }

    fn text(line: &Entity<CommandLine>, cx: &mut VisualTestContext) -> String {
        line.read_with(cx, |l, cx| l.input().read(cx).text().to_string())
    }

    /// The requests sent since the last call, as text (`Request` has no
    /// `PartialEq`).
    fn take_sent(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> Vec<String> {
        fleet.update(cx, |f, _| {
            std::mem::take(&mut f.sent)
                .into_iter()
                .map(|(r, ok)| format!("{r:?} {ok}"))
                .collect()
        })
    }

    /// What the TUI sends for `cmd`, as [`take_sent`] says it.
    fn tui(cmd: Command) -> Vec<String> {
        match cmd {
            Command::Request(req, ok) => vec![format!("{req:?} {ok}")],
            other => panic!("not a request: {other:?}"),
        }
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

    fn focused(handle: &FocusHandle, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, _| handle.is_focused(window))
    }

    fn line_focused(line: &Entity<CommandLine>, cx: &mut VisualTestContext) -> bool {
        let handle = line.read_with(cx, |l, cx| l.focus_handle(cx));
        focused(&handle, cx)
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

    #[gpui::test]
    async fn each_kind_of_line_sends_what_the_tui_sends(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        let lead = seat(&fleet, "orch-lead@docs", cx);
        let selected = seat(&fleet, "dev-app@docs", cx);
        fleet.update(cx, |f, cx| f.select(&selected, cx));
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("ctrl-;");
        assert!(line_focused(&line, cx));

        // Plain text: to the selected agent, as the TUI's line.
        cx.simulate_input("check the build");
        cx.simulate_keystrokes("enter");
        let expected = fleet.read_with(cx, |f, _| {
            tui(command::message(&f.store, "check the build", Some(&selected)))
        });
        assert_eq!(take_sent(&fleet, cx), expected);
        assert_eq!(text(&line, cx), "", "done, the line clears");
        assert!(line_focused(&line, cx), "and keeps the keyboard");
        cx.run_until_parked();
        let name = fleet.read_with(cx, |f, _| f.store.agents[&selected].label());
        assert!(
            toasts(&w, cx).contains(&format!("sent to {name}")),
            "{:?}",
            toasts(&w, cx)
        );

        // @Name: to that agent.
        cx.simulate_input("@orch-lead@docs ship it");
        cx.simulate_keystrokes("enter");
        let expected = fleet.read_with(cx, |f, _| {
            tui(command::message(&f.store, "@orch-lead@docs ship it", None))
        });
        assert_eq!(take_sent(&fleet, cx), expected);
        assert!(expected[0].contains(&lead));

        // A /command: the palette's and Comms' meaning.
        cx.simulate_input("/ps");
        cx.simulate_keystrokes("enter");
        let expected = fleet.read_with(cx, |f, _| tui(commands::parse(&f.store, "ps")));
        assert_eq!(take_sent(&fleet, cx), expected);

        // `/view inbox` is the app's: the Inbox opens and takes the keys.
        cx.simulate_input("/view inbox");
        cx.simulate_keystrokes("enter");
        let inbox = w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<InboxItem>().ok())
        });
        let inbox = inbox.expect("the Inbox is the active tab");
        assert!(focused(&inbox.read_with(cx, |i, cx| i.focus_handle(cx)), cx));
        assert_eq!(text(&line, cx), "");

        // The Inbox shows a question waiting: plain text answers it, as
        // the TUI's line in its Inbox; @Name still messages.
        let question = ainet_core::InboxItem {
            id: 3000,
            agent: lead.clone(),
            kind: InboxKind::Question,
            summary: "which platform?".into(),
            detail: "which platform?".into(),
            created: 0,
            waiting: true,
            options: vec!["linux".into(), "mac".into()],
            multi_select: false,
        };
        fleet.update(cx, |f, cx| {
            f.receive(
                vec![Msg::Event(Box::new(Event::Inbox {
                    item: question.clone(),
                }))],
                cx,
            )
        });
        inbox.update(cx, |i, cx| i.select(3000, cx));
        cx.run_until_parked();
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("ctrl-;");
        cx.simulate_input("both");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            take_sent(&fleet, cx),
            tui(command::answer_question(&question, "both".into()))
        );
        cx.simulate_input("@orch-lead@docs thanks");
        cx.simulate_keystrokes("enter");
        let sent = take_sent(&fleet, cx);
        assert!(sent.len() == 1 && sent[0].starts_with("Message"), "{sent:?}");
    }

    /// Gives the line the keyboard, wherever it is (a terminal's tile
    /// keeps `ctrl-;` for itself).
    fn focus_line(line: &Entity<CommandLine>, cx: &mut VisualTestContext) {
        line.update_in(cx, |l, window, cx| {
            if !l.is_focused(window, cx) {
                l.toggle_focus(window, cx);
            }
        });
    }

    fn run_line(line: &Entity<CommandLine>, typed: &str, cx: &mut VisualTestContext) {
        focus_line(line, cx);
        cx.simulate_input(typed);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn rig_commands_act_on_the_rig_the_view_shows(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        let selected = seat(&fleet, "dev-app@docs", cx);
        fleet.update(cx, |f, cx| f.select(&selected, cx));
        cx.run_until_parked();
        take_sent(&fleet, cx);

        // The grid shows infra; the selection is still in docs: infra, as
        // the TUI's filtered grid.
        w.update_in(cx, |w, window, cx| {
            w.show_rig_terminals(Some("infra".into()), window, cx)
        });
        cx.run_until_parked();
        run_line(&line, "/down", cx);
        let sent = take_sent(&fleet, cx);
        assert!(
            sent.len() == 1 && sent[0].starts_with("Down") && sent[0].contains("\"infra\""),
            "{sent:?}"
        );

        // `/cols 3` fixes the grid's columns, `/cols` fits them again.
        run_line(&line, "/cols 3", cx);
        let grid = w.update_in(cx, |w, window, cx| w.open_terminals(window, cx));
        assert_eq!(grid.read_with(cx, |g, _| g.fixed_cols()), Some(3));
        run_line(&line, "/cols", cx);
        assert_eq!(grid.read_with(cx, |g, _| g.fixed_cols()), None);

        // The Topology view on infra: the same.
        w.update_in(cx, |w, window, cx| {
            w.show_topology(Some("infra".into()), window, cx)
        });
        cx.run_until_parked();
        run_line(&line, "/snapshot", cx);
        let sent = take_sent(&fleet, cx);
        assert!(
            sent.len() == 1 && sent[0].contains("\"infra\"") && !sent[0].contains("\"docs\""),
            "{sent:?}"
        );

        // Elsewhere, the selected agent's rig.
        w.update_in(cx, |w, window, cx| w.open_map(window, cx));
        cx.run_until_parked();
        run_line(&line, "/snapshot", cx);
        let sent = take_sent(&fleet, cx);
        assert!(sent.len() == 1 && sent[0].contains("\"docs\""), "{sent:?}");
    }

    #[gpui::test]
    async fn answer_answers_the_question_the_inbox_selects(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        let lead = seat(&fleet, "orch-lead@docs", cx);
        let selected = seat(&fleet, "dev-app@docs", cx);
        fleet.update(cx, |f, cx| f.select(&selected, cx));
        // Two questions: the selected agent's, and the one the Inbox
        // selects.
        let question = |id: u64, agent: &str| ainet_core::InboxItem {
            id,
            agent: agent.to_string(),
            kind: InboxKind::Question,
            summary: "which platform?".into(),
            detail: "which platform?".into(),
            created: 0,
            waiting: true,
            options: vec!["linux".into(), "mac".into()],
            multi_select: false,
        };
        let (theirs, mine) = (question(3000, &lead), question(3001, &selected));
        for item in [theirs.clone(), mine] {
            fleet.update(cx, |f, cx| {
                f.receive(vec![Msg::Event(Box::new(Event::Inbox { item }))], cx)
            });
        }
        let inbox = w.update_in(cx, |w, window, cx| w.open_inbox(Some(3000), window, cx));
        inbox.update(cx, |i, cx| i.select(3000, cx));
        cx.run_until_parked();
        take_sent(&fleet, cx);
        run_line(&line, "/answer both", cx);
        assert_eq!(
            take_sent(&fleet, cx),
            tui(command::answer_question(&theirs, "both".into()))
        );
    }

    #[gpui::test]
    async fn errors_show_in_the_line_and_as_a_toast(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("ctrl-;");
        for (typed, error) in [
            ("/nope", "no command /nope; /help lists them"),
            ("@nobody hi", "no such agent"),
            ("/view nowhere", "no view or rig named nowhere"),
            ("/queue", "/queue <seat> <summary>"),
        ] {
            cx.simulate_input(typed);
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            assert_eq!(
                line.read_with(cx, |l, _| l.error().map(str::to_string)),
                Some(error.into())
            );
            assert_eq!(text(&line, cx), typed, "a mistake stays to be put right");
            assert!(toasts(&w, cx).iter().any(|t| t == error), "{:?}", toasts(&w, cx));
            // Editing it clears the error.
            cx.simulate_keystrokes("backspace");
            assert_eq!(line.read_with(cx, |l, _| l.error().map(str::to_string)), None);
            line.update(cx, |l, cx| l.input().update(cx, |i, cx| i.set_text("", cx)));
        }
        assert!(take_sent(&fleet, cx).is_empty());
        // Nothing to act on: said in the line only.
        fleet.update(cx, |f, cx| {
            f.store.selected = None;
            cx.notify()
        });
        cx.simulate_input("/close");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            line.read_with(cx, |l, _| l.error().map(str::to_string))
                .as_deref(),
            Some("nothing to act on")
        );
        assert!(take_sent(&fleet, cx).is_empty());
    }

    #[gpui::test]
    async fn tab_completes_and_up_and_down_recall(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        cx.simulate_keystrokes("ctrl-;");

        // @names as the TUI's, /command names from ainet-model's list.
        cx.simulate_input("@orch-l");
        cx.simulate_keystrokes("tab");
        let expected = fleet.read_with(cx, |f, _| command::complete(&f.store, "@orch-l").unwrap());
        assert_eq!(text(&line, cx), expected);
        line.update(cx, |l, cx| l.input().update(cx, |i, cx| i.set_text("", cx)));
        cx.simulate_input("/gr");
        cx.simulate_keystrokes("tab");
        assert_eq!(text(&line, cx), "/grow ");
        line.update(cx, |l, cx| l.input().update(cx, |i, cx| i.set_text("/snap", cx)));
        cx.simulate_keystrokes("tab");
        assert_eq!(text(&line, cx), "/snapshot", "as far as the names agree");
        cx.simulate_keystrokes("tab");
        assert_eq!(text(&line, cx), "/snapshot", "and no further");
        line.update(cx, |l, cx| l.input().update(cx, |i, cx| i.set_text("", cx)));

        // Up and down: the lines run, newest first, then what was typed.
        for typed in ["/ps", "/specs"] {
            cx.simulate_input(typed);
            cx.simulate_keystrokes("enter");
        }
        cx.simulate_input("half");
        cx.simulate_keystrokes("up");
        assert_eq!(text(&line, cx), "/specs");
        cx.simulate_keystrokes("up");
        assert_eq!(text(&line, cx), "/ps");
        cx.simulate_keystrokes("up");
        assert_eq!(text(&line, cx), "/ps", "the oldest");
        cx.simulate_keystrokes("down");
        assert_eq!(text(&line, cx), "/specs");
        cx.simulate_keystrokes("down");
        assert_eq!(text(&line, cx), "half");
        assert_eq!(line.read_with(cx, |l, _| l.history().to_vec()), ["/ps", "/specs"]);
        // The keyboard stays: up and down did not move the Fleet.
        assert!(line_focused(&line, cx));
    }

    #[gpui::test]
    async fn its_key_reaches_it_and_escape_gives_the_keyboard_back(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        let fleet_focus = w.read_with(cx, |w, cx| w.fleet_panel().read(cx).focus_handle(cx));
        assert!(focused(&fleet_focus, cx), "a new window's Fleet has the keys");
        cx.simulate_keystrokes("ctrl-;");
        assert!(line_focused(&line, cx));
        cx.simulate_keystrokes("escape");
        assert!(focused(&fleet_focus, cx), "back where it was");
        // The key again, in the line, goes back too.
        cx.simulate_keystrokes("ctrl-;");
        cx.simulate_keystrokes("ctrl-;");
        assert!(focused(&fleet_focus, cx));

        // From the Inbox, and back to it.
        let inbox = w.update_in(cx, |w, window, cx| w.open_inbox(None, window, cx));
        let inbox_focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        cx.simulate_keystrokes("ctrl-;");
        assert!(line_focused(&line, cx));
        cx.simulate_keystrokes("escape");
        assert!(focused(&inbox_focus, cx));

        // A click into the line remembers where the keyboard was.
        let bounds = cx.debug_bounds("COMMAND-LINE-INPUT").expect("the line is drawn");
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert!(line_focused(&line, cx));
        cx.simulate_keystrokes("escape");
        assert!(focused(&inbox_focus, cx));

        // A terminal keeps the key: it is the agent's.
        let agent = fleet.read_with(cx, |f, _| f.store.terminal_ids(None)[0].clone());
        w.update_in(cx, |w, window, cx| w.open_terminal(&agent, window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-;");
        assert!(!line_focused(&line, cx), "a terminal's keys are the agent's");

        // Its tab gone while the line has the keys: escape goes to the pane.
        w.update_in(cx, |w, window, cx| {
            w.command_line().update(cx, |l, cx| l.toggle_focus(window, cx))
        });
        assert!(line_focused(&line, cx));
        w.update_in(cx, |w, window, cx| {
            w.active_pane()
                .update(cx, |p, cx| p.close_active_item(window, cx))
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        let pane = w.read_with(cx, |w, cx| w.active_pane().focus_handle(cx));
        assert!(cx.update(|window, cx| pane.contains_focused(window, cx)));
    }

    #[gpui::test]
    async fn letters_typed_into_it_stay_in_it(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let (fleet, line) = (fleet(&w, cx), line(&w, cx));
        // Behind it, the Inbox (r, h, digits) and a selected agent in the
        // Fleet (j, k, m, s).
        let inbox = w.update_in(cx, |w, window, cx| w.open_inbox(None, window, cx));
        let selected = fleet.read_with(cx, |f, _| f.store.selected.clone());
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("ctrl-;");
        let typed = "jkmsrhc+-=12 ";
        cx.simulate_input(typed);
        assert_eq!(text(&line, cx), typed);
        assert!(line_focused(&line, cx));
        assert_eq!(fleet.read_with(cx, |f, _| f.store.selected.clone()), selected);
        assert!(
            !inbox.read_with(cx, |i, _| i.history_open()),
            "h did not show the history"
        );
        assert!(w.read_with(cx, |w, cx| !w.modal_layer().read(cx).has_active_modal()));
        assert!(take_sent(&fleet, cx).is_empty());
    }

    #[gpui::test]
    async fn off_in_the_settings_it_shows_only_with_the_keyboard(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let line = line(&w, cx);
        assert!(cx.debug_bounds("COMMAND-LINE-INPUT").is_some(), "on by default");
        cx.update(|_, cx| crate::settings::reload(r#"{ "command_line": false }"#, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("COMMAND-LINE-INPUT").is_none());
        cx.simulate_keystrokes("ctrl-;");
        assert!(line_focused(&line, cx));
        assert!(cx.debug_bounds("COMMAND-LINE-INPUT").is_some());
        cx.simulate_input("/ps");
        assert_eq!(text(&line, cx), "/ps");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("COMMAND-LINE-INPUT").is_none());
    }
}
