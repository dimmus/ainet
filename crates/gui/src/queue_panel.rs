//! The Queue panel, in the bottom dock: the fleet's owned work as a table,
//! the TUI's queue (in progress, blocked, pending, then done and canceled,
//! latest first; `Store::queue_items`) with Zed's `Table`: id, a state
//! chip in the TUI's colours, priority, who asked and who should do it,
//! the summary and how long since it moved. Open items only, or all.
//!
//! A right click (or `shift-f10`) on an item opens its menu: show it in
//! full (a report tab), claim, give back, move it to another state, add a
//! note, hand it off, block it. Each sends what the console's `ainet
//! queue` sends, through `ainet-model`'s `command::queue`.

use std::cell::Cell;
use std::rc::Rc;

use ainet_core::{QItem, QState};
use ainet_model::describe::age;
use ainet_model::{Palette, QueueCommand, Store, command};
use gpui::{
    Action, AnyElement, Bounds, ClickEvent, DismissEvent, Entity, FocusHandle, Focusable, Hsla, MouseButton,
    MouseDownEvent, Point, ScrollStrategy, Stateful, Subscription, WeakEntity, anchored, canvas, deferred,
    point, relative,
};
use ui::{Chip, ColumnWidthConfig, ContextMenu, Table, TableInteractionState, Tooltip, prelude::*};

use crate::actions::panel::queue_panel::ToggleFocus;
use crate::actions::queue::{
    AddNote, BlockOn, Claim, Handoff, MarkCanceled, MarkDone, MarkInProgress, MarkPending, OpenContextMenu,
    Show, ToggleOpenOnly, Unclaim,
};
use crate::critter::to_hsla;
use crate::dock::Panel;
use crate::fleet::FleetModel;
use crate::panels::{BOTTOM_DOCK_HEIGHT, bottom_header};
use crate::queue_form::{QueueForm, QueueFormModal};
use crate::themes;
use crate::workspace::Workspace;

/// The table's columns: id, state, priority, from → to, summary, age.
const COLS: usize = 6;

/// A state chip's colours, background and text: the TUI's own in ainet's
/// themes (Deepharness, Omarchy), else the theme's status colours.
pub fn state_colors(state: QState, palette: Option<&Palette>, cx: &App) -> (Hsla, Hsla) {
    if let Some(palette) = palette {
        let (bg, fg) = palette.qstate(state);
        return (to_hsla(bg), to_hsla(fg));
    }
    let (status, colors) = (cx.theme().status(), cx.theme().colors());
    match state {
        QState::InProgress => (status.success, colors.editor_background),
        QState::Blocked => (status.warning, colors.editor_background),
        QState::Pending => (status.info, colors.editor_background),
        QState::Done => (colors.element_background, colors.text_muted),
        QState::Canceled => (colors.element_background, colors.text_disabled),
    }
}

/// The ids of the items shown, in order.
pub fn rows(store: &Store, open_only: bool) -> Vec<String> {
    store
        .queue_items(open_only)
        .into_iter()
        .map(|q| q.id.clone())
        .collect()
}

/// The colour of a seat or agent named in the queue: its agent's, or the
/// accent for you.
fn who_color(who: &str, store: &Store, cx: &App) -> Color {
    if who == "operator" {
        return Color::Accent;
    }
    match store.find_agent(who) {
        Some(a) => Color::Custom(themes::agent_color(&a.id, cx).cursor),
        None => Color::Muted,
    }
}

struct DeployedMenu {
    menu: Entity<ContextMenu>,
    position: Point<Pixels>,
    _dismissed: Subscription,
}

pub struct QueuePanel {
    fleet: Entity<FleetModel>,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    table: Entity<TableInteractionState>,
    /// Leaves out the done and canceled.
    open_only: bool,
    /// What is shown, as of the last change.
    ids: Vec<String>,
    /// The selected item's id.
    selected: Option<String>,
    context_menu: Option<DeployedMenu>,
    /// Where the selected row was drawn, to open its menu from the keys.
    selected_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subscription: Subscription,
}

