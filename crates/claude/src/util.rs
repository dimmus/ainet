//! Small helpers shared by the modules: where Claude Code keeps its files,
//! Go-compatible JSON output, lexical path cleaning and RFC 3339 times.

use std::path::PathBuf;

/// The user's home directory, from `$HOME` (as Go's `os.UserHomeDir` reads
/// it on Unix). `None` when unset or empty.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Claude Code's configuration directory: `$CLAUDE_CONFIG_DIR` when set and
/// not empty, else `~/.claude`.
pub fn config_dir() -> PathBuf {
    config_dir_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        home_dir(),
    )
}

/// [`config_dir`] from explicit values of `$CLAUDE_CONFIG_DIR` and `$HOME`.
pub fn config_dir_from(claude_config_dir: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    match claude_config_dir {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => home.unwrap_or_default().join(".claude"),
    }
}

/// Escapes what Go's `encoding/json` escapes and serde_json does not: `<`,
/// `>` and `&` (as `<`, `>`, `&`) and U+2028, U+2029. In JSON
/// these characters only occur inside strings, so this is safe on a whole
/// encoded document and makes serde_json's output byte-identical to Go's.
pub(crate) fn go_escape(json: &str) -> String {
    if !json
        .chars()
        .any(|c| matches!(c, '<' | '>' | '&' | '\u{2028}' | '\u{2029}'))
    {
        return json.to_string();
    }
    let mut out = String::with_capacity(json.len() + 16);
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Removes insignificant whitespace from valid JSON text, as Go's
/// `json.Compact` does (what `json.Marshal` does to a `json.RawMessage`).
/// String contents are kept byte for byte.
pub(crate) fn compact_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_str = false;
    let mut escaped = false;
    for c in raw.chars() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            ' ' | '\t' | '\n' | '\r' => {}
            '"' => {
                in_str = true;
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// Go-style `json.Marshal` of a string value.
pub(crate) fn go_string(s: &str) -> String {
    go_escape(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()))
}

/// `filepath.Clean` for slash-separated paths: the shortest lexical
/// equivalent, "." for an empty path.
pub(crate) fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let body = parts.join("/");
    match (rooted, body.is_empty()) {
        (true, _) => format!("/{body}"),
        (false, true) => ".".into(),
        (false, false) => body,
    }
}

/// `filepath.Join`: the non-empty elements joined with "/" and cleaned.
pub(crate) fn join(elems: &[&str]) -> String {
    let kept: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if kept.is_empty() {
        return String::new();
    }
    clean(&kept.join("/"))
}

/// `filepath.Dir` of a cleaned path.
pub(crate) fn dir(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => clean(&path[..i]),
        None => ".".into(),
    }
}

/// `filepath.Base`.
pub(crate) fn base(path: &str) -> String {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        return if path.is_empty() { ".".into() } else { "/".into() };
    }
    match p.rfind('/') {
        Some(i) => p[i + 1..].to_string(),
        None => p.to_string(),
    }
}

/// Milliseconds since the epoch for an RFC 3339 time such as
/// `2026-10-08T10:00:05.000Z` or `2026-10-08T12:00:05+02:00`; `None` when it
/// does not parse.
pub(crate) fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse::<i64>().ok() };
    let (y, mo, d, h, mi, se) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        let frac = &s[start..i.min(start + 3)];
        ms = frac.parse::<i64>().ok()? * 10i64.pow(3 - frac.len() as u32);
    }
    let offset_min = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let v = oh * 60 + om;
            if *sign == b'-' { -v } else { v }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + se - offset_min * 60;
    Some(secs * 1000 + ms)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Milliseconds since the Unix epoch, now.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let p = std::env::temp_dir().join(format!("ainet-claude-{}-{n}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).expect("temp dir");
            // Resolve symlinks (macOS /tmp, or a symlinked TMPDIR) so tests
            // that compare real paths agree.
            TempDir(std::fs::canonicalize(&p).unwrap_or(p))
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
        pub fn join(&self, p: &str) -> PathBuf {
            self.0.join(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        for (input, want) in [
            ("", "."),
            ("/", "/"),
            ("a/b/../c", "a/c"),
            ("/home/user/proj//sub/./x.txt", "/home/user/proj/sub/x.txt"),
            ("/..", "/"),
            ("../x", "../x"),
            ("a/../../x", "../x"),
            ("/home/user/proj/", "/home/user/proj"),
        ] {
            assert_eq!(clean(input), want, "{input:?}");
        }
        assert_eq!(dir("/a/b"), "/a");
        assert_eq!(dir("/a"), "/");
        assert_eq!(base("/a/b.txt"), "b.txt");
        assert_eq!(join(&["/home/user", "~", "x"]), "/home/user/~/x");
    }

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-10-08T10:00:05.000Z"),
            Some(1_791_453_605_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-10-08T12:00:05.5+02:00"),
            Some(1_791_453_605_500)
        );
        assert_eq!(parse_rfc3339_ms("nope"), None);
    }

    #[test]
    fn go_escaping_and_compact() {
        assert_eq!(
            go_escape(r#"{"a":"x<y>&z"}"#),
            "{\"a\":\"x\\u003cy\\u003e\\u0026z\"}"
        );
        assert_eq!(compact_json("{ \"a\" : [ 1, \"b c\" ] }\n"), r#"{"a":[1,"b c"]}"#);
        assert_eq!(compact_json(r#"{"a":"q\" x"}"#), r#"{"a":"q\" x"}"#);
    }

    #[test]
    fn config_dir_resolution() {
        assert_eq!(
            config_dir_from(Some("/c".into()), Some("/h".into())),
            PathBuf::from("/c")
        );
        assert_eq!(
            config_dir_from(Some("".into()), Some("/h".into())),
            PathBuf::from("/h/.claude")
        );
        assert_eq!(
            config_dir_from(None, Some("/h".into())),
            PathBuf::from("/h/.claude")
        );
    }
}
