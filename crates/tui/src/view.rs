//! Drawing. Den's layout: a wordmark header with the fleet's counts, view
//! tabs, the view, an always-open command line and a key-hint footer.

use ainet_core::{Agent, AgentState, InboxItem, InboxKind, now_ms};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, HighlightSpacing, List, ListItem, Padding, Paragraph,
    Row as TableRow, Scrollbar, ScrollbarOrientation, ScrollbarState, Sparkline, Table, Wrap,
};

use crate::app::{App, Row, Tab};
use crate::sprite::{self, Size};
use crate::theme::{Palette, c, mix, pulse};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let pal = app.pal.clone();
    f.render_widget(Block::new().style(Style::new().bg(c(pal.bg)).fg(c(pal.fg))), area);

    let wide = area.width >= 72;
    let [header, body, input, footer] = Layout::vertical([
        Constraint::Length(if wide { 6 } else { 2 }),
        Constraint::Min(6),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, app, header, wide);
    app.areas.body = body;
    match app.tab {
        Tab::Fleet => draw_fleet(f, app, body),
        Tab::Terminals => crate::view_terms::draw(f, app, body),
        Tab::Comms => crate::view_comms::draw(f, app, body),
        Tab::Inbox => draw_inbox(f, app, body),
        Tab::Map => draw_map(f, app, body),
        Tab::Review => draw_review(f, app, body),
    }
    draw_input(f, app, input);
    draw_footer(f, app, footer);
    if app.help {
        draw_help(f, &pal, area);
    }
    if let Some(popup) = &app.popup {
        draw_popup(f, &pal, area, popup);
    }
}

/// A command's result, over everything; any key closes it.
fn draw_popup(f: &mut Frame, pal: &Palette, area: Rect, popup: &crate::app::Popup) {
    let lines = popup.text.lines().count() as u16;
    let widest = popup
        .text
        .lines()
        .map(|l| Span::raw(l).width())
        .max()
        .unwrap_or(20) as u16;
    let w = (widest + 6).clamp(40, area.width.saturating_sub(4));
    let h = (lines + 4).clamp(6, area.height.saturating_sub(4));
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(pal.style(pal.info))
        .title(Span::styled(format!(" {} ", popup.title), pal.bold()))
        .title_bottom(Line::styled(" ↑↓ scroll · any key closes ", pal.muted()).right_aligned())
        .style(Style::new().bg(c(pal.surface)))
        .padding(Padding::new(2, 2, 1, 0));
    f.render_widget(
        Paragraph::new(popup.text.clone())
            .style(pal.text())
            .block(block)
            .scroll((popup.scroll, 0)),
        r,
    );
}

// ---- small helpers -------------------------------------------------------

/// A rig's header row: name, status, seats running, open work.
fn rig_item(app: &App, name: &str, index: usize) -> ListItem<'static> {
    let pal = &app.pal;
    let Some(rig) = app.store.rigs.get(name) else {
        return ListItem::new("");
    };
    let running = rig
        .seats
        .iter()
        .filter(|s| s.status == ainet_core::SeatStatus::Running)
        .count();
    let color = match rig.status {
        ainet_core::RigStatus::Running => pal.ok,
        ainet_core::RigStatus::Launching => pal.info,
        ainet_core::RigStatus::Partial => pal.attn,
        ainet_core::RigStatus::Stopped => pal.muted,
    };
    let open = app
        .store
        .queue
        .values()
        .filter(|q| q.rig.as_deref() == Some(name) && q.state.is_open())
        .count();
    let mut lines = Vec::new();
    if index > 0 {
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(vec![
        Span::styled("◆ ", pal.style(color)),
        Span::styled(name.to_uppercase(), pal.bold()),
        Span::styled(
            format!("  {} · {running}/{} seats", rig.status.label(), rig.seats.len()),
            pal.dim(),
        ),
        Span::styled(
            if open > 0 {
                format!(" · {open} open")
            } else {
                String::new()
            },
            pal.muted(),
        ),
    ]));
    if let Some(s) = &rig.summary {
        lines.push(Line::styled(
            format!("  {}", s.lines().next().unwrap_or("")),
            pal.muted(),
        ));
    }
    ListItem::new(lines)
}

pub(crate) fn age(since_ms: i64) -> String {
    let s = ((now_ms() - since_ms) / 1000).max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h {}m", s / 3600, s / 60 % 60),
        _ => format!("{}d", s / 86_400),
    }
}

pub(crate) fn short_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

/// The state glyph: a spinner while working, a pulsing half moon when it
/// needs you.
pub(crate) fn glyph(pal: &Palette, state: AgentState, tick: u64) -> Span<'static> {
    match state {
        AgentState::Working => Span::styled(SPINNER[(tick % 10) as usize], pal.style(pal.ok)),
        AgentState::NeedsYou => Span::styled(
            "◐",
            pal.style(mix(pal.attn, pal.bright, pulse(tick, 10) * 0.6))
                .add_modifier(Modifier::BOLD),
        ),
        AgentState::Idle => Span::styled("○", pal.dim()),
        AgentState::Failed => Span::styled("✗", pal.style(pal.err)),
        AgentState::Gone => Span::styled("·", pal.muted()),
        AgentState::Unknown => Span::styled("◌", pal.dim()),
    }
}

