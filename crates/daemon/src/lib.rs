//! ainetd: the process that owns the agents.
//!
//! It runs each agent on a PTY it owns (`ainet-pty`), launches whole teams
//! from OpenRig RigSpecs (`rigs`, `launch`), turns Claude Code hook events
//! into agent state (`hooks`), keeps an inbox and holds permission requests
//! until you answer, lets agents talk to each other (`comms`: messages with
//! replies, a chatroom per rig) and hand each other work (`queue`), and adopts
//! agents running elsewhere (`discover`). Clients (the TUI, the CLI, the MCP
//! server, `ainet hook`) talk to it over a Unix socket, one JSON object per
//! line (`ainet_core::protocol`). What it can be told in settings.json is
//! followed while it runs (`config`), and what it keeps is pruned as
//! those settings say (`retention`).

mod admin;
mod agents;
mod always;
mod comms;
mod config;
mod discover;
mod hooks;
mod launch;
mod queue;
mod retention;
mod rigs;
mod runtimes;
mod store;
mod stream;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ainet_core::protocol;
use ainet_core::{Agent, ChatMessage, Event, InboxItem, Message, QItem, Request, Response, Watchdog};
use ainet_pty::{Pane, PaneEvent};
use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc};

pub use admin::{HookAction, claude_settings_file, hook_action};
pub use store::Store;

/// Activity sparkline: buckets of this width, this many of them.
const ACTIVITY_BUCKET_MS: i64 = 30_000;
const ACTIVITY_BUCKETS: usize = 20;

pub struct Options {
    pub socket: PathBuf,
    pub database: PathBuf,
    /// The `ainet` binary, for the MCP server and hooks injected into seats.
    pub exe: PathBuf,
    /// settings.json, followed while the daemon runs; none means the
    /// defaults (tests).
    pub settings: Option<PathBuf>,
    /// Where Claude Code announces its sessions (`~/.claude/sessions`).
    pub claude_sessions: PathBuf,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            socket: ainet_core::paths::socket(),
            database: ainet_core::paths::database(),
            exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ainet")),
            settings: Some(ainet_core::paths::settings_file()),
            claude_sessions: ainet_claude::sessions::sessions_dir(),
        }
    }
}

struct Daemon {
    agents: BTreeMap<String, Agent>,
    panes: HashMap<String, Pane>,
    /// Agents in tmux panes ainet adopted: agent id → tmux pane id.
    tmux: HashMap<String, String>,
    inbox: Vec<InboxItem>,
    next_item: u64,
    /// Held `PermissionRequest`s, by inbox item.
    held_req: HashMap<u64, hooks::Held>,
    /// Den's `/always` rules, per agent, for the life of its session or
    /// kept (`permissions.persist_always`).
    always: HashMap<String, Vec<always::Kept>>,
    activity: HashMap<String, VecDeque<i64>>,
    last_output: HashMap<String, Instant>,
    /// Agents whose terminal you are typing into right now.
    interactive: HashSet<String>,

    rigs: BTreeMap<String, rigs::RigState>,
    messages: Vec<Message>,
    next_message: u64,
    chat: Vec<ChatMessage>,
    next_chat: u64,
    queue: BTreeMap<String, QItem>,
    next_qitem: u64,
    watchdogs: BTreeMap<String, Watchdog>,
    next_watchdog: u64,

    events: broadcast::Sender<Event>,
    pane_tx: mpsc::UnboundedSender<PaneEvent>,
    store: Store,
    socket: PathBuf,
    database: PathBuf,
    exe: PathBuf,
    /// settings.json as it is in force.
    config: config::Config,
    /// Shutting down: rigs are saved as they were and stay so.
    stopping: bool,
    /// Where Claude Code announces its sessions.
    claude_sessions: PathBuf,
    /// When this ainetd started (ms since the epoch).
    started: i64,
}

type Shared = Arc<Mutex<Daemon>>;

fn lock(d: &Shared) -> MutexGuard<'_, Daemon> {
    d.lock().unwrap_or_else(|e| e.into_inner())
}

fn err(message: impl std::fmt::Display) -> Response {
    Response::Error {
        message: message.to_string(),
    }
}

impl Daemon {
    fn emit(&self, event: Event) {
        // No subscribers is fine.
        let _ = self.events.send(event);
    }

