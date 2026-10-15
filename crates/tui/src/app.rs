//! The TUI's state on top of the shared fleet [`Store`]: tabs, list
//! cursors, the input line, effects, and key and mouse handling.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use ainet_core::{Agent, Decision, InboxItem, InboxKind, Request};
pub use ainet_model::{Backend, Entry, Msg, Row};
use ainet_model::{Change, Command, Link, Pump, Store, command, terminal};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use tachyonfx::fx::{self, RepeatMode};
use tachyonfx::{EffectManager, Interpolation, Motion};

use crate::sprite::Size;
use crate::term;
use crate::theme::{Palette, c};

/// One animation tick.
pub const TICK: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Fleet,
    Terminals,
    Comms,
    Inbox,
    Map,
    Review,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Fleet,
        Tab::Terminals,
        Tab::Comms,
        Tab::Inbox,
        Tab::Map,
        Tab::Review,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Fleet => "Fleet",
            Tab::Terminals => "Terminals",
            Tab::Comms => "Comms",
            Tab::Inbox => "Inbox",
            Tab::Map => "Map",
            Tab::Review => "Review",
        }
    }
}

pub struct Flash {
    pub text: String,
    pub err: bool,
    pub at: Instant,
}

pub struct Popup {
    pub title: String,
    pub text: String,
    pub scroll: u16,
}

/// The Terminals view.
#[derive(Default)]
pub struct Grid {
    /// Fixed column count; `None` picks one from the window's shape.
    pub cols: Option<u16>,
    pub focus: Option<String>,
    /// The focused terminal fills the view.
    pub zoom: bool,
    /// Keys go to the focused terminal (leave with Ctrl+]).
    pub typing: bool,
    /// Only this rig's terminals.
    pub rig: Option<String>,
}

pub struct App {
    pub pal: Palette,
    pub tick: u64,
    pub tab: Tab,
    /// The fleet: agents, inbox, rigs, comms, terminals, the Fleet tree.
    pub store: Store,
    pub fleet_state: ListState,
    pub inbox_state: ListState,
    pub comms_state: ListState,
    pub input: String,
    /// Cursor position in `input`, in chars.
    pub cursor: usize,
    pub flash: Option<Flash>,
    pub help: bool,
    pub popup: Option<Popup>,
    /// Critters in the Fleet list, and at which size.
    pub critter_rows: Option<Size>,
    pub grid: Grid,
    pub fx: EffectManager<&'static str>,
    pub quitting: Option<Instant>,
    /// Smoothed context gauge, so it glides instead of jumping.
    pub gauge: f64,
    /// Where things were drawn last frame, for effects and mouse hits.
    pub areas: Areas,
    link: Link<mpsc::Sender<Msg>>,
    rx: mpsc::Receiver<Msg>,
    /// Which terminals to fetch, and the sizes we asked panes to take.
    pump: Pump,
}

#[derive(Default, Clone)]
pub struct Areas {
    pub body: Rect,
    pub details: Rect,
    pub tabs: Vec<(Rect, Tab)>,
    pub inbox_tab: Rect,
    /// Terminal tiles: outer rect, agent id, inner (rows, cols).
    pub tiles: Vec<(Rect, String, (u16, u16))>,
}

impl App {
    /// `view` picks the first view: `terminals`, `rig:<name>`, `comms`, …
    pub fn new(backend: Backend, view: Option<&str>) -> Self {
        let (tx, rx) = mpsc::channel();
        let (link, snapshot) = Link::new(backend, tx);
        let pal = Palette::load();
        let mut fx = EffectManager::default();
        // Opening: the screen coalesces out of noise.
        fx.add_effect(fx::coalesce((900, Interpolation::QuadOut)));
        let mut app = App {
            pal,
            tick: 0,
            tab: Tab::Fleet,
            store: Store::new(),
            fleet_state: ListState::default(),
            inbox_state: ListState::default(),
            comms_state: ListState::default(),
            input: String::new(),
            cursor: 0,
            flash: None,
            help: false,
            popup: None,
            critter_rows: None,
            grid: Grid::default(),
            fx,
            quitting: None,
            gauge: 0.0,
            areas: Areas::default(),
            link,
            rx,
            pump: Pump::new(),
        };
        if let Some(snap) = snapshot {
            app.store.connected = true;
            app.apply(Msg::Snapshot(Box::new(snap)));
        }
        if let Some(v) = view {
            app.open_view(v);
        }
        app
    }

