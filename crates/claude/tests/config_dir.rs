//! The environment-reading entry points honour `CLAUDE_CONFIG_DIR`. Tests
//! cannot set environment variables in-process (unsafe in edition 2024, and
//! racy), so the check runs in a child copy of this test binary.

use std::path::PathBuf;
use std::process::Command;

const CHILD: &str = "AINET_CLAUDE_TEST_CHILD";

#[test]
fn child_reads_the_fake_config_dir() {
    let Some(dir) = std::env::var_os(CHILD) else {
        return; // only meaningful in the child
    };
    let dir = PathBuf::from(dir);
    // The parent writes our session file once it knows our pid.
    let mine = dir.join(format!("sessions/{}.json", std::process::id()));
    for _ in 0..500 {
        if mine.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(ainet_claude::config_dir(), dir);
    assert_eq!(ainet_claude::sessions::sessions_dir(), dir.join("sessions"));
    assert_eq!(ainet_claude::hooks::settings_path(), dir.join("settings.json"));
    assert_eq!(ainet_claude::badge::store_dir(), dir.join("plugins/store"));
    assert_eq!(ainet_claude::transcript::projects_dir(), dir.join("projects"));

    let live = ainet_claude::sessions::live_sessions();
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0].session_id, "fake-live");
    assert_eq!(live[0].pid, std::process::id() as i32);
    let all = ainet_claude::sessions::scan();
    assert_eq!(all.len(), 2);
    assert!(!all[1].alive);
    let me = ainet_claude::sessions::session_for_pid(std::process::id() as i32).unwrap();
    assert_eq!(me.cwd, "/work/proj");
    assert_eq!(
        ainet_claude::transcript::path_for("fake-live"),
        Some(dir.join("projects/-work-proj/fake-live.jsonl"))
    );
    assert_eq!(
        ainet_claude::badge::load()["fake-live"]
            .labels
            .as_ref()
            .unwrap()
            .role,
        "tester"
    );
}

#[test]
fn sessions_discovery_on_a_fake_config_dir() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("ainet-claude-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::create_dir_all(dir.join("projects/-work-proj")).unwrap();
    std::fs::create_dir_all(dir.join("plugins/store")).unwrap();
    std::fs::write(dir.join("projects/-work-proj/fake-live.jsonl"), "").unwrap();
    std::fs::write(
        dir.join("plugins/store/agent-badge_x.json"),
        r#"{"badge:fake-live":{"role":"tester"}}"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("sessions/999999999.json"),
        r#"{"sessionId":"fake-gone","startedAt":0}"#,
    )
    .unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&exe)
        .args([
            "--exact",
            "child_reads_the_fake_config_dir",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, &dir)
        .env("CLAUDE_CONFIG_DIR", &dir)
        .spawn()
        .unwrap();
    // The child's own pid is the live session; a pid that cannot exist is
    // the stale one.
    // Written to a temporary name and renamed, so the child never reads half
    // a file.
    let pid = child.id();
    let tmp = dir.join("sessions/tmp");
    std::fs::write(
        &tmp,
        format!(
            r#"{{"pid":{pid},"sessionId":"fake-live","cwd":"/work/proj","startedAt":1,"status":"idle"}}"#
        ),
    )
    .unwrap();
    std::fs::rename(&tmp, dir.join(format!("sessions/{pid}.json"))).unwrap();
    let status = child.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(status.success(), "child failed: {status}");
}
