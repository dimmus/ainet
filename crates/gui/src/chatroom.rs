//! The Chatroom panel, in the bottom dock: one rig's chatroom on one topic
//! (`Store::chat_rooms`: every rig's `general`, and every topic posted
//! in), its posts oldest first with the poster's critter, and a box that
//! posts there as you (`Request::ChatSend` with the room's rig and topic,
//! through `command::chat`). The list follows the newest post, as Comms
//! does, unless you scrolled back.

use ainet_model::{Store, command};
use gpui::{
    Action, Entity, FocusHandle, Focusable, FollowMode, ListAlignment, ListState, Subscription, WeakEntity,
    list,
};
use ui::{ContextMenu, DropdownMenu, Tooltip, WithScrollbar, prelude::*};

use crate::actions::panel::chatroom_panel::ToggleFocus;
use crate::comms::{hue, is_you, poster, splices};
use crate::critter::{self, Critter};
use crate::dock::Panel;
use crate::fleet::{FleetModel, Tick};
use crate::panels::{BOTTOM_DOCK_HEIGHT, bottom_header};
use crate::text_input::{InputEvent, TextInput};

/// A chatroom: a rig and a topic.
pub type Room = (String, String);

/// The room to show when none is chosen, or the chosen one has gone: the
/// selected agent's rig's `general`, else the first.
pub fn default_room(store: &Store) -> Option<Room> {
    let rooms = store.chat_rooms();
    let rig = store.selected_agent().and_then(|a| a.rig.clone());
    rooms
        .iter()
        .find(|(r, t)| Some(r) == rig.as_ref() && t == ainet_model::store::GENERAL)
        .or(rooms.first())
        .cloned()
}

pub struct ChatroomPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
    room: Option<Room>,
    /// Chosen by you, so it is kept while it exists.
    chosen: bool,
    list: ListState,
    /// The posts the list holds, by id; `None` when the room changed and
    /// the list starts over.
    shown: Option<Vec<u64>>,
    compose: Entity<TextInput>,
    animation: critter::Animation,
    _subscriptions: Vec<Subscription>,
}

