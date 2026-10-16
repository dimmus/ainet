use super::*;
use crate::util::testutil::TempDir;
use std::io::Write;

fn write(path: &Path, lines: &[&str]) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn append(path: &Path, s: &str) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(s.as_bytes())
        .unwrap();
}

#[test]
fn read_finds_latest_title_and_model() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    let mut b = String::new();
    b.push_str("{\"type\":\"ai-title\",\"aiTitle\":\"Old title\",\"sessionId\":\"s\"}\n");
    b.push_str(
        "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-opus-5-5\",\"role\":\"assistant\"}}\n",
    );
    // padding so the file spans more than one chunk
    for _ in 0..3000 {
        b.push_str(&format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{}\"}}}}\n",
            "x".repeat(40)
        ));
    }
    b.push_str("{\"type\":\"ai-title\",\"aiTitle\":\"New title\",\"sessionId\":\"s\"}\n");
    b.push_str(
        "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-fable-5-1\",\"role\":\"assistant\"}}\n",
    );
    std::fs::write(&p, b).unwrap();
    let f = read_facts(&p);
    assert_eq!(f.title, "New title");
    assert_eq!(f.model, "claude-fable-5-1");
    assert_eq!(pretty_model(&f.model), "Fable 5.1");
    assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
    assert_eq!(pretty_model("gpt-x"), "gpt-x");
    assert_eq!(read_facts(&tmp.join("none.jsonl")), Facts::default());
}

const HAIKU_START: &str = r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-haiku-4-5-20251001","marketingName":"Haiku 4.5"},"text":"x"}}"#;
const PLAN_MODEL: &str =
    r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-sonnet-5-5"}}}"#;
const PLAN_REPLY: &str = r#"{"parentUuid":"p","isSidechain":false,"type":"assistant","message":{"model":"claude-sonnet-5-5","id":"msg_1","role":"assistant","content":[{"type":"text","text":"plan"}]},"requestId":"req_1"}"#;
const EXIT_MODEL: &str = r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-haiku-4-5-20251001"}}}"#;
const HAIKU_REPLY: &str = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_2","role":"assistant","content":[]}}"#;
const SYNTHETIC: &str = r#"{"type":"assistant","error":"authentication_failed","isApiErrorMessage":true,"message":{"model":"<synthetic>","id":"942f1410-8e27-43d1-ba40-b568d31284f6","role":"assistant","content":[{"type":"text","text":"Not logged in"}]}}"#;
const TOOL_TEXT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"{\"type\":\"attachment\",\"attachment\":{\"type\":\"model\",\"identity\":{\"modelId\":\"claude-opus-5-5\"}}}"}]}}"#;

/// The model in use, in the shapes Claude Code 2.1.289 writes.
#[test]
fn read_model_in_use() {
    let cases: &[(&str, &[&str], &str)] = &[
        (
            "started, before any reply",
            &[HAIKU_START],
            "claude-haiku-4-5-20251001",
        ),
        (
            "a plan-mode reply",
            &[HAIKU_START, PLAN_MODEL, PLAN_REPLY],
            "claude-sonnet-5-5",
        ),
        (
            "plan approved, no reply",
            &[HAIKU_START, PLAN_MODEL, PLAN_REPLY, EXIT_MODEL],
            "claude-haiku-4-5-20251001",
        ),
        (
            "after a haiku reply",
            &[HAIKU_START, PLAN_MODEL, PLAN_REPLY, EXIT_MODEL, HAIKU_REPLY],
            "claude-haiku-4-5-20251001",
        ),
        (
            "an API error after",
            &[HAIKU_START, HAIKU_REPLY, SYNTHETIC],
            "claude-haiku-4-5-20251001",
        ),
        ("logged out", &[SYNTHETIC], ""),
        (
            "an attachment as text",
            &[HAIKU_REPLY, TOOL_TEXT],
            "claude-haiku-4-5-20251001",
        ),
    ];
    for (name, lines, want) in cases {
        let tmp = TempDir::new();
        let p = tmp.join("s.jsonl");
        write(&p, lines);
        assert_eq!(read_facts(&p).model, *want, "{name}");
    }
}

