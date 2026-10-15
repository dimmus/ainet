//! The console's command language in the app: what `ainet-model`'s
//! `command::parse` makes of a `/command` line typed in Comms or the
//! command palette. Requests go to the fleet (long text opens as a
//! report); the console's own commands (views, answers, quitting) become
//! the app's actions, so a line means the same here as in the TUI.

use ainet_core::{Decision, InboxItem, InboxKind};
use ainet_model::{Command, Store, command};
use gpui::{Action, App, Entity, Window};

use crate::actions::panel::fleet_panel;
use crate::actions::{Allow, Always, Deny, OpenComms, OpenInbox, OpenMap, Quit, TileAll, ToggleCritterSize};
use crate::args::View;
use crate::fleet::FleetModel;
use crate::workspace::Workspace;

/// What a `/command` line (without its `/`) asks for. The app has no view
/// filtered to one rig, so rig commands act on the selected agent's rig
/// unless they name one; `/answer` answers the question waiting on you
/// (the selected agent's first), as the TUI's answers the one selected.
pub fn parse(store: &Store, line: &str) -> Command {
    command::parse(
        &command::Context {
            store,
            view_rig: None,
            inbox_item: question(store),
        },
        line,
    )
}

/// The oldest question waiting on you, the selected agent's first.
pub fn question(store: &Store) -> Option<&InboxItem> {
    let waiting = |i: &&InboxItem| i.waiting && i.kind == InboxKind::Question;
    let mut questions = store.inbox.iter().filter(waiting);
    let selected = store.selected.as_deref();
    questions
        .clone()
        .find(|i| Some(i.agent.as_str()) == selected)
        .or_else(|| questions.next())
}

/// Carries out a parsed command: a request (or what is wrong with the
/// line) through the fleet, the console's own commands as the app's
/// actions.
pub fn run(fleet: &Entity<FleetModel>, cmd: Command, window: &mut Window, cx: &mut App) {
    let action: Box<dyn Action> = match cmd {
        Command::Request(..) | Command::Error(_) | Command::Nothing => {
            fleet.update(cx, |fleet, cx| fleet.run(cmd, cx));
            return;
        }
        Command::Answer(Decision::Allow) => Box::new(Allow),
        Command::Answer(Decision::Always) => Box::new(Always),
        Command::Answer(Decision::Deny) => Box::new(Deny),
        Command::Quit => Box::new(Quit),
        Command::Critters => Box::new(ToggleCritterSize),
        Command::View(view) => match View::parse(&view) {
            Ok(View::Fleet) => Box::new(fleet_panel::ToggleFocus),
            Ok(View::Comms) => Box::new(OpenComms),
            Ok(View::Inbox) => Box::new(OpenInbox),
            Ok(View::Map) => Box::new(OpenMap),
            Ok(View::Terminals { rig: None }) => Box::new(TileAll),
            // A rig's terminals: that rig's, whichever agent is selected.
            Ok(View::Terminals { rig: Some(rig) }) => return tile_rig(&rig, window, cx),
            Ok(View::Named(name)) if fleet.read(cx).store.rigs.contains_key(&name) => {
                return tile_rig(&name, window, cx);
            }
            Ok(View::Named(name)) => {
                let text = format!("no view or rig named {name}");
                fleet.update(cx, |fleet, cx| fleet.show(text, true, cx));
                return;
            }
            Ok(View::Review) | Err(_) => return console_only(fleet, cx),
        },
        Command::Help | Command::Cols(_) => return console_only(fleet, cx),
    };
    window.dispatch_action(action, cx);
}

/// Tiles `rig`'s terminals in the window's workspace, once the view
/// that ran the command is done (as a dispatched action would be).
fn tile_rig(rig: &str, window: &mut Window, cx: &mut App) {
    let Some(workspace) = window.root::<Workspace>().flatten() else {
        return;
    };
    let rig = rig.to_string();
    window.defer(cx, move |window, cx| {
        workspace.update(cx, |w, cx| w.tile_rig(Some(&rig), window, cx));
    });
}

/// The console's own commands with nothing to do here.
fn console_only(fleet: &Entity<FleetModel>, cx: &mut App) {
    let text = "that is the console's command; here the keys and menus do it";
    fleet.update(cx, |fleet, cx| fleet.show(text.into(), false, cx));
}