    /// `terminals`, `rig:<name>`, `pod:<rig>/<pod>`, `comms`, `inbox`, `map`.
    pub fn open_view(&mut self, view: &str) {
        let (kind, arg) = view.split_once(':').unwrap_or((view, ""));
        match kind {
            "terminals" | "terminal" | "grid" => self.tab = Tab::Terminals,
            "rig" | "saved" => {
                self.tab = Tab::Terminals;
                self.grid.rig = Some(arg.to_string()).filter(|a| !a.is_empty());
            }
            "pod" => {
                self.tab = Tab::Terminals;
                self.grid.rig = arg.split('/').next().map(str::to_string);
            }
            "comms" | "chat" | "queue" => self.tab = Tab::Comms,
            "inbox" | "attention" | "feed" => self.tab = Tab::Inbox,
            "map" | "graph" => self.tab = Tab::Map,
            _ if self.store.rigs.contains_key(view) => {
                self.tab = Tab::Terminals;
                self.grid.rig = Some(view.to_string());
            }
            _ => {}
        }
        self.dirty_visible();
    }

    pub fn is_demo(&self) -> bool {
        self.link.is_demo()
    }

    /// True once the closing effect has played.
    pub fn done(&self) -> bool {
        self.quitting
            .is_some_and(|t| t.elapsed() > Duration::from_millis(380))
    }

    pub fn quit(&mut self) {
        if self.grid.typing {
            self.stop_typing();
        }
        if self.quitting.is_none() {
            self.quitting = Some(Instant::now());
            self.fx.add_effect(fx::dissolve((360, Interpolation::QuadIn)));
        }
    }

    /// Advances animations and the demo by one tick.
    pub fn on_tick(&mut self) {
        self.tick += 1;
        self.link.tick(self.tick);
        let target = self.selected_agent().and_then(|a| a.context).unwrap_or(0.0) as f64;
        self.gauge += (target - self.gauge) * 0.25;
        if self
            .flash
            .as_ref()
            .is_some_and(|f| f.at.elapsed() > Duration::from_secs(5))
        {
            self.flash = None;
        }
        if self.is_demo() {
            self.dirty_visible();
        }
    }