/// The model a session runs outside plan mode, for restoring it.
#[test]
fn default_model_like_den() {
    const SONNET_ATT: &str =
        r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-sonnet-5-5"}}}"#;
    const HAIKU_ATT: &str = r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-haiku-4-5-20251001"}}}"#;
    const PLAN_IN: &str = r#"{"type":"attachment","attachment":{"type":"plan_mode","reminderType":"full"}}"#;
    const PLAN_OUT: &str = r#"{"type":"attachment","attachment":{"type":"plan_mode_exit"}}"#;
    const SONNET: &str =
        r#"{"type":"assistant","message":{"model":"claude-sonnet-5-5","role":"assistant","content":[]}}"#;
    const HAIKU: &str = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","role":"assistant","content":[]}}"#;
    const PROMPT: &str =
        r#"{"type":"user","origin":{"kind":"human"},"message":{"role":"user","content":"go on"}}"#;
    const SYNTH: &str = r#"{"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","role":"assistant","content":[]}}"#;
    let h = "claude-haiku-4-5-20251001";
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        ("never in plan mode", &[HAIKU_ATT, PROMPT, HAIKU, HAIKU], Some(h)),
        (
            "closed in plan mode after haiku turns",
            &[
                HAIKU_ATT, PROMPT, HAIKU, PROMPT, SONNET_ATT, PLAN_IN, SONNET, SONNET,
            ],
            Some(h),
        ),
        (
            "planned, approved, haiku since",
            &[
                PROMPT, SONNET_ATT, PLAN_IN, SONNET, HAIKU_ATT, PLAN_OUT, HAIKU, HAIKU,
            ],
            Some(h),
        ),
        (
            "approved, no reply yet",
            &[PROMPT, SONNET_ATT, PLAN_IN, SONNET, HAIKU_ATT, PLAN_OUT],
            Some(h),
        ),
        (
            "in and out twice, in plan mode now",
            &[
                PROMPT, SONNET_ATT, PLAN_IN, SONNET, HAIKU_ATT, PLAN_OUT, HAIKU, PROMPT, SONNET_ATT, PLAN_IN,
                SONNET,
            ],
            Some(h),
        ),
        (
            "in plan mode, an API error since",
            &[HAIKU_ATT, HAIKU, SONNET_ATT, PLAN_IN, SONNET, SYNTH],
            Some(h),
        ),
        (
            "started in plan mode: not recorded",
            &[PROMPT, SONNET_ATT, PLAN_IN, SONNET, PROMPT, SONNET],
            None,
        ),
        (
            "plan mode on the same model",
            &[HAIKU_ATT, HAIKU, PLAN_IN, HAIKU],
            Some(h),
        ),
    ];
    for (name, lines, want) in cases {
        let tmp = TempDir::new();
        let p = tmp.join("s.jsonl");
        write(&p, lines);
        assert_eq!(default_model(&p).as_deref(), *want, "{name}");
    }
    assert_eq!(default_model(Path::new("/nonexistent/none.jsonl")), None);
}

