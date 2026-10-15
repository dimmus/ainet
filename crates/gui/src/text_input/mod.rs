//! A small text field on GPUI's `EntityInputHandler`, after GPUI's
//! `examples/input.rs`: Zed's own is its whole `editor` crate, far too big
//! to depend on. One line or several (soft-wrapped, up to a number of
//! visible lines), with input methods, selection by keys and mouse, the
//! clipboard, word movement, undo and redo, a placeholder, masking for
//! secrets, and `Submitted`/`Cancelled` events for forms. It also backs
//! Zed's `ui_input::InputField` (see [`erased`]).
//!
//! Keys (`TextInput` context in the keymaps) go through the actions below.
//! Enter and escape (`Confirm`, `Cancel`) are reported as events and then
//! dispatched on as `menu::Confirm` and `menu::Cancel`, so a picker or
//! modal around the input hears them too.

mod buffer;
mod element;
pub mod erased;

use std::ops::Range;

use gpui::{
    ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable, KeyContext,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Subscription,
    UTF16Selection, Window, actions, point,
};
use ui::prelude::*;

pub use buffer::Buffer;
use element::{Layout, TextElement};

actions!(
    text_input,
    [
        /// Deletes the selection, or the character before the cursor.
        Backspace,
        /// Deletes the selection, or the character after the cursor.
        Delete,
        /// Deletes to the start of the word.
        DeleteWordLeft,
        /// Deletes to the end of the word.
        DeleteWordRight,
        /// Moves the cursor one character left.
        Left,
        /// Moves the cursor one character right.
        Right,
        /// Moves the cursor one line up (several lines only).
        Up,
        /// Moves the cursor one line down (several lines only).
        Down,
        /// Extends the selection one character left.
        SelectLeft,
        /// Extends the selection one character right.
        SelectRight,
        /// Extends the selection one line up.
        SelectUp,
        /// Extends the selection one line down.
        SelectDown,
        /// Moves the cursor to the start of the word.
        WordLeft,
        /// Moves the cursor to the end of the word.
        WordRight,
        /// Extends the selection to the start of the word.
        SelectWordLeft,
        /// Extends the selection to the end of the word.
        SelectWordRight,
        /// Moves the cursor to the start of the line.
        Home,
        /// Moves the cursor to the end of the line.
        End,
        /// Extends the selection to the start of the line.
        SelectToHome,
        /// Extends the selection to the end of the line.
        SelectToEnd,
        /// Moves the cursor to the start of the text.
        MoveToStart,
        /// Moves the cursor to the end of the text.
        MoveToEnd,
        /// Selects all the text.
        SelectAll,
        /// Copies the selection.
        Copy,
        /// Cuts the selection.
        Cut,
        /// Pastes the clipboard.
        Paste,
        /// Undoes the last edit.
        Undo,
        /// Redoes the last undone edit.
        Redo,
        /// Starts a new line (several lines only).
        Newline,
        /// Opens the system's character palette.
        ShowCharacterPalette,
        /// Submits (enter): reported, then dispatched on as `menu::Confirm`.
        Confirm,
        /// Gives up (escape): reported, then dispatched on as `menu::Cancel`.
        Cancel,
    ]
);

/// What a text input tells its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// The text changed.
    Edited,
    /// Enter (`menu::Confirm`).
    Submitted,
    /// Escape (`menu::Cancel`).
    Cancelled,
    /// It lost the keyboard.
    Blurred,
}

/// What a masked input shows for each character.
const MASK: char = '•';

