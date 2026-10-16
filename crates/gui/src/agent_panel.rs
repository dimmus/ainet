//! The Agent panel (docked opposite the Fleet): the TUI's details pane for
//! the selected agent. Its critter, big and moving; name, role and who
//! directs it; task and last tool; state; a context gauge that glides as
//! the TUI's does; the activity sparkline; where it runs; and its recent
//! messages and open queue items. With nobody selected, the empty den.

use ainet_core::{Agent, AgentState};
use ainet_model::describe::{age, short_path};
use gpui::{Entity, FocusHandle, Focusable, FontWeight, Hsla, Subscription, relative};
use ui::{Chip, Divider, Indicator, ProgressBar, prelude::*};

use crate::critter::{Animation, Critter};
use crate::dock::Panel;
use crate::fleet::{FleetModel, Tick};
use crate::panels::{SIDE_DOCK_WIDTH, header, state_color};
use crate::themes;

/// How many of its messages it shows.
const MESSAGES: usize = 5;
/// How tall the activity sparkline is.
const SPARK_HEIGHT: Pixels = px(28.);

/// The context gauge's next value: a quarter of the way to `target` each
/// tick, as the TUI's, and there once close enough.
pub fn glide(gauge: f32, target: f32) -> f32 {
    let next = gauge + (target - gauge) * 0.25;
    if (target - next).abs() < 0.001 {
        target
    } else {
        next
    }
}

/// Bar heights of the activity sparkline, 0..=1 of the busiest minute.
pub fn spark(activity: &[u64]) -> Vec<f32> {
    let max = activity.iter().copied().max().unwrap_or(0).max(1) as f32;
    activity.iter().map(|&v| v as f32 / max).collect()
}

pub struct AgentPanel {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
    /// The context gauge as drawn, gliding towards the agent's.
    gauge: f32,
    animation: Animation,
    _subscriptions: Vec<Subscription>,
}