#[test]
fn usage_and_tail() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    write(
        &p,
        &[
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>ignored</local-command-caveat>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"fix the login redirect"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-fable-5-1","content":[{"type":"text","text":"Looking at it.\nMore."},{"type":"type_use"}],"usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":5}}}"#,
            r#"{"type":"assistant","message":{"model":"claude-fable-5-1","content":[{"type":"tool_use","name":"Bash","input":{"command":"go test ./...","description":"Run tests"}}],"usage":{"input_tokens":20,"cache_read_input_tokens":250000,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}"#,
        ],
    );
    let u = read_usage(&p).unwrap();
    assert_eq!((u.tokens, u.window, u.percent()), (250_020, 1_000_000, 25));
    let t = tail(&p, 10);
    let got: Vec<(EntryKind, &str, &str, &str)> = t
        .iter()
        .map(|e| (e.kind, e.text.as_str(), e.tool.as_str(), e.code.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (EntryKind::You, "fix the login redirect", "", ""),
            (EntryKind::Text, "Looking at it.\nMore.", "", ""),
            (EntryKind::Tool, "Run tests", "Bash", "go test ./..."),
        ]
    );
    let two = tail(&p, 2);
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].kind, EntryKind::Text);
    assert_eq!(window_for("claude-opus-5-5[1m]", 10), 1_000_000);
    assert_eq!(window_for("claude-opus-5-5", 10), 200_000);
}

#[test]
fn excerpt_keeps_lines_and_marks_cut() {
    assert_eq!(
        excerpt("one\n\n\n  two  \nthree\nfour", 3),
        "one\n\n  two\nthree\n…"
    );
    assert_eq!(excerpt("   ", 3), "");
}

#[test]
fn tail_attaches_results() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    write(
        &p,
        &[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/x/a.go","old_string":"a","new_string":"b"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"toolUseResult":{"filePath":"/x/a.go","structuredPatch":[{"oldStart":3,"newStart":3,"lines":[" ctx","-a","+b","+c"]}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"go test ./...","description":"Run tests"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]},"toolUseResult":{"stdout":"ok  pkg\n\nFAIL other","stderr":"","interrupted":false,"bashEditDiff":{"files":[{"filePath":"/x/b.go","hunks":[{"oldStart":1,"newStart":1,"lines":["-x","+y"]}]},{"filePath":"/x/gone.go","hunks":[],"deleted":true}],"moreFiles":0}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"Write","input":{"file_path":"/x/new.txt","content":"one\ntwo\n"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t3","content":"ok"}]},"toolUseResult":{"type":"create","filePath":"/x/new.txt","content":"one\ntwo\n","structuredPatch":[]}}"#,
        ],
    );
    let t = tail(&p, 10);
    assert_eq!(t.len(), 3);
    let (edit, bash, write) = (&t[0], &t[1], &t[2]);
    assert_eq!(edit.tool, "Edit");
    assert_eq!(edit.changes.len(), 1);
    assert_eq!(edit.changes[0].path, "/x/a.go");
    assert_eq!(edit.changes[0].hunks[0].new_start, 3);
    assert_eq!(edit.changes[0].counts(), (2, 1));
    assert_eq!(bash.output, "ok  pkg\n\nFAIL other");
    assert_eq!(bash.changes.len(), 2);
    assert_eq!(bash.changes[0].path, "/x/b.go");
    assert!(bash.changes[1].deleted);
    assert!(write.changes[0].created);
    assert_eq!(write.changes[0].hunks[0].lines, vec!["+one", "+two"]);

    let changes = Changes::new();
    let all = changes.since(&p, None);
    assert_eq!(all.len(), 4, "{all:?}");
    assert_eq!(all[0].tool, "Edit");
    assert_eq!(all[3].change.path, "/x/new.txt");
}

#[test]
fn turn_cuts_by_timestamp() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    write(
        &p,
        &[
            r#"{"type":"user","timestamp":"2026-10-08T10:00:00.000Z","message":{"content":"first prompt"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-08T10:00:05.000Z","message":{"content":[{"type":"text","text":"first reply"}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-08T10:10:00.000Z","message":{"content":"second prompt"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-08T10:10:05.000Z","message":{"content":[{"type":"text","text":"second reply"}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-08T10:20:00.000Z","message":{"content":"third prompt"}}"#,
        ],
    );
    let from = util::parse_rfc3339_ms("2026-10-08T10:09:00Z");
    let to = util::parse_rfc3339_ms("2026-10-08T10:15:00Z");
    let got = turn(&p, from, to, 10);
    assert_eq!(
        got.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        vec!["second prompt", "second reply"]
    );
    assert!(got[0].at_ms.is_some());
    assert_eq!(turn(&p, from, None, 10).len(), 3);
}

