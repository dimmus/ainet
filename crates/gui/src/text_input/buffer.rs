//! The text and selection of a [`TextInput`](super::TextInput): edits,
//! movements and the undo history, apart from GPUI so they can be tested
//! on their own. Offsets are UTF-8 byte offsets; movements step over whole
//! graphemes, as GPUI's `examples/input.rs` does.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// How much undo history is kept.
const UNDO_LIMIT: usize = 200;

/// What a snapshot in the undo history holds.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
    reversed: bool,
}

/// The kind of the last edit, so typing and deleting run together into one
/// undo step until the cursor moves or the kind changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Typed text, ending at `end`; `space` if it ended in whitespace.
    Typing { end: usize, space: bool },
    /// A backspace or delete, leaving the cursor at `at`.
    Deleting { at: usize },
}

/// Which characters a word movement treats alike, as Zed's editor does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punctuation,
}

fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

#[derive(Clone, Debug, Default)]
pub struct Buffer {
    text: String,
    /// Always `start <= end`; `reversed` says the cursor is at `start`.
    selection: Range<usize>,
    reversed: bool,
    /// Text an input method is still composing.
    marked: Option<Range<usize>>,
    multiline: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last: Option<Kind>,
    /// Counts changes to the text, so callers can tell whether one changed.
    version: u64,
}

impl Buffer {
    pub fn new(multiline: bool) -> Self {
        Buffer {
            multiline,
            ..Buffer::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Changes whenever the text does.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn multiline(&self) -> bool {
        self.multiline
    }

    /// Switches between one line and many. Going down to one line joins
    /// the lines with spaces.
    pub fn set_multiline(&mut self, multiline: bool) {
        self.multiline = multiline;
        if !multiline && self.text.contains(['\n', '\r']) {
            let text = self.text.clone();
            self.set_text(&text);
        }
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn reversed(&self) -> bool {
        self.reversed
    }

    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Where the cursor is: the moving end of the selection.
    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.clone()]
    }

    /// Replaces all the text, as one undoable edit, with the cursor at the
    /// end.
    pub fn set_text(&mut self, text: &str) {
        let all = 0..self.text.len();
        let text = self.sanitize(text).into_owned();
        self.marked = None;
        self.last = None;
        self.edit(all, &text, None);
        self.last = None;
    }

    /// Text as this buffer keeps it: on one line unless multi-line.
    fn sanitize<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if self.multiline {
            if text.contains('\r') {
                text.replace("\r\n", "\n").replace('\r', "\n").into()
            } else {
                text.into()
            }
        } else if text.contains(['\n', '\r']) {
            text.replace("\r\n", " ").replace(['\n', '\r'], " ").into()
        } else {
            text.into()
        }
    }