pub(crate) fn state_style(pal: &Palette, state: AgentState, tick: u64) -> Style {
    match state {
        AgentState::NeedsYou => pal.style(mix(pal.attn, pal.bright, pulse(tick, 10) * 0.4)),
        AgentState::Working => pal.style(pal.ok),
        AgentState::Failed => pal.style(pal.err),
        AgentState::Gone => pal.muted(),
        _ => pal.dim(),
    }
}

pub(crate) fn panel<'a>(pal: &Palette, title: &'a str, focused: bool) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(pal.style(if focused { pal.muted } else { pal.surface2() }))
        .title(Span::styled(
            format!(" {title} "),
            if focused { pal.text() } else { pal.dim() },
        ))
        .padding(Padding::horizontal(1))
}

pub(crate) fn chip(text: &str, bg: [u8; 3], fg: [u8; 3]) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::new().bg(c(bg)).fg(c(fg)).add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn role_badge(pal: &Palette, a: &Agent) -> Span<'static> {
    match &a.role {
        Some(role) => chip(role, pal.hue(&a.id), pal.bg),
        None => Span::raw(""),
    }
}

pub(crate) fn meta(a: &Agent) -> String {
    let mut parts = vec![age(a.state_since), a.runtime.label().to_string()];
    if let Some(m) = &a.model {
        parts.push(m.clone());
    }
    if let Some(ctx) = a.context {
        parts.push(format!("ctx {:.0}%", ctx * 100.0));
    }
    parts.join(" · ")
}

// ---- header ----------------------------------------------------------------

fn draw_header(f: &mut Frame, app: &mut App, area: Rect, wide: bool) {
    let pal = &app.pal;
    let count = |s: AgentState| app.store.agents.values().filter(|a| a.state == s).count();
    let live = app.store.agents.values().filter(|a| a.state.is_live()).count();
    let needs = count(AgentState::NeedsYou);
    let mut fleet = vec![Span::styled(format!("{live} agents"), pal.dim())];
    if needs > 0 {
        fleet.push(Span::styled(" · ", pal.muted()));
        fleet.push(Span::styled(format!("{needs} need you"), pal.style(pal.attn)));
    }
    fleet.push(Span::styled(
        format!(
            " · {} working · {} idle",
            count(AgentState::Working),
            count(AgentState::Idle)
        ),
        pal.dim(),
    ));

    let waiting = app.store.inbox.iter().filter(|i| i.waiting).count();
    let mut links = vec![if waiting > 0 {
        Span::styled(format!("inbox {waiting} waiting"), pal.style(pal.attn))
    } else {
        Span::styled("inbox clear", pal.dim())
    }];
    let operator_up = app
        .store
        .agents
        .values()
        .any(|a| a.rig.as_deref() == Some("operator") && a.state.is_live());
    links.push(Span::styled(
        if operator_up {
            " · operator on (^O)"
        } else {
            " · operator off (^O starts it)"
        },
        pal.dim(),
    ));
    let rigs_up = app
        .store
        .rigs
        .values()
        .filter(|r| r.status != ainet_core::RigStatus::Stopped)
        .count();
    if !app.store.rigs.is_empty() {
        links.push(Span::styled(
            format!(" · {rigs_up}/{} rigs up", app.store.rigs.len()),
            pal.dim(),
        ));
    }

    let system = if app.is_demo() {
        Span::styled("demo fleet · nothing here is real", pal.muted())
    } else if app.store.connected {
        Span::styled("ainetd connected", pal.muted())
    } else {
        Span::styled("ainetd offline · retrying", pal.style(pal.err))
    };

    let tabs_row = if wide {
        let logo = sprite::wordmark(pal, app.tick);
        let rows = [fleet, links, vec![system]];
        let mut lines = vec![Line::raw("")];
        for (l, extra) in logo.into_iter().zip(rows) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(l.spans);
            spans.push(Span::raw("    "));
            spans.extend(extra);
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), area);
        Rect {
            y: area.y + 5,
            height: 1,
            ..area
        }
    } else {
        let mut spans = vec![Span::styled(" ainet  ", pal.bold())];
        spans.extend(fleet);
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { height: 1, ..area });
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        }
    };

    // The flash message sits top right, like Den's.
    if let Some(flash) = &app.flash {
        let style = pal.style(if flash.err { pal.err } else { pal.ok });
        let r = Rect {
            y: area.y + u16::from(wide),
            height: 1,
            ..area
        }
        .inner(Margin::new(2, 0));
        f.render_widget(
            Paragraph::new(Span::styled(flash.text.clone(), style)).alignment(Alignment::Right),
            r,
        );
    }

    // Tabs, with their hit boxes for the mouse.
    let mut x = tabs_row.x + 2;
    let mut hits = Vec::new();
    let mut spans = vec![Span::raw("  ")];
    for tab in Tab::ALL {
        let on = app.tab == tab;
        let style = if on {
            pal.bold().add_modifier(Modifier::UNDERLINED)
        } else {
            pal.dim()
        };
        let title = tab.title();
        spans.push(Span::styled(title, style));
        let mut w = title.chars().count() as u16;
        if tab == Tab::Inbox {
            let badge = if waiting > 0 {
                Some(chip(&waiting.to_string(), pal.attn, pal.bg))
            } else {
                let n = app.store.inbox.len();
                (n > 0).then(|| Span::styled(format!(" {n}"), pal.muted()))
            };
            if let Some(b) = badge {
                spans.push(Span::raw(" "));
                w += 1 + b.width() as u16;
                spans.push(b);
            }
            app.areas.inbox_tab = Rect {
                x,
                y: tabs_row.y,
                width: w,
                height: 1,
            };
        }
        hits.push((
            Rect {
                x,
                y: tabs_row.y,
                width: w,
                height: 1,
            },
            tab,
        ));
        spans.push(Span::raw("   "));
        x += w + 3;
    }
    app.areas.tabs = hits;
    f.render_widget(Paragraph::new(Line::from(spans)), tabs_row);
    let right = Line::from(vec![
        chip("^T map", pal.sel, pal.fg),
        Span::raw(" "),
        chip("Tab ⇥", pal.sel, pal.fg),
        Span::raw("  "),
    ]);
    f.render_widget(Paragraph::new(right).alignment(Alignment::Right), tabs_row);
}