/// One assistant line as Claude Code writes it: a message with several
/// content blocks is several lines with the same message id.
fn msg(id: &str, model: &str, at: &str, t: [i64; 4]) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{at}","requestId":"req_{id}","message":{{"id":"{id}","model":"{model}","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":{},"cache_read_input_tokens":{}}}}}}}"#,
        t[0], t[1], t[2], t[3]
    )
}

#[test]
fn ledger_counts_each_message_once() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    let m1 = msg("m1", "claude-opus-5-5", "2026-10-11T10:00:00Z", [2, 100, 5000, 0]);
    let m2 = msg(
        "m2",
        "claude-opus-5-5",
        "2026-10-11T10:00:05Z",
        [3, 40, 100, 5000],
    );
    let m3 = msg("m3", "<synthetic>", "2026-10-11T10:00:09Z", [0, 0, 0, 0]);
    write(
        &p,
        &[
            &m1,
            &m1,
            &m1,
            &m2,
            r#"{"type":"user","message":{"role":"user","content":"hello"}}"#,
            &m3,
        ],
    );
    let l = Ledger::new();
    let a = l.account(&p);
    assert_eq!(
        a.main["claude-opus-5-5"],
        Tokens {
            input: 5,
            output: 140,
            cache_creation: 5100,
            cache_read: 5000
        }
    );
    assert!(!a.main.contains_key("<synthetic>"));
}

#[test]
fn ledger_streamed_chunk_keeps_the_largest_figures() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    write(
        &p,
        &[
            &msg("m1", "claude-sonnet-5-5", "2026-10-11T10:00:00Z", [10, 3, 200, 0]),
            &msg(
                "m1",
                "claude-sonnet-5-5",
                "2026-10-11T10:00:00Z",
                [10, 90, 200, 0],
            ),
        ],
    );
    let got = Ledger::new().account(&p).main["claude-sonnet-5-5"];
    assert_eq!((got.output, got.input, got.cache_creation), (90, 10, 200));
}

#[test]
fn ledger_model_switch_and_append_and_rewrite() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    write(
        &p,
        &[
            &msg("a", "claude-opus-5-5", "2026-10-11T10:00:00Z", [1, 10, 0, 0]),
            &msg("b", "claude-sonnet-5-5", "2026-10-11T10:01:00Z", [2, 20, 0, 0]),
            &msg("c", "claude-opus-5-5", "2026-10-11T10:02:00Z", [4, 40, 0, 0]),
        ],
    );
    let l = Ledger::new();
    let a = l.account(&p);
    assert_eq!(
        (
            a.main["claude-opus-5-5"].output,
            a.main["claude-sonnet-5-5"].output,
            a.main.len()
        ),
        (50, 20, 2)
    );

    // A half-written line is left for the next refresh.
    let line = msg("d", "claude-opus-5-5", "2026-10-11T10:03:00Z", [2, 20, 0, 0]);
    append(&p, &line[..30]);
    assert_eq!(l.account(&p).main["claude-opus-5-5"].output, 50);
    append(&p, &format!("{}\n", &line[30..]));
    assert_eq!(l.account(&p).main["claude-opus-5-5"].output, 70);
    // Replaced by a shorter file: start over.
    write(
        &p,
        &[&msg("z", "claude-opus-5-5", "2026-10-11T10:05:00Z", [0, 7, 0, 0])],
    );
    assert_eq!(l.account(&p).main["claude-opus-5-5"].output, 7);
}

