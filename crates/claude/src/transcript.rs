//! Reads what ainet needs from a session's JSONL transcript without parsing
//! the whole file: most reads scan backwards from the end in 64 KiB chunks.
//!
//! Ported from Den's `internal/transcript` (MIT). The format is internal to
//! Claude Code and changes between releases, so everything here is best
//! effort: what does not parse is skipped.
//!
//! What Den relies on (Claude Code 2.1.289):
//!
//! - Transcripts live at `<config dir>/projects/<project>/<session id>.jsonl`;
//!   a session's subagents write `<session id>/subagents/*.jsonl` beside it.
//! - One JSON object per line with a `type`: `user`, `assistant`,
//!   `attachment`, `ai-title` (`aiTitle`, also the terminal title),
//!   `cost-state` (`totalCostUSD`, `hasUnknownModelCost`, lagging the
//!   messages), `worktree-state` (near the top, for `claude --worktree`).
//! - An assistant message with several content blocks is written once per
//!   block, each line repeating `message.id` and `message.usage`; while
//!   streaming, a later line of the same message may carry larger figures.
//!   Usage is `input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
//!   `cache_read_input_tokens`.
//! - The model in use is the newest `model` attachment
//!   (`attachment.identity.modelId`) or real reply (`message.model`). Claude
//!   Code makes up records for an API error or when logged out
//!   (`"model":"<synthetic>"`, `isApiErrorMessage`), which name no model.
//!   Plan mode may run another model: entering writes the plan model's
//!   attachment then a `plan_mode` attachment; leaving writes the default
//!   model's attachment then `plan_mode_exit`.
//! - A tool's result is a `user` line with a `tool_result` block
//!   (`tool_use_id`) and a `toolUseResult` object: `structuredPatch` hunks
//!   for Edit/Write (`type: "create"` for a new file), `stdout`, `stderr`
//!   and `bashEditDiff.files` for Bash.
//! - `isMeta` lines and prompts starting with `<` (command echoes, caveats,
//!   reminders) are not the human's words.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::{sanitize, util};

/// How much of a prompt or reply an entry keeps.
const EXCERPT_LINES: usize = 40;
/// How much of a command's output an entry keeps.
const OUTPUT_LINES: usize = 5;

/// Where Claude Code keeps per-project transcripts.
pub fn projects_dir() -> PathBuf {
    util::config_dir().join("projects")
}

/// The transcript of a session, `None` when there is none.
pub fn path_for(session_id: &str) -> Option<PathBuf> {
    path_in(&projects_dir(), session_id)
}

/// [`path_for`] under an explicit projects directory.
pub fn path_in(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || session_id.contains('/') {
        return None;
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(projects_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| d.join(format!("{session_id}.jsonl")))
        .find(|p| p.is_file())
}

/// Feeds complete lines from the end of the file to `f`, newest first, until
/// `f` returns true, the file is exhausted, or `limit` bytes have been read.
/// Returns whether the start of the file was reached.
pub fn scan_backwards(path: &Path, limit: u64, mut f: impl FnMut(&[u8]) -> bool) -> std::io::Result<bool> {
    const CHUNK: u64 = 64 << 10;
    let fh = File::open(path)?;
    let mut end = fh.metadata()?.len();
    let mut read = 0u64;
    let mut carry: Vec<u8> = Vec::new();
    while end > 0 && read < limit {
        let start = end.saturating_sub(CHUNK.min(limit - read));
        let mut buf = vec![0u8; (end - start) as usize];
        fh.read_exact_at(&mut buf, start)?;
        read += buf.len() as u64;
        buf.extend_from_slice(&carry);
        let lines: Vec<&[u8]> = buf.split(|&b| b == b'\n').collect();
        // lines[0] may be a partial line; keep it for the next chunk unless
        // this is the start of the file.
        let first = if start > 0 {
            carry = lines[0].to_vec();
            1
        } else {
            carry.clear();
            0
        };
        for l in lines[first..].iter().rev() {
            if !l.is_empty() && f(l) {
                return Ok(false);
            }
        }
        end = start;
    }
    Ok(end == 0)
}

fn contains(hay: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    hay.windows(n.len()).any(|w| w == n)
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for k in path {
        match cur.get(k) {
            Some(x) => cur = x,
            None => return "",
        }
    }
    cur.as_str().unwrap_or("")
}

fn i64_at(v: &Value, path: &[&str]) -> i64 {
    let mut cur = v;
    for k in path {
        match cur.get(k) {
            Some(x) => cur = x,
            None => return 0,
        }
    }
    cur.as_i64()
        .or_else(|| cur.as_f64().map(|f| f as i64))
        .unwrap_or(0)
}

/// The title and model from the end of a transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// Claude Code's AI-generated title, also the terminal title.
    pub title: String,
    /// Raw model id in use.
    pub model: String,
}