// ---- Fleet -----------------------------------------------------------------

fn draw_fleet(f: &mut Frame, app: &mut App, area: Rect) {
    let show_details = area.width >= 96;
    let [list_area, details_area] = if show_details {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area)
    } else {
        [area, Rect::default()]
    };
    app.areas.details = details_area;
    let pal = app.pal.clone();

    let mut items = Vec::new();
    for (i, row) in app.store.rows.iter().enumerate() {
        match row {
            Row::Header(title) => {
                let mut lines = Vec::new();
                if i > 0 {
                    lines.push(Line::raw(""));
                }
                lines.push(Line::styled(title.clone(), pal.muted()));
                items.push(ListItem::new(lines));
            }
            Row::Rig(name) => items.push(rig_item(app, name, i)),
            Row::Seat { rig, address, prefix } => {
                let status = app
                    .store
                    .rigs
                    .get(rig)
                    .and_then(|r| r.seat(address))
                    .map(|s| (s.status.label(), s.role.clone().unwrap_or_default()))
                    .unwrap_or(("gone", String::new()));
                items.push(ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(prefix.clone(), pal.muted()),
                        Span::styled("◌ ", pal.muted()),
                        Span::styled(address.clone(), pal.dim()),
                        Span::styled(format!("  {}  {}", status.1, status.0), pal.muted()),
                    ]),
                    Line::styled("    no agent in this seat · /launch to start it", pal.muted()),
                ]));
            }
            Row::Agent { id, prefix } => match app.store.agents.get(id) {
                Some(a) => {
                    let mut item = agent_item(app, a, prefix);
                    if app.fleet_state.selected() == Some(i) {
                        item = item.style(Style::new().bg(c(pal.sel)));
                    }
                    items.push(item);
                }
                None => items.push(ListItem::new("")),
            },
        }
    }
    // Line-based scrolling: rows differ in height.
    let heights: Vec<usize> = items.iter().map(ListItem::height).collect();
    let total: usize = heights.iter().sum();
    let above: usize = heights.iter().take(app.fleet_state.selected().unwrap_or(0)).sum();
    let list = List::new(items)
        .block(panel(&pal, "Agents", true))
        .highlight_symbol(Line::styled("▌", pal.style(pal.bright)))
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(list, list_area, &mut app.fleet_state);
    let viewport = list_area.height.saturating_sub(2) as usize;
    if total <= viewport {
        if show_details {
            draw_details(f, app, details_area);
        }
        return;
    }
    let mut sb = ScrollbarState::new(total.saturating_sub(viewport))
        .position(above.min(total.saturating_sub(viewport)));
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(pal.muted())
            .track_style(pal.style(pal.surface2()))
            .begin_symbol(None)
            .end_symbol(None),
        list_area.inner(Margin::new(0, 1)),
        &mut sb,
    );
    if show_details {
        draw_details(f, app, details_area);
    }
}