#[test]
fn ledger_subagents_window_and_reported() {
    let tmp = TempDir::new();
    let p = tmp.join("sess.jsonl");
    write(
        &p,
        &[
            &msg("a", "claude-opus-5-5", "2026-10-11T10:00:00Z", [1, 10, 0, 0]),
            &msg("b", "claude-opus-5-5", "2026-10-11T11:00:00Z", [0, 100, 0, 0]),
            r#"{"type":"cost-state","totalCostUSD":1.5,"hasUnknownModelCost":false,"modelUsage":{}}"#,
            r#"{"type":"cost-state","totalCostUSD":2.25,"hasUnknownModelCost":true,"modelUsage":{}}"#,
        ],
    );
    write(
        &tmp.join("sess/subagents/agent-1.jsonl"),
        &[&msg(
            "s1",
            "claude-haiku-4-5-20251001",
            "2026-10-11T10:00:30Z",
            [5, 50, 0, 0],
        )],
    );
    write(
        &tmp.join("sess/subagents/agent-2.jsonl"),
        &[&msg(
            "s2",
            "claude-haiku-4-5-20251001",
            "2026-10-11T10:00:40Z",
            [1, 5, 0, 0],
        )],
    );
    let l = Ledger::new();
    let a = l.account(&p);
    assert_eq!((a.main["claude-opus-5-5"].output, a.main.len()), (110, 1));
    let s = a.sub["claude-haiku-4-5-20251001"];
    assert_eq!((s.input, s.output), (6, 55));
    assert_eq!(
        a.reported,
        Some(Reported {
            usd: 2.25,
            unknown: true
        })
    );

    let from = util::parse_rfc3339_ms("2026-10-11T10:00:20Z");
    let to = util::parse_rfc3339_ms("2026-10-11T10:30:00Z");
    let w = l.window(&p, from, to);
    assert_eq!(w.get("claude-opus-5-5"), None);
    assert_eq!(w["claude-haiku-4-5-20251001"].output, 55);
    assert_eq!(l.window(&p, from, None)["claude-opus-5-5"].output, 100);
    assert!(l.account(&tmp.join("none.jsonl")).main.is_empty());
    assert!(l.account(Path::new("")).main.is_empty());
}