    /// The nearest char boundary at or before `offset`.
    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        let (a, b) = (self.clamp(range.start), self.clamp(range.end));
        a.min(b)..a.max(b)
    }

    /// Replaces `range` (else the composing text, else the selection) with
    /// `new`, leaving the cursor after it. What the user types goes here.
    pub fn replace(&mut self, range: Option<Range<usize>>, new: &str) {
        let range = range
            .map(|r| self.clamp_range(r))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let new = self.sanitize(new).into_owned();
        let kind = (!new.is_empty() && range.is_empty()).then(|| Kind::Typing {
            end: range.start + new.len(),
            space: new.ends_with(char::is_whitespace),
        });
        self.edit(range, &new, kind);
        self.marked = None;
    }

    /// Replaces as [`Self::replace`] but marks the new text as still being
    /// composed by an input method; `selected` is relative to the new text.
    pub fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = range
            .map(|r| self.clamp_range(r))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let new = self.sanitize(new).into_owned();
        // One undo step for the whole composition.
        let composing = self.marked.is_some();
        if !composing {
            self.checkpoint();
        }
        self.splice(range.clone(), &new);
        self.marked = (!new.is_empty()).then(|| range.start..range.start + new.len());
        let end = range.start + new.len();
        self.selection = match selected {
            Some(sel) => {
                let a = (range.start + sel.start).min(end);
                let b = (range.start + sel.end).min(end);
                self.clamp(a.min(b))..self.clamp(a.max(b))
            }
            None => end..end,
        };
        self.reversed = false;
        self.last = None;
    }

    /// Accepts the composing text as typed.
    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// Makes an edit, recording it for undo unless it continues the last.
    fn edit(&mut self, range: Range<usize>, new: &str, kind: Option<Kind>) {
        if range.is_empty() && new.is_empty() {
            return;
        }
        let continues = self.marked.is_some()
            || match (self.last, kind) {
                (Some(Kind::Typing { end, space }), Some(Kind::Typing { .. })) => {
                    // A new word after a space starts a new step.
                    end == range.start && !(space && !new.starts_with(char::is_whitespace))
                }
                (Some(Kind::Deleting { at }), Some(Kind::Deleting { .. })) => {
                    at == range.end || at == range.start
                }
                _ => false,
            };
        if !continues {
            self.checkpoint();
        }
        self.splice(range.clone(), new);
        let end = range.start + new.len();
        self.selection = end..end;
        self.reversed = false;
        self.last = kind;
    }

    fn splice(&mut self, range: Range<usize>, new: &str) {
        self.text.replace_range(range, new);
        self.version += 1;
    }

    /// Saves the state before an edit, for undo; a new edit forgets redo.
    fn checkpoint(&mut self) {
        if self.undo.len() == UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            reversed: self.reversed,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.text = s.text;
        self.version += 1;
        self.selection = s.selection;
        self.reversed = s.reversed;
        self.marked = None;
        self.last = None;
    }

    /// Undoes the last edit; false if there is none.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
        true
    }

    /// Redoes the last undone edit; false if there is none.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    /// Puts the cursor at `offset`, selecting nothing.
    pub fn move_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        self.selection = offset..offset;
        self.reversed = false;
        self.last = None;
    }

    /// Moves the cursor to `offset`, keeping the other end of the selection.
    pub fn select_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        if self.reversed {
            self.selection.start = offset;
        } else {
            self.selection.end = offset;
        }
        if self.selection.end < self.selection.start {
            self.reversed = !self.reversed;
            self.selection = self.selection.end..self.selection.start;
        }
        self.last = None;
    }

    /// Selects `range`, the cursor at its start if `reversed`.
    pub fn select(&mut self, range: Range<usize>, reversed: bool) {
        self.selection = self.clamp_range(range);
        self.reversed = reversed && !self.selection.is_empty();
        self.last = None;
    }

    pub fn select_all(&mut self) {
        self.select(0..self.text.len(), false);
    }

    /// The grapheme boundary before `offset`.
    pub fn previous_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }

    /// The grapheme boundary after `offset`.
    pub fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(self.text.len())
    }

    /// Where a word movement left from `offset` lands: past any spaces,
    /// then over characters of one class.
    pub fn previous_word_start(&self, offset: usize) -> usize {
        let before: Vec<(usize, char)> = self.text[..self.clamp(offset)].char_indices().collect();
        let mut i = before.len();
        while i > 0 && class(before[i - 1].1) == CharClass::Space {
            i -= 1;
        }
        if i > 0 {
            let c = class(before[i - 1].1);
            while i > 0 && class(before[i - 1].1) == c {
                i -= 1;
            }
        }
        before.get(i).map_or(0, |(at, _)| *at)
    }

    /// Where a word movement right from `offset` lands.
    pub fn next_word_end(&self, offset: usize) -> usize {
        let start = self.clamp(offset);
        let mut chars = self.text[start..].char_indices().peekable();
        while chars.next_if(|(_, c)| class(*c) == CharClass::Space).is_some() {}
        if let Some(&(_, first)) = chars.peek() {
            let c = class(first);
            while chars.next_if(|(_, ch)| class(*ch) == c).is_some() {}
        }
        chars.peek().map_or(self.text.len(), |(at, _)| start + at)
    }

    /// The word (or run of spaces or punctuation) around `offset`, for a
    /// double-click.
    pub fn word_range(&self, offset: usize) -> Range<usize> {
        let offset = self.clamp(offset);
        let at = self.text[offset..]
            .chars()
            .next()
            .or_else(|| self.text[..offset].chars().next_back());
        let Some(at) = at else {
            return offset..offset;
        };
        let c = class(at);
        let start = self.text[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, ch)| class(*ch) == c)
            .last()
            .map_or(offset, |(i, _)| i);
        let end = self.text[offset..]
            .char_indices()
            .find(|(_, ch)| class(*ch) != c)
            .map_or(self.text.len(), |(i, _)| offset + i);
        start..end
    }

    /// The start of the hard line `offset` is on.
    pub fn line_start(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.text[..offset].rfind('\n').map_or(0, |i| i + 1)
    }

    /// The end of the hard line `offset` is on, before its newline.
    pub fn line_end(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |i| offset + i)
    }

    /// The same column `lines` hard lines up (negative) or down, counted in
    /// characters: how up and down move when nothing is laid out. Past the
    /// first or last line, the start or end of the text.
    pub fn vertical(&self, offset: usize, lines: isize) -> usize {
        let start = self.line_start(offset);
        let column = self.text[start..self.clamp(offset)].chars().count();
        let mut line = start;
        for _ in 0..lines.unsigned_abs() {
            if lines < 0 {
                if line == 0 {
                    return 0;
                }
                line = self.line_start(line - 1);
            } else {
                let end = self.line_end(line);
                if end == self.text.len() {
                    return end;
                }
                line = end + 1;
            }
        }
        let end = self.line_end(line);
        self.text[line..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(i, _)| line + i)
    }

    /// Moves left: to the selection's start, else one grapheme back.
    pub fn left(&mut self) {
        let to = if self.selection.is_empty() {
            self.previous_boundary(self.cursor())
        } else {
            self.selection.start
        };
        self.move_to(to);
    }

    /// Moves right: to the selection's end, else one grapheme on.
    pub fn right(&mut self) {
        let to = if self.selection.is_empty() {
            self.next_boundary(self.cursor())
        } else {
            self.selection.end
        };
        self.move_to(to);
    }

    /// Deletes the selection, else what `to` (from the cursor) spans.
    /// False if there was nothing to delete.
    fn delete_to(&mut self, to: impl FnOnce(&Self, usize) -> usize) -> bool {
        let range = if self.selection.is_empty() {
            let cursor = self.cursor();
            let to = to(self, cursor);
            cursor.min(to)..cursor.max(to)
        } else {
            self.selection.clone()
        };
        if range.is_empty() {
            return false;
        }
        let at = range.start;
        self.edit(range, "", Some(Kind::Deleting { at }));
        true
    }

    pub fn backspace(&mut self) -> bool {
        self.delete_to(|b, c| b.previous_boundary(c))
    }

    pub fn delete(&mut self) -> bool {
        self.delete_to(|b, c| b.next_boundary(c))
    }

    pub fn delete_word_left(&mut self) -> bool {
        self.delete_to(|b, c| b.previous_word_start(c))
    }

    pub fn delete_word_right(&mut self) -> bool {
        self.delete_to(|b, c| b.next_word_end(c))
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..self.clamp(offset)].encode_utf16().count()
    }

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (i, c) in self.text.char_indices() {
            if utf16 >= offset {
                return i;
            }
            utf16 += c.len_utf16();
        }
        self.text.len()
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Buffer {
        let mut b = Buffer::new(false);
        for g in text.graphemes(true) {
            b.replace(None, g);
        }
        b
    }

    #[test]
    fn typing_moves_the_cursor_and_replaces_the_selection() {
        let mut b = typed("hello");
        assert_eq!((b.text(), b.cursor()), ("hello", 5));
        b.select(1..4, false);
        assert_eq!(b.selected_text(), "ell");
        b.replace(None, "ipp");
        assert_eq!((b.text(), b.selection()), ("hippo", 4..4));
        // One line only: newlines become spaces.
        b.replace(None, "\nx\r\ny");
        assert_eq!(b.text(), "hipp x yo");
        let mut m = Buffer::new(true);
        m.replace(None, "a\r\nb\rc");
        assert_eq!(m.text(), "a\nb\nc");
    }

    #[test]
    fn moves_by_grapheme_word_and_line() {
        let mut b = typed("e\u{301}ab");
        b.move_to(0);
        b.right();
        assert_eq!(b.cursor(), 3, "over the combining accent");
        b.left();
        assert_eq!(b.cursor(), 0);

        let b = typed("foo.bar  baz_qux");
        assert_eq!(b.previous_word_start(16), 9);
        assert_eq!(b.previous_word_start(9), 4);
        assert_eq!(b.previous_word_start(4), 3);
        assert_eq!(b.previous_word_start(3), 0);
        assert_eq!(b.next_word_end(0), 3);
        assert_eq!(b.next_word_end(3), 4);
        assert_eq!(b.next_word_end(4), 7);
        assert_eq!(b.next_word_end(7), 16);
        assert_eq!(b.word_range(10), 9..16);
        assert_eq!(b.word_range(16), 9..16);

        let mut m = Buffer::new(true);
        m.replace(None, "one\ntwo three\nx");
        assert_eq!((m.line_start(6), m.line_end(6)), (4, 13));
        assert_eq!(m.vertical(6, -1), 2);
        assert_eq!(m.vertical(12, 1), 15, "short line: its end");
        assert_eq!(m.vertical(1, -1), 0);
        assert_eq!(m.vertical(14, 1), 15);
    }

    #[test]
    fn selection_extends_both_ways() {
        let mut b = typed("abcdef");
        b.move_to(3);
        b.select_to(5);
        assert_eq!((b.selection(), b.reversed()), (3..5, false));
        b.select_to(1);
        assert_eq!((b.selection(), b.reversed(), b.cursor()), (1..3, true, 1));
        b.right();
        assert_eq!(b.cursor(), 3, "collapses to the end");
        b.select_all();
        assert_eq!(b.selected_text(), "abcdef");
    }

    #[test]
    fn deletes_graphemes_words_and_selections() {
        let mut b = typed("one two");
        assert!(b.backspace());
        assert_eq!(b.text(), "one tw");
        assert!(b.delete_word_left());
        assert_eq!(b.text(), "one ");
        b.move_to(0);
        assert!(!b.backspace(), "nothing before the start");
        assert!(b.delete_word_right());
        assert_eq!(b.text(), " ");
        assert!(b.delete());
        assert!(!b.delete());
        assert!(b.is_empty());
    }

    #[test]
    fn undo_steps_over_words_and_redo_comes_back() {
        let mut b = typed("hello world");
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "hello wor");
        assert!(b.undo());
        assert_eq!(b.text(), "hello world", "the deletions are one step");
        assert!(b.undo());
        assert_eq!(b.text(), "hello ", "a word after a space is its own step");
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(!b.undo());
        assert!(b.redo());
        assert!(b.redo());
        assert_eq!((b.text(), b.cursor()), ("hello world", 11));
        // A new edit forgets what could be redone.
        b.undo();
        b.replace(None, "!");
        assert!(!b.redo());
        assert_eq!(b.text(), "hello !");

        // Moving the cursor ends the step.
        let mut b = typed("ab");
        b.move_to(1);
        b.replace(None, "x");
        b.undo();
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn composition_is_marked_then_one_undo_step() {
        let mut b = typed("a");
        b.replace_and_mark(None, "n", None);
        assert_eq!(b.marked(), Some(1..2));
        b.replace_and_mark(None, "ni", Some(2..2));
        assert_eq!((b.text(), b.marked(), b.cursor()), ("ani", Some(1..3), 3));
        b.replace(None, "に");
        assert_eq!((b.text(), b.marked()), ("aに", None));
        b.undo();
        assert_eq!(b.text(), "a");
        // Cancelled composition: the mark goes, the text stays.
        b.replace_and_mark(None, "k", None);
        b.unmark();
        assert_eq!((b.text(), b.marked()), ("ak", None));
    }

    #[test]
    fn converts_utf16_offsets() {
        let b = typed("a😀b");
        assert_eq!(b.offset_to_utf16(5), 3);
        assert_eq!(b.offset_from_utf16(3), 5);
        assert_eq!(b.range_from_utf16(&(1..3)), 1..5);
        assert_eq!(b.range_to_utf16(&(0..6)), 0..4);
        assert_eq!(b.offset_from_utf16(99), 6);
    }

    #[test]
    fn set_text_is_undoable_and_offsets_are_clamped() {
        let mut b = typed("x");
        b.set_text("new\ntext");
        assert_eq!((b.text(), b.cursor()), ("new text", 8));
        b.undo();
        assert_eq!(b.text(), "x");
        let mut b = typed("é");
        b.move_to(1);
        assert_eq!(b.cursor(), 0, "inside a char: its start");
        b.move_to(100);
        assert_eq!(b.cursor(), 2);
    }
}