impl AgentPanel {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&fleet, |_, _, cx| cx.notify()),
            cx.subscribe(&fleet, |this, fleet, _: &Tick, cx| {
                let target = fleet.read(cx).store.selected_agent().and_then(|a| a.context);
                let gauge = glide(this.gauge, target.unwrap_or(0.0));
                let moved = gauge != this.gauge;
                this.gauge = gauge;
                if this.animation.take() || moved {
                    cx.notify();
                }
            }),
        ];
        AgentPanel {
            fleet,
            focus_handle: cx.focus_handle(),
            gauge: 0.0,
            animation: Animation::default(),
            _subscriptions: subscriptions,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn gauge(&self) -> f32 {
        self.gauge
    }

    fn render_empty(&self, cx: &App) -> Div {
        let fleet = self.fleet.read(cx);
        let none = fleet.store.agents.is_empty();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Critter::new("empty-den", fleet.tick(), false, cx)
                    .size(px(64.))
                    .animate(&self.animation),
            )
            .child(Label::new(if none {
                "No agents yet."
            } else {
                "No agent selected"
            }))
            .when(none, |el| {
                el.child(
                    Label::new("/spawn claude ~/code/project BACKEND DEV")
                        .size(LabelSize::Small)
                        .color(Color::Accent),
                )
            })
    }

    fn render_agent(&self, a: &Agent, cx: &App) -> Div {
        let fleet = self.fleet.read(cx);
        let store = &fleet.store;
        let hue = themes::agent_color(&a.id, cx).cursor;
        let muted = |text: String| Label::new(text).size(LabelSize::Small).color(Color::Muted);

        // You › who directs it › it.
        let mut chain = h_flex().gap_1().flex_wrap().child(muted("you".into()));
        for p in store.chain(&a.id) {
            chain = chain.child(muted("›".into())).child(muted(p.name.clone()));
        }
        chain = chain.child(muted("›".into())).child(
            Label::new(a.name.clone())
                .size(LabelSize::Small)
                .color(Color::Custom(hue)),
        );

        let hero = h_flex()
            .gap_3()
            .child(
                Critter::agent(a, fleet.tick(), cx)
                    .size(px(96.))
                    .animate(&self.animation),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        Label::new(a.name.clone())
                            .size(LabelSize::Large)
                            .weight(FontWeight::BOLD),
                    )
                    .when_some(a.role.clone(), |el, role| {
                        el.child(
                            Chip::new(role)
                                .label_color(Color::Custom(hue))
                                .bg_color(hue.opacity(0.15))
                                .border_color(hue.opacity(0.4)),
                        )
                    })
                    .child(chain),
            );

        let task = v_flex()
            .gap_0p5()
            .child(Label::new(format!(
                "▸ {}",
                a.task.clone().unwrap_or_else(|| "no task yet".into())
            )))
            .when_some(a.last_tool.as_ref(), |el, tool| {
                let (name, target) = tool.split_once("  ").unwrap_or((tool, ""));
                el.child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .child(muted("↳".into()))
                        .child(Label::new(name.to_string()).size(LabelSize::Small))
                        .child(
                            Label::new(target.to_string())
                                .size(LabelSize::Small)
                                .color(Color::Accent)
                                .truncate(),
                        ),
                )
            })
            .child(muted(format!(
                "{} · {}",
                a.model.clone().unwrap_or_else(|| a.runtime.label().into()),
                short_path(&a.cwd)
            )));

        let status = h_flex()
            .gap_1p5()
            .child(Indicator::dot().color(state_color(a.state)))
            .child(Label::new(a.state.label()).color(state_color(a.state)))
            .child(muted(format!("· {}", age(a.state_since))));

        v_flex()
            .gap_3()
            .child(hero)
            .child(task)
            .child(status)
            .child(self.render_meters(a, hue, cx))
            .child(Divider::horizontal())
            .child(render_facts(a))
            .child(self.render_messages(a, cx))
            .child(self.render_queue(a, cx))
    }

    fn render_meters(&self, a: &Agent, hue: Hsla, cx: &App) -> Div {
        let ratio = self.gauge.clamp(0.0, 1.0);
        let status = cx.theme().status();
        let fill = if ratio > 0.8 {
            status.error
        } else if ratio > 0.6 {
            status.warning
        } else {
            cx.theme().colors().text_accent
        };
        let note = if a.context.is_some() {
            "of the window in use"
        } else {
            "unknown until the first turn"
        };
        let bars = spark(&a.activity);
        v_flex()
            .gap_2()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(Label::new("Context").size(LabelSize::Small))
                            .child(Label::new(format!("{:.0}%", ratio * 100.0)).size(LabelSize::Small)),
                    )
                    .child(
                        ProgressBar::new("context-gauge", ratio, 1.0, cx)
                            .fg_color(fill)
                            .bg_color(cx.theme().colors().element_background),
                    )
                    .child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Activity · 10 min").size(LabelSize::Small))
                    .child(
                        h_flex()
                            .id("activity-sparkline")
                            .h(SPARK_HEIGHT)
                            .items_end()
                            .gap_px()
                            .when(bars.is_empty(), |el| {
                                el.child(
                                    Label::new("no hook events yet")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .children(bars.into_iter().map(|h| {
                                div()
                                    .flex_1()
                                    .max_w(px(10.))
                                    .min_h(px(1.))
                                    .h(relative(h))
                                    .rounded_t_sm()
                                    .bg(hue)
                            })),
                    ),
            )
    }

    fn render_messages(&self, a: &Agent, cx: &App) -> Div {
        let store = &self.fleet.read(cx).store;
        let messages = store.messages_for(&a.id, MESSAGES);
        section("Messages").map(|el| {
            if messages.is_empty() {
                return el.child(Label::new("none yet").size(LabelSize::Small).color(Color::Muted));
            }
            el.children(messages.into_iter().map(|m| {
                let who = |id: &str| {
                    if id == "operator" {
                        Color::Accent
                    } else {
                        Color::Custom(themes::agent_color(id, cx).cursor)
                    }
                };
                v_flex()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new(m.from_label.clone())
                                    .size(LabelSize::Small)
                                    .color(who(&m.from)),
                            )
                            .child(Label::new("→").size(LabelSize::Small).color(Color::Muted))
                            .child(
                                Label::new(m.to_label.clone())
                                    .size(LabelSize::Small)
                                    .color(who(&m.to)),
                            )
                            .child(
                                Label::new(age(m.created))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        Label::new(m.body.lines().next().unwrap_or_default().to_string())
                            .size(LabelSize::Small)
                            .truncate(),
                    )
            }))
        })
    }

    fn render_queue(&self, a: &Agent, cx: &App) -> Div {
        let store = &self.fleet.read(cx).store;
        let items = store.queue_for(a);
        section("Queue").map(|el| {
            if items.is_empty() {
                return el.child(
                    Label::new("nothing open")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
            }
            el.children(items.into_iter().map(|q| {
                h_flex()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        Label::new(q.id.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Chip::new(q.state.label()))
                    .child(Label::new(q.summary.clone()).size(LabelSize::Small).truncate())
            }))
        })
    }
}

