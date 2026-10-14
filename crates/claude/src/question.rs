//! An `AskUserQuestion` tool call, and the answer a `PermissionRequest` hook
//! gives back so the question can be answered outside the terminal.
//!
//! Ported from Den's `internal/question` (MIT).
//!
//! Answering through a hook is undocumented Claude Code behaviour, which Den
//! checked against Claude Code 2.1.289 in two ways: the strings of the
//! installed binary (the hook output schema, the `AskUserQuestion` input and
//! output schemas, the function that joins multi-select answers), and the
//! real binary driven by a mock of the Messages API:
//!
//! - The input is `{"questions":[{"question","header","options":[{"label",
//!   "description"}],"multiSelect"}], …}`.
//! - The hook answers with allow and `updatedInput` = the call's own input,
//!   unchanged, plus `"answers": {"<question text>": "<answer>"}` for every
//!   question. Claude Code refuses an answer whose input changes a field the
//!   dialog showed, or that already held `answers`. It then skips (or closes)
//!   its dialog, shows "Allowed by PermissionRequest hook", and the model
//!   gets "The user answered: …". Accepted in the default, acceptEdits, plan,
//!   auto and bypassPermissions modes. A bare allow without answers is
//!   ignored and the dialog stays.
//! - A multi-select answer is one string, the labels joined with ", ", a
//!   label holding ", " or a double quote written as a JSON string ([`join`]).
//!   An array sent through the hook reached the model as `Icons,Say "hi"`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::util;

/// The tool name Claude Code gives the question dialog.
pub const TOOL: &str = "AskUserQuestion";

/// One question of a call. `text` is also its key in the answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    pub header: String,
    pub options: Vec<QOption>,
    /// multiSelect: several options may be chosen.
    pub multi: bool,
}

/// One choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QOption {
    pub label: String,
    pub description: String,
}