#[test]
fn worktree_state() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    write(
        &p,
        &[
            r#"{"type":"user","message":{"content":"x"}}"#,
            r#"{"type":"worktree-state","worktreeSession":{"originalCwd":"/r","worktreePath":"/r/.claude/worktrees/w","worktreeName":"w","worktreeBranch":"worktree-w","originalBranch":"main","originalHeadCommit":"abc"}}"#,
        ],
    );
    let w = read_worktree(&p).unwrap();
    assert_eq!(
        (w.path.as_str(), w.branch.as_str(), w.original_head.as_str()),
        ("/r/.claude/worktrees/w", "worktree-w", "abc")
    );
    write(&p, &[r#"{"type":"user"}"#]);
    assert_eq!(read_worktree(&p), None);
}

/// A synthetic session: two turns, tool calls, a title, usage repeated per
/// content block.
fn synthetic(p: &Path) {
    let lines = [
        r#"{"type":"user","timestamp":"2026-10-11T10:00:00Z","message":{"role":"user","content":"add a login page"}}"#.to_string(),
        msg("m1", "claude-opus-5-5", "2026-10-11T10:00:05Z", [10, 50, 1000, 0]),
        r#"{"type":"assistant","timestamp":"2026-10-11T10:00:06Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/w/src/app.rs"}}],"usage":{"input_tokens":10,"output_tokens":80,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0}}}"#.to_string(),
        r#"{"type":"user","timestamp":"2026-10-11T10:00:07Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"..."}]}}"#.to_string(),
        r#"{"type":"ai-title","aiTitle":"Login page"}"#.to_string(),
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta prompt"}}"#.to_string(),
        r#"{"type":"user","timestamp":"2026-10-11T10:05:00Z","message":{"role":"user","content":[{"type":"text","text":"now run the tests"}]}}"#.to_string(),
        r#"{"type":"assistant","timestamp":"2026-10-11T10:05:05Z","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test\n--quiet","description":"Run the tests"}},{"type":"text","text":"All **green**.\n\nDone."}],"usage":{"input_tokens":5,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":1010}}}"#.to_string(),
    ];
    std::fs::write(p, lines.join("\n") + "\n").unwrap();
}

#[test]
fn summarize_a_synthetic_transcript() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    synthetic(&p);
    let s = summarize(&p, 1 << 20);
    assert_eq!(s.title.as_deref(), Some("Login page"));
    assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(s.last_assistant_text.as_deref(), Some("All **green**.\n\nDone."));
    assert_eq!(s.turns, 2);
    assert!(!s.truncated);
    assert_eq!(s.recent_tools.len(), 2);
    assert_eq!(
        (s.recent_tools[0].name.as_str(), s.recent_tools[0].target.as_str()),
        ("Read", "/w/src/app.rs")
    );
    assert_eq!(
        (
            s.recent_tools[1].name.as_str(),
            s.recent_tools[1].description.as_str(),
            s.recent_tools[1].target.as_str()
        ),
        ("Bash", "Run the tests", "cargo test")
    );
    assert_eq!(s.messages.len(), 2);
    assert_eq!(
        s.messages[0].tokens,
        Tokens {
            input: 10,
            output: 80,
            cache_creation: 1000,
            cache_read: 0
        }
    );
    assert_eq!(s.messages[1].id, "m2");
    assert_eq!(s.by_model["claude-opus-5-5"].output, 100);
    let c = s.context.clone().unwrap();
    assert_eq!((c.tokens, c.window), (1015, 200_000));

    // A small window reads only the end.
    let end = summarize(&p, 600);
    assert!(end.truncated);
    assert!(end.turns <= 1);
    assert_eq!(
        summarize(&tmp.join("none.jsonl"), 1 << 20),
        TranscriptSummary::default()
    );
}

#[test]
fn conversation_with_tail_and_grep() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    synthetic(&p);
    let all = conversation(&p, 0, None);
    assert_eq!(
        all,
        vec![
            (Role::User, "add a login page".to_string()),
            (Role::Assistant, "x".to_string()),
            (Role::Tool, "Read: /w/src/app.rs".to_string()),
            (Role::User, "now run the tests".to_string()),
            (Role::Tool, "Bash: cargo test".to_string()),
            (Role::Assistant, "All **green**.\n\nDone.".to_string()),
        ]
    );
    assert_eq!(conversation(&p, 2, None).len(), 2);
    assert_eq!(
        conversation(&p, 0, Some("TESTS")),
        vec![(Role::User, "now run the tests".to_string())]
    );
}

#[test]
fn transcript_path_lookup() {
    let tmp = TempDir::new();
    let projects = tmp.join("projects");
    std::fs::create_dir_all(projects.join("-w-proj")).unwrap();
    std::fs::write(projects.join("-w-proj/abc.jsonl"), "").unwrap();
    assert_eq!(
        path_in(&projects, "abc"),
        Some(projects.join("-w-proj/abc.jsonl"))
    );
    assert_eq!(path_in(&projects, "nope"), None);
    assert_eq!(path_in(&projects, ""), None);
    assert_eq!(
        subagent_files(&projects.join("-w-proj/abc.jsonl")),
        Vec::<PathBuf>::new()
    );
}

/// The transcript fixture: a session as Claude Code 2.1.289 writes it.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcript-2.1.289/session.jsonl")
}