pub struct TextInput {
    focus_handle: FocusHandle,
    buffer: Buffer,
    placeholder: SharedString,
    /// Shows bullets instead of the text (secrets).
    masked: bool,
    read_only: bool,
    /// For several lines: how many show before it scrolls (`None`: all).
    max_lines: Option<usize>,
    /// What the last paint laid out, for the mouse, input methods and
    /// moving up and down.
    layout: Option<Layout>,
    /// How far the text is scrolled to keep the cursor in view.
    scroll: Point<Pixels>,
    /// A mouse drag is selecting.
    selecting: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    /// A one-line input.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let blurred = cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.selecting = false;
            cx.emit(InputEvent::Blurred);
            cx.notify();
        });
        TextInput {
            focus_handle,
            buffer: Buffer::new(false),
            placeholder: SharedString::default(),
            masked: false,
            read_only: false,
            max_lines: None,
            layout: None,
            scroll: Point::default(),
            selecting: false,
            _subscriptions: vec![blurred],
        }
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Makes it take several lines, showing at most `max_lines` before it
    /// scrolls.
    pub fn with_multiline(mut self, max_lines: Option<usize>) -> Self {
        self.buffer.set_multiline(true);
        self.max_lines = max_lines;
        self
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Replaces the text (undoably), the cursor at the end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.buffer.text() != text {
            self.buffer.set_text(text);
            self.edited(cx);
        }
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    /// One line (`None`) or several, showing at most `max_lines`.
    pub fn set_multiline(&mut self, lines: Option<Option<usize>>, cx: &mut Context<Self>) {
        self.buffer.set_multiline(lines.is_some());
        self.max_lines = lines.flatten();
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.buffer.select_all();
        cx.notify();
    }

    pub fn move_to_end(&mut self, cx: &mut Context<Self>) {
        self.buffer.move_to(self.buffer.text().len());
        cx.notify();
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    /// Runs an edit unless read-only, and reports it if it changed the
    /// text.
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        if self.read_only {
            return;
        }
        let version = self.buffer.version();
        f(&mut self.buffer);
        if self.buffer.version() != version {
            self.edited(cx);
        } else {
            cx.notify();
        }
    }

    fn movement(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        f(&mut self.buffer);
        cx.notify();
    }

    /// The text as painted: bullets when masked.
    fn display_text(&self) -> String {
        if self.masked {
            self.buffer.text().chars().map(|_| MASK).collect()
        } else {
            self.buffer.text().to_string()
        }
    }

    /// A text offset as an offset into [`Self::display_text`].
    fn offset_to_display(&self, offset: usize) -> usize {
        if self.masked {
            self.buffer.text()[..offset].chars().count() * MASK.len_utf8()
        } else {
            offset
        }
    }

    /// An offset into [`Self::display_text`] as a text offset.
    fn display_to_offset(&self, offset: usize) -> usize {
        if self.masked {
            let n = offset / MASK.len_utf8();
            self.buffer
                .text()
                .char_indices()
                .nth(n)
                .map_or(self.buffer.text().len(), |(i, _)| i)
        } else {
            offset.min(self.buffer.text().len())
        }
    }

    /// The text offset under a window position, once laid out.
    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        match &self.layout {
            Some(layout) if !self.buffer.is_empty() => {
                self.display_to_offset(layout.index_for_position(position))
            }
            _ => 0,
        }
    }

    /// The offset one visual line up or down from the cursor: by the
    /// layout when there is one, else by hard lines.
    fn vertical(&self, lines: isize) -> usize {
        let cursor = self.buffer.cursor();
        let Some(layout) = &self.layout else {
            return self.buffer.vertical(cursor, lines);
        };
        let at = layout.position_for_index(self.offset_to_display(cursor));
        let target = point(
            at.x,
            at.y + layout.line_height * lines as f32 + layout.line_height / 2.,
        );
        if target.y < layout.origin.y {
            0
        } else if target.y > layout.origin.y + layout.height() {
            self.buffer.text().len()
        } else {
            self.display_to_offset(layout.index_for_position(target))
        }
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |b| {
            b.backspace();
        });
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |b| {
            b.delete();
        });
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |b| {
            b.delete_word_left();
        });
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |b| {
            b.delete_word_right();
        });
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, Buffer::left);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, Buffer::right);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.multiline() {
            return cx.propagate();
        }
        let to = self.vertical(-1);
        self.movement(cx, |b| b.move_to(to));
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.multiline() {
            return cx.propagate();
        }
        let to = self.vertical(1);
        self.movement(cx, |b| b.move_to(to));
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.select_to(b.previous_boundary(b.cursor())));
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.select_to(b.next_boundary(b.cursor())));
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.multiline() {
            return cx.propagate();
        }
        let to = self.vertical(-1);
        self.movement(cx, |b| b.select_to(to));
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.multiline() {
            return cx.propagate();
        }
        let to = self.vertical(1);
        self.movement(cx, |b| b.select_to(to));
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.move_to(b.previous_word_start(b.cursor())));
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.move_to(b.next_word_end(b.cursor())));
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.select_to(b.previous_word_start(b.cursor())));
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.select_to(b.next_word_end(b.cursor())));
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.move_to(b.line_start(b.cursor())));
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.move_to(b.line_end(b.cursor())));
    }

    fn select_to_home(&mut self, _: &SelectToHome, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.select_to(b.line_start(b.cursor())));
    }

    fn select_to_end(&mut self, _: &SelectToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.select_to(b.line_end(b.cursor())));
    }

    fn move_to_start(&mut self, _: &MoveToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.move_to(0));
    }

    fn move_to_text_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.movement(cx, |b| b.move_to(b.text().len()));
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        // A secret stays in the field.
        if !self.masked && !self.buffer.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.selected_text().to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.masked || self.read_only || self.buffer.selection().is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.selected_text().to_string()));
        self.edit(cx, |b| b.replace(None, ""));
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(cx, |b| b.replace(None, &text));
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |b| {
            b.undo();
        });
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |b| {
            b.redo();
        });
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.multiline() {
            return cx.propagate();
        }
        self.edit(cx, |b| b.replace(None, "\n"));
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    /// Enter: reported, then passed on to whatever holds the input.
    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submitted);
        window.dispatch_action(Box::new(menu::Confirm), cx);
    }

    /// Escape: reported, then passed on (a modal closes on it).
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancelled);
        window.dispatch_action(Box::new(menu::Cancel), cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let at = self.index_for_position(event.position);
        match event.click_count {
            0 | 1 if event.modifiers.shift => self.buffer.select_to(at),
            0 | 1 => self.buffer.move_to(at),
            2 => {
                let word = self.buffer.word_range(at);
                self.buffer.select(word, false);
            }
            _ => {
                let line = self.buffer.line_start(at)..self.buffer.line_end(at);
                self.buffer.select(line, false);
            }
        }
        self.selecting = event.click_count <= 1;
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let at = self.index_for_position(event.position);
            self.buffer.select_to(at);
            cx.notify();
        }
    }

    fn key_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("TextInput");
        if self.buffer.multiline() {
            context.add("multiline");
        }
        context
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range);
        actual.replace(self.buffer.range_to_utf16(&range));
        Some(self.buffer.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selection()),
            reversed: self.buffer.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer.marked().map(|r| self.buffer.range_to_utf16(&r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.unmark();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|r| self.buffer.range_from_utf16(&r));
        self.edit(cx, |b| b.replace(range, text));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|r| self.buffer.range_from_utf16(&r));
        // The selection is in UTF-16 within the new text.
        let selected = selected.map(|r| {
            let byte = |utf16: usize| {
                let mut count = 0;
                for (i, c) in text.char_indices() {
                    if count >= utf16 {
                        return i;
                    }
                    count += c.len_utf16();
                }
                text.len()
            };
            byte(r.start)..byte(r.end)
        });
        self.edit(cx, |b| b.replace_and_mark(range, text, selected));
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: gpui::Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let range = self.buffer.range_from_utf16(&range);
        let start = layout.position_for_index(self.offset_to_display(range.start));
        let end = layout.position_for_index(self.offset_to_display(range.end));
        let end_x = if end.y == start.y { end.x } else { start.x };
        Some(gpui::Bounds::from_corners(
            start,
            point(end_x, start.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.layout.as_ref()?;
        let at = self.index_for_position(position);
        Some(self.buffer.offset_to_utf16(at))
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("text-input")
            .key_context(self.key_context())
            .track_focus(&self.focus_handle)
            .w_full()
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_to_home))
            .on_action(cx.listener(Self::select_to_end))
            .on_action(cx.listener(Self::move_to_start))
            .on_action(cx.listener(Self::move_to_text_end))
            .on_action(cx.listener(Self::select_all_action))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement::new(cx.entity()))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::*;
    use crate::tests::init_test;

    /// A text input in a window of its own, focused, with the bundled keys.
    fn input(cx: &mut TestAppContext, multiline: bool) -> (Entity<TextInput>, &mut VisualTestContext) {
        init_test(cx);
        let (input, cx) = cx.add_window_view(|window, cx| {
            let input = TextInput::new(window, cx).with_placeholder("Type here");
            if multiline {
                input.with_multiline(Some(4))
            } else {
                input
            }
        });
        cx.update(|window, cx| window.focus(&input.focus_handle(cx), cx));
        cx.run_until_parked();
        (input, cx)
    }

    fn text(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> String {
        input.read_with(cx, |i, _| i.text().to_string())
    }

    #[gpui::test]
    async fn types_edits_selects_and_undoes(cx: &mut TestAppContext) {
        let (input, cx) = input(cx, false);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let events = events.clone();
            window
                .subscribe(&input, cx, move |_, e: &InputEvent, _, _| {
                    events.borrow_mut().push(*e)
                })
                .detach();
        });

        // Typed text arrives through the input handler, as from the platform.
        cx.simulate_input("hello world");
        assert_eq!(text(&input, cx), "hello world");
        assert!(events.borrow().iter().all(|e| *e == InputEvent::Edited));

        cx.simulate_keystrokes("ctrl-backspace");
        assert_eq!(text(&input, cx), "hello ");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "hello world");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, cx), "hello ");
        cx.simulate_keystrokes("ctrl-shift-z");
        assert_eq!(text(&input, cx), "hello world");

        // Select a word leftwards and replace it.
        cx.simulate_keystrokes("ctrl-shift-left");
        input.read_with(cx, |i, _| assert_eq!(i.buffer().selected_text(), "world"));
        cx.simulate_input("there");
        assert_eq!(text(&input, cx), "hello there");
        cx.simulate_keystrokes("home shift-right shift-right");
        input.read_with(cx, |i, _| assert_eq!(i.buffer().selected_text(), "he"));
        cx.simulate_keystrokes("delete end backspace");
        assert_eq!(text(&input, cx), "llo ther");

        // Enter and escape are reported, and the select-all key works here.
        events.borrow_mut().clear();
        cx.simulate_keystrokes("enter escape");
        assert_eq!(*events.borrow(), [InputEvent::Submitted, InputEvent::Cancelled]);
        cx.simulate_keystrokes("ctrl-a backspace");
        assert!(input.read_with(cx, |i, _| i.is_empty()));
    }

    #[gpui::test]
    async fn copies_cuts_and_pastes(cx: &mut TestAppContext) {
        let (input, cx) = input(cx, false);
        cx.simulate_input("one two");
        cx.simulate_keystrokes("ctrl-shift-left ctrl-x");
        assert_eq!(text(&input, cx), "one ");
        cx.simulate_keystrokes("home ctrl-v");
        assert_eq!(text(&input, cx), "twoone ");
        cx.simulate_keystrokes("ctrl-a ctrl-c end ctrl-v");
        assert_eq!(text(&input, cx), "twoone twoone ");

        // One line: pasted newlines become spaces. Secrets are not copied.
        cx.write_to_clipboard(ClipboardItem::new_string("a\nb".into()));
        cx.simulate_keystrokes("ctrl-a ctrl-v");
        assert_eq!(text(&input, cx), "a b");
        input.update(cx, |i, cx| i.set_masked(true, cx));
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("a\nb")
        );
    }

    #[gpui::test]
    async fn several_lines_take_newlines_and_move_by_line(cx: &mut TestAppContext) {
        let (input, cx) = input(cx, true);
        cx.simulate_input("first");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("second");
        assert_eq!(text(&input, cx), "first\nsecond");
        cx.simulate_keystrokes("up");
        let cursor = input.read_with(cx, |i, _| i.buffer().cursor());
        assert!(cursor <= 5, "on the first line: {cursor}");
        cx.simulate_keystrokes("ctrl-end shift-up");
        input.read_with(cx, |i, _| assert!(i.buffer().selected_text().contains('\n')));
    }

    #[gpui::test]
    async fn read_only_ignores_edits_but_moves(cx: &mut TestAppContext) {
        let (input, cx) = input(cx, false);
        input.update(cx, |i, cx| {
            i.set_text("fixed", cx);
            i.set_read_only(true, cx);
        });
        cx.simulate_input("x");
        cx.simulate_keystrokes("backspace home");
        assert_eq!(text(&input, cx), "fixed");
        assert_eq!(input.read_with(cx, |i, _| i.buffer().cursor()), 0);
    }
}
