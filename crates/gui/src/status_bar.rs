//! The status bar: the connection to ainetd on the left (or why there is
//! none), what is wrong with the user's settings or keymap, the latest
//! flash, and the fleet in numbers on the right: the TUI header's agents,
//! working, need-you and waiting counts.

use ainet_model::Tally;
use gpui::{Decorations, Entity};
use theme::CLIENT_SIDE_DECORATION_ROUNDING;
use ui::{Indicator, Tooltip, prelude::*};

use crate::fleet::{Connection, FleetModel};
use crate::problems;
use crate::title_bar::connection_badge;

/// What the status bar says, apart from how it looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub connection: Connection,
    pub tally: Tally,
    /// The latest flash, and whether it is an error.
    pub flash: Option<(String, bool)>,
}

impl Status {
    pub fn of(fleet: &FleetModel) -> Status {
        Status {
            connection: fleet.connection(),
            tally: fleet.tally(),
            flash: fleet.flash().map(|n| (n.text.clone(), n.error)),
        }
    }

    /// The connection in words.
    pub fn connection_text(&self) -> String {
        match &self.connection {
            Connection::Demo => "demo fleet · nothing here is real".into(),
            Connection::Starting => "starting ainetd…".into(),
            Connection::Connected => "ainetd connected".into(),
            Connection::Offline => "ainetd offline · retrying".into(),
            Connection::Failed(problem) => problem.clone(),
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
}

impl StatusBar {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&fleet, |_, _, cx| cx.notify()).detach();
        StatusBar { fleet }
    }
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = Status::of(self.fleet.read(cx));
        let problems = problems::lines(cx);
        let (dot, _) = connection_badge(&status.connection);
        let connection_color = match status.connection {
            Connection::Failed(_) => Color::Error,
            _ => Color::Muted,
        };
        let mut counts = Vec::new();
        for (i, (text, color)) in status.counts().into_iter().enumerate() {
            if i > 0 {
                counts.push(Label::new("·").size(LabelSize::Small).color(Color::Disabled));
            }
            counts.push(Label::new(text).size(LabelSize::Small).color(color));
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
                    })
                    .when_some(status.flash.clone(), |el, (text, error)| {
                        el.child(Label::new("·").size(LabelSize::Small).color(Color::Disabled))
                            .child(
                                Label::new(text)
                                    .size(LabelSize::Small)
                                    .color(if error { Color::Error } else { Color::Success })
                                    .truncate(),
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
                    .children(counts),
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
            flash: None,
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
            flash: None,
        };
        assert_eq!(s.connection_text(), "cannot start ainetd: no ainet");
    }
}