#[test]
fn a_thread_reads_the_fixture_as_a_conversation() {
    let t = thread(&fixture(), None, 0);
    let kinds: Vec<(EntryKind, &str)> = t.items.iter().map(|i| (i.kind, i.tool.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (EntryKind::You, ""),
            (EntryKind::Text, ""),
            (EntryKind::Tool, "Bash"),
            (EntryKind::Tool, "Bash"),
            (EntryKind::Tool, "Read"),
            (EntryKind::You, ""),
            (EntryKind::Tool, "Edit"),
            (EntryKind::Text, ""),
            (EntryKind::Tool, "Bash"),
        ]
    );
    let i = &t.items;
    // The command echo and the meta line are not the human's words.
    assert_eq!(
        i[0].text,
        "Make the API tests pass. The queue item is feature-001."
    );
    assert!(i[0].at_ms.is_some());
    assert_eq!(
        (i[2].text.as_str(), i[2].code.as_str()),
        ("Show the queue item", "ainet queue show feature-001")
    );
    assert!(i[2].done && !i[2].failed);
    assert_eq!(
        i[2].output,
        "feature-001  in-progress  make the API tests pass\nfrom orch-lead@docs to dev-app@docs"
    );
    // A failed call: its result block's text, first lines.
    assert!(i[3].done && i[3].failed);
    assert_eq!(
        i[3].output,
        "error: test failed, to rerun pass `-p api --test reconnect`\n\nfailures:\n    reconnect::times_out\n    reconnect::retries\n\ntest result: FAILED. 41 passed; 2 failed"
    );
    // Read: the file's first lines, as the result says them.
    assert_eq!(i[4].code, "/work/web-app/api/tests/reconnect.rs");
    assert!(i[4].output.starts_with("1\tuse api::ws;"), "{}", i[4].output);
    assert!(i[4].output.ends_with('…'));
    // What ainetd typed in is a prompt here; ainetd tells it apart.
    assert!(i[5].text.starts_with("[ainet] message #4 from orch-lead@docs:"));
    assert_eq!(
        i[7].text,
        "Raised the timeout to 5s.\n\nNow filing the follow-up and answering the lead."
    );
    // The last call is still waiting.
    assert!(!i[8].done && i[8].output.is_empty());
    // Positions grow along the file; two blocks of one message differ.
    assert!(i.windows(2).all(|w| w[0].pos < w[1].pos));
    assert_eq!(t.model.as_deref(), Some("Opus 5.5"));
    assert_eq!(t.cwd.as_deref(), Some("/work/web-app"));
    assert_eq!(t.permission_mode.as_deref(), Some("acceptEdits"));
}

#[test]
fn a_thread_goes_on_from_where_it_was_read() {
    let tmp = TempDir::new();
    let p = tmp.join("s.jsonl");
    let text = std::fs::read_to_string(fixture()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // The session as it was before its last two messages.
    write(&p, &lines[..lines.len() - 4]);
    let first = thread(&p, None, 0);
    let last = first.items.last().unwrap().clone();
    assert_eq!(last.tool, "Edit");
    assert!(!last.done, "its result is not written yet");
    // Nothing new: nothing after it.
    assert!(thread(&p, Some(last.pos), 0).items.is_empty());
    // Read again from the call still waiting: it now has its result, and
    // what came after follows.
    for l in &lines[lines.len() - 4..] {
        append(&p, &format!("{l}\n"));
    }
    let more = thread(&p, Some(last.pos - 1), 0);
    assert_eq!(more.items[0].pos, last.pos);
    assert!(more.items[0].done);
    assert_eq!(more.items.len(), 3);
    assert!(more.items.iter().all(|i| i.pos >= last.pos));
    // A line still being written is read next time, whole.
    append(
        &p,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"half"#,
    );
    assert_eq!(thread(&p, Some(more.items[2].pos), 0).items, vec![]);
    append(&p, "\"}]}}\n");
    let half = thread(&p, Some(more.items[2].pos), 0);
    assert_eq!(half.items.len(), 1);
    assert_eq!(half.items[0].text, "half");
    // The tail, and a file that is not there.
    assert_eq!(thread(&p, None, 2).items.len(), 2);
    assert_eq!(thread(&tmp.join("none.jsonl"), None, 0), Thread::default());
}