/// A titled part of the panel.
fn section(title: &'static str) -> Div {
    v_flex()
        .gap_1()
        .child(Label::new(title).size(LabelSize::XSmall).color(Color::Muted))
}

/// The TUI's facts table: where it works, since when, which process.
fn render_facts(a: &Agent) -> Div {
    let place = match &a.branch {
        Some(b) => format!(
            "{} · {b}",
            a.cwd
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
        None => "–".into(),
    };
    let rows = [
        ("seat", a.seat.clone().unwrap_or_else(|| "–".into())),
        ("where", place),
        ("cwd", short_path(&a.cwd)),
        ("started", format!("{} ago", age(a.started_at))),
        (
            "pid",
            a.pid
                .map(|p| format!("{p} · {}", a.runtime.label()))
                .unwrap_or_else(|| "not ours (seen through hooks)".into()),
        ),
        ("session", a.id.chars().take(8).collect()),
    ];
    v_flex().gap_0p5().children(rows.into_iter().map(|(k, v)| {
        h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(60.))
                    .flex_none()
                    .child(Label::new(k).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(Label::new(v).size(LabelSize::Small).truncate())
    }))
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AgentPanel {
    fn persistent_name() -> &'static str {
        "AgentPanel"
    }

    fn default_size(&self, _: &App) -> Pixels {
        SIDE_DOCK_WIDTH
    }

    fn icon(&self) -> IconName {
        IconName::UserGroup
    }

    fn icon_tooltip(&self) -> &'static str {
        "Agent Panel"
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(crate::actions::panel::agent_panel::ToggleFocus)
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.contains_focused(window, cx);
        let agent = self.fleet.read(cx).store.selected_agent().cloned();
        let body = match &agent {
            Some(a) => self.render_agent(a, cx),
            None => self.render_empty(cx),
        };
        let gone = agent.as_ref().is_some_and(|a| a.state == AgentState::Gone);
        v_flex()
            .id("agent-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(header("Agent", IconName::UserGroup, focused, cx))
            .child(
                div()
                    .id("agent-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .when(gone, |el| el.opacity(0.8))
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use ainet_model::{Demo, Msg};
    use gpui::TestAppContext;

    use super::*;
    use crate::tests::init_test;

    #[gpui::test]
    async fn shows_the_selected_agent_and_glides_its_gauge(cx: &mut TestAppContext) {
        init_test(cx);
        let fleet = cx.new(|_| FleetModel::offline());
        // Nobody yet: the empty den.
        let (panel, cx) = cx.add_window_view(|_, cx| AgentPanel::new(fleet.clone(), cx));
        cx.run_until_parked();
        fleet.update(cx, |f, cx| {
            f.store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
            let id = f.store.selected.clone().unwrap();
            f.store.agents.get_mut(&id).unwrap().context = Some(0.5);
            cx.notify();
        });
        cx.run_until_parked();
        for _ in 0..40 {
            fleet.update(cx, |_, cx| cx.emit(Tick));
        }
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.gauge()), 0.5);
        // The critter was drawn, so the next tick redraws.
        assert!(panel.read_with(cx, |p, _| p.animation.take()));
    }

    #[test]
    fn the_gauge_glides_and_settles() {
        let mut g = 0.0;
        g = glide(g, 0.8);
        assert!((g - 0.2).abs() < 1e-6);
        for _ in 0..40 {
            g = glide(g, 0.8);
        }
        assert_eq!(g, 0.8, "settles on the target");
        assert_eq!(glide(0.5, 0.5), 0.5);
    }

    #[test]
    fn the_sparkline_is_scaled_to_the_busiest_minute() {
        assert_eq!(spark(&[0, 2, 4]), [0.0, 0.5, 1.0]);
        assert_eq!(spark(&[0, 0]), [0.0, 0.0]);
        assert!(spark(&[]).is_empty());
    }
}
