//! The console's command language in the app: what `ainet-model`'s
//! `command::parse` makes of a `/command` line typed in Comms or the
//! command palette. Requests go to the fleet (long text opens as a
//! report); the console's own commands (views, answers, quitting) become
//! the app's actions, so a line means the same here as in the TUI.

use ainet_core::Decision;
use ainet_model::{Command, Store, command};
use gpui::{Action, App, Entity, Window};

use crate::actions::{Allow, Always, Deny, OpenComms, OpenInbox, OpenMap, Quit, TileAll, ToggleCritterSize};
use crate::args::View;
use crate::fleet::FleetModel;

/// What a `/command` line (without its `/`) asks for. The app has no view
/// filtered to one rig, so rig commands act on the selected agent's rig
/// unless they name one.
pub fn parse(store: &Store, line: &str) -> Command {
    command::parse(
        &command::Context {
            store,
            view_rig: None,
            inbox_item: None,
        },
        line,
    )
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
            Ok(View::Comms) => Box::new(OpenComms),
            Ok(View::Inbox) => Box::new(OpenInbox),
            Ok(View::Map) => Box::new(OpenMap),
            Ok(View::Terminals { .. }) => Box::new(TileAll),
            _ => return console_only(fleet, cx),
        },
        Command::Help | Command::Cols(_) => return console_only(fleet, cx),
    };
    window.dispatch_action(action, cx);
}

/// The console's own commands with nothing to do here.
fn console_only(fleet: &Entity<FleetModel>, cx: &mut App) {
    let text = "that is the console's command; here the keys and menus do it";
    fleet.update(cx, |fleet, cx| fleet.show(text.into(), false, cx));
}
