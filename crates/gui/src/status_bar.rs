//! The status bar: the connection to ainetd on the left (or why there is
//! none) and what is wrong with the user's settings or keymap, and the
//! fleet in numbers on the right: the TUI header's agents, working,
//! need-you and waiting counts. At its ends, as in Zed, a button per dock
//! that opens and closes it. Flashes show as toasts (`toast_layer`).

use ainet_model::Tally;
use gpui::{Decorations, Entity, WeakEntity};
use theme::CLIENT_SIDE_DECORATION_ROUNDING;
use ui::{Indicator, Tooltip, prelude::*};

use crate::dock::Dock;
use crate::fleet::{Connection, FleetModel};
use crate::inbox::waiting_badge;
use crate::problems;
use crate::title_bar::connection_badge;
use crate::workspace::Workspace;

/// What the status bar says, apart from how it looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub connection: Connection,
    pub tally: Tally,
}

impl Status {
    pub fn of(fleet: &FleetModel) -> Status {
        Status {
            connection: fleet.connection(),
            tally: fleet.tally(),
        }
    }

    /// The connection in words.
    pub fn connection_text(&self) -> String {
        match &self.connection {
            Connection::Demo => "demo fleet · nothing here is real".into(),
            Connection::Starting => "starting ainetd…".into(),
            Connection::Connected => "ainetd connected".into(),
            Connection::Offline => "ainetd offline · retrying".into(),
            Connection::Failed(problem) | Connection::Incompatible(problem) => problem.clone(),
        }
    }

    /// The counts, in the TUI header's words.
    pub fn counts(&self) -> Vec<(String, Color)> {
        let t = &self.tally;
        let mut v = vec![(format!("{} agents", t.live), Color::Muted)];
        if t.needs_you > 0 {
            v.push((format!("{} need you", t.needs_you), Color::Warning));
        }
        v.push((format!("{} working", t.working), Color::Muted));
        v.push(if t.waiting > 0 {
            (format!("{} waiting", t.waiting), Color::Warning)
        } else {
            ("inbox clear".into(), Color::Muted)
        });
        v
    }
}

pub struct StatusBar {
    fleet: Entity<FleetModel>,
    workspace: WeakEntity<Workspace>,
    /// The left, bottom and right docks.
    docks: [Entity<Dock>; 3],
}

impl StatusBar {
    pub fn new(
        fleet: Entity<FleetModel>,
        workspace: WeakEntity<Workspace>,
        docks: [Entity<Dock>; 3],
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        for dock in &docks {
            cx.observe(dock, |_, _, cx| cx.notify()).detach();
        }
        StatusBar {
            fleet,
            workspace,
            docks,
        }
    }

    /// The button for a dock's panel, lit while the dock is open.
    fn dock_button(&self, dock: &Entity<Dock>, cx: &App) -> Option<AnyElement> {
        let dock = dock.read(cx);
        let panel = dock.active_panel()?;
        let position = dock.position();
        let action = dock.toggle_action();
        let tooltip = panel.icon_tooltip(cx);
        let workspace = self.workspace.clone();
        Some(
            IconButton::new(SharedString::from(format!("dock-{position:?}")), panel.icon(cx))
                .icon_size(IconSize::Small)
                .toggle_state(dock.is_open())
                .tooltip(move |_, cx| Tooltip::for_action(tooltip, action.as_ref(), cx))
                .on_click(move |_, window, cx| {
                    let _ = workspace.update(cx, |w, cx| w.toggle_dock(position, window, cx));
                })
                .into_any_element(),
        )
    }
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = Status::of(self.fleet.read(cx));
        let alerts = self.fleet.read(cx).alerts();
        let problems = problems::lines(cx);
        let (dot, _) = connection_badge(&status.connection);
        let connection_color = match status.connection {
            Connection::Failed(_) | Connection::Incompatible(_) => Color::Error,
            _ => Color::Muted,
        };
        let [left_dock, bottom_dock, right_dock] = &self.docks;
        let left = self.dock_button(left_dock, cx);
        let right: Vec<AnyElement> = [bottom_dock, right_dock]
            .into_iter()
            .filter_map(|d| self.dock_button(d, cx))
            .collect();
        let mut counts: Vec<AnyElement> = Vec::new();
        let all = status.counts();
        let last = all.len() - 1;
        for (i, (text, color)) in all.into_iter().enumerate() {
            if i > 0 {
                counts.push(
                    Label::new("·")
                        .size(LabelSize::Small)
                        .color(Color::Disabled)
                        .into_any_element(),
                );
            }
            // The inbox's count, last: a badge opening the Inbox while
            // something waits.
            if i == last && status.tally.waiting > 0 {
                let workspace = self.workspace.clone();
                counts.push(waiting_badge(
                    "status-waiting",
                    status.tally.waiting,
                    alerts,
                    move |_, window, cx| {
                        let _ = workspace.update(cx, |w, cx| w.open_inbox(None, window, cx));
                    },
                    cx,
                ));
            } else {
                counts.push(
                    Label::new(text)
                        .size(LabelSize::Small)
                        .color(color)
                        .into_any_element(),
                );
            }
        }