impl QueuePanel {
    pub fn new(fleet: Entity<FleetModel>, workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&fleet, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        });
        let mut panel = QueuePanel {
            fleet,
            workspace,
            focus_handle: cx.focus_handle(),
            table: cx.new(|cx| TableInteractionState::new(cx)),
            open_only: true,
            ids: Vec::new(),
            selected: None,
            context_menu: None,
            selected_bounds: Rc::default(),
            _subscription: subscription,
        };
        panel.sync(cx);
        panel
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn context_menu(&self) -> Option<&Entity<ContextMenu>> {
        self.context_menu.as_ref().map(|m| &m.menu)
    }

    /// The rows anew from the store; the selection stays on its item
    /// while that is shown, else the first.
    fn sync(&mut self, cx: &App) {
        self.ids = rows(&self.fleet.read(cx).store, self.open_only);
        if self.selected.as_ref().is_none_or(|s| !self.ids.contains(s)) {
            self.selected = self.ids.first().cloned();
        }
    }

    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.ids.iter().position(|i| i == id) {
            self.selected = Some(id.to_string());
            self.table
                .read(cx)
                .scroll_handle
                .scroll_to_item(ix, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.ids.get(ix).cloned() {
            self.select(&id, cx);
        }
    }

    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let at = self
            .selected
            .as_ref()
            .and_then(|s| self.ids.iter().position(|i| i == s));
        let next = match at {
            Some(at) => at
                .saturating_add_signed(delta)
                .min(self.ids.len().saturating_sub(1)),
            None => 0,
        };
        self.select_index(next, cx);
    }

    fn toggle_open_only(&mut self, cx: &mut Context<Self>) {
        self.open_only = !self.open_only;
        self.sync(cx);
        cx.notify();
    }

    fn selected_item<'a>(&self, store: &'a Store) -> Option<&'a QItem> {
        store.queue.get(self.selected.as_deref()?)
    }

    /// Does `cmd` to the selected item.
    fn queue_command(&mut self, cmd: QueueCommand, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else { return };
        self.fleet.update(cx, |fleet, cx| {
            let command = command::queue(&fleet.store, &id, cmd);
            fleet.run(command, cx);
        });
    }

    /// Opens one of the queue's forms for the selected item.
    fn open_form(&mut self, form: QueueForm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else { return };
        let fleet = self.fleet.clone();
        let _ = self.workspace.update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, move |window, cx| {
                QueueFormModal::new(fleet, form, id, window, cx)
            })
        });
    }

    /// The selected item's menu at `position`, as the Fleet panel's: it
    /// takes the keyboard, and gives it back to the panel when dismissed.
    pub fn deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = &self.fleet.read(cx).store;
        let Some(item) = self.selected_item(store) else {
            return;
        };
        let (state, open, owned) = (item.state, item.state.is_open(), item.owner.is_some());
        let focus = self.focus_handle.clone();
        let menu = ContextMenu::build(window, cx, move |menu, _, _| {
            menu.context(focus)
                .action("Show", Show.boxed_clone())
                .separator()
                .action_disabled_when(!open, "Claim", Claim.boxed_clone())
                .action_disabled_when(!open || !owned, "Unclaim", Unclaim.boxed_clone())
                .separator()
                .header("Update State")
                .action_disabled_when(state == QState::Pending, "Pending", MarkPending.boxed_clone())
                .action_disabled_when(
                    state == QState::InProgress,
                    "In Progress",
                    MarkInProgress.boxed_clone(),
                )
                .action_disabled_when(state == QState::Done, "Done", MarkDone.boxed_clone())
                .action_disabled_when(state == QState::Canceled, "Canceled", MarkCanceled.boxed_clone())
                .separator()
                .action("Add Note…", AddNote.boxed_clone())
                .action_disabled_when(!open, "Hand Off…", Handoff.boxed_clone())
                .action_disabled_when(!open, "Block On…", BlockOn.boxed_clone())
        });
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
            if menu.focus_handle(cx).contains_focused(window, cx) {
                window.focus(&this.focus_handle, cx);
            }
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some(DeployedMenu {
            menu,
            position,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    fn render_cells(&self, ix: usize, cx: &App) -> Vec<AnyElement> {
        let store = &self.fleet.read(cx).store;
        let Some(q) = self.ids.get(ix).and_then(|id| store.queue.get(id)) else {
            return (0..COLS).map(|_| gpui::Empty.into_any_element()).collect();
        };
        let (bg, fg) = state_colors(q.state, themes::palette(cx), cx);
        let small = |text: String, color: Color| Label::new(text).size(LabelSize::Small).color(color);
        vec![
            small(q.id.clone(), Color::Muted)
                .buffer_font(cx)
                .into_any_element(),
            Chip::new(q.state.label())
                .label_size(LabelSize::XSmall)
                .label_color(Color::Custom(fg))
                .bg_color(bg)
                .border_color(bg)
                .into_any_element(),
            small(q.priority.clone(), Color::Muted).into_any_element(),
            h_flex()
                .gap_1()
                .min_w_0()
                .overflow_hidden()
                .child(small(q.source.clone(), who_color(&q.source, store, cx)).truncate())
                .child(small("→".into(), Color::Disabled))
                .child(small(q.destination.clone(), who_color(&q.destination, store, cx)).truncate())
                .into_any_element(),
            small(
                q.summary.lines().next().unwrap_or_default().to_string(),
                if q.state.is_open() {
                    Color::Default
                } else {
                    Color::Muted
                },
            )
            .truncate()
            .into_any_element(),
            small(age(q.updated), Color::Muted).into_any_element(),
        ]
    }

    /// A row's frame: selection, the click that selects, the right click
    /// that opens the menu, and who owns it as a tooltip.
    fn render_row(
        &self,
        ix: usize,
        row: Stateful<Div>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(id) = self.ids.get(ix).cloned() else {
            return row.into_any_element();
        };
        let store = &self.fleet.read(cx).store;
        let owner = store.queue.get(&id).and_then(|q| q.owner.clone());
        let selected = self.selected.as_deref() == Some(id.as_str());
        let focused = self.focus_handle.contains_focused(window, cx);
        let colors = cx.theme().colors();
        let selector = format!("QUEUE-ROW-{id}");
        let menu_id = id.clone();
        row.debug_selector(move || selector.clone())
            .border_l_2()
            .border_color(if selected && focused {
                colors.border_focused
            } else {
                gpui::transparent_black()
            })
            .when(selected, |row| row.bg(colors.ghost_element_selected))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.select(&id, cx);
                if event.click_count() == 2 {
                    window.dispatch_action(Show.boxed_clone(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.select(&menu_id, cx);
                    this.deploy_context_menu(event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .when_some(owner, |row, owner| {
                row.tooltip(Tooltip::text(format!("owned by {owner}")))
            })
            .when(selected, |row| {
                let bounds = self.selected_bounds.clone();
                row.child(
                    canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
            })
            .into_any_element()
    }

    /// The filter: open items, or all of them, with their counts.
    fn render_filter(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = &self.fleet.read(cx).store;
        let (open, all) = (store.queue_items(true).len(), store.queue.len());
        let button = |id: &'static str, label: String, on: bool, cx: &mut Context<Self>| {
            Button::new(id, label)
                .label_size(LabelSize::Small)
                .toggle_state(on)
                .color(if on { Color::Default } else { Color::Muted })
                .when(!on, |b| {
                    b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_open_only(cx)))
                })
        };
        h_flex()
            .gap_0p5()
            .child(button("queue-open", format!("Open {open}"), self.open_only, cx))
            .child(button("queue-all", format!("All {all}"), !self.open_only, cx))
            .into_any_element()
    }
}

impl Focusable for QueuePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for QueuePanel {
    fn persistent_name() -> &'static str {
        "QueuePanel"
    }

    fn default_size(&self, _: &App) -> Pixels {
        BOTTOM_DOCK_HEIGHT
    }

    fn icon(&self) -> IconName {
        IconName::ListTodo
    }

    fn icon_tooltip(&self) -> &'static str {
        "Queue"
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn icon_label(&self, cx: &App) -> Option<usize> {
        let open = self.fleet.read(cx).store.queue_items(true).len();
        (open > 0).then_some(open)
    }
}

impl Render for QueuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        let count = self.ids.len();
        let filter = self.render_filter(cx);
        // Set again below if the selected row is drawn this frame.
        self.selected_bounds.set(None);
        let open_only = self.open_only;
        let table = Table::new(COLS)
            .interactable(&self.table)
            .hide_row_borders()
            .width_config(ColumnWidthConfig::explicit(vec![
                relative(0.07),
                relative(0.12),
                relative(0.09),
                relative(0.28),
                relative(0.36),
                relative(0.08),
            ]))
            .header(vec!["Id", "State", "Priority", "From → To", "Summary", "Moved"])
            .uniform_list(
                "queue-table",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|ix| this.render_cells(ix, cx)).collect()
                }),
            )
            .map_row(
                cx.processor(|this, (ix, row): (usize, Stateful<Div>), window, cx| {
                    this.render_row(ix, row, window, cx)
                }),
            )
            .empty_table_callback(move |_, _| {
                let text = if open_only {
                    "No open work. /queue <seat> <summary> creates some."
                } else {
                    "No owned work yet. /queue <seat> <summary> creates some."
                };
                div()
                    .p_3()
                    .child(Label::new(text).size(LabelSize::Small).color(Color::Muted))
                    .into_any_element()
            });

        v_flex()
            .id("queue-panel")
            .key_context("QueuePanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.select_relative(1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.select_relative(-1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectFirst, _, cx| this.select_index(0, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectLast, _, cx| {
                let last = this.ids.len().saturating_sub(1);
                this.select_index(last, cx)
            }))
            .on_action(
                cx.listener(|_, _: &menu::Confirm, window, cx| {
                    window.dispatch_action(Show.boxed_clone(), cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Show, _, cx| this.queue_command(QueueCommand::Show, cx)))
            .on_action(cx.listener(|this, _: &Claim, _, cx| this.queue_command(QueueCommand::Claim, cx)))
            .on_action(cx.listener(|this, _: &Unclaim, _, cx| {
                this.queue_command(QueueCommand::Unclaim { reason: None }, cx)
            }))
            .on_action(cx.listener(|this, _: &MarkPending, _, cx| {
                this.queue_command(QueueCommand::State(QState::Pending), cx)
            }))
            .on_action(cx.listener(|this, _: &MarkInProgress, _, cx| {
                this.queue_command(QueueCommand::State(QState::InProgress), cx)
            }))
            .on_action(cx.listener(|this, _: &MarkDone, _, cx| {
                this.queue_command(QueueCommand::State(QState::Done), cx)
            }))
            .on_action(cx.listener(|this, _: &MarkCanceled, _, cx| {
                this.queue_command(QueueCommand::State(QState::Canceled), cx)
            }))
            .on_action(
                cx.listener(|this, _: &AddNote, window, cx| this.open_form(QueueForm::Note, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Handoff, window, cx| this.open_form(QueueForm::Handoff, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &BlockOn, window, cx| this.open_form(QueueForm::Block, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleOpenOnly, _, cx| this.toggle_open_only(cx)))
            .on_action(cx.listener(|this, _: &OpenContextMenu, window, cx| {
                // Under the selected row, or at the mouse.
                let position = match this.selected_bounds.get() {
                    Some(b) => point(b.left() + px(24.), b.bottom()),
                    None => window.mouse_position(),
                };
                this.deploy_context_menu(position, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
            )
            .child(bottom_header(Self::persistent_name(), focused, filter, cx))
            .child(div().flex_1().min_h_0().child(table))
            .children(self.context_menu.as_ref().map(|deployed| {
                deferred(
                    anchored()
                        .position(deployed.position)
                        .anchor(gpui::Anchor::TopLeft)
                        .child(deployed.menu.clone()),
                )
                .with_priority(3)
            }))
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::Request;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::tests::init_test;

    #[gpui::test]
    fn rows_are_the_tuis_queue_open_or_all(cx: &mut TestAppContext) {
        let _ = cx;
        let mut store = Store::new();
        store.apply(ainet_model::Msg::Snapshot(Box::new(
            ainet_model::Demo::new().snapshot(),
        )));
        let tui: Vec<String> = store.queue_items(false).iter().map(|q| q.id.clone()).collect();
        assert_eq!(rows(&store, false), tui);
        // The demo: two in progress, one blocked, one pending.
        let states: Vec<QState> = rows(&store, true)
            .iter()
            .map(|id| store.queue[id].state)
            .collect();
        assert_eq!(
            states,
            [
                QState::InProgress,
                QState::InProgress,
                QState::Blocked,
                QState::Pending
            ]
        );
    }

    fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<QueuePanel>, &mut VisualTestContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        let panel = workspace
            .read_with(cx, |w, cx| w.bottom_panel::<QueuePanel>(cx))
            .expect("the Queue panel");
        (workspace, panel, cx)
    }

    fn take_sent(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<(Request, String)> {
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        fleet.update(cx, |f, _| std::mem::take(&mut f.sent))
    }

    #[gpui::test]
    async fn chips_are_coloured_as_the_tuis(cx: &mut TestAppContext) {
        let (_, panel, cx) = demo(cx);
        cx.update(|_, cx| {
            let fleet = panel.read(cx).fleet.read(cx);
            let palette = themes::palette(cx).cloned().unwrap_or_default();
            for id in panel.read(cx).ids() {
                let state = fleet.store.queue[id].state;
                let (bg, fg) = palette.qstate(state);
                assert_eq!(
                    state_colors(state, Some(&palette), cx),
                    (to_hsla(bg), to_hsla(fg)),
                    "{id}"
                );
            }
            // Ainet's own themes give the TUI's; the default theme here is
            // one of them.
            assert!(themes::palette(cx).is_some());
            // Done items are faded.
            let (_, done) = state_colors(QState::Done, Some(&palette), cx);
            assert_eq!(done, to_hsla(palette.fg_dim));
        });
    }

    #[gpui::test]
    async fn every_menu_entry_sends_what_ainet_queue_sends(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = demo(cx);
        cx.update(|window, cx| {
            let focus = panel.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
        });
        panel.update(cx, |p, cx| p.select("q-2", cx));
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        let expect = |cmd: QueueCommand, cx: &mut VisualTestContext| {
            fleet.read_with(cx, |f, _| match command::queue(&f.store, "q-2", cmd) {
                ainet_model::Command::Request(req, ok) => (format!("{req:?}"), ok),
                other => panic!("{other:?}"),
            })
        };
        take_sent(&workspace, cx);
        let actions: [(Box<dyn Action>, QueueCommand); 3] = [
            (Claim.boxed_clone(), QueueCommand::Claim),
            (Unclaim.boxed_clone(), QueueCommand::Unclaim { reason: None }),
            (MarkDone.boxed_clone(), QueueCommand::State(QState::Done)),
        ];
        for (action, cmd) in actions {
            let want = expect(cmd, cx);
            // Pick the item again: a done item leaves the open list.
            panel.update(cx, |p, cx| {
                p.open_only = false;
                p.sync(cx);
                p.select("q-2", cx);
            });
            cx.update(|window, cx| window.dispatch_action(action, cx));
            let sent: Vec<(String, String)> = take_sent(&workspace, cx)
                .into_iter()
                .map(|(r, ok)| (format!("{r:?}"), ok))
                .collect();
            assert_eq!(sent, [want]);
        }
        // The demo carried them out.
        cx.run_until_parked();
        fleet.read_with(cx, |f, _| assert_eq!(f.store.queue["q-2"].state, QState::Done));

        // The forms: a note, a handoff (two fields), a block.
        let modal_open = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |w, cx| w.modal_layer().read(cx).has_active_modal())
        };
        panel.update(cx, |p, cx| p.select("q-3", cx));
        cx.dispatch_action(AddNote);
        cx.run_until_parked();
        assert!(modal_open(cx));
        cx.simulate_keystrokes("enter");
        assert!(modal_open(cx), "an empty note stays to be written");
        assert!(take_sent(&workspace, cx).is_empty());
        cx.simulate_input("half way there");
        cx.simulate_keystrokes("enter");
        assert!(!modal_open(cx));
        let sent = take_sent(&workspace, cx);
        assert!(
            matches!(&sent[..], [(Request::QueueUpdate { id, note: Some(n), state: None, by: None }, ok)]
                if id == "q-3" && n == "half way there" && ok == "noted on q-3"),
            "{sent:?}"
        );

        cx.dispatch_action(Handoff);
        cx.run_until_parked();
        cx.simulate_input("rev-review@docs");
        cx.simulate_keystrokes("enter");
        assert!(modal_open(cx), "enter goes on to the second field");
        cx.simulate_input("check the guide");
        cx.simulate_keystrokes("enter");
        let sent = take_sent(&workspace, cx);
        assert!(
            matches!(&sent[..], [(Request::QueueHandoff { id, to, body, summary: None, by: None }, _)]
                if id == "q-3" && to == "rev-review@docs" && body == "check the guide"),
            "{sent:?}"
        );

        // The keyboard went back to the panel.
        cx.run_until_parked();
        assert!(cx.update(|window, cx| panel.read(cx).focus_handle.contains_focused(window, cx)));
        panel.update(cx, |p, cx| p.select("q-1", cx));
        cx.dispatch_action(BlockOn);
        cx.run_until_parked();
        cx.simulate_input("external:ci");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("rerun the tests");
        cx.simulate_keystrokes("enter");
        let sent = take_sent(&workspace, cx);
        assert!(
            matches!(&sent[..], [(Request::QueueBlock { id, on, continuation, .. }, ok)]
                if id == "q-1" && on == "external:ci" && continuation == "rerun the tests"
                    && ok == "q-1 is blocked on external:ci"),
            "{sent:?}"
        );

        // Escape from the second field gives up, and the keyboard comes
        // back to the panel.
        cx.run_until_parked();
        cx.dispatch_action(Handoff);
        cx.run_until_parked();
        cx.simulate_input("dev-ui@docs");
        cx.simulate_keystrokes("enter escape");
        cx.run_until_parked();
        assert!(!modal_open(cx));
        assert!(take_sent(&workspace, cx).is_empty());
        assert!(cx.update(|window, cx| panel.read(cx).focus_handle.contains_focused(window, cx)));
    }

    #[gpui::test]
    async fn a_right_click_selects_and_opens_the_menu(cx: &mut TestAppContext) {
        let (_, panel, cx) = demo(cx);
        cx.run_until_parked();
        let row = cx.debug_bounds("QUEUE-ROW-q-4").expect("q-4 is drawn");
        cx.simulate_mouse_down(row.center(), MouseButton::Right, gpui::Modifiers::none());
        panel.read_with(cx, |p, _| {
            assert_eq!(p.selected(), Some("q-4"));
            assert!(p.context_menu().is_some());
        });
        cx.simulate_keystrokes("escape");
        panel.read_with(cx, |p, _| assert!(p.context_menu().is_none()));

        // From the keys: down moves the selection, shift-f10 opens the
        // menu, enter in it shows the item (its first entry).
        let first = panel.read_with(cx, |p, _| p.ids()[0].clone());
        panel.update(cx, |p, cx| p.select(&first, cx));
        cx.simulate_keystrokes("down");
        let second = panel.read_with(cx, |p, _| p.ids()[1].clone());
        panel.read_with(cx, |p, _| assert_eq!(p.selected(), Some(second.as_str())));
        cx.simulate_keystrokes("shift-f10");
        panel.read_with(cx, |p, _| assert!(p.context_menu().is_some()));
        cx.simulate_keystrokes("escape");

        // The filter: all items include the finished ones.
        let all = panel.read_with(cx, |p, cx| p.fleet.read(cx).store.queue.len());
        cx.dispatch_action(ToggleOpenOnly);
        panel.read_with(cx, |p, _| assert_eq!(p.ids().len(), all));
    }
}
