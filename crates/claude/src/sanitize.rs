//! Makes text from outside ainet safe to print to a terminal: agents, hook
//! payloads and transcripts all carry strings ainet does not control.
//!
//! Ported from Den's `internal/sanitize` (MIT). What is removed:
//!
//! - CSI (`ESC [` … final byte), OSC (`ESC ]` … BEL or ST), DCS, SOS, PM and
//!   APC (`ESC P`, `X`, `^`, `_` … ST), each with its payload; the 8-bit C1
//!   forms U+009B, U+009D, U+0090, U+0098, U+009E and U+009F likewise. A
//!   string sequence that never ends is dropped to the end of the input. CAN
//!   and SUB end any sequence, BEL ends any string.
//! - Every other escape: two-byte ESC sequences, nF sequences such as
//!   `ESC ( B`, and a lone ESC.
//! - C0 controls other than `\n` and `\t`, DEL, and all C1 (U+0080–U+009F).
//! - Bidi controls: U+202A–U+202E, U+2066–U+2069, U+200E, U+200F, U+061C.
//! - Invisible characters: Unicode format characters (category Cf, from a
//!   built-in table), U+034F, the Hangul fillers U+115F, U+1160, U+3164,
//!   U+FFA0, and variation selectors. A zero-width joiner between two emoji,
//!   a joiner or non-joiner between two letters of a script that spells with
//!   them, and U+FE0E or U+FE0F right after the character they style are
//!   kept, one at a time.
//!
//! `\r\n` and a lone `\r` become `\n`, as do U+2028 and U+2029.
//!
//! Difference from Den: Go's `unicode` tables decide "letter or mark of a
//! joiner script"; here it is a block-range check, which can keep a joiner
//! next to a punctuation mark of those scripts. Rust strings are always
//! valid UTF-8, so the invalid-UTF-8 handling only applies to
//! [`text_bytes`].

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const DEL: u8 = 0x7f;

const ZWNJ: char = '\u{200c}';
const ZWJ: char = '\u{200d}';
const VS15: char = '\u{fe0e}';
const VS16: char = '\u{fe0f}';

/// Text with escape sequences, controls other than `\n` and `\t`, bidi
/// controls and invisible characters removed, and `\r\n`, `\r`, U+2028 and
/// U+2029 turned into `\n`. For text that may span lines.
pub fn text(s: &str) -> String {
    if is_clean(s) {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut last: Option<char> = None;
    let mut base: Option<char> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == ESC {
            i = skip_escape(b, i + 1);
            continue;
        }
        if c == b'\r' {
            out.push('\n');
            i += 1;
            if i < b.len() && b[i] == b'\n' {
                i += 1;
            }
            last = Some('\n');
            base = Some('\n');
            continue;
        }
        if c == b'\n' || c == b'\t' {
            out.push(c as char);
            last = Some(c as char);
            base = last;
            i += 1;
            continue;
        }
        if c < 0x20 || c == DEL {
            i += 1;
            continue;
        }
        if c < 0x80 {
            out.push(c as char);
            last = Some(c as char);
            base = last;
            i += 1;
            continue;
        }
        // A multi-byte character: s is valid UTF-8, so this is a char boundary.
        let r = s[i..].chars().next().unwrap_or('\u{fffd}');
        let n = r.len_utf8();
        i += n;
        let cp = r as u32;
        match r {
            '\u{9b}' => i = skip_csi(b, i),
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => i = skip_string(b, i),
            _ if (0x80..=0x9f).contains(&cp) => {}
            _ if bidi(r) => {}
            '\u{2028}' | '\u{2029}' => {
                out.push('\n');
                last = Some('\n');
                base = last;
            }
            ZWJ | ZWNJ => {
                let next = s[i..].chars().next();
                if joins(r, base, last, next) {
                    out.push(r);
                    last = Some(r);
                }
            }
            VS15 | VS16 => {
                if let Some(l) = last
                    && Some(l) == base
                    && !invisible(l)
                {
                    out.push(r);
                    last = Some(r);
                }
            }
            _ if invisible(r) => {}
            _ => {
                out.push(r);
                last = Some(r);
                base = last;
            }
        }
    }
    out
}