impl ChatroomPanel {
    pub fn new(fleet: Entity<FleetModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let compose = cx.new(|cx| TextInput::new(window, cx).with_multiline(Some(3)));
        let subscriptions = vec![
            cx.observe(&fleet, |this, _, cx| {
                this.sync(cx);
                cx.notify();
            }),
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.notify();
                }
            }),
            cx.subscribe(&compose, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Submitted => this.send(cx),
                InputEvent::Edited | InputEvent::Blurred | InputEvent::Cancelled => cx.notify(),
            }),
        ];
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        let mut panel = ChatroomPanel {
            fleet,
            focus_handle: cx.focus_handle(),
            room: None,
            chosen: false,
            list,
            shown: None,
            compose,
            animation: critter::Animation::default(),
            _subscriptions: subscriptions,
        };
        panel.sync(cx);
        panel
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn room(&self) -> Option<&Room> {
        self.room.as_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn compose(&self) -> &Entity<TextInput> {
        &self.compose
    }

    #[cfg(test)]
    pub fn list(&self) -> &ListState {
        &self.list
    }

    pub fn shown(&self) -> usize {
        self.shown.as_ref().map_or(0, Vec::len)
    }

    /// Shows `room`'s posts.
    pub fn set_room(&mut self, room: Room, cx: &mut Context<Self>) {
        self.chosen = true;
        if self.room.as_ref() != Some(&room) {
            self.room = Some(room);
            self.shown = None;
            self.sync(cx);
            self.update_placeholder(cx);
            cx.notify();
        }
    }

    /// Follows the store: the room is still there (or another is picked),
    /// and the list changes only where the room's posts did (new ones at
    /// the end, the oldest dropped at the Store's cap), keeping the scroll
    /// position.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let store = &self.fleet.read(cx).store;
        let exists = self.room.as_ref().is_some_and(|r| store.chat_rooms().contains(r));
        // A room you chose stays; otherwise the selected agent's rig's.
        self.chosen &= exists;
        let room = if self.chosen {
            self.room.clone()
        } else {
            default_room(store)
        };
        if room != self.room {
            self.room = room;
            self.shown = None;
            self.update_placeholder(cx);
        }
        let store = &self.fleet.read(cx).store;
        let ids: Vec<u64> = match &self.room {
            Some((rig, topic)) => store.chat_in(rig, topic).iter().map(|c| c.id).collect(),
            None => Vec::new(),
        };
        match &self.shown {
            None => self.list.reset(ids.len()),
            Some(shown) => {
                for (range, count) in splices(shown, &ids) {
                    self.list.splice(range, count);
                }
            }
        }
        self.shown = Some(ids);
    }

    fn update_placeholder(&mut self, cx: &mut Context<Self>) {
        let text = match &self.room {
            Some((rig, topic)) => format!("Post in #{topic} ({rig})…"),
            None => "No rigs yet: /up starter launches one".to_string(),
        };
        self.compose
            .update(cx, |input, cx| input.set_placeholder(text, cx));
    }

    /// Posts what was typed in the room, as you.
    fn send(&mut self, cx: &mut Context<Self>) {
        let body = self.compose.read(cx).text().trim().to_string();
        let Some((rig, topic)) = self.room.clone().filter(|_| !body.is_empty()) else {
            return;
        };
        self.fleet.update(cx, |fleet, cx| {
            fleet.run(command::chat(&rig, Some(&topic), body), cx)
        });
        self.compose.update(cx, |input, cx| input.set_text("", cx));
        // What you post you want to see: back to the end.
        self.list.set_follow_mode(FollowMode::Tail);
    }

    fn render_post(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let fleet = self.fleet.read(cx);
        let Some((rig, topic)) = &self.room else {
            return div().into_any_element();
        };
        let Some(post) = fleet.store.chat_in(rig, topic).get(ix).copied().cloned() else {
            return div().into_any_element();
        };
        let agent = poster(&fleet.store, &post.sender).cloned();
        let tick = fleet.tick();
        let avatar = match &agent {
            Some(agent) => Critter::agent(agent, tick, cx)
                .size(px(16.))
                .animate(&self.animation)
                .into_any_element(),
            None if is_you(&post.sender) => Icon::new(IconName::Person)
                .size(IconSize::Small)
                .color(Color::Accent)
                .into_any_element(),
            None => Critter::new(&post.sender, 0, false, cx)
                .size(px(16.))
                .into_any_element(),
        };
        let color = match &agent {
            Some(a) => hue(&a.id, cx),
            None if is_you(&post.sender) => hue("operator", cx),
            None => Color::Muted.color(cx),
        };
        h_flex()
            .id(("chat-post", ix))
            .items_start()
            .gap_2()
            .px_3()
            .py_1()
            .child(div().flex_none().pt_0p5().child(avatar))
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new(post.sender.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Custom(color)),
                            )
                            .child(
                                Label::new(ainet_model::describe::age(post.created))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(div().text_ui_sm(cx).child(post.body.clone())),
            )
            .into_any_element()
    }

    /// The room picker: every rig's rooms.
    fn render_rooms(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rooms = self.fleet.read(cx).store.chat_rooms();
        let Some((rig, topic)) = self.room.clone() else {
            return Label::new("No chatrooms")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element();
        };
        let this = cx.entity().downgrade();
        let current = (rig.clone(), topic.clone());
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            for room in rooms {
                let this: WeakEntity<Self> = this.clone();
                let label = format!("#{} · {}", room.1, room.0);
                let on = room == current;
                menu = menu.toggleable_entry(label, on, IconPosition::Start, None, move |_, cx| {
                    let _ = this.update(cx, |panel, cx| panel.set_room(room.clone(), cx));
                });
            }
            menu
        });
        DropdownMenu::new("chatroom-rooms", format!("#{topic} · {rig}"), menu)
            .trigger_size(ButtonSize::Compact)
            .trigger_tooltip(Tooltip::text("Choose a chatroom"))
            .into_any_element()
    }
}

impl Focusable for ChatroomPanel {
    /// The box's: showing the panel is for posting.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.compose.focus_handle(cx)
    }
}

impl Panel for ChatroomPanel {
    fn persistent_name() -> &'static str {
        "ChatroomPanel"
    }

    fn default_size(&self, _: &App) -> Pixels {
        BOTTOM_DOCK_HEIGHT
    }

    fn icon(&self) -> IconName {
        IconName::Hash
    }

    fn icon_tooltip(&self) -> &'static str {
        "Chatroom"
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }
}

