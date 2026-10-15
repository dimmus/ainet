//! Setting one value in a JSON-with-comments file (settings.json) by
//! editing its text: everything else (comments, order, spacing, trailing
//! commas) stays as written. Zed does this with tree-sitter
//! (`settings_json`); a small scanner does here, since only an object's
//! members need finding.

use std::ops::Range;

/// `text` with the value at `path` (keys from the top object down) set to
/// `value`, JSON already. An existing value is replaced where it stands;
/// a missing key is added first in its object, with what nests under it.
pub fn set(text: &str, path: &[&str], value: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let start = skip_trivia(bytes, 0)?;
    if start == bytes.len() {
        return Ok(format!("{{\n  {}\n}}\n", member(path, value, 1)));
    }
    if bytes[start] != b'{' {
        return Err("the file is not a JSON object".into());
    }
    let mut open = start;
    for (depth, key) in path.iter().enumerate() {
        match find(text, open, key)? {
            (Some(at), _) if depth + 1 == path.len() => {
                return Ok(format!("{}{value}{}", &text[..at.start], &text[at.end..]));
            }
            (Some(at), _) if bytes[at.start] == b'{' => open = at.start,
            (Some(_), _) => return Err(format!("\"{key}\" is not an object")),
            (None, members) => {
                let indent = "  ".repeat(depth + 1);
                let new = member(&path[depth..], value, depth + 1);
                let insert = if members > 0 {
                    format!("\n{indent}{new},")
                } else {
                    format!("\n{indent}{new}\n{}", "  ".repeat(depth))
                };
                let at = open + 1;
                return Ok(format!("{}{insert}{}", &text[..at], &text[at..]));
            }
        }
    }
    Err("no key to set".into())
}

/// `"a": { "b": value }` for the path `a.b`, indented from `depth`.
fn member(path: &[&str], value: &str, depth: usize) -> String {
    let key = serde_json::to_string(path[0]).unwrap_or_default();
    match &path[1..] {
        [] => format!("{key}: {value}"),
        rest => {
            let indent = "  ".repeat(depth + 1);
            format!(
                "{key}: {{\n{indent}{}\n{}}}",
                member(rest, value, depth + 1),
                "  ".repeat(depth)
            )
        }
    }
}

/// In the object opening at `open`: where `key`'s value is, if it is
/// there, and how many members were read.
fn find(text: &str, open: usize, key: &str) -> Result<(Option<Range<usize>>, usize), String> {
    let bytes = text.as_bytes();
    let mut i = open + 1;
    let mut members = 0;
    loop {
        i = skip_trivia(bytes, i)?;
        match bytes.get(i) {
            None => return Err("an object is not closed".into()),
            Some(b'}') => return Ok((None, members)),
            Some(b',') => i += 1,
            Some(b'"') => {
                let end = string_end(bytes, i)?;
                let name: String = serde_json::from_str(&text[i..end]).map_err(|e| e.to_string())?;
                i = skip_trivia(bytes, end)?;
                if bytes.get(i) != Some(&b':') {
                    return Err(format!("no ':' after \"{name}\""));
                }
                i = skip_trivia(bytes, i + 1)?;
                let at = value(bytes, i)?;
                members += 1;
                if name == key {
                    return Ok((Some(at), members));
                }
                i = at.end;
            }
            Some(c) => return Err(format!("unexpected '{}'", *c as char)),
        }
    }
}

/// Past whitespace and comments.
fn skip_trivia(bytes: &[u8], mut i: usize) -> Result<usize, String> {
    loop {
        match (bytes.get(i), bytes.get(i + 1)) {
            (Some(c), _) if c.is_ascii_whitespace() => i += 1,
            (Some(b'/'), Some(b'/')) => {
                while bytes.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
            }
            (Some(b'/'), Some(b'*')) => {
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
            }
            _ => return Ok(i),
        }
    }
}

/// Past the string starting at `i` (its opening quote).
fn string_end(bytes: &[u8], mut i: usize) -> Result<usize, String> {
    i += 1;
    loop {
        match bytes.get(i) {
            Some(b'\\') => i += 2,
            Some(b'"') => return Ok(i + 1),
            Some(_) => i += 1,
            None => return Err("a string is not closed".into()),
        }
    }
}

/// Where the value starting at `i` ends.
fn value(bytes: &[u8], i: usize) -> Result<Range<usize>, String> {
    match bytes.get(i) {
        None => Err("a value is missing".into()),
        Some(b'"') => Ok(i..string_end(bytes, i)?),
        Some(b'{' | b'[') => {
            let mut depth = 0;
            let mut j = i;
            loop {
                j = skip_trivia(bytes, j)?;
                match bytes.get(j) {
                    None => return Err("an object is not closed".into()),
                    Some(b'"') => j = string_end(bytes, j)?,
                    Some(b'{' | b'[') => {
                        depth += 1;
                        j += 1;
                    }
                    Some(b'}' | b']') => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            return Ok(i..j);
                        }
                    }
                    Some(_) => j += 1,
                }
            }
        }
        Some(_) => {
            let mut j = i;
            while let Some(c) = bytes.get(j) {
                if matches!(c, b',' | b'}' | b']' | b'/') || c.is_ascii_whitespace() {
                    break;
                }
                j += 1;
            }
            Ok(i..j)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_a_value_in_place_and_keeps_the_rest() {
        let text = r#"// My settings.
{
  "ui_font_size": 15, // big enough
  /* the colours */ "theme": "One Dark",
  "terminal": { "font_size": 14 },
}
"#;
        let set_theme = set(text, &["theme"], "\"Ayu Light\"").unwrap();
        assert_eq!(set_theme, text.replace("\"One Dark\"", "\"Ayu Light\""));
        let nested = set(text, &["terminal", "font_size"], "16").unwrap();
        assert_eq!(nested, text.replace("14", "16"));
    }

    #[test]
    fn adds_a_missing_key_first_in_its_object() {
        assert_eq!(
            set("{\n  \"confirm_quit\": true,\n}\n", &["theme"], "\"X\"").unwrap(),
            "{\n  \"theme\": \"X\",\n  \"confirm_quit\": true,\n}\n"
        );
        // Only comments in it, as the template.
        assert_eq!(
            set("{\n  // \"theme\": \"Y\",\n}", &["theme"], "\"X\"").unwrap(),
            "{\n  \"theme\": \"X\"\n\n  // \"theme\": \"Y\",\n}"
        );
        assert_eq!(
            set("", &["theme"], "\"X\"").unwrap(),
            "{\n  \"theme\": \"X\"\n}\n"
        );
        assert_eq!(
            set("{}", &["theme", "dark"], "\"X\"").unwrap(),
            "{\n  \"theme\": {\n    \"dark\": \"X\"\n  }\n}"
        );
    }

    #[test]
    fn strings_and_comments_hide_braces_and_keys() {
        let text = r#"{ "a": "} \" \"theme\": 1", /* "theme": 2 } */ "b": [1, {"theme": 3}], "theme": 4 }"#;
        let done = set(text, &["theme"], "5").unwrap();
        assert!(done.ends_with(r#""theme": 5 }"#), "{done}");
    }

    #[test]
    fn says_what_it_cannot_edit() {
        assert!(set("[1]", &["theme"], "1").is_err());
        assert!(set("{ \"theme\": ", &["theme"], "1").is_err());
        assert!(set("{ \"a\": 1 /* no end", &["theme"], "1").is_err());
        assert!(set("{ \"theme\": 1 }", &["theme", "dark"], "1").is_err());
    }
}
