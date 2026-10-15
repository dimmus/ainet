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
        /// Closes the selected agent.
        CloseAgent,
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

actions!(
    command_palette,
    [
        /// Opens the command palette.
        Toggle,
    ]
);
