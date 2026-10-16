//! A real ainetd on a socket, a database and a settings.json of its own,
//! for the tests of what settings change.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ainet_core::client::Client;
use ainet_core::protocol::SpawnSpec;
use ainet_core::{Agent, Request, Response, Runtime};

pub struct Daemon {
    pub socket: PathBuf,
    pub dir: PathBuf,
    /// Left in place when dropped: a test restarting the daemon on it.
    keep: bool,
}

impl Daemon {
    /// A fresh directory, and in it settings.json with `settings` (none:
    /// no file at all).
    pub fn start(name: &str, settings: Option<&str>) -> Daemon {
        let dir = std::env::temp_dir().join(format!("ainetd-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(text) = settings {
            std::fs::write(dir.join("settings.json"), text).unwrap();
        }
        Daemon::on(dir)
    }

    /// A daemon on what an earlier one in `dir` left.
    pub fn on(dir: PathBuf) -> Daemon {
        let socket = dir.join("ainetd.sock");
        let opts = ainet_daemon::Options {
            socket: socket.clone(),
            database: dir.join("state.db"),
            exe: PathBuf::from("ainet"),
            settings: Some(dir.join("settings.json")),
            claude_sessions: dir.join("claude-sessions"),
        };
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(ainet_daemon::run(opts)).unwrap();
        });
        let start = Instant::now();
        while Client::connect(&socket).is_err() {
            assert!(start.elapsed() < Duration::from_secs(10), "ainetd never listened");
            std::thread::sleep(Duration::from_millis(20));
        }
        Daemon {
            socket,
            dir,
            keep: false,
        }
    }

    /// Stops the daemon, keeping its directory for the next.
    pub fn stop(mut self) -> PathBuf {
        self.keep = true;
        let _ = self.call(&Request::Shutdown);
        let start = Instant::now();
        while self.socket.exists() {
            assert!(start.elapsed() < Duration::from_secs(10), "ainetd never stopped");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The old runtime drops its panes on its own thread.
        std::thread::sleep(Duration::from_millis(200));
        self.dir.clone()
    }

    pub fn write_settings(&self, text: &str) {
        std::fs::write(self.dir.join("settings.json"), text).unwrap();
    }

    pub fn call(&self, req: &Request) -> Response {
        let client = Client::connect(&self.socket).unwrap();
        client.set_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut client = client;
        client.call(req).unwrap()
    }

    /// A shell agent running `script`, once it printed `ready`.
    pub fn shell(&self, script: &str) -> String {
        let agent = self.spawn(Runtime::Shell, vec!["-c".into(), script.into()]);
        self.wait_for(&agent.id, "ready");
        agent.id
    }

    pub fn spawn(&self, runtime: Runtime, args: Vec<String>) -> Agent {
        let spec = SpawnSpec {
            runtime,
            cwd: self.dir.clone(),
            role: None,
            task: None,
            parent: None,
            args,
        };
        match self.call(&Request::Spawn(spec)) {
            Response::Spawned { agent } => agent,
            other => panic!("no agent: {other:?}"),
        }
    }

    /// A program standing in for Claude Code: it ignores its arguments
    /// and prints back what is typed into it.
    pub fn fake_claude(&self) -> PathBuf {
        let path = self.dir.join("fake-claude");
        std::fs::write(&path, "#!/bin/sh\nprintf 'ready\\n'\nexec cat\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    pub fn screen(&self, agent: &str) -> String {
        match self.call(&Request::Screen {
            agent: agent.into(),
            formatted: false,
            history: 0,
        }) {
            Response::Screen { rows, .. } => rows.join("\n"),
            other => panic!("no screen: {other:?}"),
        }
    }

    pub fn wait_for(&self, agent: &str, text: &str) {
        let start = Instant::now();
        loop {
            let screen = self.screen(agent);
            if screen.contains(text) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "never saw {text}: {screen}"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// Waits until `check` holds, or fails saying `what`.
    pub fn until(&self, what: &str, mut check: impl FnMut(&Daemon) -> bool) {
        let start = Instant::now();
        while !check(self) {
            assert!(start.elapsed() < Duration::from_secs(20), "never: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Ok(mut c) = Client::connect(&self.socket) {
            let _ = c.call(&Request::Shutdown);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn remove(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}
