//! An agent's terminal as a tab: [`TerminalItem`], opened from the Fleet
//! (enter, a double click, the agent's menu) by `Workspace::open_terminal`,
//! one tab per agent per pane. The tab wears the agent's critter, name and
//! state; a strip above the screen shows its seat and a button to message
//! it. The screen is ainet's copy of the agent's terminal, painted by
//! [`TerminalElement`] and kept fresh by the fleet while the tab shows.
//!
//! Typing goes to the agent while the terminal has the keyboard: every key
//! the keymap leaves alone (the `Terminal` context frees the ctrl
//! letters) is encoded as xterm would and sent as `Input`, and ainetd holds
//! messages for the agent meanwhile (`Interact`), which replaces the TUI's
//! typing mode and its `ctrl-]`. The mouse selects text to copy, the wheel
//! scrolls back through the agent's scrollback, and a right click opens
//! the terminal's menu.

use std::time::{Duration, Instant};

use ainet_core::AgentState;
use ainet_model::terminal::{Key, Mode, Mods, encode_in, paste};
use gpui::{
    Action, Anchor, ClipboardItem, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollWheelEvent, Subscription, Task,
    anchored, deferred,
};
use ui::{ContextMenu, Indicator, Tooltip, prelude::*};

use crate::actions::{MessageAgent, terminal};
use crate::critter::{Animation, Critter};
use crate::fleet::{FleetModel, ScreenChanged, Tick};
use crate::item::{Item, ItemEvent, TabContentParams};
use crate::panels::state_color;
use crate::persistence::SavedItem;
use crate::settings::{self, Blinking};
use crate::terminal_element::{Geometry, ShapedRow, TerminalElement};
use crate::terminal_input::keystroke_bytes;
use crate::terminal_layout::{self, RowCache, Selection};

/// How long the layout must hold still before the agent's pane is asked
/// to take its size: dragging a divider asks once, at the end.
pub const RESIZE_AFTER: Duration = Duration::from_millis(150);

/// The cursor's blink, as Zed's.
const BLINK_EVERY: Duration = Duration::from_millis(500);

/// Rows of scrollback asked for at least, and how many more each time the
/// wheel goes past what came.
const HISTORY_CHUNK: usize = 200;

/// How long scrollback that came shorter than a screen (nothing to scroll
/// to) is believed: the wheel asks again only after this, not on every
/// turn of a trackpad gesture.
const SHORT_HISTORY_FOR: Duration = Duration::from_secs(2);

/// Scrollback shown in place of the screen, and what it was laid out for.
struct HistoryView {
    /// (scroll, rows, cols, lines available, with colours).
    key: (usize, u16, u16, usize, bool),
    parser: vt100::Parser,
}

struct DeployedMenu {
    menu: Entity<ContextMenu>,
    position: Point<Pixels>,
    _dismissed: Subscription,
}