    /// Reads what must outlive a restart. Panes died with the old daemon, so
    /// their seats come back stopped; `ainet start` or `ainet up <rig>`
    /// restores them, or the daemon does (`daemon.restore_rigs`). Returns
    /// the rigs that had seats running.
    fn load(&mut self) -> Vec<String> {
        let mut were_running = Vec::new();
        self.rigs = self
            .store
            .scan::<rigs::RigState>("rig", "")
            .into_iter()
            .map(|(name, mut state)| {
                if state.mark_all_stopped() {
                    were_running.push(name.clone());
                }
                (name, state)
            })
            .collect();
        let keep = &self.config.settings.retention;
        let (keep_messages, keep_chat) = (keep.messages_in_memory, keep.chat_in_memory);
        self.messages = self.store.tail("message", keep_messages);
        self.next_message = self.messages.last().map(|m| m.id).unwrap_or(0);
        self.chat = self.store.tail("chat", keep_chat);
        self.next_chat = self.chat.last().map(|m| m.id).unwrap_or(0);
        self.queue = self
            .store
            .scan::<QItem>("qitem", "")
            .into_iter()
            .map(|(_, q)| (q.id.clone(), q))
            .collect();
        self.next_qitem = self.store.get("counter", "qitem").unwrap_or(0);
        self.watchdogs = self
            .store
            .scan::<Watchdog>("watchdog", "")
            .into_iter()
            .map(|(_, w)| (w.id.clone(), w))
            .collect();
        self.next_watchdog = self.store.get("counter", "watchdog").unwrap_or(0);
        self.always_load();
        were_running
    }
}

/// Runs the daemon until `Shutdown` or Ctrl+C.
pub async fn run(opts: Options) -> Result<()> {
    let listener = bind(&opts.socket)?;
    tracing::info!("listening on {}", opts.socket.display());

    let (pane_tx, pane_rx) = mpsc::unbounded_channel();
    let (events, _) = broadcast::channel(4096);
    let follower = ainet_core::settings::Follower::new(opts.settings.clone());
    let config = config::Config::new(&follower);
    config.log();
    let mut daemon = Daemon {
        agents: BTreeMap::new(),
        panes: HashMap::new(),
        tmux: HashMap::new(),
        inbox: Vec::new(),
        next_item: 0,
        held_req: HashMap::new(),
        always: HashMap::new(),
        activity: HashMap::new(),
        last_output: HashMap::new(),
        interactive: HashSet::new(),
        rigs: BTreeMap::new(),
        messages: Vec::new(),
        next_message: 0,
        chat: Vec::new(),
        next_chat: 0,
        queue: BTreeMap::new(),
        next_qitem: 0,
        watchdogs: BTreeMap::new(),
        next_watchdog: 0,
        events,
        pane_tx,
        store: Store::open(&opts.database).context("open the state database")?,
        socket: opts.socket.clone(),
        database: opts.database.clone(),
        exe: opts.exe.clone(),
        config,
        stopping: false,
        claude_sessions: opts.claude_sessions.clone(),
        started: ainet_core::now_ms(),
    };
    let were_running = daemon.load();
    let restore = daemon.settings().daemon.restore_rigs;
    let shared: Shared = Arc::new(Mutex::new(daemon));
    let shutdown = Arc::new(Notify::new());

    tokio::spawn(pane_events(Arc::clone(&shared), pane_rx));
    tokio::spawn(queue::ticker(Arc::clone(&shared)));
    tokio::spawn(discover::ticker(Arc::clone(&shared)));
    tokio::spawn(config::follow(Arc::clone(&shared), follower));
    tokio::spawn(retention::ticker(Arc::clone(&shared)));
    if restore {
        for rig in were_running {
            tracing::info!("restoring rig {rig}, running when ainetd last stopped");
            if let Response::Error { message } = rigs::restore_current(&shared, &rig) {
                tracing::warn!("restoring rig {rig}: {message}");
            }
        }
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                tokio::spawn(serve(stream, Arc::clone(&shared), Arc::clone(&shutdown)));
            }
            _ = shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    let _ = std::fs::remove_file(&opts.socket);
    // Dropping the panes kills the agents' processes; their seats are saved
    // as they were, so the next start can restore them. Their exits, heard
    // while the runtime winds down, must not save them as stopped.
    let mut d = lock(&shared);
    for state in d.rigs.values() {
        d.store.put("rig", &state.rig.name, state);
    }
    d.stopping = true;
    d.panes.clear();
    Ok(())
}

fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            bail!("ainetd is already running on {}", path.display());
        }
        std::fs::remove_file(path).context("remove stale socket")?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    // Only you may drive your agents.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

async fn pane_events(d: Shared, mut rx: mpsc::UnboundedReceiver<PaneEvent>) {
    while let Some(ev) = rx.recv().await {
        let mut d = lock(&d);
        match ev {
            PaneEvent::Output { pane } => {
                // At most ~20 output notices per pane per second.
                let now = Instant::now();
                let due = d
                    .last_output
                    .get(&pane)
                    .is_none_or(|t| now.duration_since(*t) > Duration::from_millis(50));
                if due {
                    d.last_output.insert(pane.clone(), now);
                    d.emit(Event::Output { agent: pane });
                }
            }
            PaneEvent::Exited { pane } => d.pane_exited(&pane),
        }
    }
}

async fn serve(stream: UnixStream, d: Shared, shutdown: Arc<Notify>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = write
                    .write_all(&protocol::encode(&err(format!("bad request: {e}"))))
                    .await;
                continue;
            }
        };
        if matches!(req, Request::Subscribe) {
            let mut rx = lock(&d).events.subscribe();
            if write.write_all(&protocol::encode(&Response::Ok)).await.is_err() {
                return;
            }
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Ok(event) => {
                            if write.write_all(&protocol::encode(&Response::Event { event })).await.is_err() {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => tracing::debug!("subscriber lagged {n}"),
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    line = lines.next_line() => if !matches!(line, Ok(Some(_))) { return },
                }
            }
        }
        if let Request::Attach { agent } = &req {
            match stream::attach(&d, agent) {
                Ok((id, snapshot, attachment)) => {
                    return stream::follow(id, snapshot, attachment, &mut lines, &mut write).await;
                }
                Err(why) => {
                    if write.write_all(&protocol::encode(&err(why))).await.is_err() {
                        return;
                    }
                    continue;
                }
            }
        }
        let stop = matches!(req, Request::Shutdown);
        let resp = handle(req, &d).await;
        if write.write_all(&protocol::encode(&resp)).await.is_err() {
            return;
        }
        if stop {
            shutdown.notify_one();
            return;
        }
    }
}

