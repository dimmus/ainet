//! Reports as tabs: [`ReportItem`] shows a reply that is long text to read
//! whole (an agent's transcript, a spec's YAML, a rig exported as a spec, a
//! queue item in full) in the buffer font, read-only but selectable and
//! copyable (a read-only `TextInput`: the mouse, `ctrl-a`, `ctrl-c`),
//! scrolling, instead of a toast. It asks for its text itself
//! (`FleetModel::ask`), shows "Loading…" until the reply comes, the
//! daemon's error if it fails, and asks again on Refresh.
//!
//! The workspace opens one for every request `fleet::is_report` names
//! (`Workspace::open_report`), from the Fleet's Show Transcript, the
//! Queue's Show, or `/spec` and `/export` typed in Comms.

use ainet_core::Request;
use gpui::{ClipboardItem, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle, WeakEntity};
use ui::{Tooltip, WithScrollbar, prelude::*};

use crate::fleet::FleetModel;
use crate::item::{Item, ItemEvent};
use crate::text_input::TextInput;

/// Where the reply stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportState {
    Loading,
    Ready,
    Failed(String),
}

pub struct ReportItem {
    fleet: Entity<FleetModel>,
    request: Request,
    title: SharedString,
    state: ReportState,
    text: Entity<TextInput>,
    scroll: ScrollHandle,
}

impl ReportItem {
    /// A report of `request`'s reply, titled `title`. It asks for it
    /// once [`Self::refresh`] is called.
    pub fn new(
        fleet: Entity<FleetModel>,
        request: Request,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| {
            let mut input = TextInput::new(window, cx).with_multiline(None);
            input.set_read_only(true, cx);
            input
        });
        ReportItem {
            fleet,
            request,
            title: title.into(),
            state: ReportState::Loading,
            text,
            scroll: ScrollHandle::new(),
        }
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn request(&self) -> &Request {
        &self.request
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn state(&self) -> &ReportState {
        &self.state
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn text(&self, cx: &App) -> String {
        self.text.read(cx).text().to_string()
    }

    /// Asks for the text (again).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.state = ReportState::Loading;
        cx.notify();
        let this: WeakEntity<Self> = cx.weak_entity();
        let request = self.request.clone();
        self.fleet.update(cx, |fleet, cx| {
            fleet.ask(
                request,
                move |reply, cx| {
                    // Closed meanwhile: nobody to tell.
                    let _ = this.update(cx, |report, cx| report.set_reply(reply, cx));
                },
                cx,
            )
        });
    }

    fn set_reply(&mut self, reply: Result<String, String>, cx: &mut Context<Self>) {
        match reply {
            Ok(text) => {
                let text = if text.trim().is_empty() {
                    "(nothing)".to_string()
                } else {
                    text
                };
                self.text.update(cx, |input, cx| {
                    input.set_text(&text, cx);
                    // Read from the top.
                    input.move_to_beginning(cx);
                });
                self.scroll.set_offset(gpui::Point::default());
                self.state = ReportState::Ready;
            }
            Err(why) => self.state = ReportState::Failed(why),
        }
        cx.emit(ItemEvent::UpdateTab);
        cx.notify();
    }

    fn icon(&self) -> IconName {
        match self.request {
            Request::Transcript { .. } => IconName::Chat,
            Request::QueueShow { .. } => IconName::ListTodo,
            _ => IconName::FileCode,
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.text.read(cx).text().lines().count();
        let (status, color) = match &self.state {
            ReportState::Loading => ("Loading…".to_string(), Color::Muted),
            ReportState::Ready => (format!("{lines} lines"), Color::Muted),
            ReportState::Failed(why) => (why.clone(), Color::Error),
        };
        let colors = cx.theme().colors();
        h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(colors.border_variant)
            .child(Icon::new(self.icon()).size(IconSize::Small).color(Color::Muted))
            .child(Label::new(self.title.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(status).size(LabelSize::Small).color(color).truncate()),
            )
            .child(
                IconButton::new("report-copy", IconName::Copy)
                    .icon_size(IconSize::Small)
                    .disabled(self.state != ReportState::Ready)
                    .tooltip(Tooltip::text("Copy All"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let text = this.text.read(cx).text().to_string();
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    })),
            )
            .child(
                IconButton::new("report-refresh", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .disabled(self.state == ReportState::Loading)
                    .tooltip(Tooltip::text("Ask Again"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
    }
}

impl EventEmitter<ItemEvent> for ReportItem {}

impl Focusable for ReportItem {
    /// The text's: the keys select and copy.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.text.focus_handle(cx)
    }
}

impl Item for ReportItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        self.title.clone()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        let color = match self.state {
            ReportState::Failed(_) => Color::Error,
            _ => Color::Muted,
        };
        Some(
            Icon::new(self.icon())
                .size(IconSize::Small)
                .color(color)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }

    fn can_split(&self) -> bool {
        true
    }

    /// A copy with the same text, not asked for again.
    fn clone_on_split(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Self>> {
        let (fleet, request, title) = (self.fleet.clone(), self.request.clone(), self.title.to_string());
        let (state, text) = (self.state.clone(), self.text.read(cx).text().to_string());
        Some(cx.new(|cx| {
            let mut copy = ReportItem::new(fleet, request, title, window, cx);
            copy.text.update(cx, |input, cx| {
                input.set_text(&text, cx);
                input.move_to_beginning(cx);
            });
            copy.state = state;
            copy
        }))
    }
}

impl Render for ReportItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        // The text is always drawn (empty until the reply), so the tab
        // can take the keyboard while it waits.
        let status = match &self.state {
            ReportState::Loading => Some(Label::new("Loading…").color(Color::Muted)),
            ReportState::Failed(why) => Some(Label::new(why.clone()).color(Color::Error)),
            ReportState::Ready => None,
        };
        let body = div()
            .id("report-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_4()
            .py_2()
            .children(status)
            .child(div().font_buffer(cx).text_ui_sm(cx).child(self.text.clone()));
        v_flex()
            .id("report")
            .size_full()
            .bg(colors.editor_background)
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .vertical_scrollbar_for(&self.scroll, window, cx),
            )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::actions::ShowTranscript;
    use crate::tests::init_test;
    use crate::workspace::Workspace;

    fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        init_test(cx);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let fleet = cx.new(FleetModel::demo);
            Workspace::new(fleet, None, window, cx)
        });
        cx.run_until_parked();
        (workspace, cx)
    }

