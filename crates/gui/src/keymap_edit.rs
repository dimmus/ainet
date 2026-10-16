//! Changing one binding in keymap.json by editing its text, as the
//! settings are edited: a JSON array of sections (`context`, `bindings`),
//! with comments and trailing commas kept where they are. A binding goes
//! into the last section whose context is exactly the one asked for (the
//! later section wins a tie), or into a new section at the end.

use std::ops::Range;

use serde_json::Value;

use crate::keymap;

/// `text` with `keys` bound to `action` (an action name, `[name, input]`,
/// or `null` to unbind a default) in `context`.
pub fn bind(text: &str, context: &str, keys: &str, action: &Value) -> Result<String, String> {
    let json = serde_json::to_string(action).map_err(|e| e.to_string())?;
    let sections = sections(text)?;
    let edited = match sections.iter().rev().find(|s| s.context == context) {
        Some(section) => {
            let old = &text[section.range.clone()];
            let new = ainet_core::jsonc::set(old, &["bindings", keys], &json)
                .map_err(|e| format!("keymap.json: {e}"))?;
            let new = reindent(old, &new, indent_at(text, section.range.start));
            format!(
                "{}{new}{}",
                &text[..section.range.start],
                &text[section.range.end..]
            )
        }
        None => append(text, &sections, &new_section(context, keys, &json))?,
    };
    checked(edited)
}

/// `text` without the user's binding of `keys` in `context` (in every
/// section of that context), so the default, if any, applies again.
pub fn unbind(text: &str, context: &str, keys: &str) -> Result<String, String> {
    let mut edited = text.to_string();
    // From the last, so the ranges before it stay where they are.
    for section in sections(text)?.iter().rev().filter(|s| s.context == context) {
        let old = &edited[section.range.clone()];
        let new =
            ainet_core::jsonc::unset(old, &["bindings", keys]).map_err(|e| format!("keymap.json: {e}"))?;
        edited = format!(
            "{}{new}{}",
            &edited[..section.range.start],
            &edited[section.range.end..]
        );
    }
    checked(edited)
}

/// What was written must read back as a keymap.
fn checked(edited: String) -> Result<String, String> {
    keymap::parse(&edited).map_err(|e| format!("keymap.json would not read: {e}"))?;
    Ok(edited)
}

/// A section of the file: where its object is, and its context.
struct Section {
    range: Range<usize>,
    context: String,
}

/// The sections of a keymap file's text, where they are. Comments and
/// trailing commas are blanked out first, which keeps every position.
fn sections(text: &str) -> Result<Vec<Section>, String> {
    let plain = ainet_core::jsonc::strip(text).map_err(|e| format!("keymap.json: {e}"))?;
    let bytes = plain.as_bytes();
    let mut i = skip_space(bytes, 0);
    if i == bytes.len() {
        return Ok(Vec::new());
    }
    if bytes[i] != b'[' {
        return Err("keymap.json is not a list of sections".into());
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        i = skip_space(bytes, i);
        match bytes.get(i) {
            None => return Err("keymap.json: the list is not closed".into()),
            Some(b']') => return Ok(out),
            Some(b',') => i += 1,
            Some(b'{') => {
                let end = object_end(bytes, i)?;
                let value: Value =
                    serde_json::from_str(&plain[i..end]).map_err(|e| format!("keymap.json: {e}"))?;
                let context = value
                    .get("context")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                out.push(Section {
                    range: i..end,
                    context,
                });
                i = end;
            }
            Some(c) => return Err(format!("keymap.json: unexpected '{}'", *c as char)),
        }
    }
}

fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Just past the object opening at `open`, strings minded.
fn object_end(bytes: &[u8], open: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(&c) = bytes.get(i) {
        match c {
            b'"' => {
                i += 1;
                loop {
                    match bytes.get(i) {
                        Some(b'\\') => i += 2,
                        Some(b'"') => break,
                        Some(_) => i += 1,
                        None => return Err("keymap.json: a string is not closed".into()),
                    }
                }
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err("keymap.json: a section is not closed".into())
}

/// The spaces before `at` on its line, when nothing else is there.
fn indent_at(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let before = &text[start..at];
    if before.chars().all(|c| c == ' ' || c == '\t') {
        before
    } else {
        ""
    }
}

/// `new` (`old` edited as if it began a line of its own) with the lines
/// it gained indented by `indent`, as the section they are in.
fn reindent(old: &str, new: &str, indent: &str) -> String {
    if indent.is_empty() {
        return new.to_string();
    }
    let mut prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    // From the start of an added line, not part way into its indent.
    if let Some(nl) = new[..prefix].rfind('\n')
        && new[nl + 1..prefix].chars().all(|c| c == ' ' || c == '\t')
    {
        prefix = nl;
    }
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let mut suffix = suffix;
    while !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    let (head, middle, tail) = (
        &new[..prefix],
        &new[prefix..new.len() - suffix],
        &new[new.len() - suffix..],
    );
    format!("{head}{}{tail}", middle.replace('\n', &format!("\n{indent}")))
}

/// A section of its own for one binding, indented as a top-level item.
fn new_section(context: &str, keys: &str, action: &str) -> String {
    let keys = serde_json::to_string(keys).unwrap_or_default();
    let context = match context {
        "" => String::new(),
        c => format!(
            "\n    \"context\": {},",
            serde_json::to_string(c).unwrap_or_default()
        ),
    };
    format!("{{{context}\n    \"bindings\": {{\n      {keys}: {action}\n    }}\n  }}")
}

/// `text` with `section` added after the last section (or as the first).
fn append(text: &str, sections: &[Section], section: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Ok(format!("[\n  {section}\n]\n"));
    }
    let at = match sections.last() {
        Some(last) => last.range.end,
        None => {
            let plain = ainet_core::jsonc::strip(text).map_err(|e| format!("keymap.json: {e}"))?;
            plain.find('[').ok_or("keymap.json is not a list of sections")? + 1
        }
    };
    let comma = if sections.is_empty() { "" } else { "," };
    Ok(format!("{}{comma}\n  {section}{}", &text[..at], &text[at..]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn bound(text: &str) -> Vec<(String, String, Value)> {
        keymap::parse(text)
            .unwrap()
            .into_iter()
            .flat_map(|s| {
                let context = s.context.clone();
                s.bindings
                    .0
                    .into_iter()
                    .map(move |(k, v)| (context.clone(), k, v))
            })
            .collect()
    }

    #[test]
    fn binds_into_the_section_of_that_context_keeping_comments() {
        let text = "// Mine.\n[\n  {\n    \"context\": \"Workspace\",\n    \"bindings\": {\n      \"ctrl-1\": \"workspace::OpenMap\", // the map\n    },\n  },\n]\n";
        let edited = bind(text, "Workspace", "ctrl-2", &json!("workspace::OpenInbox")).unwrap();
        assert!(edited.starts_with("// Mine.\n"), "{edited}");
        assert!(edited.contains("\"workspace::OpenMap\", // the map"), "{edited}");
        assert!(
            edited.contains("\n      \"ctrl-2\": \"workspace::OpenInbox\","),
            "indented as its section: {edited}"
        );
        assert_eq!(bound(&edited).len(), 2);
        // The same keys again: replaced where they stand.
        let again = bind(&edited, "Workspace", "ctrl-1", &Value::Null).unwrap();
        assert!(again.contains("\"ctrl-1\": null, // the map"), "{again}");
    }

    #[test]
    fn a_context_not_there_gets_a_section_at_the_end() {
        let empty = bind("", "Terminal", "ctrl-alt-k", &json!(["some::Action", {"x": 1}])).unwrap();
        assert_eq!(
            bound(&empty),
            [(
                "Terminal".into(),
                "ctrl-alt-k".into(),
                json!(["some::Action", {"x": 1}])
            )]
        );
        let template = bind(keymap::TEMPLATE, "", "ctrl-q", &Value::Null).unwrap();
        assert!(template.starts_with("// Your key bindings"), "{template}");
        assert_eq!(bound(&template), [("".into(), "ctrl-q".into(), Value::Null)]);
        let two = bind(&empty, "Workspace", "ctrl-1", &json!("workspace::OpenMap")).unwrap();
        assert_eq!(bound(&two).len(), 2);
        assert_eq!(bound(&two)[1].0, "Workspace");
    }

    #[test]
    fn the_last_section_of_a_context_wins_and_unbinding_clears_them_all() {
        let text = r#"[
  { "context": "Inbox", "bindings": { "a": "inbox::Allow" } },
  { "context": "Inbox", "bindings": { "a": "inbox::Deny", } },
]"#;
        let edited = bind(text, "Inbox", "b", &json!("inbox::Always")).unwrap();
        let second = edited.rfind("\"context\"").unwrap();
        assert!(edited.find("inbox::Always").unwrap() > second, "{edited}");
        let gone = unbind(&edited, "Inbox", "a").unwrap();
        assert_eq!(
            bound(&gone),
            [("Inbox".into(), "b".into(), json!("inbox::Always"))]
        );
        assert_eq!(unbind(&gone, "Nowhere", "a").unwrap(), gone, "nothing to remove");
    }

    #[test]
    fn a_broken_file_is_left_alone() {
        assert!(bind("[{", "", "a", &json!("x::Y")).is_err());
        assert!(bind("{}", "", "a", &json!("x::Y")).is_err());
        assert!(unbind("[ { \"bindings\": ", "", "a").is_err());
    }
}