async fn handle(req: Request, d: &Shared) -> Response {
    match req {
        Request::Hello { version } if version != protocol::VERSION => err(format!(
            "protocol {version} is not {}; restart ainetd (ainet shutdown)",
            protocol::VERSION
        )),
        Request::Hello { .. } => Response::Hello {
            version: protocol::VERSION,
            pid: std::process::id(),
        },
        Request::Snapshot => lock(d).snapshot(),
        Request::Subscribe | Request::Shutdown => Response::Ok,

        Request::Spawn(spec) => match agents::spawn_loose(d, spec) {
            Ok(agent) => Response::Spawned { agent },
            Err(e) => err(format!("{e:#}")),
        },
        Request::Send { agent, text } => comms::type_text(d, &agent, &text).await,
        Request::Input { agent, data } => agents::input(d, &agent, &data),
        Request::Resize { agent, rows, cols } => agents::resize(d, &agent, rows, cols),
        Request::Screen {
            agent,
            formatted,
            history,
        } => agents::screen(d, &agent, formatted, history),
        // A stream of its own, served by `serve` before it comes here.
        Request::Attach { .. } => err("attach is answered as a stream"),
        Request::Close { agent } => agents::close(d, &agent),
        Request::Interact { agent, on } => comms::interact(d, &agent, on).await,
        Request::Answer {
            item,
            decision,
            message,
        } => hooks::answer(d, item, decision, message),
        Request::AnswerQuestion { item, answer } => hooks::answer_question(d, item, &answer),
        Request::Hook {
            agent,
            payload,
            tmux_pane,
        } => hooks::handle(d, agent, payload, tmux_pane).await,
        Request::Whoami { agent } => rigs::whoami(d, &agent),
        Request::Capture {
            targets,
            rig,
            pod,
            lines,
        } => agents::capture(d, &targets, rig.as_deref(), pod.as_deref(), lines),
        Request::Transcript { agent, tail, grep } => agents::transcript(d, &agent, tail, grep.as_deref()),
        Request::Conversation { agent, tail, after } => agents::conversation(d, &agent, tail, after),

        Request::Up(spec) => rigs::up(d, spec),
        Request::Down {
            rig,
            snapshot,
            delete,
        } => rigs::down(d, &rig, snapshot, delete),
        Request::Ps => Response::Rigs {
            rigs: lock(d).rigs.values().map(|s| s.rig.clone()).collect(),
        },
        Request::Status { rig } => rigs::status(d, &rig),
        Request::SnapshotCreate { rig, label } => rigs::snapshot_create(d, &rig, label),
        Request::SnapshotList { rig } => rigs::snapshot_list(d, &rig),
        Request::Restore { rig, snapshot, fresh } => rigs::restore(d, &rig, snapshot.as_deref(), &fresh),
        Request::Grow { rig, seats, edges } => rigs::grow(d, &rig, seats, edges),
        Request::Shrink { rig, pod } => rigs::shrink(d, &rig, &pod),
        Request::RemoveSeat { seat } => rigs::remove_seat(d, &seat),
        Request::Launch { seat, fresh } => rigs::launch_seat(d, &seat, fresh),
        Request::Create { rig, seat } => rigs::create(d, &rig, seat),
        Request::Export { rig } => rigs::export(d, &rig),
        Request::Import { path } => rigs::import(d, &path),
        Request::Specs => Response::Specs {
            specs: ainet_spec::library_from(&lock(d).spec_dirs()),
        },
        Request::SpecShow { name } => match ainet_spec::spec_yaml_from(&lock(d).spec_dirs(), &name) {
            Ok(text) => Response::Text { text },
            Err(e) => err(format!("{e:#}")),
        },
        Request::Validate { path } => Response::Text {
            text: ainet_spec::validate_from(&lock(d).spec_dirs(), &path).to_string(),
        },
        Request::Discover => discover::discover(d),
        Request::Adopt { id, rig, seat } => discover::adopt(d, &id, &rig, seat),
        Request::Release { seat } => discover::release(d, &seat),
        Request::TypingGuard { seat, on } => comms::typing_guard(d, &seat, on).await,
        Request::Held { seat } => comms::held(d, &seat),
        Request::SeatContinue { seat } => rigs::seat_continue(d, &seat),

        Request::Message {
            from,
            to,
            rig,
            pod,
            body,
            reply_to,
            force,
        } => {
            comms::message(
                d,
                from,
                &to,
                rig.as_deref(),
                pod.as_deref(),
                body,
                reply_to,
                force,
            )
            .await
        }
        Request::Inbox {
            agent,
            all,
            limit,
            mark_read,
        } => comms::inbox(d, &agent, all, limit, mark_read),
        Request::Thread { id } => comms::thread(d, id),
        Request::WaitReply { id, timeout_secs } => comms::wait_reply(d, id, timeout_secs).await,
        Request::ChatSend {
            rig,
            topic,
            sender,
            body,
        } => comms::chat_send(d, &rig, topic, sender, body).await,
        Request::ChatHistory {
            rig,
            topic,
            after,
            limit,
        } => comms::chat_history(d, &rig, topic.as_deref(), after, limit),
        Request::ChatWait {
            rig,
            topic,
            after,
            timeout_secs,
        } => comms::chat_wait(d, &rig, topic, after, timeout_secs).await,
        Request::QueueCreate(spec) => queue::create(d, spec).await,
        Request::QueueList { rig, who, state, all } => {
            queue::list(d, rig.as_deref(), who.as_deref(), state, all)
        }
        Request::QueueShow { id } => queue::show(d, &id),
        Request::QueueClaim { id, by } => queue::claim(d, &id, by).await,
        Request::QueueUnclaim { id, by, reason } => queue::unclaim(d, &id, by, reason).await,
        Request::QueueUpdate { id, by, note, state } => queue::update(d, &id, by, note, state).await,
        Request::QueueHandoff {
            id,
            by,
            to,
            body,
            summary,
        } => queue::handoff(d, &id, by, &to, body, summary).await,
        Request::QueueBlock {
            id,
            by,
            on,
            continuation,
            wake_after_secs,
        } => queue::block(d, &id, by, on, continuation, wake_after_secs).await,
        Request::WatchdogRegister {
            target,
            interval_secs,
            message,
            registered_by,
        } => queue::watchdog_register(d, &target, interval_secs, message, registered_by),
        Request::WatchdogList => Response::Watchdogs {
            watchdogs: lock(d).watchdogs.values().cloned().collect(),
        },
        Request::WatchdogStop { id, reason } => queue::watchdog_stop(d, &id, reason),
        Request::AlwaysList { agent } => always::list(d, agent),
        Request::AlwaysRevoke { agent, rule } => always::revoke(d, &agent, rule),
        Request::SettingsGet { key } => admin::settings_get(d, key.as_deref()),
        Request::SettingsSet { key, value } => admin::settings_set(d, key, Some(value)).await,
        Request::SettingsUnset { key } => admin::settings_set(d, key, None).await,
        Request::SettingsReload => admin::settings_reload(d, None).await,
        Request::HookStatus => admin::hooks(d, admin::HookAction::Status).await,
        Request::HookInstall => admin::hooks(d, admin::HookAction::Install).await,
        Request::HookUninstall => admin::hooks(d, admin::HookAction::Uninstall).await,
        Request::Paths => admin::paths(d),
        Request::LogTail { lines } => admin::log_tail(d, lines).await,
    }
}

