//! Toasts over the bottom right of the workspace, driven by the fleet's
//! notices, after Zed's status toasts and notifications: a daemon flash
//! shows for a few seconds (as long as the TUI's flash line), an error
//! stays longer, and a report (the reply to an `ainet ps`-like command,
//! which has a title) stays until dismissed. A request that waits on you
//! shows its agent's critter and a button that opens it in the Inbox. A
//! few show at once, the newest at the bottom; the close button or a
//! middle click dismisses one.

use std::collections::VecDeque;
use std::time::Duration;

use gpui::{Entity, MouseButton, Task, WeakEntity};
use ui::{IconButtonShape, Tooltip, prelude::*};

use crate::critter::Critter;
use crate::fleet::{FleetModel, Notice};
use crate::workspace::Workspace;

/// How long a flash shows: the TUI's flash time.
pub const FLASH_FOR: Duration = Duration::from_secs(5);
/// How long a request waiting on you shows.
pub const ATTENTION_FOR: Duration = Duration::from_secs(10);
/// How long an error shows.
pub const ERROR_FOR: Duration = Duration::from_secs(15);
/// How many toasts show at once; older ones make way.
pub const MAX_TOASTS: usize = 3;

/// How long a notice stays up; `None` until dismissed.
pub fn lifetime(notice: &Notice) -> Option<Duration> {
    if notice.title.is_some() {
        None
    } else if notice.attention.is_some() {
        Some(ATTENTION_FOR)
    } else if notice.error {
        Some(ERROR_FOR)
    } else {
        Some(FLASH_FOR)
    }
}

struct Toast {
    id: usize,
    notice: Notice,
    _expiry: Option<Task<()>>,
}

pub struct ToastLayer {
    fleet: Entity<FleetModel>,
    /// Where "Review" opens the Inbox.
    workspace: Option<WeakEntity<Workspace>>,
    toasts: VecDeque<Toast>,
    next_id: usize,
}

impl ToastLayer {
    /// Follows `fleet`'s notices.
    pub fn new(fleet: &Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(fleet, |this, _, notice: &Notice, cx| {
            this.show(notice.clone(), cx)
        })
        .detach();
        ToastLayer {
            fleet: fleet.clone(),
            workspace: None,
            toasts: VecDeque::new(),
            next_id: 0,
        }
    }

    /// Requests waiting on you get a button opening them in `workspace`'s
    /// Inbox.
    pub fn with_workspace(mut self, workspace: WeakEntity<Workspace>) -> Self {
        self.workspace = Some(workspace);
        self
    }

    pub fn show(&mut self, notice: Notice, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        let expiry = lifetime(&notice).map(|after| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(after).await;
                let _ = this.update(cx, |this, cx| this.dismiss(id, cx));
            })
        });
        if self.toasts.len() == MAX_TOASTS {
            self.toasts.pop_front();
        }
        self.toasts.push_back(Toast {
            id,
            notice,
            _expiry: expiry,
        });
        cx.notify();
    }

    pub fn dismiss(&mut self, id: usize, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }

    /// The texts showing, oldest first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn texts(&self) -> Vec<&str> {
        self.toasts.iter().map(|t| t.notice.text.as_str()).collect()
    }

    fn render_toast(&self, toast: &Toast, cx: &mut Context<Self>) -> impl IntoElement {
        let id = toast.id;
        let notice = &toast.notice;
        let (icon, color) = if notice.error {
            (IconName::XCircle, Color::Error)
        } else if notice.title.is_some() {
            (IconName::Info, Color::Muted)
        } else if notice.attention.is_some() {
            (IconName::Bell, Color::Warning)
        } else {
            (IconName::Check, Color::Success)
        };
        // A request waiting on you: its agent's critter, and a way to it.
        let fleet = self.fleet.read(cx);
        let critter = notice.attention.as_ref().and_then(|a| {
            let agent = fleet.store.agents.get(&a.agent)?;
            Some(Critter::agent(agent, fleet.tick(), cx).size(px(20.)))
        });
        let review = notice
            .attention
            .as_ref()
            .zip(self.workspace.clone())
            .map(|(attention, workspace)| {
                let item = attention.item;
                Button::new(("review", id), "Review")
                    .style(ButtonStyle::Outlined)
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let _ = workspace.update(cx, |w, cx| w.open_inbox(Some(item), window, cx));
                        this.dismiss(id, cx);
                    }))
            });
        h_flex()
            .id(("toast", id))
            .occlude()
            .elevation_3(cx)
            .items_start()
            .gap_2()
            .py_1p5()
            .pl_2p5()
            .pr_1p5()
            .max_w(px(420.))
            .bg(cx.theme().colors().surface_background)
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.dismiss(id, cx)),
            )
            .child(match critter {
                Some(critter) => div().child(critter).into_any_element(),
                None => div()
                    .pt_0p5()
                    .child(Icon::new(icon).size(IconSize::Small).color(color))
                    .into_any_element(),
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .when_some(notice.title.clone(), |el, title| {
                        el.child(Label::new(title).weight(gpui::FontWeight::SEMIBOLD))
                    })
                    .child(
                        Label::new(notice.text.clone())
                            .size(if notice.title.is_some() {
                                LabelSize::Small
                            } else {
                                LabelSize::Default
                            })
                            .color(if notice.title.is_some() {
                                Color::Muted
                            } else {
                                Color::Default
                            }),
                    ),
            )
            .children(review)
            .child(
                IconButton::new(("dismiss", id), IconName::Close)
                    .shape(IconButtonShape::Square)
                    .icon_size(IconSize::Small)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text("Dismiss"))
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(id, cx))),
            )
    }
}