fn agent_item(app: &App, a: &Agent, prefix: &str) -> ListItem<'static> {
    let pal = &app.pal;
    let cont = prefix.replace("├─ ", "│  ").replace("└─ ", "   ");
    let gone = a.state == AgentState::Gone;
    let name_style = if gone {
        pal.muted()
    } else if app.store.selected.as_deref() == Some(&a.id) {
        pal.bold()
    } else {
        pal.text()
    };
    let role = a.role.clone().unwrap_or_default();
    let kids = app
        .store
        .agents
        .values()
        .filter(|k| k.parent.as_deref() == Some(a.id.as_str()) && k.state.is_live())
        .count();

    let mut first = vec![
        Span::styled(prefix.to_string(), pal.muted()),
        glyph(pal, a.state, app.tick),
        Span::raw(" "),
        Span::styled(a.name.clone(), name_style),
        Span::raw("  "),
        Span::styled(
            role,
            if gone {
                pal.muted()
            } else {
                pal.style(pal.hue(&a.id))
            },
        ),
    ];
    if kids > 0 {
        first.push(Span::styled(format!("  directs {kids}"), pal.muted()));
    }
    let task = Line::from(vec![
        Span::styled(format!("{cont}  ▸ "), pal.muted()),
        Span::styled(a.task.clone().unwrap_or_else(|| "no task yet".into()), pal.dim()),
    ]);
    let meta = Line::from(vec![
        Span::styled(format!("{cont}  "), pal.muted()),
        Span::styled(meta(a), pal.muted()),
    ]);

    let Some(size) = app.critter_rows else {
        return ListItem::new(vec![Line::from(first), task, meta]);
    };
    // Critter rows: a small animated face beside the three lines.
    let face = if gone {
        sprite::ghost(pal, &a.id, size)
    } else {
        sprite::critter(pal, &a.id, app.tick, a.state == AgentState::Working, size)
    };
    let text = [
        Line::from(first[1..].to_vec()),
        Line::from(task.spans[1..].to_vec()),
        Line::from(meta.spans[1..].to_vec()),
        Line::raw(""),
    ];
    let lines: Vec<Line> = face
        .into_iter()
        .zip(text)
        .map(|(face, text)| {
            let mut spans = vec![Span::styled(cont.clone(), pal.muted())];
            spans.extend(face.spans);
            spans.push(Span::raw(" "));
            spans.extend(text.spans);
            Line::from(spans)
        })
        .collect();
    ListItem::new(lines)
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let pal = &app.pal;
    let block = panel(pal, "Details", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(a) = app.selected_agent() else {
        let mut lines = sprite::critter(pal, "empty-den", app.tick, false, Size::Big);
        lines.push(Line::raw(""));
        lines.push(Line::styled("No agents yet.", pal.text()));
        lines.push(Line::styled(
            "/spawn claude ~/code/project BACKEND DEV",
            pal.style(pal.code),
        ));
        f.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            inner.inner(Margin::new(0, 2)),
        );
        return;
    };
    let [title, hero, status, meters, facts, term] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(9),
        Constraint::Length(2),
        Constraint::Length(4),
        Constraint::Length(6),
        Constraint::Min(3),
    ])
    .areas(inner);

    // Name, role and chain of command.
    let mut chain = vec![Span::styled("you", pal.dim())];
    let mut cur = a.parent.clone();
    let mut up = Vec::new();
    while let Some(p) = cur.and_then(|p| app.store.agents.get(&p)) {
        up.push(p.name.clone());
        cur = p.parent.clone();
    }
    for name in up.into_iter().rev() {
        chain.push(Span::styled(" › ", pal.muted()));
        chain.push(Span::styled(name, pal.dim()));
    }
    chain.push(Span::styled(" › ", pal.muted()));
    chain.push(Span::styled(
        a.name.clone(),
        pal.style(pal.hue(&a.id)).add_modifier(Modifier::BOLD),
    ));
    let head = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(a.name.clone(), pal.bold()),
            Span::raw("  "),
            role_badge(pal, a),
        ]),
        Line::from(chain),
    ];
    f.render_widget(Paragraph::new(head), title);

    // The critter, big, with what it is doing beside it.
    let [face, info] = Layout::horizontal([Constraint::Length(20), Constraint::Min(10)]).areas(hero);
    let sprite = if a.state == AgentState::Gone {
        sprite::ghost(pal, &a.id, Size::Big)
    } else {
        sprite::critter(pal, &a.id, app.tick, a.state == AgentState::Working, Size::Big)
    };
    f.render_widget(Paragraph::new(sprite), face.inner(Margin::new(1, 0)));
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("▸ ", pal.muted()),
            Span::styled(a.task.clone().unwrap_or_else(|| "no task yet".into()), pal.text()),
        ]),
    ];
    if let Some(tool) = &a.last_tool {
        let (name, target) = tool.split_once("  ").unwrap_or((tool, ""));
        lines.push(Line::from(vec![
            Span::styled("↳ ", pal.muted()),
            Span::styled(name.to_string(), pal.bold()),
            Span::raw("  "),
            Span::styled(target.to_string(), pal.style(pal.code)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!(
            "{} · {}",
            a.model.clone().unwrap_or_else(|| a.runtime.label().into()),
            short_path(&a.cwd)
        ),
        pal.dim(),
    ));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), info);

    // Status, pulsing when it needs you.
    let st = Line::from(vec![
        Span::styled("status   ", pal.muted()),
        glyph(pal, a.state, app.tick),
        Span::raw(" "),
        Span::styled(a.state.label(), state_style(pal, a.state, app.tick)),
        Span::styled(format!(" · {}", age(a.state_since)), pal.dim()),
    ]);
    f.render_widget(Paragraph::new(st), status);

    // Context gauge and activity sparkline.
    let [ctx_area, act_area] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
        .spacing(3)
        .areas(meters);
    let [ctx_label, ctx_bar, ctx_note] = Layout::vertical([Constraint::Length(1); 3]).areas(ctx_area);
    f.render_widget(Paragraph::new(Span::styled("Context", pal.dim())), ctx_label);
    let ratio = app.gauge.clamp(0.0, 1.0);
    let fill = if ratio > 0.8 {
        pal.err
    } else if ratio > 0.6 {
        pal.attn
    } else {
        pal.bright
    };
    f.render_widget(
        Gauge::default()
            .ratio(ratio)
            .use_unicode(true)
            .gauge_style(Style::new().fg(c(fill)).bg(c(pal.surface2())))
            .label(Span::styled(format!("{:.0}%", ratio * 100.0), pal.text())),
        ctx_bar,
    );
    let note = if a.context.is_some() {
        "of the window in use"
    } else {
        "unknown until the first turn"
    };
    f.render_widget(Paragraph::new(Span::styled(note, pal.muted())), ctx_note);
    let [act_label, act_spark] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).areas(act_area);
    f.render_widget(
        Paragraph::new(Span::styled("Activity · 10 min", pal.dim())),
        act_label,
    );
    f.render_widget(
        Sparkline::default()
            .data(&a.activity)
            .style(pal.style(pal.hue(&a.id)))
            .bar_set(ratatui::symbols::bar::NINE_LEVELS),
        act_spark,
    );

    // Facts.
    let where_ = match &a.branch {
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
        ("where", where_),
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
    let table = Table::new(
        rows.into_iter().map(|(k, v)| {
            TableRow::new([
                Cell::from(Span::styled(k, pal.muted())),
                Cell::from(Span::styled(v, pal.text())),
            ])
        }),
        [Constraint::Length(9), Constraint::Min(10)],
    );
    f.render_widget(table, facts.inner(Margin::new(0, 0)));

    // A live look at the agent's terminal.
    let term_block = Block::new()
        .borders(Borders::TOP)
        .border_style(pal.style(pal.surface2()))
        .title(Span::styled(" terminal ", pal.dim()));
    let term_inner = term_block.inner(term);
    f.render_widget(term_block, term);
    let rows = app.store.screens.get(&a.id).cloned().unwrap_or_default();
    let visible = rows.len().saturating_sub(term_inner.height as usize);
    let lines: Vec<Line> = rows[visible..]
        .iter()
        .map(|r| Line::styled(r.clone(), pal.style(mix(pal.fg, pal.fg_dim, 0.4))))
        .collect();
    let text = if lines.is_empty() {
        Text::styled("no screen: this session runs outside ainetd", pal.muted())
    } else {
        Text::from(lines)
    };
    f.render_widget(Paragraph::new(text), term_inner);
}