/// [`text`] for bytes that may not be valid UTF-8: invalid bytes are dropped,
/// so a stray byte cannot change the width of what is shown.
pub fn text_bytes(b: &[u8]) -> String {
    let mut valid = String::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        valid.push_str(chunk.valid());
    }
    text(&valid)
}

/// [`text`] for a single line: a name, a title, a label. Newlines and tabs
/// become spaces, runs of spaces collapse to one, and the ends are trimmed.
pub fn line(s: &str) -> String {
    let t = text(s);
    let mut out = String::with_capacity(t.len());
    let mut space = false;
    for c in t.chars() {
        if c == ' ' || c == '\n' || c == '\t' {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// [`text`] for one line of code or a path: spaces and tabs are kept, a
/// newline becomes a space, so the line stays one line.
pub fn code(s: &str) -> String {
    let t = text(s);
    if !t.contains('\n') {
        return t;
    }
    t.replace('\n', " ")
}

/// [`line`] capped at `max` characters; a cut label ends in "…", which counts
/// toward `max`. A `max` below 1 means no cap.
pub fn label(s: &str, max: usize) -> String {
    let l = line(s);
    if max < 1 || l.chars().count() <= max {
        return l;
    }
    let cut: String = l.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end_matches(' '))
}

/// The first character `text` would remove or change, with none of its
/// exceptions: for text to refuse rather than clean.
pub fn hidden(s: &str) -> Option<char> {
    s.chars().find(|&r| {
        let cp = r as u32;
        (cp < 0x20 && r != '\n' && r != '\t')
            || (0x7f..=0x9f).contains(&cp)
            || bidi(r)
            || invisible(r)
            || r == ZWJ
            || r == ZWNJ
            || r == '\u{2028}'
            || r == '\u{2029}'
    })
}

/// Printable ASCII, `\n` and `\t` only: the common case.
fn is_clean(s: &str) -> bool {
    s.bytes()
        .all(|c| !((c < 0x20 && c != b'\n' && c != b'\t') || c >= DEL))
}

fn bidi(r: char) -> bool {
    matches!(r as u32, 0x202a..=0x202e | 0x2066..=0x2069 | 0x200e | 0x200f | 0x061c)
}

/// Unicode format characters (Cf) and the other invisible characters Den
/// removes. Bidi controls, joiners and U+FE0E/U+FE0F are handled where met.
fn invisible(r: char) -> bool {
    matches!(r as u32,
        // Cf, Unicode 15.
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x0890..=0x0891 | 0x08e2 | 0x180e
        | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff
        | 0xfff9..=0xfffb | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f
        // Others that hide text.
        | 0x034f | 0x115f | 0x1160 | 0x3164 | 0xffa0
        | 0xfe00..=0xfe0f | 0xe0100..=0xe01ef)
}

fn joins(j: char, base: Option<char>, last: Option<char>, next: Option<char>) -> bool {
    if last != base && !(j == ZWJ && last == Some(VS16)) {
        return false; // one joiner at a time, right after what it joins
    }
    let (Some(base), Some(next)) = (base, next) else {
        return false;
    };
    if j == ZWJ && emoji(base) && emoji(next) {
        return true;
    }
    joiner_script(base) && joiner_script(next)
}

/// A letter or mark of a script that spells with joiners (block ranges).
fn joiner_script(r: char) -> bool {
    if r.is_numeric() || r.is_whitespace() || invisible(r) {
        return false;
    }
    matches!(r as u32,
        0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefe // Arabic
        | 0x0700..=0x074f | 0x0860..=0x086f // Syriac
        | 0x07c0..=0x07ff // N'Ko
        | 0x0900..=0x0dff // Devanagari .. Sinhala
        | 0xa8e0..=0xa8ff // Devanagari extended
        | 0x1800..=0x18af // Mongolian
        | 0x1780..=0x17ff | 0x19e0..=0x19ff // Khmer
        | 0x1000..=0x109f | 0xa9e0..=0xa9ff | 0xaa60..=0xaa7f) // Myanmar
}

fn emoji(r: char) -> bool {
    matches!(r as u32,
        0x1f000..=0x1faff | 0x2300..=0x23ff | 0x2600..=0x27bf | 0x2b00..=0x2bff
        | 0x00a9 | 0x00ae | 0x203c | 0x2049 | 0x2122 | 0x2139)
}

fn skip_escape(s: &[u8], i: usize) -> usize {
    let Some(&c) = s.get(i) else {
        return i;
    };
    match c {
        b'[' => skip_csi(s, i + 1),
        b']' | b'P' | b'X' | b'^' | b'_' => skip_string(s, i + 1),
        0x20..=0x2f => {
            let mut i = i;
            while i < s.len() && (0x20..=0x2f).contains(&s[i]) {
                i += 1;
            }
            if i < s.len() && (0x30..=0x7e).contains(&s[i]) {
                i += 1;
            }
            i
        }
        0x30..=0x7e => i + 1,
        _ => i, // a lone ESC
    }
}

fn skip_csi(s: &[u8], mut i: usize) -> usize {
    while i < s.len() {
        let c = s[i];
        match c {
            0x40..=0x7e => return i + 1,
            0x20..=0x3f => i += 1,
            CAN | SUB => return i + 1,
            _ => return i,
        }
    }
    i
}

fn skip_string(s: &[u8], mut i: usize) -> usize {
    while i < s.len() {
        let c = s[i];
        match c {
            BEL | CAN | SUB => return i + 1,
            ESC => {
                if s.get(i + 1) == Some(&b'\\') {
                    return i + 2;
                }
                return i;
            }
            0xc2 if s.get(i + 1) == Some(&0x9c) => return i + 2,
            _ => {}
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_escapes_and_controls() {
        assert_eq!(text("a\x1b[31mred\x1b[0m b"), "ared b");
        assert_eq!(text("x\x1b]52;c;aGVsbG8=\x07y"), "xy");
        assert_eq!(text("x\x1b]0;title\x1b\\y"), "xy");
        assert_eq!(text("a\rb\r\nc"), "a\nb\nc");
        assert_eq!(text("a\u{202e}b\u{2066}c"), "abc");
        assert_eq!(text("a\u{200b}b\u{feff}c\u{e0041}"), "abc");
        assert_eq!(text("a\u{2028}b"), "a\nb");
        assert_eq!(text("tab\tok\n"), "tab\tok\n");
        assert_eq!(text("\x1b(Bx"), "x");
        assert_eq!(text("never ends \x1b]8;;http://x"), "never ends ");
        assert_eq!(text("c1 \u{9b}31mx"), "c1 x");
    }

    #[test]
    fn keeps_ordinary_unicode() {
        for s in [
            "日本語",
            "café",
            "👨\u{200d}👩\u{200d}👧",
            "❤\u{fe0f}",
            "می\u{200c}خواهم",
        ] {
            assert_eq!(text(s), s, "{s:?}");
        }
        // A joiner between Latin letters hides text: removed.
        assert_eq!(text("a\u{200d}b"), "ab");
        assert_eq!(text("a\u{200d}\u{200d}👩"), "a👩");
    }

    #[test]
    fn idempotent() {
        for s in ["a\x1b[1mb", "x\ry", "👨\u{200d}👩", "a\u{200d}b"] {
            assert_eq!(text(&text(s)), text(s));
        }
    }

    #[test]
    fn line_code_label() {
        assert_eq!(line("  a\n\tb   c  "), "a b c");
        assert_eq!(code("a  b\nc"), "a  b c");
        assert_eq!(label("hello world", 6), "hello…");
        assert_eq!(label("hi", 6), "hi");
        assert_eq!(hidden("ok"), None);
        assert_eq!(hidden("a\rb"), Some('\r'));
        assert_eq!(text_bytes(b"a\xffb"), "ab");
    }
}