#[derive(Deserialize)]
struct RawInput {
    #[serde(default)]
    questions: Vec<RawQuestion>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawQuestion {
    question: Option<String>,
    header: Option<String>,
    #[serde(rename = "multiSelect")]
    multi_select: Option<bool>,
    options: Option<Vec<RawOption>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawOption {
    label: Option<String>,
    description: Option<String>,
}

/// Reads a call's `tool_input`. `None` when there is no question, a question
/// has no text, or two share a text (answers are keyed by it).
pub fn parse(tool_input: &str) -> Option<Vec<Question>> {
    let input: RawInput = serde_json::from_str(tool_input).ok()?;
    if input.questions.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for q in input.questions {
        let text = q.question.unwrap_or_default();
        if text.is_empty() || !seen.insert(text.clone()) {
            return None;
        }
        out.push(Question {
            text,
            header: q.header.unwrap_or_default(),
            multi: q.multi_select.unwrap_or_default(),
            options: q
                .options
                .unwrap_or_default()
                .into_iter()
                .map(|o| QOption {
                    label: o.label.unwrap_or_default(),
                    description: o.description.unwrap_or_default(),
                })
                .collect(),
        });
    }
    Some(out)
}

/// Turns what the human typed into the answer for `q`. A number picks that
/// option; for a multi-select question several numbers, separated by spaces
/// or commas, pick several (repeats count once, in the order typed). Anything
/// else is the human's own answer, as the dialog's "Type something" row takes
/// it; a number meant as text goes in double quotes (`"3"`). `None` for an
/// empty answer, several numbers for a single-select question, or a number
/// with no option.
pub fn pick(q: &Question, typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    if typed.len() >= 2 && typed.starts_with('"') && typed.ends_with('"') {
        let lit = typed[1..typed.len() - 1].trim();
        return (!lit.is_empty()).then(|| lit.to_string());
    }
    let mut nums = Vec::new();
    for f in typed.split([' ', ',']).filter(|f| !f.is_empty()) {
        match f.parse::<i64>() {
            Ok(n) => nums.push(n),
            Err(_) => return Some(typed.to_string()), // words: the human's own answer
        }
    }
    if nums.len() > 1 && !q.multi {
        return None;
    }
    let mut labels: Vec<String> = Vec::new();
    let mut chosen = std::collections::HashSet::new();
    for n in nums {
        if n < 1 || n as usize > q.options.len() {
            return None;
        }
        if chosen.insert(n) {
            labels.push(q.options[n as usize - 1].label.clone());
        }
    }
    if !q.multi {
        return labels.into_iter().next(); // the label as it is, as the dialog sends it
    }
    Some(join(&labels))
}

/// Several chosen labels as one answer, the way Claude Code's own dialog
/// joins them: ", " between, and a label that holds ", " or a double quote
/// written as a JSON string. Choosing Icons and Say "hi" gives
/// `Icons, "Say \"hi\""`.
pub fn join<S: AsRef<str>>(labels: &[S]) -> String {
    labels
        .iter()
        .map(|l| {
            let l = l.as_ref();
            if l.contains(", ") || l.contains('"') {
                util::go_string(l)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The hook's answer for the call `tool_input` once every question has an
/// answer: the full `hookSpecificOutput` line (without the newline), allow
/// with the call's own input plus `answers` as `updatedInput`. `None`, and
/// nothing must be printed, when an answer is missing, blank or for an
/// unknown question, or the input cannot carry answers (not an object, or
/// it already has `answers`); the dialog then stays.
pub fn output(tool_input: &str, answers: &BTreeMap<String, String>) -> Option<String> {
    let qs = parse(tool_input)?;
    if answers.len() != qs.len() {
        return None;
    }
    for q in &qs {
        match answers.get(&q.text) {
            Some(a) if !a.trim().is_empty() => {}
            _ => return None,
        }
    }
    let mut fields: BTreeMap<String, Box<RawValue>> = serde_json::from_str(tool_input).ok()?;
    if fields.contains_key("answers") {
        return None;
    }
    fields.insert(
        "answers".into(),
        RawValue::from_string(serde_json::to_string(answers).ok()?).ok()?,
    );
    let updated = util::compact_json(&serde_json::to_string(&fields).ok()?);
    Some(crate::hooks::output_allow_updated(&updated))
}

/// What the human is asked now, for display: the question's text, an intent
/// line ("Question 2 of 3 · Header · choose one or more") and the numbered
/// options ("Label — description"), all sanitised. `step` counts from 0.
pub fn describe(qs: &[Question], step: usize) -> Option<(String, String, Vec<String>)> {
    let q = qs.get(step)?;
    let mut intent = Vec::new();
    if qs.len() > 1 {
        intent.push(format!("Question {} of {}", step + 1, qs.len()));
    }
    if !q.header.is_empty() {
        intent.push(q.header.clone());
    }
    if q.multi {
        intent.push("choose one or more".into());
    }
    let options = q
        .options
        .iter()
        .map(|o| {
            let mut l = o.label.clone();
            if !o.description.is_empty() {
                l = format!("{l} — {}", o.description);
            }
            crate::sanitize::line(&l)
        })
        .collect();
    Some((
        crate::sanitize::line(&q.text),
        crate::sanitize::line(&intent.join(" · ")),
        options,
    ))
}

/// A one-line title for a question call's notice: the first question, with
/// "(+N more)" when there are several, and its option labels. Den's
/// `describeQuestion`, for a call shown but not held.
pub fn summary(tool_input: &str) -> (String, Vec<String>) {
    let v: serde_json::Value = serde_json::from_str(tool_input).unwrap_or_default();
    let Some(qs) = v
        .get("questions")
        .and_then(|q| q.as_array())
        .filter(|q| !q.is_empty())
    else {
        return ("Has a question for you".into(), Vec::new());
    };
    let q = &qs[0];
    let opts = q
        .get("options")
        .and_then(|o| o.as_array())
        .map(|o| {
            o.iter()
                .map(|x| crate::sanitize::line(x.get("label").and_then(|l| l.as_str()).unwrap_or("")))
                .collect()
        })
        .unwrap_or_default();
    let mut title = q
        .get("question")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if qs.len() > 1 {
        title.push_str(&format!(" (+{} more)", qs.len() - 1));
    }
    (crate::sanitize::line(&title), opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call Den's mock used against the real binary: one single-select
    /// question and one multi-select with awkward labels.
    const TWO_QUESTIONS: &str = r#"{"questions":[
 {"question":"Which colour scheme should the demo use?","header":"Colours","options":[{"label":"Warm","description":"Oranges and reds"},{"label":"Cool, calm","description":"Blues"}],"multiSelect":false},
 {"question":"Which extras should ship?","header":"Extras","options":[{"label":"Icons","description":"An icon set"},{"label":"Sounds","description":"Short sounds"},{"label":"Say \"hi\"","description":"A greeting"}],"multiSelect":true}
],"metadata":{"source":"test"}}"#;

    #[test]
    fn parses() {
        let qs = parse(TWO_QUESTIONS).unwrap();
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].header, "Colours");
        assert!(!qs[0].multi);
        assert_eq!(
            qs[0].options[1],
            QOption {
                label: "Cool, calm".into(),
                description: "Blues".into()
            }
        );
        assert!(qs[1].multi && qs[1].options.len() == 3);
        for bad in [
            "",
            "{}",
            r#"{"questions":[]}"#,
            r#"{"questions":[{"question":""}]}"#,
            r#"{"questions":[{"question":"A?"},{"question":"A?"}]}"#,
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn picks() {
        let qs = parse(TWO_QUESTIONS).unwrap();
        let (single, multi) = (&qs[0], &qs[1]);
        let cases: &[(&Question, &str, Option<&str>)] = &[
            (single, "2", Some("Cool, calm")),
            (single, " 1 ", Some("Warm")),
            (single, "1 2", None),
            (single, "3", None),
            (single, "0", None),
            (single, "", None),
            (single, "Something else, typed", Some("Something else, typed")),
            (single, "\"2\"", Some("2")),
            (single, "\"\"", None),
            (multi, "1 3", Some(r#"Icons, "Say \"hi\"""#)),
            (multi, "3,1", Some(r#""Say \"hi\"", Icons"#)),
            (multi, "1, 1", Some("Icons")),
            (multi, "2", Some("Sounds")),
            (multi, "1 4", None),
            (multi, "1 and 3", Some("1 and 3")),
        ];
        for (q, typed, want) in cases {
            assert_eq!(pick(q, typed).as_deref(), *want, "{} {typed:?}", q.text);
        }
    }

    #[test]
    fn join_matches_the_dialog() {
        assert_eq!(join(&["Icons", r#"Say "hi""#]), r#"Icons, "Say \"hi\"""#);
        assert_eq!(join(&["Cool, calm", "Warm"]), r#""Cool, calm", Warm"#);
        assert_eq!(join(&["a,b"]), "a,b");
    }

    #[test]
    fn output_keeps_the_input_and_adds_answers() {
        let answers = BTreeMap::from([
            (
                "Which colour scheme should the demo use?".to_string(),
                "Something else, typed".to_string(),
            ),
            (
                "Which extras should ship?".to_string(),
                r#"Icons, "Say \"hi\"""#.to_string(),
            ),
        ]);
        let out = output(TWO_QUESTIONS, &answers).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let d = &v["hookSpecificOutput"]["decision"];
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(d["behavior"], "allow");
        let orig: serde_json::Value = serde_json::from_str(TWO_QUESTIONS).unwrap();
        for (k, val) in orig.as_object().unwrap() {
            assert_eq!(&d["updatedInput"][k], val, "{k} changed");
        }
        let got: BTreeMap<String, String> =
            serde_json::from_value(d["updatedInput"]["answers"].clone()).unwrap();
        assert_eq!(got, answers);
        // Byte-level: top-level keys sorted, inner order kept, compact.
        assert!(out.starts_with(
            r#"{"hookSpecificOutput":{"decision":{"behavior":"allow","updatedInput":{"answers":{"Which colour scheme should the demo use?":"Something else, typed","Which extras should ship?":"Icons, \"Say \\\"hi\\\"\""},"metadata":{"source":"test"},"questions":[{"question":"Which colour scheme should the demo use?","header":"Colours","#
        ), "{out}");
    }

    #[test]
    fn output_refuses_incomplete_answers() {
        let one = |k: &str, v: &str| (k.to_string(), v.to_string());
        for answers in [
            BTreeMap::new(),
            BTreeMap::from([one("Which colour scheme should the demo use?", "Warm")]),
            BTreeMap::from([
                one("Which colour scheme should the demo use?", "Warm"),
                one("Which extras should ship?", " "),
            ]),
            BTreeMap::from([
                one("Which colour scheme should the demo use?", "Warm"),
                one("Another question?", "Icons"),
            ]),
        ] {
            assert_eq!(output(TWO_QUESTIONS, &answers), None, "{answers:?}");
        }
        let answered = r#"{"questions":[{"question":"A?","options":[]}],"answers":{"A?":"x"}}"#;
        assert_eq!(output(answered, &BTreeMap::from([one("A?", "y")])), None);
    }

    #[test]
    fn describes() {
        let qs = parse(TWO_QUESTIONS).unwrap();
        let (title, intent, options) = describe(&qs, 1).unwrap();
        assert_eq!(title, "Which extras should ship?");
        assert_eq!(intent, "Question 2 of 2 · Extras · choose one or more");
        assert_eq!(options[0], "Icons — An icon set");
        assert!(describe(&qs, 2).is_none());
        let (t, opts) = summary(TWO_QUESTIONS);
        assert_eq!(t, "Which colour scheme should the demo use? (+1 more)");
        assert_eq!(opts, vec!["Warm", "Cool, calm"]);
        assert_eq!(summary("{}").0, "Has a question for you");
    }
}