pub struct TerminalItem {
    fleet: Entity<FleetModel>,
    agent: String,
    focus_handle: FocusHandle,
    animation: Animation,
    /// Where the grid was last painted, for the mouse.
    geometry: Option<Geometry>,
    /// The size (rows, cols) last laid out, and the request for it while
    /// the layout settles.
    laid_out: Option<(u16, u16)>,
    resize: Option<Task<()>>,
    /// Rows scrolled back into the scrollback; 0 is the live screen.
    scroll: usize,
    /// Wheel movement not yet a whole row.
    scroll_rest: Pixels,
    /// Rows of scrollback last asked for.
    history_asked: usize,
    history: Option<HistoryView>,
    /// When the scrollback last came with nothing to scroll back to.
    short_history: Option<Instant>,
    selection: Option<Selection>,
    /// Where a drag with the left button started.
    drag_from: Option<(u16, u16)>,
    /// The cursor is in the "on" half of its blink.
    blink_on: bool,
    blink: Option<Task<()>>,
    context_menu: Option<DeployedMenu>,
    /// Rows shaped on the frames before, for the element to reuse.
    row_cache: RowCache<ShapedRow>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalItem {
    /// The terminal of agent `agent` (its id).
    pub fn new(
        fleet: Entity<FleetModel>,
        agent: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let id = cx.entity_id();
        let subscriptions = vec![
            // Name, state and critter show in the tab; screens land in
            // the store.
            cx.observe(&fleet, |_, _, cx| {
                cx.emit(ItemEvent::UpdateTab);
                cx.notify();
            }),
            // Its screen moved on: only this terminal redraws.
            cx.subscribe(&fleet, |this: &mut Self, _, changed: &ScreenChanged, cx| {
                if changed.0 == this.agent {
                    cx.notify();
                }
            }),
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.emit(ItemEvent::UpdateTab);
                    cx.notify();
                }
            }),
            cx.on_focus_in(&focus_handle, window, |this, _, cx| this.set_focused(true, cx)),
            cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
                this.set_focused(false, cx)
            }),
            // Gone without being closed (its window closed): the fleet
            // forgets it.
            cx.on_release(move |this, cx| {
                let agent = this.agent.clone();
                this.fleet.update(cx, |fleet, cx| {
                    fleet.hide_terminal(id);
                    fleet.set_at_terminal(id, &agent, false, cx);
                });
            }),
        ];
        TerminalItem {
            fleet,
            agent,
            focus_handle,
            animation: Animation::default(),
            geometry: None,
            laid_out: None,
            resize: None,
            scroll: 0,
            scroll_rest: Pixels::ZERO,
            history_asked: 0,
            history: None,
            short_history: None,
            selection: None,
            drag_from: None,
            blink_on: true,
            blink: None,
            context_menu: None,
            row_cache: RowCache::default(),
            _subscriptions: subscriptions,
        }
    }

    /// The agent whose terminal this is.
    pub fn agent_id(&self) -> &str {
        &self.agent
    }

    /// The screen to paint: the scrollback while scrolled back (once it
    /// came), else ainet's copy of the agent's screen.
    pub fn screen<'a>(&'a self, cx: &'a App) -> Option<&'a vt100::Screen> {
        match &self.history {
            Some(view) if self.scroll > 0 => Some(view.parser.screen()),
            _ => self
                .fleet
                .read(cx)
                .store
                .terms
                .get(&self.agent)
                .map(|p| p.screen()),
        }
    }

    /// The rows shaped so far, lent to the element while it lays the
    /// screen out ([`TerminalItem::put_row_cache`] gives them back).
    pub fn take_row_cache(&mut self) -> RowCache<ShapedRow> {
        std::mem::take(&mut self.row_cache)
    }

    pub fn put_row_cache(&mut self, cache: RowCache<ShapedRow>) {
        self.row_cache = cache;
    }

    /// Rows found in the cache and rows shaped, so far.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn row_cache_counts(&self) -> (usize, usize) {
        (self.row_cache.hits, self.row_cache.misses)
    }

    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Rows scrolled back; 0 shows the live screen.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn scrolled(&self) -> usize {
        self.scroll
    }

    /// The element was laid out at `geometry`. A new size is asked of the
    /// agent's pane once it has held for [`RESIZE_AFTER`]; scrollback is
    /// laid out again to fit.
    pub fn laid_out(&mut self, geometry: Geometry, cx: &mut Context<Self>) {
        self.geometry = Some(geometry);
        let size = (geometry.rows, geometry.cols);
        if self.laid_out != Some(size) {
            self.laid_out = Some(size);
            let agent = self.agent.clone();
            self.resize = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RESIZE_AFTER).await;
                let _ = this.update(cx, |this, cx| {
                    this.resize = None;
                    this.fleet
                        .update(cx, |fleet, cx| fleet.resize_terminal(&agent, size, cx));
                });
            }));
        }
        self.update_history(cx);
    }

    /// Lays the scrollback out for the current scroll and size, keeping
    /// the scroll within what there is.
    fn update_history(&mut self, cx: &mut Context<Self>) {
        let (Some(g), true) = (self.geometry, self.scroll > 0) else {
            self.history = None;
            return;
        };
        let fleet = self.fleet.clone();
        let store = &fleet.read(cx).store;
        let Some(lines) = store.history.get(&self.agent) else {
            return;
        };
        let styled = store.styled_history.get(&self.agent).map(Vec::as_slice);
        let max = terminal_layout::max_scroll(lines.len(), g.rows);
        if max == 0 {
            self.short_history = Some(cx.background_executor().now());
        }
        self.scroll = self.scroll.min(max);
        let key = (self.scroll, g.rows, g.cols, lines.len(), styled.is_some());
        if self.scroll == 0 {
            self.history = None;
        } else if self.history.as_ref().is_none_or(|h| h.key != key) {
            self.history = Some(HistoryView {
                key,
                parser: terminal_layout::history_screen(lines, styled, self.scroll, g.rows, g.cols),
            });
        }
    }

    fn rows(&self) -> u16 {
        self.geometry
            .map_or(ainet_model::terminal::DEFAULT_SIZE.0, |g| g.rows)
    }

    fn live_screen<'a>(&self, cx: &'a App) -> Option<&'a vt100::Screen> {
        self.fleet
            .read(cx)
            .store
            .terms
            .get(&self.agent)
            .map(|p| p.screen())
    }

    /// What the screen asked of the keyboard.
    fn mode(&self, cx: &App) -> Mode {
        Mode {
            app_cursor: self.live_screen(cx).is_some_and(|s| s.application_cursor()),
        }
    }

    /// The terminal got or lost the keyboard: ainetd holds the agent's
    /// messages while you are at it, the cursor shows and blinks, and the
    /// agent becomes the Fleet's selection, as Zed's project panel follows
    /// the active file.
    fn set_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        let (id, agent) = (cx.entity_id(), self.agent.clone());
        self.fleet.update(cx, |fleet, cx| {
            fleet.set_at_terminal(id, &agent, focused, cx);
            if focused {
                fleet.select(&agent, cx);
            }
        });
        if focused {
            self.restart_blink(cx);
        } else {
            self.blink = None;
        }
        cx.notify();
    }

    /// Shows the cursor and starts its blink over (typing pauses it).
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_on = true;
        if settings::get(cx).terminal.blinking != Blinking::On {
            self.blink = None;
            return;
        }
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_EVERY).await;
                let blinked = this.update(cx, |this, cx| {
                    this.blink_on = !this.blink_on;
                    cx.notify();
                });
                if blinked.is_err() {
                    return;
                }
            }
        }));
    }

    /// Sends `bytes` to the agent as typed, back at the live screen.
    fn send(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.scroll_to_bottom(cx);
        self.restart_blink(cx);
        let agent = self.agent.clone();
        self.fleet.update(cx, |fleet, cx| fleet.input(&agent, bytes, cx));
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed into the terminal's own menu are the menu's.
        if !self.focus_handle.is_focused(window) {
            return;
        }
        if let Some(bytes) = keystroke_bytes(&event.keystroke, self.mode(cx)) {
            self.send(&bytes, cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &terminal::Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = match (&self.selection, self.screen(cx)) {
            (Some(selection), Some(screen)) => selection.text(screen),
            _ => return,
        };
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Pastes the clipboard's text, bracketed if the screen asked.
    fn paste(&mut self, _: &terminal::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let bracketed = self.live_screen(cx).is_some_and(|s| s.bracketed_paste());
        self.send(&paste(&text, bracketed), cx);
    }

    fn clear_selection(&mut self, _: &terminal::Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = None;
        cx.notify();
    }

    fn send_ctrl_c(&mut self, _: &terminal::SendCtrlC, _: &mut Window, cx: &mut Context<Self>) {
        self.send(b"\x03", cx);
    }

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        if self.scroll > 0 {
            self.scroll = 0;
            self.history = None;
            cx.notify();
        }
    }

    /// Scrolls `lines` rows back (forward if negative). Full-screen
    /// programs (on the alternate screen) get arrow keys instead, as in
    /// xterm; otherwise the scrollback is fetched as far as needed.
    pub fn scroll_by(&mut self, lines: i32, cx: &mut Context<Self>) {
        if lines == 0 {
            return;
        }
        if self.scroll == 0 && self.live_screen(cx).is_some_and(|s| s.alternate_screen()) {
            let key = if lines > 0 { Key::Up } else { Key::Down };
            if let Some(bytes) = encode_in(key, Mods::default(), self.mode(cx)) {
                let bytes = bytes.repeat(lines.unsigned_abs() as usize);
                let agent = self.agent.clone();
                self.fleet.update(cx, |fleet, cx| fleet.input(&agent, &bytes, cx));
            }
            return;
        }
        let was = self.scroll;
        let now = cx.background_executor().now();
        if was == 0
            && lines > 0
            && self
                .short_history
                .is_some_and(|at| now.duration_since(at) < SHORT_HISTORY_FOR)
        {
            return;
        }
        self.scroll = self.scroll.saturating_add_signed(lines as isize);
        if self.scroll == 0 {
            self.scroll_to_bottom(cx);
            return;
        }
        // Leaving the live screen: the scrollback as it is now.
        let need = self.scroll + usize::from(self.rows());
        if was == 0 || need > self.history_asked {
            self.history_asked = need.next_multiple_of(HISTORY_CHUNK);
            let (agent, asked) = (self.agent.clone(), self.history_asked);
            self.fleet.update(cx, |fleet, _| {
                fleet.store.history.remove(&agent);
                fleet.fetch_history(&agent, asked);
            });
        }
        self.update_history(cx);
        cx.notify();
    }

    fn scroll_page(&mut self, up: bool, cx: &mut Context<Self>) {
        let page = i32::from(self.rows().saturating_sub(1).max(1));
        self.scroll_by(if up { page } else { -page }, cx);
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.geometry.map_or(px(20.), |g| g.cell.height);
        self.scroll_rest += event.delta.pixel_delta(line).y;
        let lines = (self.scroll_rest / line).trunc();
        self.scroll_rest -= line * lines;
        self.scroll_by(lines as i32, cx);
        cx.stop_propagation();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let Some(g) = self.geometry else { return };
        let cell = g.cell_at(event.position);
        match &mut self.selection {
            Some(selection) if event.modifiers.shift => {
                selection.head = cell;
                self.drag_from = Some(selection.anchor);
            }
            _ => {
                self.selection = None;
                self.drag_from = Some(cell);
            }
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(anchor), Some(g)) = (self.drag_from, self.geometry) else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag_from = None;
            return;
        }
        let head = g.cell_at(event.position);
        if self.selection.is_some() || head != anchor {
            self.selection = Some(Selection { anchor, head });
            cx.notify();
        }
    }

    /// The terminal's menu, at `position`.
    pub fn deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.focus_handle.clone();
        let selected = self.selection.is_some();
        let menu = ContextMenu::build(window, cx, move |menu, _, _| {
            menu.context(focus)
                .action_disabled_when(!selected, "Copy", terminal::Copy.boxed_clone())
                .action("Paste", terminal::Paste.boxed_clone())
                .action_disabled_when(!selected, "Clear Selection", terminal::Clear.boxed_clone())
                .separator()
                .action("Send Ctrl-C", terminal::SendCtrlC.boxed_clone())
                .action("Message…", MessageAgent.boxed_clone())
                .separator()
                .action("Zoom", terminal::ToggleZoom.boxed_clone())
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

    /// The strip above the screen: who, where, in what state, and a
    /// button to message the agent.
    fn render_header(&self, focused: bool, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let fleet = self.fleet.read(cx);
        let agent = fleet.store.agents.get(&self.agent)?;
        let seat = agent
            .seat
            .clone()
            .unwrap_or_else(|| agent.runtime.label().to_string());
        let id = agent.id.clone();
        let fleet_entity = self.fleet.clone();
        let focus = self.focus_handle.clone();
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
                .child(Label::new(agent.name.clone()).size(LabelSize::Small))
                .child(
                    Label::new(seat)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .buffer_font(cx),
                )
                .child(Indicator::dot().color(state_color(agent.state)))
                .child(Label::new(agent.state.label()).size(LabelSize::Small).color(
                    if agent.state == AgentState::Gone {
                        Color::Disabled
                    } else {
                        Color::Muted
                    },
                ))
                .child(div().flex_1())
                .when(self.scroll > 0, |this| {
                    this.child(
                        Label::new(format!("{} lines back", self.scroll))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                })
                .when(focused, |this| {
                    this.child(
                        Label::new("Typing · messages held")
                            .size(LabelSize::Small)
                            .color(Color::Accent),
                    )
                })
                .child(
                    IconButton::new("message-agent", IconName::Envelope)
                        .icon_size(IconSize::Small)
                        .tooltip(move |_, cx| {
                            Tooltip::for_action_in("Message Agent…", &MessageAgent, &focus, cx)
                        })
                        .on_click(move |_, window, cx| {
                            fleet_entity.update(cx, |fleet, cx| fleet.select(&id, cx));
                            window.dispatch_action(MessageAgent.boxed_clone(), cx);
                        }),
                ),
        )
    }

    /// While no screen has come: the critter and a word.
    fn render_waiting(&self, cx: &App) -> AnyElement {
        let fleet = self.fleet.read(cx);
        let gone = fleet
            .store
            .agents
            .get(&self.agent)
            .is_none_or(|a| a.state == AgentState::Gone || !a.terminal);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Label::new(if gone {
                "No terminal: this agent is gone or runs outside ainetd"
            } else {
                "Connecting to its terminal…"
            }))
            .into_any_element()
    }
}

impl EventEmitter<ItemEvent> for TerminalItem {}

impl Focusable for TerminalItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for TerminalItem {
    fn tab_content_text(&self, cx: &App) -> SharedString {
        let store = &self.fleet.read(cx).store;
        store
            .agents
            .get(&self.agent)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Terminal".into())
            .into()
    }

    /// The name, and a dot in the colour of the agent's state.
    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> AnyElement {
        let state = self.fleet.read(cx).store.agents.get(&self.agent).map(|a| a.state);
        h_flex()
            .gap_1p5()
            .child(Label::new(self.tab_content_text(cx)).color(params.text_color()))
            .children(state.map(|s| Indicator::dot().color(state_color(s))))
            .into_any_element()
    }

    fn tab_icon(&self, _: &Window, cx: &App) -> Option<AnyElement> {
        let fleet = self.fleet.read(cx);
        let agent = fleet.store.agents.get(&self.agent)?;
        Some(
            Critter::agent(agent, fleet.tick(), cx)
                .size(px(16.))
                .animate(&self.animation)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        let agent = self.fleet.read(cx).store.agents.get(&self.agent)?;
        Some(format!("{} · {}", agent.label(), agent.state.label()).into())
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        Some(SavedItem::Terminal {
            agent: self.agent.clone(),
        })
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Self>> {
        let (fleet, agent) = (self.fleet.clone(), self.agent.clone());
        Some(cx.new(|cx| TerminalItem::new(fleet, agent, window, cx)))
    }

    /// On show: the fleet fetches its screen as the agent prints.
    fn activated(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (id, agent) = (cx.entity_id(), self.agent.clone());
        self.fleet
            .update(cx, |fleet, cx| fleet.show_terminal(id, &agent, cx));
    }

    fn deactivated(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.fleet.update(cx, |fleet, _| fleet.hide_terminal(id));
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (id, agent) = (cx.entity_id(), self.agent.clone());
        self.fleet.update(cx, |fleet, cx| {
            fleet.hide_terminal(id);
            fleet.set_at_terminal(id, &agent, false, cx);
        });
    }
}

impl Render for TerminalItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        // The element is there before the first screen, so the pane is
        // asked to take the right size at once.
        let waiting = self
            .live_screen(cx)
            .is_none()
            .then(|| div().absolute().inset_0().child(self.render_waiting(cx)));
        let element = TerminalElement::new(cx.entity(), focused && self.blink_on && self.scroll == 0);
        v_flex()
            .id("terminal-item")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().terminal_background)
            .text_color(cx.theme().colors().terminal_foreground)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear_selection))
            .on_action(cx.listener(Self::send_ctrl_c))
            .on_action(cx.listener(|this, _: &terminal::ScrollUp, _, cx| this.scroll_page(true, cx)))
            .on_action(cx.listener(|this, _: &terminal::ScrollDown, _, cx| this.scroll_page(false, cx)))
            .children(self.render_header(focused, cx))
            .child(
                div()
                    .id("terminal-screen")
                    .debug_selector(|| "TERMINAL-SCREEN".into())
                    .flex_1()
                    .min_h_0()
                    .px_1()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.drag_from = None),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.drag_from = None),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle, cx);
                            this.deploy_context_menu(event.position, window, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                    .relative()
                    .child(element)
                    .children(waiting),
            )
            .children(self.context_menu.as_ref().map(|deployed| {
                deferred(
                    anchored()
                        .position(deployed.position)
                        .anchor(Anchor::TopLeft)
                        .child(deployed.menu.clone()),
                )
                .with_priority(3)
            }))
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::Request;
    use ainet_model::Msg;
    use gpui::{
        Modifiers, MouseUpEvent, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point, size,
    };

    use super::*;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    /// The demo fleet in a window, with the selected agent's terminal open
    /// (and focused) in the active pane.
    fn terminal(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, Entity<TerminalItem>, &mut VisualTestContext) {
        init_test(cx);
        let (w, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        // Focus events need an active window.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let id = fleet(&w, cx).read_with(cx, |f, _| f.store.selected.clone().unwrap());
        w.update_in(cx, |w, window, cx| w.open_terminal(&id, window, cx));
        cx.run_until_parked();
        let item = w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .unwrap()
                .to_any()
                .downcast::<TerminalItem>()
                .unwrap()
        });
        (w, item, cx)
    }

    fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
        w.read_with(cx, |w, _| w.fleet().clone())
    }

    /// The requests sent since the last call.
    fn take_sent(fleet: &Entity<FleetModel>, cx: &mut VisualTestContext) -> Vec<Request> {
        fleet.update(cx, |f, _| {
            std::mem::take(&mut f.sent).into_iter().map(|(r, _)| r).collect()
        })
    }

    /// Lets the keyboard settle, so ainetd hears where it is.
    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(crate::fleet::INTERACT_SETTLE);
        cx.run_until_parked();
    }

    fn interacts(sent: &[Request]) -> Vec<bool> {
        sent.iter()
            .filter_map(|r| match r {
                Request::Interact { on, .. } => Some(*on),
                _ => None,
            })
            .collect()
    }

    fn inputs(sent: &[Request]) -> Vec<String> {
        sent.iter()
            .filter_map(|r| match r {
                Request::Input { data, .. } => Some(data.clone()),
                _ => None,
            })
            .collect()
    }

    fn resizes(sent: &[Request]) -> Vec<(u16, u16)> {
        sent.iter()
            .filter_map(|r| match r {
                Request::Resize { rows, cols, .. } => Some((*rows, *cols)),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    async fn the_terminal_shows_the_agents_screen(cx: &mut TestAppContext) {
        let (_, item, cx) = terminal(cx);
        let screen = item.read_with(cx, |t, cx| t.screen(cx).map(|s| s.contents()));
        let text = screen.expect("the demo drew a screen");
        assert!(
            text.contains("Welcome to Claude Code") || text.contains("OpenAI Codex"),
            "{text}"
        );
        assert!(item.read_with(cx, |t, _| t.geometry.is_some()), "it was painted");
    }

    #[gpui::test]
    async fn focus_holds_the_agents_messages(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        let agent = item.read_with(cx, |t, _| t.agent_id().to_string());
        settle(cx);
        let sent = take_sent(&fleet, cx);
        assert_eq!(interacts(&sent), [true], "{sent:?}");
        assert!(
            sent.iter()
                .any(|r| matches!(r, Request::Interact { agent: a, .. } if *a == agent))
        );

        // The keyboard goes to the Fleet: messages flow again.
        cx.update(|window, cx| {
            let focus = w.read(cx).fleet_panel().focus_handle(cx);
            window.focus(&focus, cx);
        });
        settle(cx);
        assert_eq!(interacts(&take_sent(&fleet, cx)), [false]);

        // Back, then across to a second view of the same agent: ainetd is
        // told once.
        cx.update(|window, cx| window.focus(&item.focus_handle(cx), cx));
        settle(cx);
        assert_eq!(interacts(&take_sent(&fleet, cx)), [true]);
        cx.simulate_keystrokes("ctrl-alt-\\");
        settle(cx);
        assert_eq!(w.read_with(cx, |w, _| w.panes().len()), 2);
        assert_eq!(interacts(&take_sent(&fleet, cx)), Vec::<bool>::new());

        // Closing the tab lets go too.
        cx.dispatch_action(crate::actions::CloseAllItems);
        cx.update(|window, cx| {
            let focus = w.read(cx).fleet_panel().focus_handle(cx);
            window.focus(&focus, cx);
        });
        settle(cx);
        let left: Vec<bool> = interacts(&take_sent(&fleet, cx));
        assert_eq!(left.last(), Some(&false), "{left:?}");
    }

    #[gpui::test]
    async fn opening_a_terminal_from_the_inbox_gives_it_the_keys(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        let agent = item.read_with(cx, |t, _| t.agent_id().to_string());
        // The Inbox opens over the terminal, in the same pane.
        let inbox = w.update_in(cx, |w, window, cx| w.open_inbox(None, window, cx));
        settle(cx);
        let pane = w.read_with(cx, |w, _| w.active_pane().clone());
        assert!(pane.read_with(cx, |p, _| p.index_of(item.entity_id()).is_some()));
        take_sent(&fleet, cx);

        // Its "Answer in the agent's terminal" button: the agent's tab
        // comes back with the keyboard, and ainetd holds its messages.
        fleet.update(cx, |f, cx| f.select(&agent, cx));
        let focus = inbox.read_with(cx, |i, cx| i.focus_handle(cx));
        assert!(cx.update(|window, _| focus.is_focused(window)));
        cx.dispatch_action(crate::actions::OpenTerminal);
        settle(cx);
        assert!(cx.update(|window, cx| item.focus_handle(cx).is_focused(window)));
        assert_eq!(interacts(&take_sent(&fleet, cx)), [true]);
        cx.simulate_keystrokes("x");
        assert_eq!(inputs(&take_sent(&fleet, cx)), ["x"]);
    }

    #[gpui::test]
    async fn ctrl_o_talks_to_the_operator_or_starts_one(cx: &mut TestAppContext) {
        let (w, _, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        cx.update(|window, cx| {
            let focus = w.read(cx).fleet_panel().focus_handle(cx);
            window.focus(&focus, cx);
        });
        settle(cx);
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("ctrl-o");
        let sent = take_sent(&fleet, cx);
        assert!(
            matches!(&sent[..], [Request::Up(spec)] if spec.source == "operator"),
            "{sent:?}"
        );

        // Once it runs, ctrl-o opens its terminal to type into.
        let mut operator = fleet.read_with(cx, |f, _| f.store.selected_agent().unwrap().clone());
        operator.id = "operator-1".into();
        operator.rig = Some("operator".into());
        operator.seat = None;
        fleet.update(cx, |f, cx| {
            let event = ainet_core::Event::Agent {
                agent: operator.clone(),
            };
            f.receive(vec![Msg::Event(Box::new(event))], cx)
        });
        cx.simulate_keystrokes("ctrl-o");
        settle(cx);
        let open = w.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<TerminalItem>().ok())
        });
        let open = open.expect("a terminal tab");
        assert_eq!(open.read_with(cx, |t, _| t.agent_id().to_string()), "operator-1");
        assert!(cx.update(|window, cx| open.focus_handle(cx).is_focused(window)));
        let sent = take_sent(&fleet, cx);
        assert!(
            sent.iter()
                .any(|r| matches!(r, Request::Interact { agent, on: true } if agent == "operator-1")),
            "{sent:?}"
        );
    }

    #[gpui::test]
    async fn keys_go_to_the_agent(cx: &mut TestAppContext) {
        let (w, _, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        take_sent(&fleet, cx);
        cx.simulate_keystrokes("h i shift-a enter escape up ctrl-c ctrl-b ctrl-j alt-x");
        assert_eq!(
            inputs(&take_sent(&fleet, cx)),
            [
                "h", "i", "A", "\r", "\x1b", "\x1b[A", "\x03", "\x02", "\n", "\x1bx"
            ]
        );
        // The workspace's keys stepped aside: the docks stayed.
        let docks = w.read_with(cx, |w, cx| w.docks(cx));
        assert!(docks.fleet && docks.bottom);
    }

    #[gpui::test]
    async fn a_new_size_is_asked_once_it_holds(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        cx.executor().advance_clock(RESIZE_AFTER * 2);
        cx.run_until_parked();
        let first = resizes(&take_sent(&fleet, cx));
        let laid_out = item.read_with(cx, |t, _| t.laid_out.unwrap());
        assert_eq!(first, [laid_out], "the first layout asks for its size");

        // A window dragged through several sizes asks once, for the last.
        for width in [900., 860., 820., 780.] {
            cx.simulate_resize(size(px(width), px(700.)));
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            cx.executor().advance_clock(RESIZE_AFTER / 4);
            cx.run_until_parked();
        }
        assert!(resizes(&take_sent(&fleet, cx)).is_empty(), "still settling");
        cx.executor().advance_clock(RESIZE_AFTER);
        cx.run_until_parked();
        let last = item.read_with(cx, |t, _| t.laid_out.unwrap());
        assert_ne!(last, laid_out);
        assert_eq!(resizes(&take_sent(&fleet, cx)), [last]);
        let agent = item.read_with(cx, |t, _| t.agent_id().to_string());
        assert_eq!(fleet.read_with(cx, |f, _| f.terminal_size(&agent)), last);
        // The demo draws its screen at that size.
        cx.executor().advance_clock(crate::fleet::TICK);
        cx.run_until_parked();
        let drawn = item.read_with(cx, |t, cx| t.screen(cx).unwrap().size());
        assert_eq!(drawn, last);
    }

    #[gpui::test]
    async fn the_wheel_scrolls_back_through_history(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        take_sent(&fleet, cx);
        let screen = cx.debug_bounds("TERMINAL-SCREEN").expect("the screen is drawn");
        cx.simulate_event(ScrollWheelEvent {
            position: screen.center(),
            delta: ScrollDelta::Lines(point(0., 3.)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        let asked: Vec<usize> = take_sent(&fleet, cx)
            .iter()
            .filter_map(|r| match r {
                Request::Screen {
                    history,
                    formatted: false,
                    ..
                } => Some(*history),
                _ => None,
            })
            .collect();
        assert_eq!(asked.len(), 1);
        assert!(asked[0] >= 3);
        let (scroll, shown) = item.read_with(cx, |t, cx| (t.scrolled(), t.screen(cx).unwrap().contents()));
        assert_eq!(scroll, 3);
        assert!(shown.contains("step"), "the demo's scrollback shows: {shown}");

        // Scrolling on reuses what came; typing goes back to the screen.
        cx.simulate_event(ScrollWheelEvent {
            position: screen.center(),
            delta: ScrollDelta::Lines(point(0., 2.)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        assert_eq!(item.read_with(cx, |t, _| t.scrolled()), 5);
        assert!(take_sent(&fleet, cx).is_empty());
        cx.simulate_keystrokes("x");
        assert_eq!(item.read_with(cx, |t, _| t.scrolled()), 0);
        assert_eq!(inputs(&take_sent(&fleet, cx)), ["x"]);
    }

    #[gpui::test]
    async fn short_scrollback_is_not_asked_for_on_every_wheel(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        let agent = item.read_with(cx, |t, _| t.agent_id().to_string());
        let screen = cx.debug_bounds("TERMINAL-SCREEN").expect("the screen is drawn");
        let wheel = |cx: &mut VisualTestContext| {
            cx.simulate_event(ScrollWheelEvent {
                position: screen.center(),
                delta: ScrollDelta::Lines(point(0., 1.)),
                modifiers: Modifiers::none(),
                touch_phase: TouchPhase::Moved,
            });
            cx.run_until_parked();
        };
        let asked = |cx: &mut VisualTestContext| {
            take_sent(&fleet, cx)
                .iter()
                .filter(|r| matches!(r, Request::Screen { formatted: false, .. }))
                .count()
        };
        take_sent(&fleet, cx);
        wheel(cx);
        assert_eq!(asked(cx), 1);
        // The agent has less scrollback than a screen: back to the live
        // screen, and the next turns of the wheel ask nothing.
        fleet.update(cx, |f, cx| {
            f.receive(
                vec![Msg::History {
                    id: agent.clone(),
                    rows: vec!["only".into()],
                    styled: None,
                }],
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(item.read_with(cx, |t, _| t.scrolled()), 0);
        for _ in 0..5 {
            wheel(cx);
        }
        assert_eq!(asked(cx), 0);
        assert_eq!(item.read_with(cx, |t, _| t.scrolled()), 0);
        // A while later, it may have grown: asked again.
        cx.executor().advance_clock(SHORT_HISTORY_FOR);
        wheel(cx);
        assert_eq!(asked(cx), 1);
    }

    #[gpui::test]
    async fn selections_copy_and_pastes_type(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        let g = item.read_with(cx, |t, _| t.geometry.unwrap());
        // Drag across the second row's first eight cells.
        let at = |col: f32| {
            point(
                g.origin.x + g.cell.width * (col + 0.5),
                g.origin.y + g.cell.height * 1.5,
            )
        };
        cx.simulate_mouse_down(at(0.), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(at(4.), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(at(7.), MouseButton::Left, Modifiers::none());
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at(7.),
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        let selection = item.read_with(cx, |t, _| t.selection().copied());
        assert_eq!(
            selection,
            Some(Selection {
                anchor: (1, 0),
                head: (1, 7)
            })
        );
        let expected = item.read_with(cx, |t, cx| selection.unwrap().text(t.screen(cx).unwrap()));
        assert!(!expected.is_empty());
        cx.dispatch_action(terminal::Copy);
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(expected));
        cx.dispatch_action(terminal::Clear);
        assert_eq!(item.read_with(cx, |t, _| t.selection().copied()), None);

        // Pastes are bracketed only when the screen asks.
        take_sent(&fleet, cx);
        cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo".into()));
        cx.dispatch_action(terminal::Paste);
        let agent = item.read_with(cx, |t, _| t.agent_id().to_string());
        fleet.update(cx, |f, _| {
            let mut parser = vt100::Parser::new(24, 80, 0);
            parser.process(b"\x1b[?2004h");
            f.store.terms.insert(agent, parser);
        });
        cx.dispatch_action(terminal::Paste);
        cx.dispatch_action(terminal::SendCtrlC);
        assert_eq!(
            inputs(&take_sent(&fleet, cx)),
            ["one\rtwo", "\x1b[200~one\ntwo\x1b[201~", "\x03"]
        );
    }

    #[gpui::test]
    async fn tile_all_lays_the_rigs_terminals_out_as_panes(cx: &mut TestAppContext) {
        let (w, _, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        let ids = fleet.read_with(cx, |f, _| {
            let rig = f.store.selected_agent().and_then(|a| a.rig.clone());
            f.store.terminal_ids(rig.as_deref())
        });
        assert!(ids.len() > 1, "the demo's rig has several terminals");
        let shown = |cx: &mut VisualTestContext| {
            w.read_with(cx, |w, cx| {
                w.panes()
                    .iter()
                    .map(|p| {
                        p.read(cx)
                            .active_item()
                            .and_then(|i| i.to_any().downcast::<TerminalItem>().ok())
                            .map(|t| t.read(cx).agent_id().to_string())
                    })
                    .collect::<Vec<_>>()
            })
        };
        cx.dispatch_action(crate::actions::TileAll);
        let want: Vec<Option<String>> = ids.iter().cloned().map(Some).collect();
        assert_eq!(shown(cx), want);
        // Again: the same grid, not a second one.
        cx.dispatch_action(crate::actions::TileAll);
        assert_eq!(shown(cx), want);
    }

    /// A screen of `text` lines, as one comes from ainetd.
    fn screen_msg(agent: &str, text: &str) -> Msg {
        Msg::Term {
            id: agent.to_string(),
            ansi: text.replace('\n', "\r\n"),
            size: (24, 80),
            rows: text.lines().map(str::to_string).collect(),
        }
    }

    #[gpui::test]
    async fn only_terminals_whose_screen_changed_draw_again(cx: &mut TestAppContext) {
        let (w, _, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        cx.dispatch_action(crate::actions::TileAll);
        cx.run_until_parked();
        let items: Vec<Entity<TerminalItem>> = w.read_with(cx, |w, cx| {
            w.panes()
                .iter()
                .filter_map(|p| p.read(cx).active_item()?.to_any().downcast::<TerminalItem>().ok())
                .collect()
        });
        assert!(items.len() > 1);
        let counts = |cx: &mut VisualTestContext| {
            items
                .iter()
                .map(|t| t.read_with(cx, |t, _| t.row_cache_counts()))
                .collect::<Vec<_>>()
        };
        let a = items[0].read_with(cx, |t, _| t.agent_id().to_string());
        let notified = std::rc::Rc::new(std::cell::Cell::new(0));
        let _watch = cx.update(|_, cx| {
            let notified = notified.clone();
            cx.observe(&fleet, move |_, _| notified.set(notified.get() + 1))
        });

        // A new screen for one agent: its terminal draws, no other does,
        // and the fleet itself changed nothing to redraw for.
        let before = counts(cx);
        let text = "one\ntwo\nthree";
        fleet.update(cx, |f, cx| f.receive(vec![screen_msg(&a, text)], cx));
        cx.run_until_parked();
        let after = counts(cx);
        assert_ne!(after[0], before[0], "it drew");
        assert_eq!(after[1..], before[1..], "the others did not");
        assert_eq!(notified.get(), 0);

        // One row changed: only that row is shaped again.
        let (hits, misses) = after[0];
        fleet.update(cx, |f, cx| f.receive(vec![screen_msg(&a, "one\nTWO\nthree")], cx));
        cx.run_until_parked();
        let (more_hits, more_misses) = counts(cx)[0];
        assert_eq!(more_misses, misses + 1);
        assert!(more_hits > hits + 2, "the other rows came from the cache");

        // Another theme: every row is shaped anew, in its colours (the
        // blank ones once, as they look the same).
        cx.update(|_, cx| crate::settings::reload(r#"{ "theme": "One Light" }"#, cx));
        cx.run_until_parked();
        let (_, themed) = counts(cx)[0];
        assert_eq!(themed, more_misses + 4, "one, TWO, three and a blank row");
    }

    #[gpui::test]
    async fn scrollback_keeps_its_colours(cx: &mut TestAppContext) {
        let (w, item, cx) = terminal(cx);
        let fleet = fleet(&w, cx);
        let agent = item.read_with(cx, |t, _| t.agent_id().to_string());
        let rows: Vec<String> = (1..=60).map(|i| format!("line {i}")).collect();
        let styled: Vec<String> = (1..=60).map(|i| format!("\x1b[0;32mline\x1b[0m {i}")).collect();
        // Scrolled back, the scrollback is asked for (the demo's comes
        // plain); ainetd's comes with colours.
        item.update(cx, |t, cx| t.scroll_by(5, cx));
        cx.run_until_parked();
        fleet.update(cx, |f, cx| {
            f.receive(
                vec![Msg::History {
                    id: agent.clone(),
                    rows,
                    styled: Some(styled),
                }],
                cx,
            )
        });
        cx.run_until_parked();
        let green = item.read_with(cx, |t, cx| {
            assert!(t.scrolled() > 0);
            t.screen(cx).unwrap().cell(0, 0).unwrap().fgcolor()
        });
        assert_eq!(green, vt100::Color::Idx(2));
    }
}
