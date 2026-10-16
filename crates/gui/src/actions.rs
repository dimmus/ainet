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
        /// Switches to a rig: selects its first live agent, and the
        /// panels follow.
        SwitchRig,
        /// Talks to the operator, as the TUI's `ctrl-o`.
        Operator,
        /// Selects the next agent waiting for you.
        NextWaiting,
        /// Opens the Terminals grid: every live terminal at once, as the
        /// TUI's `^G`.
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
        /// Writes to the selected request's agent: on a waiting request,
        /// a reason to deny it with; on an answered one, a message.
        ReplyToAgent,
        /// Shows only the requests waiting on you, or every item.
        ToggleHistory,
    ]
);

actions!(
    comms,
    [
        /// Writes a new message or chat post.
        Compose,
        /// Replies to the selected message.
        Reply,
        /// Completes the `@name` in the compose box.
        Complete,
    ]
);

/// The command line's actions, in a module of their own:
/// `command_line::Complete` and `comms::Complete` would otherwise clash.
pub mod command_line {
    use gpui::actions;

    actions!(
        command_line,
        [
            /// Gives the command line at the bottom the keyboard, or gives
            /// the keyboard back to where it was.
            ToggleFocus,
            /// Completes the `@name` or `/command` being typed.
            Complete,
            /// Recalls the line typed before.
            PreviousLine,
            /// Recalls the line typed after, or what was being typed.
            NextLine,
        ]
    );
}

actions!(
    terminals,
    [
        /// One column more in the Terminals grid.
        MoreColumns,
        /// One column fewer in the Terminals grid.
        FewerColumns,
        /// Lets the Terminals grid pick its columns from its shape again.
        AutoColumns,
        /// Shows the next rig's terminals in the grid: all, each rig, all.
        CycleRig,
        /// Gives the Terminals grid itself the keyboard, from one of its
        /// tiles.
        FocusGrid,
    ]
);

actions!(
    map,
    [
        /// Zooms the map in, about its centre.
        ZoomIn,
        /// Zooms the map out, about its centre.
        ZoomOut,
        /// Fits the whole fleet in the map again.
        FitView,
    ]
);

/// The Queue panel's actions, in a module of their own:
/// `queue::OpenContextMenu` and `fleet::OpenContextMenu` would otherwise
/// clash.
pub mod queue {
    use gpui::actions;

    actions!(
        queue,
        [
            /// Claims the selected queue item: you own it, in progress.
            Claim,
            /// Gives the selected queue item back.
            Unclaim,
            /// Moves the selected queue item back to pending.
            MarkPending,
            /// Marks the selected queue item in progress.
            MarkInProgress,
            /// Marks the selected queue item done.
            MarkDone,
            /// Cancels the selected queue item.
            MarkCanceled,
            /// Adds a note to the selected queue item.
            AddNote,
            /// Hands the selected queue item off to another seat.
            Handoff,
            /// Blocks the selected queue item on another item.
            BlockOn,
            /// Shows the selected queue item in full.
            Show,
            /// Shows only the open queue items, or all of them.
            ToggleOpenOnly,
            /// Opens the selected queue item's menu, as a right click does.
            OpenContextMenu,
        ]
    );
}

/// The panels' `ToggleFocus` actions, a module each as in Zed
/// (`project_panel::ToggleFocus`): show the panel and give it the
/// keyboard, or give the keyboard back to the center if it has it.
pub mod panel {
    pub mod fleet_panel {
        gpui::actions!(
            fleet_panel,
            [
                /// Shows the Fleet panel and gives it the keyboard.
                ToggleFocus
            ]
        );
    }
    pub mod agent_panel {
        gpui::actions!(
            agent_panel,
            [
                /// Shows the Agent panel and gives it the keyboard.
                ToggleFocus
            ]
        );
    }
    pub mod queue_panel {
        gpui::actions!(
            queue_panel,
            [
                /// Shows the Queue panel and gives it the keyboard.
                ToggleFocus
            ]
        );
    }
    pub mod chatroom_panel {
        gpui::actions!(
            chatroom_panel,
            [
                /// Shows the Chatroom panel and gives it the keyboard.
                ToggleFocus
            ]
        );
    }
    pub mod events_panel {
        gpui::actions!(
            events_panel,
            [
                /// Shows the Events panel and gives it the keyboard.
                ToggleFocus
            ]
        );
    }
}

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

/// The command palette's action, in a module of its own as each
/// picker's `Toggle`.
pub mod command_palette {
    gpui::actions!(
        command_palette,
        [
            /// Opens the command palette.
            Toggle
        ]
    );
}

/// The theme selector's action.
pub mod theme_selector {
    gpui::actions!(
        theme_selector,
        [
            /// Opens the theme selector.
            Toggle
        ]
    );
}

/// The tab switcher's action.
pub mod tab_switcher {
    gpui::actions!(
        tab_switcher,
        [
            /// Switches to another tab of the pane, the latest used first.
            Toggle
        ]
    );
}

actions!(
    form,
    [
        /// Moves to the form's next field.
        NextField,
        /// Moves to the form's previous field.
        PreviousField,
    ]
);