/// Scans the last 2 MiB backwards for the latest title and model.
pub fn read_facts(path: &Path) -> Facts {
    let mut f = Facts::default();
    let ok = scan_backwards(path, 2 << 20, |line| {
        if f.title.is_empty()
            && contains(line, "\"type\":\"ai-title\"")
            && let Ok(v) = serde_json::from_slice::<Value>(line)
        {
            f.title = sanitize::line(str_at(&v, &["aiTitle"]));
        }
        if f.model.is_empty()
            && let Some(m) = line_model(line)
        {
            f.model = m;
        }
        !f.title.is_empty() && !f.model.is_empty()
    });
    if ok.is_err() { Facts::default() } else { f }
}

/// The model a line says is in use, `None` when it says none.
fn line_model(line: &[u8]) -> Option<String> {
    if contains(line, "\"type\":\"assistant\"") {
        if contains(line, "\"isApiErrorMessage\":true") {
            return None;
        }
        let key = b"\"model\":\"";
        let i = line.windows(key.len()).position(|w| w == key)? + key.len();
        let len = line[i..].iter().position(|&b| b == b'"')?;
        let m = String::from_utf8_lossy(&line[i..i + len]).into_owned();
        if m.is_empty() || m == "<synthetic>" {
            return None;
        }
        return Some(sanitize::line(&m));
    }
    if contains(line, "\"type\":\"attachment\"") && contains(line, "\"modelId\"") {
        let v: Value = serde_json::from_slice(line).ok()?;
        if str_at(&v, &["type"]) == "attachment" && str_at(&v, &["attachment", "type"]) == "model" {
            let m = sanitize::line(str_at(&v, &["attachment", "identity", "modelId"]));
            return (!m.is_empty()).then_some(m);
        }
    }
    None
}

#[derive(Debug, Clone, Default)]
struct ModelRec {
    model: String,
    attach: bool,
    mode: &'static str,
}

/// The model a session runs outside plan mode, `None` when the transcript
/// does not say (a session started in plan mode has no record of it). For
/// resuming a session on the right model; reads up to 16 MiB back.
pub fn default_model(path: &Path) -> Option<String> {
    let mut recs: Vec<ModelRec> = Vec::new(); // newest first
    let mut settled: Option<String> = None;
    let _ = scan_backwards(path, 16 << 20, |line| {
        if let Some(mode) = plan_marker(line) {
            recs.push(ModelRec {
                mode,
                ..Default::default()
            });
        } else if let Some(m) = line_model(line) {
            recs.push(ModelRec {
                model: m,
                attach: contains(line, "\"type\":\"attachment\""),
                mode: "",
            });
        }
        settled = default_from(&recs);
        settled.is_some()
    });
    if settled.is_some() {
        return settled;
    }
    if recs.iter().any(|r| !r.mode.is_empty()) {
        return None;
    }
    first_model(&recs)
}

fn default_from(recs: &[ModelRec]) -> Option<String> {
    for (i, r) in recs.iter().enumerate() {
        match r.mode {
            "plan_mode_exit" => {
                if let Some(m) = first_model(&recs[..i]) {
                    return Some(m);
                }
                // Left plan mode with no reply since: the attachment that
                // came with the exit, just before it.
                return recs
                    .get(i + 1)
                    .filter(|r| !r.model.is_empty())
                    .map(|r| r.model.clone());
            }
            "plan_mode" => {
                let mut rest = &recs[i + 1..];
                if rest.first().is_some_and(|r| r.attach) {
                    rest = &rest[1..]; // the plan model's own attachment
                }
                return first_model(rest);
            }
            _ => {}
        }
    }
    None
}

fn first_model(recs: &[ModelRec]) -> Option<String> {
    recs.iter().find(|r| !r.model.is_empty()).map(|r| r.model.clone())
}

fn plan_marker(line: &[u8]) -> Option<&'static str> {
    if !contains(line, "\"type\":\"attachment\"") || !contains(line, "\"plan_mode") {
        return None;
    }
    let v: Value = serde_json::from_slice(line).ok()?;
    if str_at(&v, &["type"]) != "attachment" {
        return None;
    }
    match str_at(&v, &["attachment", "type"]) {
        "plan_mode" => Some("plan_mode"),
        "plan_mode_exit" => Some("plan_mode_exit"),
        _ => None,
    }
}

/// "claude-fable-5-1" → "Fable 5.1"; "claude-haiku-4-5-20251001" → "Haiku 4.5".
pub fn pretty_model(id: &str) -> String {
    let parts: Vec<&str> = id.split('-').collect();
    if parts.len() >= 3 && parts[0] == "claude" && !parts[1].is_empty() {
        let mut name = parts[1].to_string();
        if let Some(first) = name.get(..1) {
            name = first.to_uppercase() + &name[1..];
        }
        let mut ver = parts[2..].join(".");
        for date in [".2025", ".2026"] {
            if let Some(i) = ver.find(date).filter(|&i| i > 0) {
                ver.truncate(i);
            }
        }
        return format!("{name} {ver}");
    }
    id.to_string()
}

/// The context the last assistant turn consumed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextUsage {
    /// input + cache read + cache creation of the last assistant message.
    pub tokens: i64,
    /// The model's context window, best effort ([`window_for`]).
    pub window: i64,
    pub model: String,
}