// ---- Inbox -----------------------------------------------------------------

fn kind_chip(pal: &Palette, item: &InboxItem) -> Span<'static> {
    let (bg, label) = match item.kind {
        InboxKind::Permission => (pal.attn, "PERMISSION"),
        InboxKind::Plan => (pal.info, "PLAN"),
        InboxKind::Question => (pal.code, "QUESTION"),
        InboxKind::Finished => (pal.surface2(), "FINISHED"),
        InboxKind::Failed => (pal.err, "FAILED"),
    };
    let fg = if item.kind == InboxKind::Finished {
        pal.fg_dim
    } else {
        pal.bg
    };
    let bg = if item.waiting || item.kind == InboxKind::Finished {
        bg
    } else {
        mix(bg, pal.bg, 0.55)
    };
    chip(label, bg, fg)
}

fn draw_inbox(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.pal.clone();
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
    let selected = app.inbox_state.selected();
    let items: Vec<ListItem> = app
        .store
        .inbox
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let agent = app.store.agents.get(&it.agent);
            let name = agent
                .map(|a| a.name.clone())
                .unwrap_or_else(|| it.agent.chars().take(8).collect());
            let mark = if it.waiting {
                Span::styled(
                    "◐ ",
                    pal.style(mix(pal.attn, pal.bright, pulse(app.tick, 10) * 0.6)),
                )
            } else {
                Span::styled(
                    if it.kind == InboxKind::Failed {
                        "✗ "
                    } else {
                        "✓ "
                    },
                    pal.muted(),
                )
            };
            ListItem::new(vec![
                Line::from(vec![
                    mark,
                    kind_chip(&pal, it),
                    Span::raw("  "),
                    Span::styled(name, pal.style(pal.hue(&it.agent))),
                    Span::styled(format!("  {}", age(it.created)), pal.muted()),
                ]),
                Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        it.summary.clone(),
                        if it.waiting { pal.text() } else { pal.dim() },
                    ),
                ]),
                Line::raw(""),
            ])
            .style(if selected == Some(i) {
                Style::new().bg(c(pal.sel))
            } else {
                Style::new()
            })
        })
        .collect();
    let empty = items.is_empty();
    let list = List::new(items)
        .block(panel(&pal, "Inbox", true))
        .highlight_symbol(Line::styled("▌", pal.style(pal.bright)))
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(list, list_area, &mut app.inbox_state);
    if empty {
        let mut lines = sprite::critter(&pal, "inbox-zero", app.tick, false, Size::Big);
        lines.push(Line::raw(""));
        lines.push(Line::styled("Inbox zero. Nothing needs you.", pal.dim()));
        f.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            list_area.inner(Margin::new(1, 3)),
        );
    }

    let block = panel(&pal, "Request", false);
    let inner = block.inner(detail_area);
    f.render_widget(block, detail_area);
    let Some(it) = app.inbox_state.selected().and_then(|i| app.store.inbox.get(i)) else {
        return;
    };
    let agent = app.store.agents.get(&it.agent);
    let [who, body, actions] =
        Layout::vertical([Constraint::Length(5), Constraint::Min(3), Constraint::Length(3)]).areas(inner);
    let [face, text] = Layout::horizontal([Constraint::Length(10), Constraint::Min(10)]).areas(who);
    let working = agent.is_some_and(|a| a.state == AgentState::Working);
    f.render_widget(
        Paragraph::new(sprite::critter(&pal, &it.agent, app.tick, working, Size::Small)),
        face.inner(Margin::new(1, 0)),
    );
    let mut lines = vec![Line::from(vec![
        Span::styled(agent.map(|a| a.name.clone()).unwrap_or_default(), pal.bold()),
        Span::raw("  "),
        agent.map(|a| role_badge(&pal, a)).unwrap_or_default(),
    ])];
    lines.push(Line::from(vec![
        kind_chip(&pal, it),
        Span::styled(format!("  {} ago", age(it.created)), pal.muted()),
    ]));
    lines.push(Line::styled(it.summary.clone(), pal.text()));
    f.render_widget(Paragraph::new(lines), text);

    let code = Block::new()
        .borders(Borders::LEFT)
        .border_type(BorderType::Thick)
        .border_style(pal.style(pal.surface2()))
        .padding(Padding::left(1));
    let detail: Vec<Line> = it
        .detail
        .lines()
        .map(|l| {
            let style = match l.chars().next() {
                Some('+') => pal.style(pal.ok),
                Some('-') => pal.style(pal.err),
                Some('@') => pal.style(pal.info),
                _ => pal.style(pal.code),
            };
            Line::styled(l.to_string(), style)
        })
        .collect();
    f.render_widget(
        Paragraph::new(detail).block(code).wrap(Wrap { trim: false }),
        body.inner(Margin::new(0, 1)),
    );

    let mut lines = vec![Line::raw("")];
    if it.waiting && it.kind == InboxKind::Question {
        for (i, o) in it.options.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), pal.style(pal.attn)),
                Span::styled(o.clone(), pal.text()),
            ]));
        }
        let how = if it.multi_select {
            "type numbers (\"1 3\") or your own answer, then Enter"
        } else {
            "type a number or your own answer, then Enter"
        };
        lines.push(Line::from(vec![
            chip("answer", pal.attn, pal.bg),
            Span::styled(format!("  {how} · ^N leaves it to the terminal"), pal.muted()),
        ]));
    } else if it.waiting {
        lines.push(Line::from(vec![
            chip("^Y allow", pal.ok, pal.bg),
            Span::raw("  "),
            chip("^A always", mix(pal.ok, pal.info, 0.5), pal.bg),
            Span::raw("  "),
            chip("^N deny", pal.err, pal.bg),
            Span::styled("   or answer in the agent's own terminal", pal.muted()),
        ]));
    } else if it.kind == InboxKind::Question {
        lines.push(Line::styled(
            "a message for you: answer it from Comms (select it, type, Enter)",
            pal.muted(),
        ));
    } else {
        lines.push(Line::styled("nothing to answer", pal.muted()));
    }
    let rows = lines.len() as u16;
    let actions = Rect {
        y: actions.bottom().saturating_sub(rows),
        height: rows,
        ..actions
    };
    f.render_widget(Paragraph::new(lines), actions);
}

