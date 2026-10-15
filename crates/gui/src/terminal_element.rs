//! Paints an agent's terminal, after Zed's `terminal_view::TerminalElement`:
//! measures a cell of the terminal font (`terminal.font_family`,
//! `font_size`, `line_height`), fits rows × columns in its bounds and tells
//! the [`TerminalItem`] (which asks the agent's pane to take that size),
//! then paints the screen ainet keeps for the agent: backgrounds merged
//! along rows, text shaped per run of one style, the selection and the
//! cursor. What to paint is worked out in `terminal_layout`.

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, Entity, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, ShapedLine, Size,
    Style, TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point, px, relative, size,
};
use theme::ActiveTheme;

use crate::settings::{self, CursorShape};
use crate::terminal_item::TerminalItem;
use crate::terminal_layout::{self, Palette};

/// Thickness of the bar and underline cursors.
const CURSOR_THICKNESS: Pixels = px(2.);

/// Where a terminal's grid was painted: its top left corner, the size of
/// a cell, and how many rows and columns fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub origin: Point<Pixels>,
    pub cell: Size<Pixels>,
    pub rows: u16,
    pub cols: u16,
}

impl Geometry {
    /// The cell under `position`, kept within the grid.
    pub fn cell_at(&self, position: Point<Pixels>) -> (u16, u16) {
        let x = ((position.x - self.origin.x) / self.cell.width).floor();
        let y = ((position.y - self.origin.y) / self.cell.height).floor();
        let clamp = |v: f32, n: u16| (v.max(0.) as u16).min(n.saturating_sub(1));
        (clamp(y, self.rows), clamp(x, self.cols))
    }

    fn at(&self, row: u16, col: u16) -> Point<Pixels> {
        point(
            self.origin.x + self.cell.width * f32::from(col),
            self.origin.y + self.cell.height * f32::from(row),
        )
    }

    fn cells(&self, row: u16, col: u16, n: u16) -> Bounds<Pixels> {
        let origin = self.at(row, col);
        Bounds::new(
            point(origin.x.floor(), origin.y),
            size((self.cell.width * f32::from(n)).ceil(), self.cell.height),
        )
    }
}

/// The terminal font and its metrics, from the settings.
pub struct Metrics {
    pub font: gpui::Font,
    pub font_size: Pixels,
    pub cell: Size<Pixels>,
}

impl Metrics {
    pub fn new(window: &Window, cx: &App) -> Self {
        let theme_settings = theme::theme_settings(cx);
        let font = gpui::Font {
            // Ligatures would join cells.
            features: FontFeatures::disable_ligatures(),
            ..theme_settings.buffer_font(cx).clone()
        };
        let font_size = theme_settings.buffer_font_size(cx);
        let line_height = font_size * settings::get(cx).terminal.line_height.ratio();
        let text = window.text_system();
        let width = text
            .advance(text.resolve_font(&font), font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        Metrics {
            font,
            font_size,
            cell: size(width.max(px(1.)), line_height.round().max(px(1.))),
        }
    }
}

pub struct TerminalElement {
    item: Entity<TerminalItem>,
    /// Whether the cursor shows: focused, and not blinked off.
    cursor: bool,
}

impl TerminalElement {
    pub fn new(item: Entity<TerminalItem>, cursor: bool) -> Self {
        TerminalElement { item, cursor }
    }
}

/// A shaped run and where it goes.
type Glyphs = (Point<Pixels>, ShapedLine);

pub struct PrepaintState {
    geometry: Geometry,
    backgrounds: Vec<PaintQuad>,
    selection: Vec<PaintQuad>,
    lines: Vec<Glyphs>,
    cursor: Option<(PaintQuad, Option<Glyphs>)>,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let metrics = Metrics::new(window, cx);
        let fit = |room: Pixels, cell: Pixels| ((room / cell).floor().max(1.) as u16).max(1);
        let geometry = Geometry {
            origin: bounds.origin,
            cell: metrics.cell,
            rows: fit(bounds.size.height, metrics.cell.height),
            cols: fit(bounds.size.width, metrics.cell.width),
        };
        // The item learns its size (and asks the pane to take it) first,
        // so the scrollback it shows fits.
        self.item.update(cx, |item, cx| item.laid_out(geometry, cx));

