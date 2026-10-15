//! The Comms view: what agents say to each other (direct messages, replies,
//! the rig chatrooms) as one stream, the thread of the selected message,
//! and the queue of owned work. Typing a line answers the selected message.

use ainet_core::{QItem, QState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{HighlightSpacing, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Entry};
use crate::theme::{Palette, Styles as _, c, mix};
use crate::view::{age, chip, panel};

fn hue_of(app: &App, id: &str) -> [u8; 3] {
    if id == "operator" {
        app.pal.bright
    } else {
        app.pal.hue(id)
    }
}

fn clock(ms: i64) -> String {
    // Minutes and seconds ago read better than wall time in a busy stream.
    age(ms)
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.pal.clone();
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area);
    let [queue_area, thread_area] =
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(right);

    // The stream.
    let entries = app.entries();
    let selected = app.comms_state.selected();
    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let item = match e {
                Entry::Message(m) => {
                    let mut head = vec![
                        Span::styled(format!("{:>4} ", clock(m.created)), pal.muted()),
                        Span::styled(m.from_label.clone(), pal.style(hue_of(app, &m.from))),
                        Span::styled(" → ", pal.muted()),
                        Span::styled(m.to_label.clone(), pal.style(hue_of(app, &m.to))),
                        Span::styled(format!("  #{}", m.id), pal.muted()),
                    ];
                    if let Some(r) = m.reply_to {
                        head.push(Span::styled(format!(" ↳ #{r}"), pal.dim()));
                    }
                    if m.held {
                        head.push(Span::raw(" "));
                        head.push(chip("held", pal.attn, pal.bg));
                    } else if m.read.is_none() && m.to != "operator" {
                        head.push(Span::styled(" ●", pal.style(pal.info)));
                    }
                    let body = first_lines(&m.body, 2);
                    let mut lines = vec![Line::from(head)];
                    lines.extend(
                        body.into_iter()
                            .map(|b| Line::styled(format!("      {b}"), pal.text())),
                    );
                    lines.push(Line::raw(""));
                    ListItem::new(lines)
                }
                Entry::Chat(ch) => ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(format!("{:>4} ", clock(ch.created)), pal.muted()),
                        chip(
                            &format!("#{} {}", ch.topic, ch.rig),
                            mix(pal.info, pal.bg, 0.35),
                            pal.bg,
                        ),
                        Span::raw(" "),
                        Span::styled(ch.sender.clone(), pal.bold()),
                    ]),
                    Line::styled(format!("      {}", first_lines(&ch.body, 1).join(" ")), pal.dim()),
                    Line::raw(""),
                ]),
            };
            if selected == Some(i) {
                item.style(Style::new().bg(c(pal.sel)))
            } else {
                item
            }
        })
        .collect();
    let empty = items.is_empty();
    let list = List::new(items)
        .block(panel(&pal, "Conversations", true))
        .highlight_symbol(Line::styled("▌", pal.style(pal.bright)))
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(list, left, &mut app.comms_state);
    if empty {
        let hint = Paragraph::new(vec![
            Line::styled("Nobody has said anything yet.", pal.dim()),
            Line::raw(""),
            Line::styled(
                "Agents talk with the ainet tools send_message, ask_agent and chat_send;",
                pal.muted(),
            ),
            Line::styled(
                "you can join in with @seat text, /broadcast and /chat.",
                pal.muted(),
            ),
        ])
        .wrap(Wrap { trim: true });
        f.render_widget(
            hint,
            Rect {
                x: left.x + 3,
                y: left.y + 2,
                width: left.width.saturating_sub(6),
                height: 5,
            },
        );
    }

    // The queue.
    let mut items: Vec<&QItem> = app.store.queue.values().collect();
    let order = |s: QState| match s {
        QState::InProgress => 0,
        QState::Blocked => 1,
        QState::Pending => 2,
        QState::Done => 3,
        QState::Canceled => 4,
    };
    items.sort_by_key(|q| (order(q.state), std::cmp::Reverse(q.updated)));
    let lines: Vec<Line> = items
        .iter()
        .take(queue_area.height.saturating_sub(2) as usize)
        .map(|q| {
            let (bg, fg) = state_colors(&pal, q.state);
            Line::from(vec![
                Span::styled(format!("{:<5}", q.id), pal.muted()),
                chip(&format!("{:<11}", q.state.label()), bg, fg),
                Span::styled(
                    format!(
                        " {:<18} ",
                        short(q.owner.as_deref().unwrap_or(&q.destination), 18)
                    ),
                    pal.dim(),
                ),
                Span::styled(
                    q.summary.clone(),
                    if q.state.is_open() {
                        pal.text()
                    } else {
                        pal.muted()
                    },
                ),
            ])
        })
        .collect();
    let open = app.store.queue.values().filter(|q| q.state.is_open()).count();
    let title = format!("Queue · {open} open");
    let queue = Paragraph::new(if lines.is_empty() {
        vec![Line::styled(
            "No owned work yet. /queue <seat> <summary> creates some.",
            pal.muted(),
        )]
    } else {
        lines
    })
    .block(panel(&pal, &title, false));
    f.render_widget(queue, queue_area);

    // The selected thread, whole.
    let thread_lines: Vec<Line> = match app.selected_entry() {
        Some(Entry::Message(m)) => {
            let mut out = Vec::new();
            for x in app.store.messages.iter().filter(|x| x.thread == m.thread) {
                out.push(Line::from(vec![
                    Span::styled(format!("#{} ", x.id), pal.muted()),
                    Span::styled(x.from_label.clone(), pal.style(hue_of(app, &x.from))),
                    Span::styled(" → ", pal.muted()),
                    Span::styled(x.to_label.clone(), pal.style(hue_of(app, &x.to))),
                ]));
                out.extend(x.body.lines().map(|l| Line::styled(format!("  {l}"), pal.text())));
                out.push(Line::raw(""));
            }
            out.push(Line::styled(
                "Type a line and Enter to answer the selected message.",
                pal.muted(),
            ));
            out
        }
        Some(Entry::Chat(ch)) => vec![
            Line::from(vec![
                Span::styled(ch.sender.clone(), pal.bold()),
                Span::styled(format!(" in #{} ({})", ch.topic, ch.rig), pal.muted()),
            ]),
            Line::styled(ch.body.clone(), pal.text()),
            Line::raw(""),
            Line::styled("Type a line and Enter to post in this chatroom.", pal.muted()),
        ],
        None => vec![Line::styled("Select a message to see its thread.", pal.muted())],
    };
    f.render_widget(
        Paragraph::new(thread_lines)
            .wrap(Wrap { trim: false })
            .block(panel(&pal, "Thread", false)),
        thread_area,
    );
}

fn state_colors(pal: &Palette, s: QState) -> ([u8; 3], [u8; 3]) {
    match s {
        QState::InProgress => (pal.ok, pal.bg),
        QState::Blocked => (pal.attn, pal.bg),
        QState::Pending => (pal.info, pal.bg),
        QState::Done => (pal.surface2(), pal.fg_dim),
        QState::Canceled => (pal.surface2(), pal.muted),
    }
}

fn first_lines(s: &str, n: usize) -> Vec<String> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .map(|l| short(l.trim(), 110))
        .collect()
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    } else {
        s.to_string()
    }
}