/// A parent → child link on the map: from, to, and the child.
type Edge<'a> = ((f64, f64), (f64, f64), &'a Agent);

// ---- Map -------------------------------------------------------------------

fn draw_map(f: &mut Frame, app: &App, area: Rect) {
    let pal = &app.pal;
    let block = panel(pal, "Map · who directs whom", true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let live: Vec<&Agent> = app.store.agents.values().filter(|a| a.state.is_live()).collect();
    if live.is_empty() || inner.width < 20 || inner.height < 8 {
        return;
    }
    // Layer by depth in the tree; spread each layer across the width.
    let depth = |a: &Agent| {
        let mut d = 0;
        let mut cur = a.parent.clone();
        while let Some(p) = cur
            .and_then(|p| app.store.agents.get(&p))
            .filter(|p| p.state.is_live())
        {
            d += 1;
            cur = p.parent.clone();
        }
        d
    };
    let max_depth = live.iter().map(|a| depth(a)).max().unwrap_or(0);
    let mut pos: std::collections::HashMap<String, (f64, f64)> = std::collections::HashMap::new();
    for d in 0..=max_depth {
        let mut layer: Vec<&&Agent> = live.iter().filter(|a| depth(a) == d).collect();
        layer.sort_by_key(|a| (a.parent.clone(), a.started_at));
        let n = layer.len() as f64;
        for (i, a) in layer.iter().enumerate() {
            let x = (i as f64 + 0.5) / n * 100.0;
            let y = if max_depth == 0 {
                55.0
            } else {
                82.0 - d as f64 * (64.0 / max_depth as f64)
            };
            pos.insert(a.id.clone(), (x, y));
        }
    }
    let edges: Vec<Edge> = live
        .iter()
        .filter_map(|a| Some((*pos.get(a.parent.as_ref()?)?, *pos.get(&a.id)?, *a)))
        .collect();
    let tick = app.tick;
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, 100.0])
        .y_bounds([0.0, 100.0])
        .paint(|ctx| {
            for (from, to, _) in &edges {
                ctx.draw(&CanvasLine {
                    x1: from.0,
                    y1: from.1 - 6.0,
                    x2: to.0,
                    y2: to.1 + 6.0,
                    color: c(mix(pal.surface2(), pal.muted, 0.5)),
                });
            }
            ctx.layer();
            // Messages in flight: work flows down to busy agents, questions
            // flow up from the ones that need you.
            for (from, to, child) in &edges {
                let (upward, color) = match child.state {
                    AgentState::Working => (false, pal.hue(&child.id)),
                    AgentState::NeedsYou => (true, pal.attn),
                    _ => continue,
                };
                // Along the drawn edge, from under the parent to over the child.
                let (x0, y0, x1, y1) = (from.0, from.1 - 6.0, to.0, to.1 + 6.0);
                let at = |t: f64| (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
                let phase = (tick + u64::from(ainet_critter::hash(&child.id)) % 40) % 40;
                for k in 0..3 {
                    let mut t = ((phase + k * 13) % 40) as f64 / 40.0;
                    if upward {
                        t = 1.0 - t;
                    }
                    // A fading trail behind each message, then the message itself.
                    for (lag, fade) in [(0.06, 0.75), (0.03, 0.45)] {
                        let tt = if upward {
                            (t + lag).min(1.0)
                        } else {
                            (t - lag).max(0.0)
                        };
                        ctx.draw(&Points {
                            coords: &[at(tt)],
                            color: c(mix(color, pal.bg, fade)),
                        });
                    }
                    let (x, y) = at(t);
                    let blob: Vec<(f64, f64)> =
                        [(-0.35, 0.0), (0.0, 0.0), (0.35, 0.0), (0.0, 0.7), (0.0, -0.7)]
                            .iter()
                            .map(|(dx, dy)| (x + dx, y + dy))
                            .collect();
                    ctx.draw(&Points {
                        coords: &blob,
                        color: c(color),
                    });
                }
            }
        });
    f.render_widget(canvas, inner);

    // Critters on top of the canvas, with their names.
    for a in &live {
        let Some((x, y)) = pos.get(&a.id) else { continue };
        let col = inner.x + ((x / 100.0) * f64::from(inner.width.saturating_sub(1))) as u16;
        let row = inner.y + (((100.0 - y) / 100.0) * f64::from(inner.height.saturating_sub(1))) as u16;
        let face = Rect {
            x: col.saturating_sub(4),
            y: row.saturating_sub(2),
            width: 8,
            height: 4,
        }
        .intersection(inner);
        f.render_widget(
            Paragraph::new(sprite::critter(
                pal,
                &a.id,
                tick,
                a.state == AgentState::Working,
                Size::Small,
            )),
            face,
        );
        let label_w = 22u16;
        let label = Rect {
            x: col.saturating_sub(label_w / 2),
            y: row + 2,
            width: label_w,
            height: 2,
        }
        .intersection(inner);
        let name = Line::from(vec![
            glyph(pal, a.state, tick),
            Span::raw(" "),
            Span::styled(
                a.name.clone(),
                if app.store.selected.as_deref() == Some(&a.id) {
                    pal.bold()
                } else {
                    pal.text()
                },
            ),
        ]);
        let role = Line::styled(a.role.clone().unwrap_or_default(), pal.style(pal.hue(&a.id)));
        f.render_widget(
            Paragraph::new(vec![name, role]).alignment(Alignment::Center),
            label,
        );
    }
    let legend = Line::from(vec![
        Span::styled("⠒⠒ directs   ", pal.muted()),
        Span::styled("⠂ ", pal.style(pal.ok)),
        Span::styled("work flowing down   ", pal.muted()),
        Span::styled("⠂ ", pal.style(pal.attn)),
        Span::styled("a question flowing up", pal.muted()),
    ]);
    f.render_widget(
        Paragraph::new(legend),
        Rect {
            y: inner.bottom().saturating_sub(1),
            height: 1,
            ..inner
        },
    );
}

