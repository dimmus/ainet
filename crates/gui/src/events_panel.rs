//! The Events panel, in the bottom dock: what happened in the fleet, one
//! line each, the latest last (`FleetModel::events`, made by
//! `ainet_model::journal` from what ainetd says): agents joining, leaving
//! and changing state, rigs, the inbox, messages, chat, the queue, the
//! connection. The log keeps the last `fleet::KEEP_EVENTS`; the list
//! follows its end unless you scrolled back.

use std::rc::Rc;

use ainet_model::describe::age;
use ainet_model::journal::{Happening, Kind};
use gpui::{
    Action, Entity, FocusHandle, Focusable, FollowMode, ListAlignment, ListState, Subscription, list,
};
use ui::{WithScrollbar, prelude::*};

use crate::actions::panel::events_panel::ToggleFocus;
use crate::dock::Panel;
use crate::fleet::FleetModel;
use crate::panels::{BOTTOM_DOCK_HEIGHT, bottom_header};
use crate::themes;

/// How a kind of event looks: an icon and its colour.
pub fn look(kind: Kind) -> (IconName, Color) {
    match kind {
        Kind::Joined => (IconName::UserCheck, Color::Success),
        Kind::Left => (IconName::XCircle, Color::Muted),
        Kind::State => (IconName::ArrowRightLeft, Color::Muted),
        Kind::Rig => (IconName::Server, Color::Info),
        Kind::Inbox { waiting: true } => (IconName::BellRing, Color::Warning),
        Kind::Inbox { waiting: false } => (IconName::Bell, Color::Muted),
        Kind::Answered => (IconName::Check, Color::Success),
        Kind::Message => (IconName::Envelope, Color::Muted),
        Kind::Chat => (IconName::Hash, Color::Muted),
        Kind::Queue => (IconName::ListTodo, Color::Muted),
        Kind::Connection { up: true } => (IconName::Server, Color::Success),
        Kind::Connection { up: false } => (IconName::Server, Color::Error),
    }
}

/// How the list changes when the log goes from `(logged, len)` to
/// `(now_logged, now_len)`: how many lines left the front (the cap) and
/// how many joined the end.
pub fn moved(logged: usize, len: usize, now_logged: usize, now_len: usize) -> (usize, usize) {
    let added = now_logged.saturating_sub(logged).min(now_len);
    let dropped = (len + added).saturating_sub(now_len).min(len);
    (dropped, added)
}

pub struct EventsPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
    list: ListState,
    /// The log as the list holds it: what it showed, and how many were
    /// ever logged then.
    lines: Vec<Rc<Happening>>,
    logged: usize,
    _subscription: Subscription,
}

impl EventsPanel {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&fleet, |this, _, cx| {
            if this.sync(cx) {
                cx.notify();
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        let mut panel = EventsPanel {
            fleet,
            focus_handle: cx.focus_handle(),
            list,
            lines: Vec::new(),
            logged: 0,
            _subscription: subscription,
        };
        panel.sync(cx);
        panel
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn lines(&self) -> &[Rc<Happening>] {
        &self.lines
    }

    /// Takes in what was logged since; true if anything was.
    fn sync(&mut self, cx: &App) -> bool {
        let fleet = self.fleet.read(cx);
        let (logged, events) = (fleet.events_logged(), fleet.events());
        if logged == self.logged {
            return false;
        }
        let (dropped, added) = moved(self.logged, self.lines.len(), logged, events.len());
        if dropped > 0 {
            self.list.splice(0..dropped, 0);
        }
        let end = self.lines.len() - dropped;
        self.list.splice(end..end, added);
        self.lines = events.iter().cloned().collect();
        self.logged = logged;
        true
    }

    fn render_line(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(line) = self.lines.get(ix) else {
            return div().into_any_element();
        };
        let (icon, color) = look(line.kind);
        let text_color = match &line.agent {
            Some(id) if line.kind == Kind::Joined => Color::Custom(themes::agent_color(id, cx).cursor),
            _ => Color::Default,
        };
        h_flex()
            .id(("event", ix))
            .debug_selector(move || format!("EVENTS-LINE-{ix}"))
            .gap_2()
            .px_3()
            .py_0p5()
            .child(Icon::new(icon).size(IconSize::XSmall).color(color))
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(line.text.clone())
                        .size(LabelSize::Small)
                        .color(text_color)
                        .truncate(),
                ),
            )
            .child(
                Label::new(age(line.at))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .into_any_element()
    }
}

impl Focusable for EventsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for EventsPanel {
    fn persistent_name() -> &'static str {
        "EventsPanel"
    }