    /// The reports open in any pane.
    fn reports(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Entity<ReportItem>> {
        w.read_with(cx, |w, cx| {
            w.panes()
                .iter()
                .flat_map(|p| {
                    p.read(cx)
                        .items()
                        .filter_map(|i| i.to_any().downcast::<ReportItem>().ok())
                        .collect::<Vec<_>>()
                })
                .collect()
        })
    }

    fn toasts(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
        w.read_with(cx, |w, cx| {
            w.toast_layer()
                .read(cx)
                .texts()
                .into_iter()
                .map(String::from)
                .collect()
        })
    }

    #[gpui::test]
    async fn a_transcript_opens_as_a_tab_with_the_reply(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        let fleet = w.read_with(cx, |w, _| w.fleet().clone());
        let (lead, task) = fleet.read_with(cx, |f, _| {
            let a = f.store.selected_agent().unwrap();
            (a.label(), a.task.clone().unwrap())
        });
        cx.dispatch_action(ShowTranscript);
        // Asked for, and loading until the reply comes.
        let report = reports(&w, cx).pop().expect("a report tab");
        cx.run_until_parked();
        report.read_with(cx, |r, cx| {
            assert_eq!(r.title().as_ref(), format!("transcript of {lead}"));
            assert!(matches!(r.request(), ainet_core::Request::Transcript { .. }));
            assert_eq!(r.state(), &ReportState::Ready);
            let text = r.text(cx);
            assert!(text.contains(&task), "{text}");
        });
        // It is the active tab, and the keys go to its text.
        let active = w.read_with(cx, |w, cx| {
            w.active_pane().read(cx).active_item().unwrap().item_id()
        });
        assert_eq!(active, report.entity_id());
        assert!(toasts(&w, cx).is_empty(), "no toast for a report");

        // Asking again reuses the tab.
        cx.dispatch_action(ShowTranscript);
        cx.run_until_parked();
        assert_eq!(reports(&w, cx).len(), 1);

        // The text can be selected and copied.
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        let copied = cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default();
        assert!(copied.contains(&task));
    }

    #[gpui::test]
    async fn specs_and_queue_items_open_as_reports_and_errors_say_why(cx: &mut TestAppContext) {
        let (w, cx) = demo(cx);
        // `/spec docs` and `/export nope` typed in Comms.
        let comms = w.update_in(cx, |w, window, cx| w.open_comms(window, cx));
        let compose = comms.read_with(cx, |c, _| c.compose().clone());
        cx.focus(&compose);
        cx.simulate_input("/spec docs");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let spec = reports(&w, cx).pop().expect("the spec");
        spec.read_with(cx, |r, cx| {
            assert_eq!(r.title().as_ref(), "spec docs");
            assert!(r.text(cx).contains("name: docs"), "{}", r.text(cx));
        });

        // Back to Comms, which the report covered.
        w.update_in(cx, |w, window, cx| w.open_comms(window, cx));
        cx.simulate_input("/export nope");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let failed = reports(&w, cx)
            .into_iter()
            .find(|r| r.read_with(cx, |r, _| r.title().as_ref() == "nope as RigSpec"))
            .expect("the export");
        failed.read_with(cx, |r, _| {
            assert_eq!(
                r.state(),
                &ReportState::Failed("no spec named nope in the demo".into())
            )
        });

        // The Queue's Show: the item in full, as `ainet queue show`.
        let panel = w
            .read_with(cx, |w, cx| w.bottom_panel::<crate::queue_panel::QueuePanel>(cx))
            .unwrap();
        panel.update(cx, |p, cx| p.select("q-2", cx));
        cx.update(|window, cx| {
            let focus = panel.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let item = reports(&w, cx)
            .into_iter()
            .find(|r| r.read_with(cx, |r, _| r.title().as_ref() == "q-2"))
            .expect("the queue item");
        item.read_with(cx, |r, cx| {
            let text = r.text(cx);
            assert!(text.starts_with("q-2  pending  screen-001"), "{text}");
            assert!(text.contains("from orch-lead@docs to dev-ui@docs"), "{text}");
        });
        assert!(toasts(&w, cx).is_empty(), "{:?}", toasts(&w, cx));
    }
}
