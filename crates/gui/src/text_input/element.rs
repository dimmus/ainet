//! Lays out and paints a [`TextInput`]: the text (or its placeholder),
//! soft-wrapped when it has several lines, the selection, the cursor, and
//! the composing text underlined; scrolls to keep the cursor in view; and
//! hands the window the input handler while focused.

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, SharedString, Size,
    Style, TextAlign, TextRun, UnderlineStyle, Window, WrappedLine, fill, point, px, relative, size,
};
use theme::ActiveTheme;

use super::TextInput;

/// The cursor's width, as Zed's bar cursor.
const CURSOR_WIDTH: Pixels = px(2.);

/// The lines of the last paint, kept by the input for the mouse, input
/// methods and moving up and down. Offsets are into the displayed text.
pub struct Layout {
    lines: Vec<LaidLine>,
    /// Where the first line starts, scrolling included.
    pub origin: Point<Pixels>,
    pub line_height: Pixels,
}

struct LaidLine {
    /// Where the hard line starts in the displayed text.
    start: usize,
    /// Its top, from the layout's origin.
    y: Pixels,
    /// How many rows it wraps to.
    rows: usize,
    line: WrappedLine,
}

impl Layout {
    fn new(lines: Vec<WrappedLine>, origin: Point<Pixels>, line_height: Pixels) -> Self {
        let mut laid = Vec::with_capacity(lines.len().max(1));
        let (mut start, mut y) = (0, Pixels::ZERO);
        for line in lines {
            let rows = line.wrap_boundaries().len() + 1;
            let len = line.len();
            laid.push(LaidLine { start, y, rows, line });
            start += len + 1;
            y += line_height * rows as f32;
        }
        if laid.is_empty() {
            laid.push(LaidLine {
                start: 0,
                y: Pixels::ZERO,
                rows: 1,
                line: WrappedLine::default(),
            });
        }
        Layout {
            lines: laid,
            origin,
            line_height,
        }
    }

    /// The height of all the rows.
    pub fn height(&self) -> Pixels {
        self.line_height * self.lines.iter().map(|l| l.rows).sum::<usize>() as f32
    }

    fn line_of(&self, index: usize) -> &LaidLine {
        self.lines
            .iter()
            .rev()
            .find(|l| l.start <= index)
            .unwrap_or(&self.lines[0])
    }

    /// The top left of the character at `index`, in window coordinates.
    pub fn position_for_index(&self, index: usize) -> Point<Pixels> {
        let line = self.line_of(index);
        let local = line
            .line
            .position_for_index(index - line.start, self.line_height)
            .unwrap_or_else(|| point(line.line.width(), self.line_height * (line.rows - 1) as f32));
        point(self.origin.x + local.x, self.origin.y + line.y + local.y)
    }

    /// The character boundary nearest a window position.
    pub fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let local = position - self.origin;
        if local.y < Pixels::ZERO {
            return 0;
        }
        let line = self
            .lines
            .iter()
            .find(|l| local.y < l.y + self.line_height * l.rows as f32)
            .unwrap_or_else(|| self.lines.last().expect("a layout has a line"));
        let within = point(
            local.x.max(Pixels::ZERO),
            (local.y - line.y).min(self.line_height * (line.rows as f32 - 0.5)),
        );
        match line.line.closest_index_for_position(within, self.line_height) {
            Ok(i) | Err(i) => line.start + i,
        }
    }

    /// Rectangles covering the displayed text between two offsets, one per
    /// row; `right` is where a row that runs on ends.
    fn selection(&self, range: std::ops::Range<usize>, right: Pixels) -> Vec<Bounds<Pixels>> {
        let mut rects = Vec::new();
        for line in &self.lines {
            let end = line.start + line.line.len();
            let (s, e) = (range.start.max(line.start), range.end.min(end));
            if s > e || (s == e && range.end <= end) {
                continue;
            }
            let (a, b) = (self.position_for_index(s), self.position_for_index(e));
            // A selected newline shows as a little room after the line.
            let b_x = if range.end > end {
                b.x + self.line_height / 4.
            } else {
                b.x
            };
            if a.y == b.y {
                rects.push(Bounds::from_corners(a, point(b_x, a.y + self.line_height)));
            } else {
                rects.push(Bounds::from_corners(a, point(right, a.y + self.line_height)));
                let mut y = a.y + self.line_height;
                while y < b.y {
                    rects.push(Bounds::from_corners(
                        point(self.origin.x, y),
                        point(right, y + self.line_height),
                    ));
                    y += self.line_height;
                }
                rects.push(Bounds::from_corners(
                    point(self.origin.x, b.y),
                    point(b_x, b.y + self.line_height),
                ));
            }
        }
        rects
    }
}

