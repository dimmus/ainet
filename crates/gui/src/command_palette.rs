//! The command palette (`ctrl-shift-p`, as Zed's): every action you can
//! take where the keyboard is, by a readable name ("workspace: toggle left
//! dock") with its key binding, fuzzy filtered, the ones run from here
//! lately first. A line starting with `/`, or one that matches no action,
//! is the console's command language (`/up docs ~/app`, `/spawn codex`),
//! run as the TUI's command line runs it, with what is wrong with it said
//! in its row.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;

use ainet_model::Command;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{Action, DismissEvent, Entity, FocusHandle, Global, Task};
use ui::{HighlightedLabel, KeyBinding, ListItem, ListItemSpacing, prelude::*};

use crate::commands;
use crate::fleet::FleetModel;
use crate::picker::{Picker, PickerDelegate, match_strings_up_to};

/// Namespaces whose actions are not listed: the list keys (`menu::*`)
/// mean nothing outside a list.
const HIDDEN: [&str; 1] = ["menu"];

/// What was run from the palette, by name, and when (a counter): the
/// latest first next time. Kept with the workspace (`recent`).
#[derive(Default)]
struct History {
    next: u64,
    used: HashMap<SharedString, u64>,
}

impl Global for History {}

/// The actions run from the palette, the latest last, at most
/// `persistence::RECENT_ACTIONS`: kept with the workspace.
pub fn recent(cx: &App) -> Vec<String> {
    let Some(history) = cx.try_global::<History>() else {
        return Vec::new();
    };
    let mut used: Vec<(&SharedString, u64)> = history.used.iter().map(|(n, at)| (n, *at)).collect();
    used.sort_by_key(|(_, at)| *at);
    let skip = used.len().saturating_sub(crate::persistence::RECENT_ACTIONS);
    used.into_iter().skip(skip).map(|(n, _)| n.to_string()).collect()
}

/// Remembers `names` (a saved workspace's) as run lately, the last the
/// latest.
pub fn set_recent(names: &[String], cx: &mut App) {
    let mut history = History::default();
    for name in names {
        history.next += 1;
        history.used.insert(name.clone().into(), history.next);
    }
    cx.set_global(history);
}

struct PaletteCommand {
    name: SharedString,
    action: Box<dyn Action>,
}

pub struct CommandPalette {
    fleet: Entity<FleetModel>,
    /// Where the keyboard was: actions run there, and their bindings are
    /// the ones in force there.
    previous_focus: FocusHandle,
    /// Lately used first, then by name; candidate ids index this.
    commands: Vec<PaletteCommand>,
    /// How many of `commands` were used lately (they come first).
    used: usize,
    candidates: Arc<Vec<StringMatchCandidate>>,
    matches: Vec<StringMatch>,
    selected: usize,
    /// The query as a console command (without its `/`), listed after the
    /// matches: when it starts with `/`, or matches no action.
    line: Option<String>,
}

impl CommandPalette {
    /// The actions available where `previous_focus` is (the keyboard
    /// before the palette opened).
    pub fn new(fleet: Entity<FleetModel>, previous_focus: FocusHandle, window: &Window, cx: &App) -> Self {
        let history = cx.try_global::<History>();
        let last_used = |name: &SharedString| history.and_then(|h| h.used.get(name).copied());
        let mut commands: Vec<PaletteCommand> = window
            .available_actions(cx)
            .into_iter()
            .filter(|action| {
                let namespace = action.name().split("::").next().unwrap_or_default();
                !HIDDEN.contains(&namespace)
            })
            .map(|action| PaletteCommand {
                name: humanize(action.name()).into(),
                action,
            })
            .collect();
        commands.sort_by_key(|c| (Reverse(last_used(&c.name)), c.name.clone()));
        commands.dedup_by(|a, b| a.name == b.name);
        let used = commands.iter().filter(|c| last_used(&c.name).is_some()).count();
        let candidates = commands
            .iter()
            .enumerate()
            .map(|(ix, c)| StringMatchCandidate::new(ix, &c.name))
            .collect();
        CommandPalette {
            fleet,
            previous_focus,
            commands,
            used,
            candidates: Arc::new(candidates),
            matches: Vec::new(),
            selected: 0,
            line: None,
        }
    }

