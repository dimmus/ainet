//! JSON with comments and trailing commas, as Zed's settings files and
//! ainet's `settings.json` are written: reading it ([`parse`],
//! [`to_value`]) and changing one value in its text ([`set`], [`unset`])
//! with everything else (comments, order, spacing) left as written.
//!
//! The reader blanks out comments and trailing commas and hands the rest
//! to `serde_json`, so line and column numbers in its errors are the
//! file's own.

mod edit;

pub use edit::{set, unset};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Reads JSON with `//` and `/* */` comments and trailing commas.
pub fn parse<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(&strip(text)?).map_err(|e| e.to_string())
}

/// [`parse`] into a `serde_json::Value`.
pub fn to_value(text: &str) -> Result<Value, String> {
    parse(text)
}

/// `text` with comments and trailing commas turned into spaces (newlines
/// kept), so plain JSON remains at the same positions.
pub fn strip(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    // Where the last comma outside strings and comments was, until
    // something other than whitespace or a comment follows it.
    let mut comma: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1)) {
            (b'"', _) => {
                comma = None;
                i = edit::string_end(bytes, i)?;
            }
            (b'/', Some(b'/')) => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                let start = i;
                i += 2;
                loop {
                    match (bytes.get(i), bytes.get(i + 1)) {
                        (Some(b'*'), Some(b'/')) => {
                            i += 2;
                            break;
                        }
                        (Some(_), _) => i += 1,
                        (None, _) => return Err("a comment is not closed".into()),
                    }
                }
                for b in &mut out[start..i] {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
            }
            (b',', _) => {
                comma = Some(i);
                i += 1;
            }
            (b'}' | b']', _) => {
                if let Some(at) = comma.take() {
                    out[at] = b' ';
                }
                i += 1;
            }
            (c, _) if c.is_ascii_whitespace() => i += 1,
            _ => {
                comma = None;
                i += 1;
            }
        }
    }
    // Only ASCII bytes were replaced, by ASCII spaces.
    String::from_utf8(out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_comments_and_trailing_commas() {
        let v = to_value(
            "// top\n{\n  \"a\": [1, 2,], /* in /* line */\n  \"b\": { \"c\": \"// not a comment, \",},\n}\n",
        )
        .unwrap();
        assert_eq!(v["a"], serde_json::json!([1, 2]));
        assert_eq!(v["b"]["c"], "// not a comment, ");
    }

    #[test]
    fn errors_point_at_the_file() {
        let e = to_value("// one\n// two\n{ \"a\": tru }").unwrap_err();
        assert!(e.contains("line 3"), "{e}");
        assert!(to_value("{ /* open").is_err());
        assert!(to_value("{ \"a\": \"open").is_err());
        assert!(to_value("[1,,]").is_err(), "only one trailing comma");
    }

    #[test]
    fn escapes_in_strings() {
        let v = to_value(r#"{ "a": "q\"/*,}", "b": 1, }"#).unwrap();
        assert_eq!(v["a"], "q\"/*,}");
        assert_eq!(v["b"], 1);
    }
}