        let theme = cx.theme();
        let palette = Palette::from_theme(theme.colors());
        let player = theme.players().local();
        let shape_setting = settings::get(cx).terminal.cursor_shape;
        let item = self.item.read(cx);
        let Some(screen) = item.screen(cx) else {
            return PrepaintState {
                geometry,
                backgrounds: Vec::new(),
                selection: Vec::new(),
                lines: Vec::new(),
                cursor: None,
            };
        };
        let layout = terminal_layout::layout(screen, geometry.rows, geometry.cols, &palette);
        let backgrounds = layout
            .backgrounds
            .iter()
            .map(|b| fill(geometry.cells(b.row, b.col, b.cells), b.color))
            .collect();
        let selection = item
            .selection()
            .map(|s| {
                s.rows(geometry.cols)
                    .into_iter()
                    .map(|(row, col, n)| fill(geometry.cells(row, col, n), player.selection))
                    .collect()
            })
            .unwrap_or_default();
        let shape = |text: String, run: TextRun, window: &mut Window| {
            window
                .text_system()
                .shape_line(text.into(), metrics.font_size, &[run], Some(metrics.cell.width))
        };
        let text_run = |r: &terminal_layout::Run| TextRun {
            len: r.text.len(),
            font: gpui::Font {
                weight: if r.bold {
                    FontWeight::BOLD
                } else {
                    metrics.font.weight
                },
                style: if r.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
                ..metrics.font.clone()
            },
            color: r.color,
            background_color: None,
            underline: r.underline.then_some(UnderlineStyle {
                color: Some(r.color),
                thickness: px(1.),
                wavy: false,
            }),
            strikethrough: None,
        };
        let lines = layout
            .runs
            .iter()
            .map(|r| {
                (
                    geometry.at(r.row, r.col),
                    shape(r.text.clone(), text_run(r), window),
                )
            })
            .collect();

        let cursor = (self.cursor && !screen.hide_cursor())
            .then(|| screen.cursor_position())
            .filter(|(row, col)| *row < geometry.rows && *col < geometry.cols)
            .map(|(row, col)| {
                let cell = screen.cell(row, col);
                let wide = cell.is_some_and(|c| c.is_wide());
                let cells = geometry.cells(row, col, if wide { 2 } else { 1 });
                let color = player.cursor;
                let quad = match shape_setting {
                    CursorShape::Block => fill(cells, color),
                    CursorShape::Bar => fill(
                        Bounds::new(cells.origin, size(CURSOR_THICKNESS, cells.size.height)),
                        color,
                    ),
                    CursorShape::Underline => fill(
                        Bounds::new(
                            point(cells.origin.x, cells.bottom() - CURSOR_THICKNESS),
                            size(cells.size.width, CURSOR_THICKNESS),
                        ),
                        color,
                    ),
                    CursorShape::Hollow => outline(cells, color, gpui::BorderStyle::Solid),
                };
                // A block covers its character: draw it again over the
                // block, in the background's colour, as terminals do.
                let glyph = (shape_setting == CursorShape::Block)
                    .then_some(cell)
                    .flatten()
                    .filter(|c| !c.contents().trim().is_empty())
                    .map(|c| {
                        let run = terminal_layout::Run {
                            row,
                            col,
                            cells: 1,
                            text: c.contents().to_string(),
                            color: palette.background,
                            bold: c.bold(),
                            italic: c.italic(),
                            underline: false,
                        };
                        (
                            geometry.at(row, col),
                            shape(run.text.clone(), text_run(&run), window),
                        )
                    });
                (quad, glyph)
            });
        PrepaintState {
            geometry,
            backgrounds,
            selection,
            lines,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = state.geometry.cell.height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in state.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            for quad in state.selection.drain(..) {
                window.paint_quad(quad);
            }
            for (origin, line) in &state.lines {
                let _ = line.paint(*origin, line_height, TextAlign::Left, None, window, cx);
            }
            if let Some((quad, glyph)) = state.cursor.take() {
                window.paint_quad(quad);
                if let Some((origin, line)) = glyph {
                    let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_map_to_cells_within_the_grid() {
        let g = Geometry {
            origin: point(px(10.), px(20.)),
            cell: size(px(8.), px(16.)),
            rows: 4,
            cols: 10,
        };
        assert_eq!(g.cell_at(point(px(10.), px(20.))), (0, 0));
        assert_eq!(g.cell_at(point(px(27.), px(37.))), (1, 2));
        assert_eq!(g.cell_at(point(px(0.), px(0.))), (0, 0));
        assert_eq!(g.cell_at(point(px(1000.), px(1000.))), (3, 9));
        assert_eq!(
            g.cells(1, 2, 3),
            Bounds::new(point(px(26.), px(36.)), size(px(24.), px(16.)))
        );
    }
}