pub struct TextElement {
    input: Entity<TextInput>,
}

impl TextElement {
    pub fn new(input: Entity<TextInput>) -> Self {
        TextElement { input }
    }
}

pub struct PrepaintState {
    layout: Option<Layout>,
    scroll: Point<Pixels>,
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// What is painted: the text, or the placeholder in its colour.
fn shown(input: &TextInput, window: &Window, cx: &App) -> (SharedString, TextRun) {
    let style = window.text_style();
    let (text, color): (SharedString, _) = if input.buffer.is_empty() {
        (input.placeholder.clone(), cx.theme().colors().text_placeholder)
    } else {
        (input.display_text().into(), style.color)
    };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    (text, run)
}

impl Element for TextElement {
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
        let input = self.input.read(cx);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if !input.buffer.multiline() {
            style.size.height = line_height.into();
            return (window.request_layout(style, [], cx), ());
        }
        // Several lines: as tall as the wrapped text, up to `max_lines`.
        let (text, run) = shown(input, window, cx);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let max_lines = input.max_lines;
        let layout = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let rows = window
                .text_system()
                .shape_text(text.clone(), font_size, std::slice::from_ref(&run), width, None)
                .map(|lines| lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum::<usize>())
                .unwrap_or(1)
                .max(1);
            let rows = max_lines.map_or(rows, |max| rows.min(max.max(1)));
            Size {
                width: width.unwrap_or_default(),
                height: line_height * rows as f32,
            }
        });
        (layout, ())
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
        let input = self.input.read(cx);
        let line_height = window.line_height();
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let (text, run) = shown(input, window, cx);
        let marked = input
            .buffer
            .marked()
            .filter(|_| !input.buffer.is_empty())
            .map(|m| input.offset_to_display(m.start)..input.offset_to_display(m.end));
        let runs = match marked {
            Some(m) => [
                TextRun {
                    len: m.start,
                    ..run.clone()
                },
                TextRun {
                    len: m.end - m.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - m.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            None => vec![run],
        };
        let multiline = input.buffer.multiline();
        let wrap = multiline.then_some(bounds.size.width);
        let lines = window
            .text_system()
            .shape_text(text, font_size, &runs, wrap, None)
            .map(|lines| lines.into_iter().collect())
            .unwrap_or_default();
        let mut layout = Layout::new(lines, bounds.origin, line_height);

        // Scroll so the cursor shows.
        let cursor = if input.buffer.is_empty() {
            0
        } else {
            input.offset_to_display(input.buffer.cursor())
        };
        let at = layout.position_for_index(cursor) - bounds.origin;
        let mut scroll = input.scroll;
        if multiline {
            let room = bounds.size.height;
            if at.y + line_height - scroll.y > room {
                scroll.y = at.y + line_height - room;
            }
            if at.y < scroll.y {
                scroll.y = at.y;
            }
            scroll.y = scroll
                .y
                .min((layout.height() - room).max(Pixels::ZERO))
                .max(Pixels::ZERO);
            scroll.x = Pixels::ZERO;
        } else {
            let room = bounds.size.width - CURSOR_WIDTH;
            if at.x - scroll.x > room {
                scroll.x = at.x - room;
            }
            if at.x < scroll.x {
                scroll.x = at.x;
            }
            let width = layout.lines[0].line.width();
            scroll.x = scroll.x.min((width - room).max(Pixels::ZERO)).max(Pixels::ZERO);
            scroll.y = Pixels::ZERO;
        }
        layout.origin = bounds.origin - scroll;

        let colors = cx.theme().players().local();
        let selected = input.buffer.selection();
        let selection = if selected.is_empty() || input.buffer.is_empty() {
            Vec::new()
        } else {
            let range = input.offset_to_display(selected.start)..input.offset_to_display(selected.end);
            layout
                .selection(range, bounds.right())
                .into_iter()
                .map(|r| fill(r, colors.selection))
                .collect()
        };
        let cursor = (selected.is_empty() && !input.read_only).then(|| {
            let at = layout.position_for_index(cursor);
            fill(Bounds::new(at, size(CURSOR_WIDTH, line_height)), colors.cursor)
        });
        PrepaintState {
            layout: Some(layout),
            scroll,
            selection,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let Some(layout) = prepaint.layout.take() else {
            return;
        };
        let focused = focus.is_focused(window);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }
            for line in &layout.lines {
                let origin = point(layout.origin.x, layout.origin.y + line.y);
                let _ = line
                    .line
                    .paint(origin, layout.line_height, TextAlign::Left, None, window, cx);
            }
            if focused && let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });
        let scroll = prepaint.scroll;
        self.input.update(cx, |input, _| {
            input.layout = Some(layout);
            input.scroll = scroll;
        });
    }
}