    /// The names listed, in order (tests).
    #[cfg(test)]
    pub fn listed(&self) -> Vec<SharedString> {
        self.matches
            .iter()
            .map(|m| self.commands[m.candidate_id].name.clone())
            .collect()
    }

    /// The action named `name`, if listed at all (tests).
    #[cfg(test)]
    pub fn action(&self, name: &str) -> Option<&dyn Action> {
        self.commands
            .iter()
            .find(|c| c.name.as_ref() == name)
            .map(|c| c.action.as_ref())
    }

    /// The console command the query is, if it is one (tests).
    #[cfg(test)]
    pub fn line(&self) -> Option<&str> {
        self.line.as_deref()
    }

    /// What the console command would do, or why it cannot.
    fn parse_line(&self, cx: &App) -> Option<Command> {
        let line = self.line.as_deref()?;
        Some(commands::parse(&self.fleet.read(cx).store, line))
    }

    fn run_line(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(cmd) = self.parse_line(cx) else {
            return;
        };
        // Wrong: it says why in its row, and stays to be put right.
        if matches!(cmd, Command::Error(_)) {
            return;
        }
        window.focus(&self.previous_focus, cx);
        cx.emit(DismissEvent);
        commands::run(&self.fleet, cmd, window, cx);
    }

    fn render_line(&self, ix: usize, selected: bool, cx: &App) -> Option<ListItem> {
        let line = self.line.as_deref()?;
        let (note, color) = match self.parse_line(cx)? {
            Command::Error(why) => (Some(why), Color::Error),
            Command::Nothing => (Some("nothing to act on".to_string()), Color::Muted),
            Command::Request(_, ok) if !ok.is_empty() => (Some(ok), Color::Muted),
            _ => (None, Color::Muted),
        };
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(Icon::new(IconName::Terminal).color(Color::Muted))
                .child(
                    v_flex()
                        .child(Label::new(format!("/{line}")).buffer_font(cx))
                        .children(note.map(|note| Label::new(note).size(LabelSize::Small).color(color))),
                )
                .end_slot(
                    Label::new("console command")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        )
    }
}

impl PickerDelegate for CommandPalette {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len() + usize::from(self.line.is_some())
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = ix;
    }

    fn placeholder_text(&self, _: &App) -> SharedString {
        "Execute a command, or /up, /spawn, /down…".into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let query = query.trim().to_string();
        if let Some(line) = query.strip_prefix('/') {
            self.matches.clear();
            self.line = Some(line.trim().to_string());
            self.selected = 0;
            return Task::ready(());
        }
        let found = match_strings_up_to(self.candidates.clone(), normalize(&query), usize::MAX, cx);
        cx.spawn(async move |picker, cx| {
            let mut matches = found.await;
            let _ = picker.update(cx, |picker, cx| {
                let palette = &mut picker.delegate;
                // Lately used first, in the order used; the rest by score.
                let used = palette.used;
                matches.sort_by_key(|m| {
                    if m.candidate_id < used {
                        m.candidate_id
                    } else {
                        usize::MAX
                    }
                });
                palette.line = (matches.is_empty() && !query.is_empty()).then(|| query.clone());
                palette.matches = matches;
                palette.selected = 0;
                cx.notify();
            });
        })
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(m) = self.matches.get(self.selected) else {
            return self.run_line(window, cx);
        };
        let command = &self.commands[m.candidate_id];
        let history = cx.default_global::<History>();
        history.next += 1;
        history.used.insert(command.name.clone(), history.next);
        let action = command.action.boxed_clone();
        // As Zed: back where the keyboard was, then run the action there.
        window.focus(&self.previous_focus, cx);
        cx.emit(DismissEvent);
        window.dispatch_action(action, cx);
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let Some(m) = self.matches.get(ix) else {
            return self.render_line(ix, selected, cx);
        };
        let command = self.commands.get(m.candidate_id)?;
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(
                    h_flex()
                        .w_full()
                        .py_px()
                        .justify_between()
                        .gap_2()
                        .child(HighlightedLabel::new(command.name.clone(), m.positions.clone()).truncate())
                        .child(h_flex().flex_shrink_0().child(KeyBinding::for_action_in(
                            command.action.as_ref(),
                            &self.previous_focus,
                            cx,
                        ))),
                ),
        )
    }

    fn render_footer(&self, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        let hint = if self.line.is_some() {
            "Runs as the console's command line"
        } else {
            "Start with / for a console command"
        };
        Some(
            h_flex()
                .w_full()
                .p_1p5()
                .gap_1()
                .justify_between()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(Label::new(hint).size(LabelSize::Small).color(Color::Muted).ml_1())
                .child(
                    Button::new("run", "Run")
                        .key_binding(KeyBinding::for_action(&menu::Confirm, cx).size(rems_from_px(12_f32)))
                        .on_click(|_, window, cx| window.dispatch_action(menu::Confirm.boxed_clone(), cx)),
                )
                .into_any_element(),
        )
    }
}

