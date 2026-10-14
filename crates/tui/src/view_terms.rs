//! The Terminals view: every agent's live terminal at once, in rows and
//! columns. Arrows move the focus, Enter zooms in and types into it (Ctrl+]
//! leaves), `+`/`-`/`=` change the columns, Ctrl+R picks one rig.

use ainet_core::AgentState;
use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::App;
use crate::sprite::{self, Size};
use crate::term;
use crate::theme::{c, mix, pulse};
use crate::view::{chip, glyph, panel};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.pal.clone();
    let ids = app.grid_ids();
    app.areas.tiles.clear();
    if ids.is_empty() {
        let block = panel(&pal, "Terminals", true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = sprite::critter(&pal, "no-terminals", app.tick, false, Size::Big);
        lines.push(Line::raw(""));
        lines.push(Line::styled("No terminals to show yet.", pal.text()));
        lines.push(Line::styled(
            "/up starter launches a team here · /spawn claude starts one agent",
            pal.style(pal.code),
        ));
        if app.grid.rig.is_some() {
            lines.push(Line::styled(
                "(showing one rig: Ctrl+R cycles back to all)",
                pal.muted(),
            ));
        }
        f.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            inner.inner(Margin::new(0, 2)),
        );
        return;
    }
    let shown: Vec<String> = match (&app.grid.focus, app.grid.zoom) {
        (Some(focus), true) if ids.contains(focus) => vec![focus.clone()],
        _ => ids.clone(),
    };
    let cols = term::grid_cols(shown.len(), area, app.grid.cols);
    let rects = term::tiles(shown.len(), cols, area);
    for (id, rect) in shown.iter().zip(rects) {
        tile(f, app, id, rect);
    }
}

fn tile(f: &mut Frame, app: &mut App, id: &str, rect: Rect) {
    let pal = app.pal.clone();
    let Some(agent) = app.agents.get(id).cloned() else {
        return;
    };
    let focused = app.grid.focus.as_deref() == Some(id);
    let typing = focused && app.grid.typing;
    let hue = pal.hue(id);
    let border = if typing {
        mix(pal.attn, pal.bright, pulse(app.tick, 12) * 0.5)
    } else if focused {
        hue
    } else if agent.state == AgentState::NeedsYou {
        mix(pal.attn, pal.surface2(), 1.0 - pulse(app.tick, 10))
    } else {
        pal.surface2()
    };
    // Name, then the role as far as it fits; the seat goes on the bottom edge.
    let room = rect.width.saturating_sub(4) as usize;
    let mut title = vec![Span::raw(" "), glyph(&pal, agent.state, app.tick), Span::raw(" ")];
    title.push(Span::styled(
        agent.name.clone(),
        if focused { pal.bold() } else { pal.text() },
    ));
    let used = 3 + agent.name.chars().count();
    let mail = if agent.unread > 0 {
        format!(" ✉ {}", agent.unread)
    } else {
        String::new()
    };
    if let Some(role) = &agent.role {
        let left = room.saturating_sub(used + 4 + mail.chars().count());
        if left >= 3 {
            let role: String = if role.chars().count() > left {
                format!("{}…", role.chars().take(left - 1).collect::<String>())
            } else {
                role.clone()
            };
            title.push(Span::raw(" "));
            title.push(chip(
                &role,
                if focused { hue } else { mix(hue, pal.bg, 0.45) },
                pal.bg,
            ));
        }
    }
    if !mail.is_empty() {
        title.push(Span::styled(mail, pal.style(pal.attn)));
    }
    title.push(Span::raw(" "));
    let seat = Line::from(vec![Span::styled(
        format!(
            " {} ",
            agent.seat.clone().unwrap_or_else(|| agent.runtime.label().into())
        ),
        pal.style(if focused { mix(hue, pal.fg, 0.4) } else { pal.muted }),
    )])
    .left_aligned();
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_type(if typing {
            BorderType::Thick
        } else if focused {
            BorderType::Double
        } else {
            BorderType::Rounded
        })
        .border_style(Style::new().fg(c(border)))
        .title(Line::from(title))
        .title_bottom(seat);
    if typing {
        block = block.title_bottom(
            Line::from(vec![Span::styled(
                " typing · Ctrl+] to leave ",
                Style::new()
                    .fg(c(pal.bg))
                    .bg(c(pal.attn))
                    .add_modifier(Modifier::BOLD),
            )])
            .centered(),
        );
    } else if focused && rect.width > 60 {
        block = block.title_bottom(Line::styled(" Enter type · ^Z zoom ", pal.muted()).right_aligned());
    }
    let inner = block.inner(rect);
    app.areas
        .tiles
        .push((rect, id.to_string(), (inner.height, inner.width)));
    match app.terms.get(id) {
        Some(parser) => {
            let mut cursor = Cursor::default();
            if !typing {
                cursor.hide();
            }
            let screen = PseudoTerminal::new(parser.screen()).block(block).cursor(cursor);
            f.render_widget(screen, rect);
        }
        None => {
            f.render_widget(block, rect);
            let mut lines = sprite::critter(
                &pal,
                id,
                app.tick,
                agent.state == AgentState::Working,
                Size::Small,
            );
            lines.push(Line::styled("connecting to its terminal…", pal.muted()));
            f.render_widget(
                Paragraph::new(lines).alignment(Alignment::Center),
                inner.inner(Margin::new(0, 1)),
            );
        }
    }
}