impl Render for ChatroomPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        let rooms = self.render_rooms(window, cx);
        let posts = if self.shown() == 0 {
            div()
                .p_3()
                .child(
                    Label::new("Nobody has posted here yet.")
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
                        cx.processor(|this, ix: usize, _, cx| this.render_post(ix, cx)),
                    )
                    .size_full(),
                )
                .vertical_scrollbar_for(&self.list, window, cx)
                .into_any_element()
        };
        let compose_focused = self.compose.focus_handle(cx).is_focused(window);
        let colors = cx.theme().colors();
        let empty = self.compose.read(cx).is_empty();
        v_flex()
            .id("chatroom-panel")
            .key_context("ChatroomPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(bottom_header(Self::persistent_name(), focused, rooms, cx))
            .child(div().flex_1().min_h_0().child(posts))
            .child(
                h_flex()
                    .flex_none()
                    .items_end()
                    .gap_2()
                    .mx_2()
                    .mb_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if compose_focused {
                        colors.border_focused
                    } else {
                        colors.border
                    })
                    .bg(colors.editor_background)
                    .child(div().flex_1().min_w_0().child(self.compose.clone()))
                    .child(
                        IconButton::new("chatroom-send", IconName::Send)
                            .icon_size(IconSize::Small)
                            .icon_color(if empty { Color::Muted } else { Color::Accent })
                            .disabled(empty || self.room.is_none())
                            .tooltip(Tooltip::text("Post (enter)"))
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    ),
            )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use ainet_core::Request;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<ChatroomPanel>, &mut VisualTestContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        let panel = workspace
            .read_with(cx, |w, cx| w.bottom_panel::<ChatroomPanel>(cx))
            .expect("the Chatroom panel");
        (workspace, panel, cx)
    }

    #[gpui::test]
    async fn posts_go_to_the_rooms_rig_and_topic(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = demo(cx);
        // The selected agent's rig, general.
        panel.read_with(cx, |p, _| {
            assert_eq!(p.room(), Some(&("docs".to_string(), "general".to_string())));
            assert_eq!(p.shown(), 2, "the demo's two posts in docs");
        });
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        fleet.update(cx, |f, _| f.sent.clear());

        // Its tab shows it, with the keyboard in the box.
        cx.dispatch_action(ToggleFocus);
        workspace.read_with(cx, |w, cx| {
            let dock = w.dock(crate::dock::DockPosition::Bottom).read(cx);
            assert_eq!(dock.active_panel_name(), Some("ChatroomPanel"));
        });
        let compose = panel.read_with(cx, |p, _| p.compose().clone());
        assert!(cx.update(|window, cx| compose.focus_handle(cx).is_focused(window)));

        // Another room, then a post there.
        panel.update(cx, |p, cx| p.set_room(("infra".into(), "general".into()), cx));
        cx.simulate_input("rollout at noon");
        cx.simulate_keystrokes("enter");
        let sent = fleet.update(cx, |f, _| std::mem::take(&mut f.sent));
        assert!(
            matches!(&sent[..], [(Request::ChatSend { rig, topic: Some(t), sender: None, body }, ok)]
                if rig == "infra" && t == "general" && body == "rollout at noon"
                    && ok == "posted in #general infra"),
            "{sent:?}"
        );
        cx.run_until_parked();
        panel.read_with(cx, |p, cx| {
            assert!(p.compose().read(cx).is_empty());
            assert_eq!(p.shown(), 2, "the demo's post in infra, and ours");
        });

        // Nothing typed, nothing sent.
        cx.simulate_keystrokes("enter");
        assert!(fleet.update(cx, |f, _| std::mem::take(&mut f.sent)).is_empty());
    }

    /// At the Store's cap each post pushes the oldest out, maybe from
    /// another room: the list keeps every post on its own row and a
    /// reader scrolled back on the post they read.
    #[gpui::test]
    async fn at_the_cap_the_list_follows_the_rooms_posts(cx: &mut TestAppContext) {
        use ainet_core::{ChatMessage, Event};
        use ainet_model::Msg;
        use gpui::ListOffset;

        let (workspace, panel, cx) = demo(cx);
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        let post = |id: u64, rig: &str| {
            Msg::Event(Box::new(Event::Chat {
                message: ChatMessage {
                    id,
                    rig: rig.into(),
                    topic: "general".into(),
                    sender: "you (operator)".into(),
                    body: format!("post {id}"),
                    created: id as i64,
                },
            }))
        };
        let ids = |cx: &mut VisualTestContext| {
            fleet.read_with(cx, |f, _| {
                f.store
                    .chat_in("docs", "general")
                    .iter()
                    .map(|c| c.id)
                    .collect::<Vec<u64>>()
            })
        };
        // Fill the Store with this room's posts, past its cap.
        let full: Vec<Msg> = (0..2100).map(|i| post(10_000 + i, "docs")).collect();
        fleet.update(cx, |f, cx| f.receive(full, cx));
        cx.run_until_parked();
        let before = ids(cx);
        assert_eq!(panel.read_with(cx, |p, _| p.list().item_count()), before.len());
        let reading = ListOffset {
            item_ix: 100,
            offset_in_item: px(0.),
        };
        panel.read_with(cx, |p, _| p.list().scroll_to(reading));
        let on_top = |cx: &mut VisualTestContext| {
            let ix = panel.read_with(cx, |p, _| p.list().logical_scroll_top().item_ix);
            ids(cx)[ix]
        };

        // A post here, the oldest here out: as many rows, moved up one.
        fleet.update(cx, |f, cx| f.receive(vec![post(20_000, "docs")], cx));
        cx.run_until_parked();
        let after = ids(cx);
        assert_eq!(after.len(), before.len());
        assert_eq!(panel.read_with(cx, |p, _| p.list().item_count()), after.len());
        assert_eq!(on_top(cx), before[100], "still on the post read");

        // A post elsewhere, the oldest here out: one row fewer.
        fleet.update(cx, |f, cx| f.receive(vec![post(20_001, "infra")], cx));
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.list().item_count()), after.len() - 1);
        assert_eq!(on_top(cx), before[100], "still on the post read");
    }
}