// ---- Review ----------------------------------------------------------------

fn draw_review(f: &mut Frame, app: &App, area: Rect) {
    let pal = &app.pal;
    let block = panel(pal, "Review", true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines = vec![Line::raw("")];
    lines.extend(sprite::critter(pal, "review-nap", app.tick, false, Size::Big));
    lines.push(Line::raw(""));
    lines.push(Line::styled("Review arrives in milestone 3", pal.bold()));
    lines.push(Line::styled(
        "each agent's diff, every line in the colour of the agent that wrote it,",
        pal.dim(),
    ));
    lines.push(Line::styled("with comments that go back to the agent", pal.dim()));
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

// ---- command line, footer, help ------------------------------------------

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let pal = &app.pal;
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(pal.style(pal.surface2()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Who a plain line goes to depends on the view.
    let target = match app.tab {
        Tab::Terminals => app.grid.focus.as_ref().and_then(|f| app.store.agents.get(f)),
        _ => app.selected_agent(),
    };
    let accent = target.map(|a| pal.hue(&a.id)).unwrap_or(pal.fg);
    let prompt = Span::styled(" › ", pal.style(accent).add_modifier(Modifier::BOLD));
    let line = if app.input.is_empty() {
        let to = target.map(|a| a.label()).unwrap_or_else(|| "an agent".into());
        let hint = match app.tab {
            Tab::Comms => match app.selected_entry() {
                Some(crate::app::Entry::Message(m)) => format!(
                    "Answer #{} from {} · @seat text to message anyone · /chat · /queue",
                    m.id, m.from_label
                ),
                Some(crate::app::Entry::Chat(ch)) => format!(
                    "Post in #{} ({}) · @seat text to message anyone",
                    ch.topic, ch.rig
                ),
                None => "@seat text to message anyone · /broadcast · /chat · /queue".into(),
            },
            Tab::Inbox
                if app
                    .inbox_state
                    .selected()
                    .and_then(|i| app.store.inbox.get(i))
                    .is_some_and(|i| i.waiting && i.kind == InboxKind::Question) =>
            {
                "Type an option's number (or your own answer) and Enter".into()
            }
            Tab::Terminals => {
                format!("Message {to} · Enter on an empty line types into its terminal · /up starter")
            }
            _ => format!("Message {to}, or @seat another · Ctrl+O operator · /up · /help"),
        };
        Line::from(vec![prompt, Span::styled(hint, pal.muted())])
    } else {
        Line::from(vec![prompt, Span::styled(app.input.clone(), pal.text())])
    };
    f.render_widget(Paragraph::new(line), inner);
    if !app.help && app.quitting.is_none() {
        let before: String = app.input.chars().take(app.cursor).collect();
        f.set_cursor_position((inner.x + 3 + Span::raw(before).width() as u16, inner.y));
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let pal = &app.pal;
    let keys: &[(&str, &str)] = match app.tab {
        _ if app.grid.typing => &[
            ("Ctrl+]", "leave the terminal"),
            ("", "every other key goes to the agent"),
        ],
        Tab::Terminals => &[
            ("←↑↓→", "focus"),
            ("Enter", "type into it"),
            ("text+Enter", "message it"),
            ("^Z", "zoom"),
            ("+ - =", "columns"),
            ("^R", "rig"),
            ("Tab", "view"),
            ("?", "help"),
        ],
        Tab::Comms => &[
            ("↑↓", "select"),
            ("text+Enter", "answer it"),
            ("@seat text", "message"),
            ("/chat", "post"),
            ("/queue", "assign"),
            ("Tab", "view"),
            ("?", "help"),
        ],
        Tab::Inbox => &[
            ("↑↓", "select"),
            ("^Y ^A ^N", "allow always deny"),
            ("1-9 Enter", "answer a question"),
            ("Tab", "view"),
            ("?", "help"),
        ],
        _ => &[
            ("↑↓", "select"),
            ("Tab", "view"),
            ("Enter", "message"),
            ("^Y ^N", "answer"),
            ("^G", "terminals"),
            ("^T", "map"),
            ("/up", "launch a team"),
            ("?", "help"),
            ("^C", "quit"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, d) in keys {
        spans.push(Span::styled(*k, pal.text()));
        spans.push(Span::styled(format!(" {d}   "), pal.muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(f: &mut Frame, pal: &Palette, area: Rect) {
    let w = 112.min(area.width.saturating_sub(4));
    let h = 28.min(area.height.saturating_sub(2));
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(pal.style(pal.info))
        .title(Span::styled(" keys & commands ", pal.bold()))
        .title_bottom(Line::styled(" any key closes ", pal.muted()).right_aligned())
        .style(Style::new().bg(c(pal.surface)))
        .padding(Padding::new(2, 2, 1, 0));
    let rows = [
        (
            "Tab  Shift+Tab",
            "views: Fleet, Terminals, Comms, Inbox, Map, Review",
        ),
        ("↑ ↓  PgUp PgDn", "select an agent, message or inbox item"),
        ("Enter", "message the selected agent (Terminals: type into it)"),
        (
            "@seat text",
            "message any agent: @dev.review, @dev-review@starter, @Mossy",
        ),
        ("^Y  ^A  ^N", "allow / always allow / deny the request on screen"),
        ("^G  ^T  ^E", "terminals grid / map / critters in the list"),
        (
            "Terminals",
            "←↑↓→ focus · Enter type (Ctrl+] leaves) · ^Z zoom · + - = columns · ^R rig",
        ),
        ("^U ^W  ^C", "clear the line / delete a word / quit"),
        ("", ""),
        (
            "/up <spec|name> [dir]",
            "launch a team (an OpenRig spec, or starter, workshop, factory…)",
        ),
        ("/plan <spec|name>", "what /up would launch"),
        (
            "/down /restore /snapshot",
            "stop with a snapshot, bring back, snapshot (current rig)",
        ),
        (
            "/grow <pod> <member> [rt] [ROLE]",
            "add a seat; /shrink <pod>, /remove <seat>",
        ),
        (
            "/launch /relaunch /continue",
            "start, restart fresh, nudge a seat",
        ),
        ("/broadcast /chat <text>", "to every seat / the rig's chatroom"),
        (
            "/queue <seat> <summary>",
            "give work an owner; /watchdog <seat> <min>",
        ),
        (
            "/discover  /adopt <id> <rig> <pod> <m>",
            "agents outside ainet, take one into a seat",
        ),
        (
            "/ps /specs /spec /export /capture",
            "rigs, library, a spec, a rig as YAML, a terminal",
        ),
        (
            "/guard <seat> on|off",
            "hold automatic messages while you type there",
        ),
        (
            "/spawn [claude|codex|shell] [dir]",
            "one agent outside any rig; /close [name]",
        ),
    ];
    let table = Table::new(
        rows.into_iter().map(|(k, d)| {
            TableRow::new([
                Cell::from(Span::styled(k, pal.style(pal.code))),
                Cell::from(Span::styled(d, pal.dim())),
            ])
        }),
        [Constraint::Length(40), Constraint::Min(10)],
    )
    .block(block);
    f.render_widget(table, popup);
}