/// An action's name as words, as Zed's palette shows it:
/// `workspace::ToggleLeftDock` is "workspace: toggle left dock".
pub fn humanize(name: &str) -> String {
    let (namespace, action) = name.rsplit_once("::").unwrap_or(("", name));
    let mut words = String::new();
    let chars: Vec<char> = action.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1);
        // A new word: lower to upper ("ToggleLeft"), or the end of a run
        // of capitals ("CtrlC", "URLBar").
        let starts = c.is_uppercase()
            && prev.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
            });
        if starts {
            words.push(' ');
        }
        words.extend(c.to_lowercase());
    }
    if namespace.is_empty() {
        words
    } else {
        format!("{}: {words}", namespace.replace("::", " ").replace('_', " "))
    }
}

/// A query as the names are written: one space between words, `::` as
/// `:`, `_` as a space, so a keymap-style name finds its action too.
fn normalize(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    let mut last = None;
    for c in query.trim().chars() {
        let c = if c == '_' { ' ' } else { c };
        match (last, c) {
            (Some(':'), ':') => continue,
            (Some(l), c) if char::is_whitespace(l) && c.is_whitespace() => continue,
            _ => last = Some(c),
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    mod in_the_workspace {
        use ainet_core::Request;
        use gpui::{Entity, Focusable, TestAppContext, VisualTestContext};

        use super::*;
        use crate::tests::{demo_workspace, modal, sent};
        use crate::workspace::Workspace;

        type Palette = Picker<CommandPalette>;

        fn palette(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<Palette>> {
            modal::<Palette>(w, cx)
        }

        fn listed(p: &Entity<Palette>, cx: &mut VisualTestContext) -> Vec<String> {
            p.read_with(cx, |p, _| {
                p.delegate.listed().iter().map(|n| n.to_string()).collect()
            })
        }

        #[gpui::test]
        async fn lists_the_actions_with_their_keys_and_runs_the_one_picked(cx: &mut TestAppContext) {
            let (w, cx) = demo_workspace(cx);
            let fleet_focus = w.read_with(cx, |w, cx| w.fleet_panel().focus_handle(cx));
            cx.simulate_keystrokes("ctrl-shift-p");
            let p = palette(&w, cx).expect("the palette is open");
            let names = listed(&p, cx);
            for name in [
                "workspace: toggle left dock",
                "fleet: up",
                "fleet: spawn",
                "theme selector: toggle",
                "ainet: quit",
            ] {
                assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
            }
            assert!(!names.iter().any(|n| n.starts_with("menu:")), "{names:?}");
            // Its binding where the keyboard was: the Fleet panel's.
            let keys = p.read_with(cx, |p, _| {
                p.delegate
                    .action("workspace: toggle left dock")
                    .unwrap()
                    .boxed_clone()
            });
            let binding = cx.update(|window, _| {
                window
                    .highest_precedence_binding_for_action_in(keys.as_ref(), &fleet_focus)
                    .map(|b| b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>())
            });
            assert_eq!(binding, Some(vec!["ctrl-b".to_string()]));

            cx.simulate_input("toggle left dock");
            assert_eq!(listed(&p, cx)[0], "workspace: toggle left dock");
            cx.simulate_keystrokes("enter");
            assert!(palette(&w, cx).is_none());
            assert!(!w.read_with(cx, |w, cx| w.docks(cx).fleet), "the dock closed");
            // Lately used comes first next time.
            cx.simulate_keystrokes("ctrl-shift-p");
            let p = palette(&w, cx).unwrap();
            assert_eq!(listed(&p, cx)[0], "workspace: toggle left dock");
            cx.simulate_keystrokes("escape");
            assert!(palette(&w, cx).is_none());
        }

        #[gpui::test]
        async fn a_console_command_runs_or_says_what_is_wrong(cx: &mut TestAppContext) {
            let (w, cx) = demo_workspace(cx);
            cx.simulate_keystrokes("ctrl-shift-p");
            let p = palette(&w, cx).unwrap();
            cx.simulate_input("/up");
            p.read_with(cx, |p, _| {
                assert_eq!(p.delegate.line(), Some("up"));
                assert_eq!(p.delegate.match_count(), 1, "the line only");
            });
            let note = p.read_with(cx, |p, cx| p.delegate.parse_line(cx));
            assert!(
                matches!(&note, Some(Command::Error(e)) if e == "/up <spec file | library name | rig>"),
                "{note:?}"
            );
            let before = sent(&w, cx).len();
            cx.simulate_keystrokes("enter");
            assert!(palette(&w, cx).is_some(), "a usage error stays to be put right");
            assert_eq!(sent(&w, cx).len(), before);

            // No action matches: the words are a console command too.
            p.update_in(cx, |p, window, cx| p.set_query("zzqx", window, cx));
            cx.run_until_parked();
            p.read_with(cx, |p, cx| {
                assert_eq!(p.delegate.line(), Some("zzqx"));
                assert!(matches!(p.delegate.parse_line(cx), Some(Command::Error(e)) if e.contains("/help")));
            });

            // The selected agent's rig, as the TUI's /down.
            p.update_in(cx, |p, window, cx| p.set_query("/down", window, cx));
            cx.run_until_parked();
            cx.simulate_keystrokes("enter");
            assert!(palette(&w, cx).is_none());
            assert!(matches!(
                sent(&w, cx).last(),
                Some(Request::Down { rig, snapshot: true, .. }) if rig == "docs"
            ));

            // The console's own commands are the app's actions.
            cx.simulate_keystrokes("ctrl-shift-p");
            cx.simulate_input("/view inbox");
            cx.simulate_keystrokes("enter");
            let pane = w.read_with(cx, |w, _| w.active_pane().clone());
            let title = pane.read_with(cx, |p, cx| p.active_item().unwrap().tab_content_text(cx));
            assert_eq!(title.as_ref(), "Inbox");
        }

        fn run_line(line: &str, cx: &mut VisualTestContext) {
            cx.simulate_keystrokes("ctrl-shift-p");
            cx.simulate_input(line);
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
        }

        /// `/answer` answers the question waiting on you, as the TUI's
        /// answers the one selected.
        #[gpui::test]
        async fn answer_answers_the_waiting_question(cx: &mut TestAppContext) {
            use ainet_core::{Event, InboxItem, InboxKind};
            use ainet_model::Msg;

            let (w, cx) = demo_workspace(cx);
            let fleet = w.read_with(cx, |w, _| w.fleet().clone());
            run_line("/answer yes", cx);
            assert!(
                !matches!(sent(&w, cx).last(), Some(Request::AnswerQuestion { .. })),
                "no question waits yet"
            );
            let agent = fleet.read_with(cx, |f, _| f.store.selected.clone().unwrap());
            let item = InboxItem {
                id: 950,
                agent,
                kind: InboxKind::Question,
                summary: "Ship it?".into(),
                detail: "Ship it today?".into(),
                created: 0,
                waiting: true,
                options: vec![],
                multi_select: false,
            };
            fleet.update(cx, |f, cx| {
                f.receive(vec![Msg::Event(Box::new(Event::Inbox { item }))], cx)
            });
            run_line("/answer yes, today", cx);
            assert!(
                matches!(sent(&w, cx).last(),
                    Some(Request::AnswerQuestion { item: 950, answer }) if answer == "yes, today"),
                "{:?}",
                sent(&w, cx).last()
            );
        }

        /// `/view rig:<name>` and `/view <rig>` show that rig's terminals in
        /// the Terminals grid, whichever agent is selected; `/view
        /// terminals` shows every one's.
        #[gpui::test]
        async fn view_a_rig_shows_that_rigs_terminals(cx: &mut TestAppContext) {
            use crate::terminals::TerminalsItem;

            let (w, cx) = demo_workspace(cx);
            let fleet = w.read_with(cx, |w, _| w.fleet().clone());
            let tiled = |cx: &mut VisualTestContext| {
                w.read_with(cx, |w, cx| {
                    let grids: Vec<_> = w
                        .panes()
                        .iter()
                        .flat_map(|p| p.read(cx).items().map(|i| i.to_any()).collect::<Vec<_>>())
                        .filter_map(|i| i.downcast::<TerminalsItem>().ok())
                        .collect();
                    assert_eq!(grids.len(), 1, "one grid");
                    grids[0].read(cx).agents()
                })
            };
            let infra = fleet.read_with(cx, |f, _| {
                assert_ne!(f.store.selected_agent().unwrap().rig.as_deref(), Some("infra"));
                f.store.terminal_ids(Some("infra"))
            });
            assert!(!infra.is_empty());
            run_line("/view rig:infra", cx);
            assert_eq!(tiled(cx), infra);

            let docs = fleet.read_with(cx, |f, _| f.store.terminal_ids(Some("docs")));
            run_line("/view docs", cx);
            assert_eq!(tiled(cx), docs);
            run_line("/view terminals", cx);
            assert_eq!(tiled(cx), fleet.read_with(cx, |f, _| f.store.terminal_ids(None)));

            run_line("/view nowhere", cx);
            let said = fleet.read_with(cx, |f, _| f.notices().last().map(|n| n.text.clone()));
            assert_eq!(said.as_deref(), Some("no view or rig named nowhere"));
        }
    }

    #[test]
    fn names_read_as_words() {
        assert_eq!(
            humanize("workspace::ToggleLeftDock"),
            "workspace: toggle left dock"
        );
        assert_eq!(humanize("fleet_panel::ToggleFocus"), "fleet panel: toggle focus");
        assert_eq!(humanize("terminal::SendCtrlC"), "terminal: send ctrl c");
        assert_eq!(humanize("command_palette::Toggle"), "command palette: toggle");
        assert_eq!(humanize("map::ZoomIn"), "map: zoom in");
        assert_eq!(humanize("x::OpenURLBar"), "x: open url bar");
        assert_eq!(humanize("Quit"), "quit");
    }

    #[test]
    fn queries_are_written_as_the_names() {
        assert_eq!(
            normalize("  workspace::toggle_left   dock "),
            "workspace:toggle left dock"
        );
        assert_eq!(normalize("fleet: up"), "fleet: up");
    }
}