impl ContextUsage {
    /// How full the context window is, 0–100.
    pub fn percent(&self) -> i64 {
        if self.window == 0 {
            return 0;
        }
        (self.tokens * 100 / self.window).min(100)
    }
}

/// The context window Den assumes: 1M for a model id that says so (`[1m]`,
/// `-1m`) or a context already past 200k, 200k otherwise. agent-badge's
/// figure (from Claude Code itself) is better when there is one.
pub fn window_for(model: &str, tokens: i64) -> i64 {
    if model.contains("[1m]") || model.contains("-1m") || tokens > 200_000 {
        1_000_000
    } else {
        200_000
    }
}

/// The last assistant message's context usage (reads up to 4 MiB back).
pub fn read_usage(path: &Path) -> Option<ContextUsage> {
    let mut found = None;
    let _ = scan_backwards(path, 4 << 20, |line| {
        if !contains(line, "\"type\":\"assistant\"") || !contains(line, "\"usage\"") {
            return false;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            return false;
        };
        let u = context_of(&v);
        let done = u.tokens > 0;
        found = Some(u);
        done
    });
    found.filter(|u| u.tokens > 0)
}

fn context_of(v: &Value) -> ContextUsage {
    let model = str_at(v, &["message", "model"]).to_string();
    let tokens = i64_at(v, &["message", "usage", "input_tokens"])
        + i64_at(v, &["message", "usage", "cache_read_input_tokens"])
        + i64_at(v, &["message", "usage", "cache_creation_input_tokens"]);
    ContextUsage {
        window: window_for(&model, tokens),
        tokens,
        model,
    }
}

/// What a recent-activity entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A prompt from the human.
    You,
    /// The assistant's words.
    Text,
    /// A tool call.
    Tool,
}

/// One line of recent activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Unix ms, when the line has a timestamp.
    pub at_ms: Option<i64>,
    pub kind: EntryKind,
    /// The prompt, the reply, or a tool call's description.
    pub text: String,
    /// The tool name, for a tool call.
    pub tool: String,
    /// The command, path or other main argument of a tool call.
    pub code: String,
    /// Files the call changed, as Claude Code recorded them.
    pub changes: Vec<FileChange>,
    /// The first lines a command printed.
    pub output: String,
}

/// One file an Edit, Write or Bash call changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileChange {
    /// Safe to print.
    pub path: String,
    /// The path as the transcript gave it, when cleaning it for printing
    /// changed it (it has control characters: do not show its content).
    pub raw: Option<String>,
    pub hunks: Vec<Hunk>,
    pub created: bool,
    pub deleted: bool,
}

impl FileChange {
    /// The added and removed lines of the change.
    pub fn counts(&self) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for l in self.hunks.iter().flat_map(|h| &h.lines) {
            if l.starts_with('+') {
                added += 1;
            } else if l.starts_with('-') {
                removed += 1;
            }
        }
        (added, removed)
    }

    /// The path the change was made to, to match it against files.
    pub fn real(&self) -> &str {
        self.raw.as_deref().unwrap_or(&self.path)
    }
}

/// One changed region of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: i64,
    pub new_start: i64,
    /// Each prefixed " ", "-" or "+", sanitised to one line.
    pub lines: Vec<String>,
}

fn hunks(raw: Option<&Value>) -> Vec<Hunk> {
    raw.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|h| Hunk {
            old_start: i64_at(h, &["oldStart"]),
            new_start: i64_at(h, &["newStart"]),
            lines: h
                .get("lines")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|l| sanitize::code(l.as_str().unwrap_or("")))
                .collect(),
        })
        .collect()
}

fn file_change(p: &str) -> FileChange {
    let path = sanitize::code(p);
    let raw = (path != p).then(|| p.to_string());
    FileChange {
        path,
        raw,
        ..Default::default()
    }
}

