//! Every action the keymap can bind, in namespaces that mirror Zed's
//! (docs/GUI-PLAN.md, "Actions"). Some do something already; the rest are
//! declared so keymaps can name them, and come alive with their views in
//! later phases. Each maps to the same request the TUI sends, through
//! `ainet-model`'s command layer.

use gpui::actions;

actions!(
    ainet,
    [
        /// Quits the app (asking first if `confirm_quit` is set).
        Quit,
        /// Cycles the critters between small and large.
        ToggleCritterSize,
    ]
);

actions!(
    workspace,
    [
        /// Shows or hides the left dock.
        ToggleLeftDock,
        /// Shows or hides the right dock.
        ToggleRightDock,
        /// Shows or hides the bottom dock.
        ToggleBottomDock,
        /// Opens another window on the same fleet.
        NewWindow,
        /// Closes the window.
        CloseWindow,
        /// Opens settings.json in the system's editor for JSON.
        OpenSettings,
        /// Opens keymap.json in the system's editor for JSON.
        OpenKeymap,
        /// Opens the Comms stream as a tab.
        OpenComms,
        /// Opens the Inbox as a tab.
        OpenInbox,
        /// Opens the fleet map as a tab.
        OpenMap,
        /// Makes the active pane fill the window, or puts it back.
        ToggleZoom,
        /// Moves the keyboard to the next pane.
        ActivateNextPane,
        /// Moves the keyboard to the previous pane.
        ActivatePreviousPane,
        /// Moves the keyboard to the pane on the left.
        ActivatePaneLeft,
        /// Moves the keyboard to the pane on the right.
        ActivatePaneRight,
        /// Moves the keyboard to the pane above.
        ActivatePaneUp,
        /// Moves the keyboard to the pane below.
        ActivatePaneDown,
    ]
);

actions!(
    pane,
    [
        /// Splits the pane, the new one to the right.
        SplitRight,
        /// Splits the pane, the new one below.
        SplitDown,
        /// Closes the active tab.
        CloseActiveItem,
        /// Activates the next tab.
        ActivateNextItem,
        /// Activates the previous tab.
        ActivatePreviousItem,
        /// Closes every tab but the active one.
        CloseOtherItems,
        /// Closes every tab.
        CloseAllItems,
    ]
);

actions!(
    fleet,
    [
        /// Selects the next agent in the Fleet.
        SelectNext,
        /// Selects the previous agent in the Fleet.
        SelectPrev,
        /// Opens the selected agent's terminal as a tab.
        OpenTerminal,
        /// Starts a message to the selected agent.
        MessageAgent,
        /// Closes the selected agent (its process ends), after asking.
        CloseAgent,
        /// Shows the selected agent's Claude Code transcript.
        ShowTranscript,
        /// Starts a new session in the selected agent's seat.
        Relaunch,
        /// Nudges the selected agent to carry on.
        Continue,
        /// Turns the typing guard of the selected agent's seat on or off.
        ToggleTypingGuard,
        /// Shows the last lines of the selected agent's terminal.
        Capture,
        /// Lets the selected agent's seat go; its process runs on.
        ReleaseSeat,
        /// Takes the selected agent's seat out of its rig, after asking.
        RemoveSeat,
        /// Folds away the rig of the selected agent.
        CollapseRig,
        /// Unfolds the rig of the selected agent.
        ExpandRig,
        /// Opens the selected agent's menu, as a right click does.
        OpenContextMenu,
        /// Spawns an agent.
        Spawn,
        /// Brings a rig up from a spec.
        Up,
        /// Takes a rig down.
        Down,
        /// Restores a rig from a snapshot.
        Restore,
        /// Adds seats to a rig.
        Grow,
        /// Talks to the operator, as the TUI's `ctrl-o`.
        Operator,
        /// Selects the next agent waiting for you.
        NextWaiting,
        /// Lays the rig's terminals out side by side, as the TUI's grid.
        TileAll,
        /// Finds an agent by name, seat, role or rig.
        ToggleSwitcher,
    ]
);

actions!(
    inbox,
    [
        /// Allows the waiting request once.
        Allow,
        /// Allows the waiting request and others like it.
        Always,
        /// Denies the waiting request.
        Deny,
        /// Answers the waiting question.
        Answer,
    ]
);

actions!(
    comms,
    [
        /// Writes a new message or chat post.
        Compose,
        /// Replies to the selected message.
        Reply,
    ]
);

/// A terminal's actions, in a module of their own: `terminal::ToggleZoom`
/// and `workspace::ToggleZoom` would otherwise clash.
pub mod terminal {
    use gpui::actions;

    actions!(
        terminal,
        [
            /// Copies the selection.
            Copy,
            /// Pastes into the terminal.
            Paste,
            /// Clears the selection.
            Clear,
            /// Sends ctrl-c to the agent.
            SendCtrlC,
            /// Scrolls back through history.
            ScrollUp,
            /// Scrolls forward through history.
            ScrollDown,
            /// Makes the terminal fill the window, or not.
            ToggleZoom,
        ]
    );
}

actions!(
    command_palette,
    [
        /// Opens the command palette.
        Toggle,
    ]
);