    /// Applies everything the background threads sent.
    pub fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.apply(msg);
        }
    }

    fn apply(&mut self, msg: Msg) {
        match self.store.apply(msg) {
            Change::None
            | Change::InboxResolved
            | Change::Comms
            | Change::Term(_)
            | Change::History(_)
            // The TUI never asks for a reply of its own.
            | Change::Reply { .. }
            | Change::Listing { .. } => {}
            Change::Snapshot => {
                self.select_waiting();
                self.after_fleet();
                self.dirty_visible();
            }
            Change::Fleet => self.after_fleet(),
            Change::InboxAdded { index, alert } => {
                if alert {
                    // Make the Inbox tab glow a few times.
                    let glow = fx::repeat(
                        fx::ping_pong(fx::hsl_shift(
                            Some([0.0, 20.0, 25.0]),
                            Some([0.0, 10.0, 15.0]),
                            (300, Interpolation::SineInOut),
                        )),
                        RepeatMode::Times(3),
                    );
                    self.fx
                        .add_unique_effect("inbox-tab", glow.with_area(self.areas.inbox_tab));
                    if let Some(text) = self.store.inbox_alert(index) {
                        self.flash(text, false);
                    }
                }
                if self.inbox_state.selected().is_none() {
                    self.select_waiting();
                }
            }
            Change::Output(agent) => {
                if self.shows_terminal(&agent) {
                    self.pump.mark(&agent);
                }
            }
            Change::Flash(text, err) => self.flash(text, err),
            Change::Popup(title, text) => {
                self.popup = Some(Popup {
                    title,
                    text,
                    scroll: 0,
                })
            }
            Change::Connected(up) => self.flash(
                if up {
                    "connected to ainetd".into()
                } else {
                    "ainetd is not answering".into()
                },
                !up,
            ),
        }
    }

    /// After the Fleet tree was rebuilt: the list cursor follows the
    /// selected agent, and the grid keeps a focus that still exists.
    fn after_fleet(&mut self) {
        self.fleet_state.select(self.store.selected_row());
        if self
            .grid
            .focus
            .as_ref()
            .is_none_or(|f| !self.store.agents.contains_key(f))
        {
            self.grid.focus = self.grid_ids().into_iter().next();
        }
    }

    pub fn flash(&mut self, text: String, err: bool) {
        self.flash = Some(Flash {
            text,
            err,
            at: Instant::now(),
        });
    }

    // ---- the Fleet list ------------------------------------------------

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.store.selected_agent()
    }

    fn move_selection(&mut self, delta: i32) {
        match self.tab {
            Tab::Inbox => step(&mut self.inbox_state, self.store.inbox.len(), delta),
            Tab::Comms => {
                let n = self.entries().len();
                step(&mut self.comms_state, n, delta);
            }
            Tab::Terminals => self.move_focus(delta, 0),
            _ => {
                if let Some(id) = self.store.move_selection(delta) {
                    self.fleet_state.select(self.store.selected_row());
                    // The details pane fades in for the new agent.
                    let fade = fx::fade_from_fg(c(self.pal.bg), (280, Interpolation::SineOut));
                    self.fx
                        .add_unique_effect("details", fade.with_area(self.areas.details));
                    self.pump.mark(&id);
                }
            }
        }
    }

    fn cycle_critters(&mut self) {
        self.critter_rows = match self.critter_rows {
            None => Some(Size::Small),
            Some(Size::Small) => Some(Size::Octant),
            Some(_) => None,
        };
        let label = match self.critter_rows {
            None => "critters off",
            Some(Size::Small) => "small critters",
            Some(_) => "octant critters (needs a Unicode 16 font)",
        };
        self.flash(label.into(), false);
    }

    /// Puts the Inbox cursor on the oldest request still waiting, else the newest item.
    fn select_waiting(&mut self) {
        self.inbox_state.select(self.store.waiting_index());
    }

    fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        if self.grid.typing {
            self.stop_typing();
        }
        if tab == Tab::Inbox && self.inbox_state.selected().is_none() {
            self.select_waiting();
        }
        if tab == Tab::Comms && self.comms_state.selected().is_none() {
            let n = self.entries().len();
            self.comms_state.select(n.checked_sub(1));
        }
        let forward = Tab::ALL.iter().position(|t| *t == tab) > Tab::ALL.iter().position(|t| *t == self.tab);
        self.tab = tab;
        let motion = if forward {
            Motion::LeftToRight
        } else {
            Motion::RightToLeft
        };
        let sweep = fx::sweep_in(motion, 14, 0, c(self.pal.bg), (380, Interpolation::QuadOut));
        self.fx
            .add_unique_effect("body", sweep.with_area(self.areas.body));
        self.dirty_visible();
    }

    fn cycle_tab(&mut self, back: bool) {
        let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let n = Tab::ALL.len();
        self.set_tab(Tab::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }]);
    }

    // ---- terminals -------------------------------------------------------

    /// Agents shown in the Terminals view, in rig and seat order.
    pub fn grid_ids(&self) -> Vec<String> {
        self.store.terminal_ids(self.grid.rig.as_deref())
    }

    fn shows_terminal(&self, id: &str) -> bool {
        match self.tab {
            Tab::Terminals => {
                self.areas.tiles.iter().any(|(_, t, _)| t == id) || self.grid.focus.as_deref() == Some(id)
            }
            Tab::Fleet => self.store.selected.as_deref() == Some(id),
            _ => false,
        }
    }

    fn dirty_visible(&mut self) {
        match self.tab {
            Tab::Terminals => {
                let ids = self.grid_ids();
                self.pump.mark_all(ids);
            }
            Tab::Fleet => {
                if let Some(s) = self.store.selected.clone() {
                    self.pump.mark(&s);
                }
            }
            _ => {}
        }
    }

    /// Fetches screens that changed, at most ~15 times a second each.
    pub fn pump(&mut self) {
        for (id, size) in self.pump.due(Instant::now()) {
            if let Some(agent) = self.store.agents.get(&id) {
                self.link.fetch(agent, self.tick, size);
            }
        }
    }

    /// After a frame: ask each pane to take the size of its tile.
    pub fn sync_sizes(&mut self) {
        if self.tab != Tab::Terminals {
            return;
        }
        let tiles = self.areas.tiles.clone();
        for (_, id, size) in tiles {
            if self.pump.resize(&id, size) {
                self.link.quiet(Request::Resize {
                    agent: id,
                    rows: size.0,
                    cols: size.1,
                });
            }
        }
    }

    fn move_focus(&mut self, dx: i32, dy: i32) {
        let ids = self.grid_ids();
        if ids.is_empty() {
            return;
        }
        let cols = self.areas.tiles.len().max(1);
        let cols = term::grid_cols(ids.len(), self.areas.body, self.grid.cols).min(cols);
        let cur = self
            .grid
            .focus
            .as_ref()
            .and_then(|f| ids.iter().position(|i| i == f))
            .unwrap_or(0) as i32;
        let next = (cur + dx + dy * cols as i32).clamp(0, ids.len() as i32 - 1) as usize;
        self.grid.focus = Some(ids[next].clone());
    }

    /// Ctrl+O: talk to the operator, OpenRig's kernel agent that picks and
    /// launches teams. Starts it (here) when it is not running.
    fn operator(&mut self) {
        match self.store.operator().map(|a| a.id.clone()) {
            Some(id) => {
                self.set_tab(Tab::Terminals);
                self.grid.focus = Some(id);
                self.start_typing();
            }
            None => self.run(command::operator_up()),
        }
    }

    fn start_typing(&mut self) {
        let Some(id) = self.grid.focus.clone() else { return };
        self.grid.zoom = true;
        self.grid.typing = true;
        self.request(
            Request::Interact {
                agent: id.clone(),
                on: true,
            },
            String::new(),
        );
        let name = self.store.agents.get(&id).map(|a| a.label()).unwrap_or_default();
        self.flash(format!("typing into {name} · Ctrl+] to leave"), false);
        self.pump.mark(&id);
    }

    fn stop_typing(&mut self) {
        self.grid.typing = false;
        if let Some(id) = self.grid.focus.clone() {
            self.request(Request::Interact { agent: id, on: false }, String::new());
        }
    }

    fn type_into(&mut self, bytes: Vec<u8>) {
        let Some(id) = self.grid.focus.clone() else { return };
        let data = String::from_utf8_lossy(&bytes).into_owned();
        self.link.quiet(Request::Input {
            agent: id.clone(),
            data,
        });
        self.pump.mark(&id);
    }

    pub fn on_paste(&mut self, text: String) {
        if self.grid.typing {
            self.type_into(terminal::paste(&text, true));
        } else {
            let at = self.byte_at(self.cursor);
            self.input.insert_str(at, &text.replace(['\r', '\n'], " "));
            self.cursor += text.chars().count();
        }
    }

    // ---- comms -------------------------------------------------------------

    /// Messages and chat, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.store.entries()
    }

    pub fn selected_entry(&self) -> Option<Entry> {
        self.comms_state
            .selected()
            .and_then(|i| self.entries().get(i).cloned())
    }

    // ---- talking to the backend ----------------------------------------

    /// Sends a request; the outcome comes back as a flash or a popup.
    fn request(&mut self, req: Request, ok: String) {
        self.link.request(req, ok);
    }

    /// Carries out a command from the shared console language.
    fn run(&mut self, cmd: Command) {
        match cmd {
            Command::Request(req, ok) => self.request(*req, ok),
            Command::Answer(decision) => self.answer(decision),
            Command::Help => self.help = true,
            Command::Quit => self.quit(),
            Command::Critters => self.cycle_critters(),
            Command::View(view) => self.open_view(&view),
            Command::Cols(cols) => self.grid.cols = cols,
            Command::Error(text) => self.flash(text, true),
            Command::Nothing => {}
        }
    }

    fn selected_item(&self) -> Option<InboxItem> {
        self.inbox_state
            .selected()
            .and_then(|i| self.store.inbox.get(i))
            .cloned()
    }

    /// ^Y / ^N / ^A: answers the request on screen. Elsewhere it first opens
    /// the Inbox on the oldest waiting request, so you read before you allow.
    fn answer(&mut self, decision: Decision) {
        if self.tab == Tab::Inbox {
            let Some(item) = self.selected_item() else { return };
            match self.store.answer(&item, decision) {
                Ok((req, ok)) => self.request(req, ok),
                // Answering what no longer waits is a mistake; a question
                // just wants its answer typed.
                Err(e) => self.flash(e, !item.waiting),
            }
            return;
        }
        match self.store.next_waiting() {
            Some(p) => {
                self.inbox_state.select(Some(p));
                self.set_tab(Tab::Inbox);
            }
            None => self.flash("nothing is waiting on you".into(), false),
        }
    }

    fn submit(&mut self) {
        let line = std::mem::take(&mut self.input);
        self.cursor = 0;
        let line = line.trim().to_string();
        if line.is_empty() {
            if self.tab == Tab::Terminals {
                self.start_typing();
            }
            return;
        }
        if let Some(cmd) = line.strip_prefix('/') {
            self.command(cmd);
            return;
        }
        // A waiting question in the Inbox takes the line as its answer.
        if self.tab == Tab::Inbox
            && let Some(item) = self
                .selected_item()
                .filter(|i| i.waiting && i.kind == InboxKind::Question)
        {
            self.run(command::answer_question(&item, line));
            return;
        }
        // In Comms, a line answers the selected message (or `@Name`s one).
        if self.tab == Tab::Comms {
            let entry = self.selected_entry();
            self.run(command::comms(&self.store, entry.as_ref(), &line));
            return;
        }
        // @Name message, or a message for the selected agent / focused tile.
        let default = if self.tab == Tab::Terminals {
            self.grid.focus.clone()
        } else {
            self.store.selected.clone()
        };
        let cmd = command::message(&self.store, &line, default.as_deref());
        self.run(cmd);
    }

    /// Slash commands: the operator console.
    fn command(&mut self, cmd: &str) {
        let item = self.selected_item();
        let view_rig = self.grid.rig.as_deref().filter(|_| self.tab == Tab::Terminals);
        let parsed = command::parse(
            &command::Context {
                store: &self.store,
                view_rig,
                inbox_item: item.as_ref(),
            },
            cmd,
        );
        self.run(parsed);
    }

    /// Tab after `@Mos` completes the name.
    fn complete(&mut self) -> bool {
        match command::complete(&self.store, &self.input) {
            Some(done) => {
                self.input = done;
                self.cursor = self.input.chars().count();
                true
            }
            None => false,
        }
    }

    // ---- keys and mouse -------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.grid.typing {
            if term::is_leave_key(&key) {
                self.stop_typing();
                self.flash("left the terminal".into(), false);
            } else if let Some(bytes) = term::encode_key(key) {
                self.type_into(bytes);
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.help {
            self.help = false;
            return;
        }
        if let Some(p) = &mut self.popup {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => p.scroll = p.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => p.scroll = p.scroll.saturating_sub(1),
                KeyCode::PageDown => p.scroll = p.scroll.saturating_add(10),
                KeyCode::PageUp => p.scroll = p.scroll.saturating_sub(10),
                _ => self.popup = None,
            }
            return;
        }
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => self.quit(),
            KeyCode::Char('y') if ctrl => self.answer(Decision::Allow),
            KeyCode::Char('n') if ctrl => self.answer(Decision::Deny),
            KeyCode::Char('a') if ctrl => self.answer(Decision::Always),
            KeyCode::Char('t') if ctrl => self.set_tab(Tab::Map),
            KeyCode::Char('o') if ctrl => self.operator(),
            KeyCode::Char('g') if ctrl => self.set_tab(Tab::Terminals),
            KeyCode::Char('e') if ctrl => self.cycle_critters(),
            KeyCode::Char('z') if ctrl && self.tab == Tab::Terminals => self.grid.zoom = !self.grid.zoom,
            KeyCode::Char('r') if ctrl && self.tab == Tab::Terminals => {
                // Cycle the rig filter: all → each rig → all.
                let names: Vec<String> = self.store.rigs.keys().cloned().collect();
                self.grid.rig = match &self.grid.rig {
                    None => names.first().cloned(),
                    Some(r) => names.iter().skip_while(|n| *n != r).nth(1).cloned(),
                };
                self.dirty_visible();
            }
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let chars: Vec<char> = self.input.chars().collect();
                let mut i = self.cursor;
                while i > 0 && chars[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && chars[i - 1] != ' ' {
                    i -= 1;
                }
                self.input = chars[..i].iter().chain(&chars[self.cursor..]).collect();
                self.cursor = i;
            }
            KeyCode::Char('?') if self.input.is_empty() => self.help = true,
            KeyCode::Char('+') if self.input.is_empty() && self.tab == Tab::Terminals => {
                let n = self.grid_ids().len().max(1) as u16;
                let cur = self
                    .grid
                    .cols
                    .unwrap_or_else(|| term::grid_cols(n as usize, self.areas.body, None) as u16);
                self.grid.cols = Some((cur + 1).min(n));
            }
            KeyCode::Char('-') if self.input.is_empty() && self.tab == Tab::Terminals => {
                let n = self.grid_ids().len().max(1);
                let cur = self
                    .grid
                    .cols
                    .unwrap_or_else(|| term::grid_cols(n, self.areas.body, None) as u16);
                self.grid.cols = Some(cur.saturating_sub(1).max(1));
            }
            KeyCode::Char('=') if self.input.is_empty() && self.tab == Tab::Terminals => {
                self.grid.cols = None
            }
            KeyCode::Char(ch) if !ctrl => {
                let byte = self.byte_at(self.cursor);
                self.input.insert(byte, ch);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let byte = self.byte_at(self.cursor);
                self.input.remove(byte);
            }
            KeyCode::Delete if self.cursor < self.input.chars().count() => {
                let byte = self.byte_at(self.cursor);
                self.input.remove(byte);
            }
            KeyCode::Left if self.tab == Tab::Terminals && self.input.is_empty() => self.move_focus(-1, 0),
            KeyCode::Right if self.tab == Tab::Terminals && self.input.is_empty() => self.move_focus(1, 0),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Up if self.tab == Tab::Terminals => self.move_focus(0, -1),
            KeyCode::Down if self.tab == Tab::Terminals => self.move_focus(0, 1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-5),
            KeyCode::PageDown => self.move_selection(5),
            KeyCode::Tab => {
                if !self.complete() {
                    self.cycle_tab(false);
                }
            }
            KeyCode::BackTab => self.cycle_tab(true),
            KeyCode::Esc => {
                if self.grid.zoom && self.input.is_empty() {
                    self.grid.zoom = false;
                }
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Enter => self.submit(),
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::Down(_) => {
                let at = (m.column, m.row).into();
                if let Some(tab) = self
                    .areas
                    .tabs
                    .iter()
                    .find(|(r, _)| r.contains(at))
                    .map(|(_, t)| *t)
                {
                    self.set_tab(tab);
                } else if let Some(id) = self
                    .areas
                    .tiles
                    .iter()
                    .find(|(r, _, _)| r.contains(at))
                    .map(|(_, id, _)| id.clone())
                {
                    if self.grid.focus.as_deref() == Some(&id) && !self.grid.typing {
                        self.start_typing();
                    } else {
                        if self.grid.typing {
                            self.stop_typing();
                        }
                        self.grid.focus = Some(id);
                    }
                }
            }
            _ => {}
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.input
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }
}

fn step(state: &mut ListState, n: usize, delta: i32) {
    if n == 0 {
        return;
    }
    let cur = state.selected().unwrap_or(n - 1) as i32;
    state.select(Some((cur + delta).clamp(0, n as i32 - 1) as usize));
}