        h_flex()
            .id("status-bar")
            .w_full()
            .justify_between()
            .gap(DynamicSpacing::Base08.rems(cx))
            .py(DynamicSpacing::Base04.rems(cx))
            .px(DynamicSpacing::Base08.rems(cx))
            .bg(cx.theme().colors().status_bar_background)
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .map(|el| match window.window_decorations() {
                Decorations::Server => el,
                Decorations::Client { tiling } => el
                    .when(!(tiling.bottom || tiling.right), |el| {
                        el.rounded_br(CLIENT_SIDE_DECORATION_ROUNDING)
                    })
                    .when(!(tiling.bottom || tiling.left), |el| {
                        el.rounded_bl(CLIENT_SIDE_DECORATION_ROUNDING)
                    }),
            })
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .overflow_x_hidden()
                    .children(left)
                    .child(
                        Icon::new(IconName::Server)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Indicator::dot().color(dot))
                    .child(
                        Label::new(status.connection_text())
                            .size(LabelSize::Small)
                            .color(connection_color)
                            .truncate(),
                    )
                    .when_some(problems.first().cloned(), |el, first| {
                        let all = problems.join("\n");
                        el.child(Label::new("·").size(LabelSize::Small).color(Color::Disabled))
                            .child(
                                h_flex()
                                    .id("config-problems")
                                    .gap_1()
                                    .min_w_0()
                                    .child(
                                        Icon::new(IconName::Warning)
                                            .size(IconSize::Small)
                                            .color(Color::Warning),
                                    )
                                    .child(
                                        Label::new(first)
                                            .size(LabelSize::Small)
                                            .color(Color::Warning)
                                            .truncate(),
                                    )
                                    .tooltip(Tooltip::text(all)),
                            )
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_none()
                    .child(
                        Icon::new(IconName::UserGroup)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .children(counts)
                    .children(right),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(tally: Tally) -> Status {
        Status {
            connection: Connection::Connected,
            tally,
        }
    }

    fn texts(s: &Status) -> Vec<String> {
        s.counts().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn counts_read_like_the_tui_header() {
        let quiet = status(Tally {
            live: 3,
            working: 1,
            idle: 2,
            ..Tally::default()
        });
        assert_eq!(texts(&quiet), ["3 agents", "1 working", "inbox clear"]);
        let busy = status(Tally {
            live: 7,
            working: 2,
            idle: 1,
            needs_you: 4,
            waiting: 3,
        });
        assert_eq!(texts(&busy), ["7 agents", "4 need you", "2 working", "3 waiting"]);
    }

    #[test]
    fn a_failed_start_says_why() {
        let s = Status {
            connection: Connection::Failed("cannot start ainetd: no ainet".into()),
            tally: Tally::default(),
        };
        assert_eq!(s.connection_text(), "cannot start ainetd: no ainet");
    }
}