    fn default_size(&self, _: &App) -> Pixels {
        BOTTOM_DOCK_HEIGHT
    }

    fn icon(&self) -> IconName {
        IconName::HistoryRerun
    }

    fn icon_tooltip(&self) -> &'static str {
        "Events"
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }
}

impl Render for EventsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        let count = Label::new(format!("{} events", self.lines.len()))
            .size(LabelSize::Small)
            .color(Color::Muted);
        let body = if self.lines.is_empty() {
            div()
                .p_3()
                .child(
                    Label::new("Nothing has happened yet.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .child(
                    list(
                        self.list.clone(),
                        cx.processor(|this, ix: usize, _, cx| this.render_line(ix, cx)),
                    )
                    .size_full()
                    .py_1(),
                )
                .vertical_scrollbar_for(&self.list, window, cx)
                .into_any_element()
        };
        v_flex()
            .id("events-panel")
            .key_context("EventsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
            )
            .child(bottom_header(Self::persistent_name(), focused, count, cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod pure_tests {
    use super::moved;

    #[test]
    fn the_list_drops_what_the_cap_drops_and_adds_the_new() {
        // Three more on a log far from full.
        assert_eq!(moved(10, 10, 13, 13), (0, 3));
        // Full at 5: two more push two out.
        assert_eq!(moved(5, 5, 7, 5), (2, 2));
        // More arrived than the log keeps: all shown go, the log is new.
        assert_eq!(moved(5, 5, 20, 5), (5, 5));
        assert_eq!(moved(3, 3, 3, 3), (0, 0));
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_model::journal::{Happening, Kind};
    use gpui::TestAppContext;

    use super::*;
    use crate::fleet::KEEP_EVENTS;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    #[gpui::test]
    async fn the_log_is_capped_newest_last(cx: &mut TestAppContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        let panel = workspace
            .read_with(cx, |w, cx| w.bottom_panel::<EventsPanel>(cx))
            .expect("the Events panel");
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        // On show, so its rows are drawn.
        cx.dispatch_action(ToggleFocus);
        cx.run_until_parked();
        assert!(cx.debug_bounds("EVENTS-LINE-0").is_none(), "nothing yet");

        // What the demo does is logged: a post in the chatroom.
        fleet.update(cx, |f, cx| {
            f.request(
                ainet_core::Request::ChatSend {
                    rig: "docs".into(),
                    topic: None,
                    sender: None,
                    body: "hello all".into(),
                },
                String::new(),
                cx,
            )
        });
        cx.run_until_parked();
        let last = panel
            .read_with(cx, |p, _| p.lines().last().cloned())
            .expect("a line");
        assert_eq!(last.kind, Kind::Chat);
        assert_eq!(last.text, "#general docs · you (operator): hello all");
        assert!(cx.debug_bounds("EVENTS-LINE-0").is_some(), "its row is drawn");

        // More than the log keeps: the oldest make way, the newest last.
        fleet.update(cx, |f, cx| {
            for i in 0..KEEP_EVENTS + 7 {
                f.log_for_test(
                    Happening {
                        at: 0,
                        kind: Kind::Queue,
                        agent: None,
                        text: format!("line {i}"),
                    },
                    cx,
                );
            }
        });
        cx.run_until_parked();
        panel.read_with(cx, |p, _| {
            assert_eq!(p.lines().len(), KEEP_EVENTS);
            assert_eq!(p.lines().first().unwrap().text, "line 7");
            assert_eq!(
                p.lines().last().unwrap().text,
                format!("line {}", KEEP_EVENTS + 6)
            );
            assert_eq!(p.list.item_count(), KEEP_EVENTS);
        });
    }
}