impl Daemon {
    fn snapshot(&self) -> Response {
        let open: Vec<QItem> = self
            .queue
            .values()
            .filter(|q| q.state.is_open())
            .cloned()
            .collect();
        let mut done: Vec<QItem> = self
            .queue
            .values()
            .filter(|q| !q.state.is_open())
            .cloned()
            .collect();
        done.sort_by_key(|q| std::cmp::Reverse(q.updated));
        done.truncate(50);
        Response::Snapshot {
            agents: self.agents.values().cloned().collect(),
            inbox: self.inbox.clone(),
            rigs: self.rigs.values().map(|s| s.rig.clone()).collect(),
            messages: self.messages.iter().rev().take(300).rev().cloned().collect(),
            chat: self.chat.iter().rev().take(300).rev().cloned().collect(),
            queue: open.into_iter().chain(done).collect(),
        }
    }

    /// Finds an agent by id, seat address, logical id (`pod.member`, in
    /// `rig` or unique across rigs), critter name ("Mossy Lemur",
    /// "mossy-lemur", "mossy") or id prefix.
    fn find_agent(&self, key: &str, rig: Option<&str>) -> Option<String> {
        let key = key.trim().trim_start_matches('@');
        if key.is_empty() {
            return None;
        }
        if self.agents.contains_key(key) {
            return Some(key.to_string());
        }
        if let Some(seat) = self.find_seat(key, rig)
            && let Some(agent) = &seat.agent
        {
            return Some(agent.clone());
        }
        let norm = |s: &str| s.to_lowercase().replace([' ', '-', '_'], "");
        let k = norm(key);
        let live = || self.agents.values().filter(|a| a.state.is_live());
        live()
            .find(|a| norm(&a.name) == k)
            .or_else(|| live().find(|a| norm(&a.name).starts_with(&k)))
            .or_else(|| {
                (key.len() >= 4)
                    .then(|| live().find(|a| a.id.starts_with(key)))
                    .flatten()
            })
            .map(|a| a.id.clone())
    }

    /// Finds a seat by address (`pod-member@rig`) or logical id (`pod.member`,
    /// optionally `pod.member@rig`), preferring `rig`.
    fn find_seat(&self, key: &str, rig: Option<&str>) -> Option<&ainet_core::Seat> {
        let (key, at_rig) = match key.split_once('@') {
            Some((k, r)) if !k.contains('-') || k.contains('.') => (k, Some(r)),
            _ => (key, None),
        };
        let wanted_rig = at_rig.or(rig);
        let seats = || self.rigs.values().flat_map(|s| s.rig.seats.iter());
        if let Some(s) = seats().find(|s| s.address == key) {
            return Some(s);
        }
        let matches: Vec<&ainet_core::Seat> = seats().filter(|s| s.logical == key).collect();
        match wanted_rig {
            Some(r) => matches.into_iter().find(|s| s.rig == r),
            None if matches.len() == 1 => matches.into_iter().next(),
            None => None,
        }
    }

    /// How an agent is shown to others: its seat address, else its name.
    fn label(&self, id: &str) -> String {
        if id == "operator" {
            return "operator".into();
        }
        self.agents
            .get(id)
            .map(Agent::label)
            .unwrap_or_else(|| id.chars().take(8).collect())
    }

    /// The rig an agent sits in.
    fn rig_of(&self, id: &str) -> Option<String> {
        self.agents.get(id).and_then(|a| a.rig.clone())
    }
}
