//! A session's conversation as a front end shows it, read forwards from a
//! place in the transcript: prompts, the assistant's words, and its tool
//! calls with the first lines of their results attached. Each item has a
//! position that grows along the file, so a reader can ask again for only
//! what came after the last item it has.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use super::{EntryKind, OUTPUT_LINES, excerpt, line_model, pretty_model, str_at, tool_brief};
use crate::{sanitize, util};

/// How many lines of a prompt or reply an item keeps.
const TEXT_LINES: usize = 200;

/// How much of the end of a transcript is read at most.
const READ_LIMIT: u64 = 8 << 20;

/// Positions count content blocks within a line: a line at byte `b` gives
/// positions from `b * BLOCKS` on.
const BLOCKS: u64 = 64;

/// One item of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadItem {
    /// Grows along the file: the line's byte offset and the block's place
    /// in it.
    pub pos: u64,
    /// Unix ms, when the line has a timestamp.
    pub at_ms: Option<i64>,
    pub kind: EntryKind,
    /// The prompt, the reply, or a tool call's description.
    pub text: String,
    /// The tool's name, for a tool call.
    pub tool: String,
    /// The command, path or other main argument of a tool call.
    pub code: String,
    /// The first lines of a tool call's result.
    pub output: String,
    /// A tool call's result came.
    pub done: bool,
    /// A tool call's result is an error.
    pub failed: bool,
}

/// A conversation read from a transcript, and what it says of the session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thread {
    /// Oldest first.
    pub items: Vec<ThreadItem>,
    /// The model last used, as people name it ("Opus 5.5").
    pub model: Option<String>,
    pub cwd: Option<String>,
    /// Claude Code's permission mode on the latest prompt.
    pub permission_mode: Option<String>,
}

/// The conversation in the transcript at `path`: with `after`, the items
/// past that position (read from its line on); without, from the last
/// 8 MiB. Only whole lines are read, so a line being written is read next
/// time. `tail` keeps the last that many items (0: all read). A missing
/// file is an empty thread.
pub fn thread(path: &Path, after: Option<u64>, tail: usize) -> Thread {
    let mut out = Thread::default();
    let Ok(mut file) = File::open(path) else {
        return out;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let from = after.map_or(0, |a| a / BLOCKS).min(len);
    let mut start = from.max(len.saturating_sub(READ_LIMIT));
    if file.seek(SeekFrom::Start(start)).is_err() {
        return out;
    }
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    // Started inside a line (the limit cut it): skip to the next one.
    if start > 0 && start != from {
        match reader.read_until(b'\n', &mut line) {
            Ok(n) if line.ends_with(b"\n") => start += n as u64,
            _ => return out,
        }
    }
    let mut at = start;
    // Tool calls by id, waiting for their results.
    let mut calls: HashMap<String, usize> = HashMap::new();
    loop {
        line.clear();
        let Ok(n) = reader.read_until(b'\n', &mut line) else {
            break;
        };
        if n == 0 || !line.ends_with(b"\n") {
            break; // the end, or a line still being written
        }
        let pos = at * BLOCKS;
        at += n as u64;
        read_line(&line, pos, &mut out, &mut calls);
    }
    if let Some(after) = after {
        out.items.retain(|i| i.pos > after);
    }
    if tail > 0 && out.items.len() > tail {
        out.items.drain(..out.items.len() - tail);
    }
    out
}

fn read_line(line: &[u8], pos: u64, out: &mut Thread, calls: &mut HashMap<String, usize>) {
    if let Some(m) = line_model(line) {
        out.model = Some(pretty_model(&m));
    }
    let Ok(rec) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    if rec.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }
    let kind = str_at(&rec, &["type"]);
    if kind != "user" && kind != "assistant" {
        return;
    }
    let cwd = str_at(&rec, &["cwd"]);
    if !cwd.is_empty() {
        out.cwd = Some(sanitize::line(cwd));
    }
    let at = util::parse_rfc3339_ms(str_at(&rec, &["timestamp"]));
    let item = |n: usize, kind, text: String| ThreadItem {
        pos: pos + (n as u64).min(BLOCKS - 1),
        at_ms: at,
        kind,
        text,
        tool: String::new(),
        code: String::new(),
        output: String::new(),
        done: false,
        failed: false,
    };
    let content = rec.get("message").and_then(|m| m.get("content"));
    if kind == "user" {
        let mode = str_at(&rec, &["permissionMode"]);
        let mut prompted = false;
        let mut prompt = |n: usize, text: &str, out: &mut Thread| {
            let t = prompt_text(text);
            if !t.is_empty() {
                out.items.push(item(n, EntryKind::You, t));
                prompted = true;
            }
        };
        match content {
            Some(Value::String(s)) => prompt(0, s, out),
            Some(Value::Array(blocks)) => {
                for (n, b) in blocks.iter().enumerate() {
                    match str_at(b, &["type"]) {
                        "text" => prompt(n, str_at(b, &["text"]), out),
                        "tool_result" => {
                            let id = str_at(b, &["tool_use_id"]);
                            if let Some(&ix) = calls.get(id) {
                                attach(&mut out.items[ix], b, rec.get("toolUseResult"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if prompted && !mode.is_empty() {
            out.permission_mode = Some(sanitize::line(mode));
        }
        return;
    }
    let Some(Value::Array(blocks)) = content else {
        return;
    };
    for (n, b) in blocks.iter().enumerate() {
        match str_at(b, &["type"]) {
            "text" => {
                let t = excerpt(str_at(b, &["text"]), TEXT_LINES);
                if !t.is_empty() {
                    out.items.push(item(n, EntryKind::Text, t));
                }
            }
            "tool_use" => {
                let (desc, code) = tool_brief(b.get("input").unwrap_or(&Value::Null));
                let mut call = item(n, EntryKind::Tool, desc);
                call.tool = sanitize::line(str_at(b, &["name"]));
                call.code = code;
                let id = str_at(b, &["id"]);
                if !id.is_empty() {
                    calls.insert(id.to_string(), out.items.len());
                }
                out.items.push(call);
            }
            _ => {}
        }
    }
}

/// A prompt's words, or nothing for what the human did not type: command
/// echoes, caveats and reminders (they start with `<`) and Claude Code's
/// own notes ("[Request interrupted by user]").
fn prompt_text(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() || s.starts_with('<') || s.starts_with("[Request interrupted") {
        return String::new();
    }
    excerpt(s, TEXT_LINES)
}

/// A tool call's result: what it printed (stdout, else stderr, else the
/// result block's text), first lines only, and whether it failed.
fn attach(call: &mut ThreadItem, block: &Value, result: Option<&Value>) {
    call.done = true;
    call.failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let printed = result
        .filter(|r| r.is_object())
        .map(|r| {
            let out = str_at(r, &["stdout"]).trim();
            if out.is_empty() {
                str_at(r, &["stderr"]).trim()
            } else {
                out
            }
        })
        .unwrap_or("");
    let text = if printed.is_empty() {
        match block.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter(|p| str_at(p, &["type"]) == "text")
                .map(|p| str_at(p, &["text"]))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    } else {
        printed.to_string()
    };
    call.output = excerpt(&text, OUTPUT_LINES);
}
