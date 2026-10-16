//! Streamed terminals end to end: a real ainetd on a socket of its own,
//! a shell agent on one of its panes, and blocking clients as the front
//! ends use them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ainet_core::client::Client;
use ainet_core::protocol::SpawnSpec;
use ainet_core::{Event, Request, Response, Runtime};

/// A daemon of its own for one test, shut down when dropped.
struct Daemon {
    socket: PathBuf,
    dir: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        let dir = std::env::temp_dir().join(format!("ainetd-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("ainetd.sock");
        let opts = ainet_daemon::Options {
            socket: socket.clone(),
            database: dir.join("state.db"),
            exe: PathBuf::from("ainet"),
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
        Daemon { socket, dir }
    }

    fn call(&self, req: &Request) -> Response {
        let mut client = client(&self.socket);
        client.call(req).unwrap()
    }

    /// A shell agent running `script`, once it printed `ready`.
    fn shell(&self, script: &str) -> String {
        let spec = SpawnSpec {
            runtime: Runtime::Shell,
            cwd: self.dir.clone(),
            role: None,
            task: None,
            parent: None,
            args: vec!["-c".into(), script.into()],
        };
        let Response::Spawned { agent } = self.call(&Request::Spawn(spec)) else {
            panic!("no agent");
        };
        self.wait_for(&agent.id, "ready");
        agent.id
    }

    fn wait_for(&self, agent: &str, text: &str) {
        let start = Instant::now();
        loop {
            let screen = self.call(&Request::Screen {
                agent: agent.into(),
                formatted: false,
                history: 0,
            });
            if let Response::Screen { rows, .. } = &screen
                && rows.iter().any(|r| r.contains(text))
            {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "never saw {text}: {screen:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut c) = Client::connect(&self.socket) {
            let _ = c.call(&Request::Shutdown);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn client(socket: &Path) -> Client {
    let client = Client::connect(socket).unwrap();
    client.set_timeout(Some(Duration::from_secs(5))).unwrap();
    client
}

/// A client's copy of a streamed screen.
struct Copy {
    parser: vt100::Parser,
    bytes: usize,
    screens: usize,
}

impl Copy {
    fn apply(&mut self, resp: Response) {
        match resp {
            Response::Screen { ansi, size, .. } => {
                self.parser = vt100::Parser::new(size.0, size.1, 0);
                self.parser.process(ansi.unwrap_or_default().as_bytes());
                self.screens += 1;
            }
            Response::Event {
                event: Event::Bytes { data, .. },
            } => {
                self.parser.process(data.as_bytes());
                self.bytes += 1;
            }
            other => panic!("not a stream: {other:?}"),
        }
    }

    fn contents(&self) -> String {
        self.parser.screen().contents()
    }
}

#[test]
fn attaching_sends_the_screen_then_the_output() {
    let daemon = Daemon::start("stream");
    let agent = daemon.shell("printf 'ready\\n'; read x; printf 'got %s\\n' \"$x\"; read y");
    let mut stream = client(&daemon.socket);
    let first = stream.call(&Request::Attach { agent: agent.clone() }).unwrap();
    let mut copy = Copy {
        parser: vt100::Parser::new(1, 1, 0),
        bytes: 0,
        screens: 0,
    };
    copy.apply(first);
    assert_eq!(copy.screens, 1);
    assert!(copy.contents().contains("ready"), "{}", copy.contents());

    daemon.call(&Request::Input {
        agent: agent.clone(),
        data: "abc\r".into(),
    });
    while !copy.contents().contains("got abc") {
        copy.apply(stream.receive().unwrap());
    }
    assert!(copy.bytes > 0, "the output came as bytes, not screens");
    // Once the output settles, the whole screen comes again, the same.
    let screens = copy.screens;
    while copy.screens == screens {
        copy.apply(stream.receive().unwrap());
    }
    let Response::Screen { rows, .. } = daemon.call(&Request::Screen {
        agent: agent.clone(),
        formatted: false,
        history: 0,
    }) else {
        panic!("no screen");
    };
    assert!(
        copy.contents().starts_with(&rows.join("\n")),
        "{:?}",
        copy.contents()
    );

    // A resize sends the screen at its new size.
    daemon.call(&Request::Resize {
        agent: agent.clone(),
        rows: 12,
        cols: 50,
    });
    while copy.parser.screen().size() != (12, 50) {
        copy.apply(stream.receive().unwrap());
    }
}

#[test]
fn closing_either_end_detaches() {
    let daemon = Daemon::start("detach");
    let agent = daemon.shell("printf 'ready\\n'; while read x; do echo \"<$x>\"; done");
    // The client goes away: the pane carries on, and attaches again.
    let mut first = client(&daemon.socket);
    assert!(matches!(
        first.call(&Request::Attach { agent: agent.clone() }),
        Ok(Response::Screen { .. })
    ));
    drop(first);
    daemon.call(&Request::Input {
        agent: agent.clone(),
        data: "one\r".into(),
    });
    daemon.wait_for(&agent, "<one>");
    let mut second = client(&daemon.socket);
    let Ok(Response::Screen { rows, .. }) = second.call(&Request::Attach { agent: agent.clone() }) else {
        panic!("no second attach");
    };
    assert!(rows.iter().any(|r| r == "<one>"), "{rows:?}");
    // The program ends: the stream ends after what it printed.
    daemon.call(&Request::Close { agent });
    let start = Instant::now();
    loop {
        match second.receive() {
            Ok(_) => assert!(start.elapsed() < Duration::from_secs(10)),
            Err(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof, "{e}");
                break;
            }
        }
    }
}

#[test]
fn what_cannot_be_attached_says_why_and_the_connection_goes_on() {
    let daemon = Daemon::start("refuse");
    let mut c = client(&daemon.socket);
    let Ok(Response::Error { message }) = c.call(&Request::Attach {
        agent: "nobody".into(),
    }) else {
        panic!("attached to nobody");
    };
    assert!(message.contains("no agent nobody"), "{message}");
    assert!(matches!(
        c.call(&Request::Hello {
            version: ainet_core::protocol::VERSION
        }),
        Ok(Response::Hello { .. })
    ));
}

#[test]
fn scrollback_comes_with_its_colours_when_asked() {
    let daemon = Daemon::start("styled");
    let agent = daemon.shell(
        "for i in $(seq 1 60); do printf '\\033[32mline\\033[0m %s\\n' $i; done; printf 'ready\\n'; read x",
    );
    let screen = |formatted| {
        daemon.call(&Request::Screen {
            agent: agent.clone(),
            formatted,
            history: 100,
        })
    };
    let Response::Screen {
        rows,
        styled_rows: Some(styled),
        ..
    } = screen(true)
    else {
        panic!("no styled rows");
    };
    assert_eq!(rows.len(), styled.len());
    assert_eq!(rows[0], "line 1");
    assert_eq!(styled[0], "\x1b[0;32mline\x1b[0m 1");
    // Plain, as old clients ask: no colours.
    assert!(matches!(
        screen(false),
        Response::Screen {
            styled_rows: None,
            ..
        }
    ));
}