/// The files a tool's `toolUseResult` says it changed.
fn result_changes(tool: &str, r: &Value) -> Vec<FileChange> {
    let file_path = str_at(r, &["filePath"]);
    let patch = r.get("structuredPatch");
    let has_patch = patch.and_then(Value::as_array).is_some_and(|a| !a.is_empty());
    match tool {
        "Edit" | "MultiEdit" | "NotebookEdit" if !file_path.is_empty() && has_patch => {
            vec![FileChange {
                hunks: hunks(patch),
                ..file_change(file_path)
            }]
        }
        "Write" if !file_path.is_empty() => {
            if str_at(r, &["type"]) == "create" || !has_patch {
                let content = sanitize::text(str_at(r, &["content"]));
                let lines = content
                    .trim_end_matches('\n')
                    .split('\n')
                    .map(|l| format!("+{l}"))
                    .collect();
                vec![FileChange {
                    created: true,
                    hunks: vec![Hunk {
                        old_start: 0,
                        new_start: 1,
                        lines,
                    }],
                    ..file_change(file_path)
                }]
            } else {
                vec![FileChange {
                    hunks: hunks(patch),
                    ..file_change(file_path)
                }]
            }
        }
        "Bash" => r
            .get("bashEditDiff")
            .and_then(|d| d.get("files"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|f| FileChange {
                hunks: hunks(f.get("hunks")),
                created: f.get("created").and_then(Value::as_bool).unwrap_or(false),
                deleted: f.get("deleted").and_then(Value::as_bool).unwrap_or(false),
                ..file_change(str_at(f, &["filePath"]))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The last `n` human prompts, assistant texts and tool calls, oldest
/// first. Tool results, system notices and command echoes are skipped.
pub fn tail(path: &Path, n: usize) -> Vec<Entry> {
    turn(path, None, None, n)
}

/// The newest `n` entries whose timestamps fall between `from_ms` and
/// `to_ms` (Unix ms; `None` is open), oldest first. Reads up to 8 MiB back.
pub fn turn(path: &Path, from_ms: Option<i64>, to_ms: Option<i64>, n: usize) -> Vec<Entry> {
    let mut rev: Vec<Entry> = Vec::new();
    // Results follow their calls in the file, so scanning backwards each
    // result comes first and is attached when its call turns up.
    let mut results: HashMap<String, Value> = HashMap::new();
    if n == 0 {
        return rev;
    }
    let _ = scan_backwards(path, 8 << 20, |line| {
        let Ok(rec) = serde_json::from_slice::<Value>(line) else {
            return false;
        };
        if rec.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
            return false;
        }
        let at = util::parse_rfc3339_ms(str_at(&rec, &["timestamp"]));
        if let (Some(to), Some(at)) = (to_ms, at)
            && at > to
        {
            return false; // newer than the window: keep scanning back
        }
        if let (Some(from), Some(at)) = (from_ms, at)
            && at < from
        {
            return true; // older than the window: done
        }
        let entry = |kind, text: String| Entry {
            at_ms: at,
            kind,
            text,
            tool: String::new(),
            code: String::new(),
            changes: Vec::new(),
            output: String::new(),
        };
        let content = rec.get("message").and_then(|m| m.get("content"));
        match str_at(&rec, &["type"]) {
            "user" => match content {
                Some(Value::String(s)) => {
                    let t = clean_prompt(s);
                    if !t.is_empty() {
                        rev.push(entry(EntryKind::You, t));
                    }
                }
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        match str_at(b, &["type"]) {
                            "text" => {
                                let t = clean_prompt(str_at(b, &["text"]));
                                if !t.is_empty() {
                                    rev.push(entry(EntryKind::You, t));
                                }
                            }
                            "tool_result" => {
                                let id = str_at(b, &["tool_use_id"]);
                                if let Some(r) = rec.get("toolUseResult").filter(|r| r.is_object())
                                    && !id.is_empty()
                                {
                                    results.insert(id.to_string(), r.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            "assistant" => {
                if let Some(Value::Array(blocks)) = content {
                    for b in blocks.iter().rev() {
                        match str_at(b, &["type"]) {
                            "text" => {
                                let t = excerpt(str_at(b, &["text"]), EXCERPT_LINES);
                                if !t.is_empty() {
                                    rev.push(entry(EntryKind::Text, t));
                                }
                            }
                            "tool_use" => {
                                let name = str_at(b, &["name"]);
                                let (desc, code) = tool_brief(b.get("input").unwrap_or(&Value::Null));
                                let mut e = entry(EntryKind::Tool, desc);
                                e.tool = sanitize::line(name);
                                e.code = code;
                                if let Some(r) = results.get(str_at(b, &["id"])) {
                                    e.changes = result_changes(name, r);
                                    let out = str_at(r, &["stdout"]).trim();
                                    let err = str_at(r, &["stderr"]).trim();
                                    if !out.is_empty() {
                                        e.output = excerpt(out, OUTPUT_LINES);
                                    } else if !err.is_empty() {
                                        e.output = excerpt(err, OUTPUT_LINES);
                                    }
                                }
                                rev.push(e);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
        rev.len() >= n
    });
    rev.truncate(n);
    rev.reverse();
    rev
}

fn clean_prompt(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() || s.starts_with('<') {
        return String::new(); // command echoes, caveats, reminders
    }
    excerpt(s, EXCERPT_LINES)
}

/// The first `n` non-empty lines of `s` (sanitised) and the paragraph
/// breaks between them (runs of blank lines collapse to one), a cut marked
/// with "…".
pub fn excerpt(s: &str, n: usize) -> String {
    let mut out: Vec<&str> = Vec::new();
    let t = sanitize::text(s);
    let mut kept = 0;
    for l in t.trim().split('\n') {
        let l = l.trim_end_matches([' ', '\t']);
        if l.trim().is_empty() {
            if out.last().is_some_and(|x| !x.is_empty()) {
                out.push("");
            }
            continue;
        }
        if kept == n {
            out.push("…");
            break;
        }
        out.push(l);
        kept += 1;
    }
    out.join("\n")
}

fn first_line(s: &str) -> String {
    let t = sanitize::text(s);
    let t = t.trim();
    t.split('\n').next().unwrap_or("").trim().to_string()
}

/// What a tool call was for (its description, when the model gave one) and
/// the call itself: the command, path or other main argument, up to six
/// lines. Hook payloads carry the same input shape.
pub fn tool_brief(input: &Value) -> (String, String) {
    let Some(obj) = input.as_object() else {
        return (String::new(), String::new());
    };
    let desc = obj
        .get("description")
        .and_then(Value::as_str)
        .filter(|d| !d.trim().is_empty())
        .map(first_line)
        .unwrap_or_default();
    let code = [
        "command",
        "file_path",
        "pattern",
        "url",
        "query",
        "prompt",
        "to",
        "skill",
    ]
    .iter()
    .find_map(|k| {
        obj.get(*k)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
    })
    .map(|v| excerpt(v, 6))
    .unwrap_or_default();
    (desc, code)
}

/// A count of tokens by what Claude Code bills them as.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
}

impl Tokens {
    /// Every token counted, cached or not.
    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }

    pub fn add(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input + o.input,
            output: self.output + o.output,
            cache_creation: self.cache_creation + o.cache_creation,
            cache_read: self.cache_read + o.cache_read,
        }
    }

    fn sub(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input - o.input,
            output: self.output - o.output,
            cache_creation: self.cache_creation - o.cache_creation,
            cache_read: self.cache_read - o.cache_read,
        }
    }

    fn max(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.max(o.input),
            output: self.output.max(o.output),
            cache_creation: self.cache_creation.max(o.cache_creation),
            cache_read: self.cache_read.max(o.cache_read),
        }
    }
}

fn tokens_of(v: &Value) -> Tokens {
    Tokens {
        input: i64_at(v, &["message", "usage", "input_tokens"]),
        output: i64_at(v, &["message", "usage", "output_tokens"]),
        cache_creation: i64_at(v, &["message", "usage", "cache_creation_input_tokens"]),
        cache_read: i64_at(v, &["message", "usage", "cache_read_input_tokens"]),
    }
}

/// One assistant message's usage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spend {
    /// Unix ms, when the line had a timestamp.
    pub at_ms: Option<i64>,
    pub model: String,
    /// `message.id`, else the line's `requestId`.
    pub id: String,
    pub tokens: Tokens,
}

/// What Claude Code wrote about its own cost (the last `cost-state` line).
/// It lags the messages: show it beside an estimate, never add it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reported {
    pub usd: f64,
    /// Claude Code could not price a model.
    pub unknown: bool,
}

/// A session's usage by model: its own messages and its subagents'.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Account {
    pub main: HashMap<String, Tokens>,
    pub sub: HashMap<String, Tokens>,
    pub reported: Option<Reported>,
}

#[derive(Default)]
struct LedgerFile {
    offset: u64,
    index: HashMap<String, usize>,
    spends: Vec<Spend>,
    by_model: HashMap<String, Tokens>,
    reported: Option<Reported>,
}

impl LedgerFile {
    fn take(&mut self, line: &[u8]) {
        if contains(line, "\"type\":\"assistant\"") && contains(line, "\"usage\"") {
            let Ok(v) = serde_json::from_slice::<Value>(line) else {
                return;
            };
            let model = str_at(&v, &["message", "model"]).to_string();
            if model.starts_with('<') {
                return; // <synthetic>
            }
            let tk = tokens_of(&v);
            let mut id = str_at(&v, &["message", "id"]).to_string();
            if id.is_empty() {
                id = str_at(&v, &["requestId"]).to_string();
            }
            // One line per content block: count each message once, keeping
            // the largest figures a streamed message reached.
            if !id.is_empty()
                && let Some(&i) = self.index.get(&id)
            {
                let old = self.spends[i].clone();
                let merged = old.tokens.max(&tk);
                if merged != old.tokens {
                    let cur = self.by_model.entry(old.model.clone()).or_default();
                    *cur = cur.sub(&old.tokens).add(&merged);
                    self.spends[i].tokens = merged;
                }
                return;
            }
            if !id.is_empty() {
                self.index.insert(id.clone(), self.spends.len());
            }
            let at_ms = util::parse_rfc3339_ms(str_at(&v, &["timestamp"]));
            let e = self.by_model.entry(model.clone()).or_default();
            *e = e.add(&tk);
            self.spends.push(Spend {
                at_ms,
                model,
                id,
                tokens: tk,
            });
        } else if contains(line, "\"type\":\"cost-state\"")
            && let Ok(v) = serde_json::from_slice::<Value>(line)
        {
            self.reported = Some(Reported {
                usd: v.get("totalCostUSD").and_then(Value::as_f64).unwrap_or(0.0),
                unknown: v
                    .get("hasUnknownModelCost")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
    }
}

/// Reads usage from transcripts incrementally: it remembers how far into
/// each file it has read, so a refresh only parses what was appended. A
/// last line without its newline is still being written and is left for
/// the next refresh; a file that shrank is read again from the start.
#[derive(Default)]
pub struct Ledger {
    files: Mutex<HashMap<PathBuf, LedgerFile>>,
}

impl Ledger {
    pub fn new() -> Ledger {
        Ledger::default()
    }

    /// A ledger shared by the whole process.
    pub fn shared() -> &'static Ledger {
        static SHARED: OnceLock<Ledger> = OnceLock::new();
        SHARED.get_or_init(Ledger::new)
    }

    /// The usage of the session whose transcript is at `path` and of its
    /// subagents (`<path without .jsonl>/subagents/*.jsonl`).
    pub fn account(&self, path: &Path) -> Account {
        let mut a = Account::default();
        if path.as_os_str().is_empty() {
            return a;
        }
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let main = refresh(&mut files, path);
        a.main = main.by_model.clone();
        a.reported = main.reported;
        for p in subagent_files(path) {
            for (k, v) in &refresh(&mut files, &p).by_model {
                let e = a.sub.entry(k.clone()).or_default();
                *e = e.add(v);
            }
        }
        a
    }

    /// The usage of the session and its subagents between `from_ms` and
    /// `to_ms` (Unix ms, `None` open), by model.
    pub fn window(&self, path: &Path, from_ms: Option<i64>, to_ms: Option<i64>) -> HashMap<String, Tokens> {
        let mut out: HashMap<String, Tokens> = HashMap::new();
        if path.as_os_str().is_empty() {
            return out;
        }
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let mut paths = vec![path.to_path_buf()];
        paths.extend(subagent_files(path));
        for p in paths {
            for s in &refresh(&mut files, &p).spends {
                // As in Den, a message without a time is before any `from`.
                if from_ms.is_some_and(|f| s.at_ms.is_none_or(|a| a < f))
                    || to_ms.is_some_and(|t| s.at_ms.is_some_and(|a| a > t))
                {
                    continue;
                }
                let e = out.entry(s.model.clone()).or_default();
                *e = e.add(&s.tokens);
            }
        }
        out
    }

    /// Every message's usage in the transcript at `path` (not subagents),
    /// in file order, each message once.
    pub fn spends(&self, path: &Path) -> Vec<Spend> {
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        refresh(&mut files, path).spends.clone()
    }
}

fn refresh<'a>(files: &'a mut HashMap<PathBuf, LedgerFile>, path: &Path) -> &'a LedgerFile {
    let f = files.entry(path.to_path_buf()).or_default();
    let Ok(mut fh) = File::open(path) else {
        return f;
    };
    let Ok(size) = fh.metadata().map(|m| m.len()) else {
        return f;
    };
    if size < f.offset {
        *f = LedgerFile::default(); // rewritten or replaced: start over
    }
    if size == f.offset || fh.seek(SeekFrom::Start(f.offset)).is_err() {
        return f;
    }
    let mut r = BufReader::with_capacity(256 << 10, (&mut fh).take(size - f.offset));
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line.last() != Some(&b'\n') => break, // still being written
            Ok(n) => {
                f.offset += n as u64;
                f.take(&line);
            }
        }
    }
    f
}

/// The transcripts of a session's subagents, beside the session's own file.
pub fn subagent_files(path: &Path) -> Vec<PathBuf> {
    let s = path.to_string_lossy();
    let base = s.strip_suffix(".jsonl").unwrap_or(&s);
    let dir = PathBuf::from(format!("{base}/subagents"));
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    out.sort();
    out
}

/// A file change with when it landed and the tool that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedChange {
    pub at_ms: Option<i64>,
    pub tool: String,
    pub change: FileChange,
}

#[derive(Default)]
struct ChangeLog {
    offset: u64,
    pending: HashMap<String, String>, // tool_use id -> tool name
    changes: Vec<TimedChange>,
}

/// Every file change in transcripts, read once and then only what was
/// appended (for a review view).
#[derive(Default)]
pub struct Changes {
    logs: Mutex<HashMap<PathBuf, ChangeLog>>,
}

impl Changes {
    pub fn new() -> Changes {
        Changes::default()
    }

    /// Every change in the transcript at `path` made at or after `since_ms`
    /// (`None` for all), oldest first.
    pub fn since(&self, path: &Path, since_ms: Option<i64>) -> Vec<TimedChange> {
        let mut logs = self.logs.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(size) = std::fs::metadata(path).map(|m| m.len()) else {
            logs.remove(path);
            return Vec::new();
        };
        let cl = logs.entry(path.to_path_buf()).or_default();
        if size < cl.offset {
            *cl = ChangeLog::default();
        }
        if size > cl.offset
            && let Ok(mut fh) = File::open(path)
            && fh.seek(SeekFrom::Start(cl.offset)).is_ok()
        {
            let mut r = BufReader::with_capacity(256 << 10, fh);
            let mut line = Vec::new();
            loop {
                line.clear();
                match r.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.last() != Some(&b'\n') => break,
                    Ok(n) => {
                        cl.offset += n as u64;
                        cl.consume(&line);
                    }
                }
            }
        }
        cl.changes
            .iter()
            .filter(|c| match (since_ms, c.at_ms) {
                (Some(s), Some(at)) => at >= s,
                _ => true,
            })
            .cloned()
            .collect()
    }
}

impl ChangeLog {
    fn consume(&mut self, line: &[u8]) {
        let Ok(rec) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if rec.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
            return;
        }
        let Some(blocks) = rec
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            return;
        };
        match str_at(&rec, &["type"]) {
            "assistant" => {
                for b in blocks {
                    let id = str_at(b, &["id"]);
                    if str_at(b, &["type"]) == "tool_use" && !id.is_empty() {
                        self.pending
                            .insert(id.to_string(), str_at(b, &["name"]).to_string());
                    }
                }
            }
            "user" => {
                let at = util::parse_rfc3339_ms(str_at(&rec, &["timestamp"]));
                for b in blocks {
                    if str_at(b, &["type"]) != "tool_result" {
                        continue;
                    }
                    let Some(name) = self.pending.remove(str_at(b, &["tool_use_id"])) else {
                        continue;
                    };
                    let Some(r) = rec.get("toolUseResult").filter(|r| r.is_object()) else {
                        continue;
                    };
                    for c in result_changes(&name, r) {
                        self.changes.push(TimedChange {
                            at_ms: at,
                            tool: sanitize::line(&name),
                            change: c,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// What Claude Code records at the top of a session that runs in a worktree
/// it made (`claude --worktree`, or EnterWorktree).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeState {
    /// The repository the session was started in.
    pub original_cwd: String,
    /// The worktree.
    pub path: String,
    pub name: String,
    /// The worktree's branch (`worktree-<name>`).
    pub branch: String,
    pub original_branch: String,
    /// The commit the worktree was made from.
    pub original_head: String,
}

/// The `worktree-state` record in the first 20 lines, if any.
pub fn read_worktree(path: &Path) -> Option<WorktreeState> {
    let fh = File::open(path).ok()?;
    let mut r = BufReader::new(fh);
    let mut line = Vec::new();
    for _ in 0..20 {
        line.clear();
        let n = r.by_ref().take(256 << 10).read_until(b'\n', &mut line).ok()?;
        if n == 0 || (line.last() != Some(&b'\n') && n >= 256 << 10) {
            return None;
        }
        if !contains(&line, "\"worktree-state\"") {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let s = &v["worktreeSession"];
        if str_at(&v, &["type"]) != "worktree-state"
            || !s.is_object()
            || str_at(s, &["worktreePath"]).is_empty()
        {
            continue;
        }
        return Some(WorktreeState {
            original_cwd: str_at(s, &["originalCwd"]).into(),
            path: str_at(s, &["worktreePath"]).into(),
            name: str_at(s, &["worktreeName"]).into(),
            branch: str_at(s, &["worktreeBranch"]).into(),
            original_branch: str_at(s, &["originalBranch"]).into(),
            original_head: str_at(s, &["originalHeadCommit"]).into(),
        });
    }
    None
}

/// A tool call, for a "recent" list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub at_ms: Option<i64>,
    pub name: String,
    /// The model's description of the call, first line.
    pub description: String,
    /// The command, path, pattern, URL… first line.
    pub target: String,
}

/// What the end of a transcript says, from one backward scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptSummary {
    pub title: Option<String>,
    /// Raw model id in use.
    pub model: Option<String>,
    /// The newest assistant text (sanitised, whole).
    pub last_assistant_text: Option<String>,
    /// The newest tool calls, oldest first, at most [`RECENT_TOOLS`].
    pub recent_tools: Vec<ToolCall>,
    /// Usage per assistant message in the part read, oldest first, each
    /// message once (largest figures).
    pub messages: Vec<Spend>,
    /// The sum of `messages` by model.
    pub by_model: HashMap<String, Tokens>,
    /// The last message's context usage.
    pub context: Option<ContextUsage>,
    /// Human prompts in the part read.
    pub turns: usize,
    /// The part read did not reach the start of the file.
    pub truncated: bool,
}

/// How many tool calls a summary keeps.
pub const RECENT_TOOLS: usize = 20;

/// Summarises the last `max_bytes` of a transcript: title, model, last
/// assistant text, recent tool calls, per-message usage, context, turns.
/// A missing file is an empty summary.
pub fn summarize(path: &Path, max_bytes: u64) -> TranscriptSummary {
    let mut s = TranscriptSummary::default();
    let mut tools_rev: Vec<ToolCall> = Vec::new();
    let mut spends_rev: Vec<Spend> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let res = scan_backwards(path, max_bytes, |line| {
        if s.title.is_none()
            && contains(line, "\"type\":\"ai-title\"")
            && let Ok(v) = serde_json::from_slice::<Value>(line)
        {
            s.title = Some(sanitize::line(str_at(&v, &["aiTitle"]))).filter(|t| !t.is_empty());
        }
        if s.model.is_none() {
            s.model = line_model(line);
        }
        let is_assistant = contains(line, "\"type\":\"assistant\"");
        let is_user = contains(line, "\"type\":\"user\"");
        if !is_assistant && !is_user {
            return false;
        }
        let Ok(rec) = serde_json::from_slice::<Value>(line) else {
            return false;
        };
        if rec.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
            return false;
        }
        let at = util::parse_rfc3339_ms(str_at(&rec, &["timestamp"]));
        let content = rec.get("message").and_then(|m| m.get("content"));
        match str_at(&rec, &["type"]) {
            "user" => {
                let prompt = match content {
                    Some(Value::String(t)) => !clean_prompt(t).is_empty(),
                    Some(Value::Array(blocks)) => blocks.iter().any(|b| {
                        str_at(b, &["type"]) == "text" && !clean_prompt(str_at(b, &["text"])).is_empty()
                    }),
                    _ => false,
                };
                if prompt {
                    s.turns += 1;
                }
            }
            "assistant" => {
                if let Some(Value::Array(blocks)) = content {
                    for b in blocks.iter().rev() {
                        match str_at(b, &["type"]) {
                            "text" if s.last_assistant_text.is_none() => {
                                let t = sanitize::text(str_at(b, &["text"])).trim().to_string();
                                if !t.is_empty() {
                                    s.last_assistant_text = Some(t);
                                }
                            }
                            "tool_use" if tools_rev.len() < RECENT_TOOLS => {
                                let (description, code) = tool_brief(b.get("input").unwrap_or(&Value::Null));
                                tools_rev.push(ToolCall {
                                    at_ms: at,
                                    name: sanitize::line(str_at(b, &["name"])),
                                    description,
                                    target: code.split('\n').next().unwrap_or("").to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                let model = str_at(&rec, &["message", "model"]);
                if rec.get("message").and_then(|m| m.get("usage")).is_some() && !model.starts_with('<') {
                    if s.context.is_none() {
                        let c = context_of(&rec);
                        if c.tokens > 0 {
                            s.context = Some(c);
                        }
                    }
                    let tk = tokens_of(&rec);
                    let mut id = str_at(&rec, &["message", "id"]).to_string();
                    if id.is_empty() {
                        id = str_at(&rec, &["requestId"]).to_string();
                    }
                    match seen.get(&id) {
                        Some(&i) if !id.is_empty() => spends_rev[i].tokens = spends_rev[i].tokens.max(&tk),
                        _ => {
                            if !id.is_empty() {
                                seen.insert(id.clone(), spends_rev.len());
                            }
                            spends_rev.push(Spend {
                                at_ms: at,
                                model: model.to_string(),
                                id,
                                tokens: tk,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        false
    });
    s.truncated = !matches!(res, Ok(true));
    if res.is_err() {
        s.truncated = false;
        return s;
    }
    tools_rev.reverse();
    spends_rev.reverse();
    for sp in &spends_rev {
        let e = s.by_model.entry(sp.model.clone()).or_default();
        *e = e.add(&sp.tokens);
    }
    s.recent_tools = tools_rev;
    s.messages = spends_rev;
    s
}

/// Who said a line of a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A tool call by the assistant ("Name: target").
    Tool,
}

/// A transcript as a conversation, oldest first: the human's prompts, the
/// assistant's words and its tool calls, sanitised. `grep` keeps lines that
/// contain it (case-insensitive); `tail` keeps the last that many (0 for
/// all). Reads the whole file.
pub fn conversation(path: &Path, tail: usize, grep: Option<&str>) -> Vec<(Role, String)> {
    let Ok(fh) = File::open(path) else {
        return Vec::new();
    };
    let needle = grep.map(str::to_lowercase).filter(|g| !g.is_empty());
    let mut out: Vec<(Role, String)> = Vec::new();
    let mut push = |role: Role, text: String| {
        if text.is_empty() {
            return;
        }
        if let Some(n) = &needle
            && !text.to_lowercase().contains(n.as_str())
        {
            return;
        }
        out.push((role, text));
    };
    let prompt = |s: &str| {
        let s = s.trim();
        if s.starts_with('<') {
            String::new()
        } else {
            sanitize::text(s).trim().to_string()
        }
    };
    for line in BufReader::new(fh).split(b'\n').map_while(Result::ok) {
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if rec.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let content = rec.get("message").and_then(|m| m.get("content"));
        match (str_at(&rec, &["type"]), content) {
            ("user", Some(Value::String(t))) => push(Role::User, prompt(t)),
            ("user", Some(Value::Array(blocks))) => {
                for b in blocks.iter().filter(|b| str_at(b, &["type"]) == "text") {
                    push(Role::User, prompt(str_at(b, &["text"])));
                }
            }
            ("assistant", Some(Value::Array(blocks))) => {
                for b in blocks {
                    match str_at(b, &["type"]) {
                        "text" => push(
                            Role::Assistant,
                            sanitize::text(str_at(b, &["text"])).trim().to_string(),
                        ),
                        "tool_use" => {
                            let (desc, code) = tool_brief(b.get("input").unwrap_or(&Value::Null));
                            let target = code.split('\n').next().unwrap_or("").to_string();
                            let what = if target.is_empty() { desc } else { target };
                            let name = sanitize::line(str_at(b, &["name"]));
                            push(
                                Role::Tool,
                                if what.is_empty() {
                                    name
                                } else {
                                    format!("{name}: {what}")
                                },
                            );
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if tail > 0 && out.len() > tail {
        out.drain(..out.len() - tail);
    }
    out
}

#[cfg(test)]
mod tests;