impl Render for ToastLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.toasts.is_empty() {
            return div().into_any_element();
        }
        let toasts: Vec<AnyElement> = self
            .toasts
            .iter()
            .map(|t| self.render_toast(t, cx).into_any_element())
            .collect();
        v_flex()
            .id("toast-layer")
            .absolute()
            .bottom_10()
            .right_3()
            .items_end()
            .gap_2()
            .children(toasts)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    fn notice(title: Option<&str>, text: &str, error: bool) -> Notice {
        Notice {
            title: title.map(String::from),
            text: text.into(),
            error,
            attention: None,
        }
    }

    #[test]
    fn errors_stay_longer_and_reports_until_dismissed() {
        assert_eq!(lifetime(&notice(None, "ok", false)), Some(FLASH_FOR));
        assert_eq!(lifetime(&notice(None, "bad", true)), Some(ERROR_FOR));
        assert!(ERROR_FOR > FLASH_FOR);
        assert_eq!(lifetime(&notice(Some("ps"), "2 rigs", false)), None);
        let asks = Notice {
            attention: Some(crate::fleet::Attention {
                agent: "a".into(),
                item: 1,
            }),
            ..notice(None, "lead: Bash: ls", false)
        };
        assert_eq!(lifetime(&asks), Some(ATTENTION_FOR));
    }

    #[gpui::test]
    async fn flashes_from_the_fleet_expire(cx: &mut TestAppContext) {
        let fleet = cx.new(|_| FleetModel::offline());
        let toasts = cx.new(|cx| ToastLayer::new(&fleet, cx));
        fleet.update(cx, |f, cx| {
            f.show("saved".into(), false, cx);
            f.show("cannot reach seat".into(), true, cx);
        });
        toasts.update(cx, |t, cx| t.show(notice(Some("ps"), "2 rigs", false), cx));
        cx.run_until_parked();
        toasts.read_with(cx, |t, _| {
            assert_eq!(t.texts(), ["saved", "cannot reach seat", "2 rigs"])
        });

        cx.executor().advance_clock(FLASH_FOR + Duration::from_millis(10));
        cx.run_until_parked();
        toasts.read_with(cx, |t, _| assert_eq!(t.texts(), ["cannot reach seat", "2 rigs"]));
        cx.executor().advance_clock(ERROR_FOR);
        cx.run_until_parked();
        toasts.read_with(cx, |t, _| assert_eq!(t.texts(), ["2 rigs"]));

        // A report stays until dismissed; only so many show at once.
        for i in 0..MAX_TOASTS + 1 {
            fleet.update(cx, |f, cx| f.show(format!("n{i}"), false, cx));
        }
        toasts.read_with(cx, |t, _| {
            assert_eq!(t.texts().len(), MAX_TOASTS);
            assert_eq!(*t.texts().last().unwrap(), format!("n{MAX_TOASTS}"));
        });
        let first = toasts.read_with(cx, |t, _| t.toasts[0].id);
        toasts.update(cx, |t, cx| t.dismiss(first, cx));
        toasts.read_with(cx, |t, _| assert_eq!(t.texts().len(), MAX_TOASTS - 1));
    }
}
