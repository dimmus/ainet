//! Rigs: teams launched from OpenRig RigSpecs, and their lifecycle.
//!
//! `up` loads a spec (a file, a library name, or a rig ainet already knows),
//! plans its seats and launches them in dependency order. `down` snapshots
//! and stops it; `restore` brings seats back, resuming each Claude Code
//! conversation where its restore policy allows. `grow`, `shrink`,
//! `remove`, `launch` and `create` change a running rig; `export` writes it
//! back as YAML.

use std::collections::BTreeMap;
use std::path::Path;

use ainet_core::protocol::{SeatSpec, UpSpec};
use ainet_core::settings::RuntimeSettings;
use ainet_core::{
    Edge, EdgeKind, Event, Response, Rig, RigStatus, Seat, SeatStatus, SnapshotInfo, Transport, Whoami,
    now_ms,
};
use serde::{Deserialize, Serialize};

use crate::launch::{RigPlan, SeatPlan, launch_seats, on_path, program};
use crate::store::num_key;
use crate::{Daemon, Shared, err, lock};

/// A rig as the daemon keeps it: what you see (`rig`) and how to launch it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RigState {
    pub rig: Rig,
    pub plan: RigPlan,
}

impl RigState {
    /// A rig from its plan; its seats' typing guards start as
    /// `typing_guard` says (`messaging.typing_guard`).
    fn new(plan: RigPlan, typing_guard: bool) -> Self {
        let seats = plan
            .seats
            .iter()
            .map(|s| seat_record(&plan.name, s, typing_guard))
            .collect();
        let rig = Rig {
            name: plan.name.clone(),
            summary: plan.summary.clone(),
            spec_path: plan.spec_path.clone(),
            root: plan.root.clone(),
            status: RigStatus::Stopped,
            seats,
            edges: plan.edges.clone(),
            created: now_ms(),
        };
        RigState { rig, plan }
    }

    /// After a daemon restart: panes are gone, sessions are kept for resume.
    /// True when any of them was running (or starting).
    pub(crate) fn mark_all_stopped(&mut self) -> bool {
        let mut were_running = false;
        for s in &mut self.rig.seats {
            if s.transport == Transport::Pane && s.status != SeatStatus::Stopped {
                were_running |= matches!(s.status, SeatStatus::Running | SeatStatus::Launching);
                s.status = SeatStatus::Stopped;
            }
        }
        self.recompute();
        were_running
    }

    fn recompute(&mut self) {
        let seats = &self.rig.seats;
        let running = seats.iter().filter(|s| s.status == SeatStatus::Running).count();
        let launching = seats
            .iter()
            .any(|s| matches!(s.status, SeatStatus::Launching | SeatStatus::Pending));
        self.rig.status = if launching {
            RigStatus::Launching
        } else if running == seats.len() && running > 0 {
            RigStatus::Running
        } else if running == 0 {
            RigStatus::Stopped
        } else {
            RigStatus::Partial
        };
    }
}

fn seat_record(rig: &str, s: &SeatPlan, typing_guard: bool) -> Seat {
    Seat {
        address: s.address.clone(),
        logical: s.logical.clone(),
        rig: rig.to_string(),
        pod: s.pod.clone(),
        member: s.member.clone(),
        label: s.label.clone(),
        role: s.role.clone(),
        runtime: s.runtime,
        model: s.model.clone(),
        cwd: s.cwd.clone(),
        agent: None,
        status: SeatStatus::Pending,
        transport: Transport::Pane,
        restore_policy: s.restore_policy.clone(),
        typing_guard,
        held: 0,
    }
}

/// A snapshot as stored: the plan and the session in every seat.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SnapshotRecord {
    info: SnapshotInfo,
    plan: RigPlan,
    sessions: BTreeMap<String, String>,
}

impl Daemon {
    /// Where your own specs are: `~/.config/ainet/specs`, then the
    /// directories settings.json adds (`rigs.spec_dirs`).
    pub(crate) fn spec_dirs(&self) -> Vec<std::path::PathBuf> {
        std::iter::once(ainet_core::paths::specs_dir())
            .chain(
                self.settings()
                    .rigs
                    .spec_dirs
                    .iter()
                    .map(|p| ainet_core::paths::expand(p)),
            )
            .collect()
    }

    /// Where a seat or agent runs when it was given no directory:
    /// `rigs.default_cwd`, else your home.
    pub(crate) fn default_cwd(&self) -> std::path::PathBuf {
        match &self.settings().rigs.default_cwd {
            Some(p) => ainet_core::paths::expand(p),
            None => std::env::var_os("HOME")
                .map(Into::into)
                .unwrap_or_else(|| "/".into()),
        }
    }

    /// Typing guards of new seats (`messaging.typing_guard`).
    fn typing_guard(&self) -> bool {
        self.settings().messaging.typing_guard
    }

    pub(crate) fn save_rig(&mut self, name: &str) {
        if self.stopping {
            return;
        }
        if let Some(state) = self.rigs.get_mut(name) {
            state.recompute();
            for seat in &mut state.rig.seats {
                seat.held = seat
                    .agent
                    .as_ref()
                    .map(|a| self.messages.iter().filter(|m| &m.to == a && m.held).count() as u32)
                    .unwrap_or(0);
            }
            let state = state.clone();
            self.store.put("rig", name, &state);
            self.emit(Event::Rig { rig: state.rig });
        }
    }

    pub(crate) fn set_seat_status(&mut self, rig: &str, address: &str, status: SeatStatus) {
        if let Some(state) = self.rigs.get_mut(rig)
            && let Some(s) = state.rig.seats.iter_mut().find(|s| s.address == address)
        {
            s.status = status;
        }
        self.save_rig(rig);
    }

    pub(crate) fn set_seat_agent(&mut self, rig: &str, address: &str, agent: &str, status: SeatStatus) {
        if let Some(state) = self.rigs.get_mut(rig)
            && let Some(s) = state.rig.seats.iter_mut().find(|s| s.address == address)
        {
            s.agent = Some(agent.to_string());
            s.status = status;
        }
        self.save_rig(rig);
    }

    /// A seat's process ended.
    pub(crate) fn seat_exited(&mut self, agent: &str) {
        let found = self.rigs.iter().find_map(|(name, st)| {
            st.rig
                .seats
                .iter()
                .find(|s| s.agent.as_deref() == Some(agent))
                .map(|s| (name.clone(), s.address.clone()))
        });
        if let Some((rig, address)) = found {
            self.set_seat_status(&rig, &address, SeatStatus::Stopped);
        }
    }

    /// Points each agent in a rig at its seat and at the seat that directs
    /// it (the source of a `delegates_to` edge), for the Fleet tree and Map.
    pub(crate) fn relink(&mut self, rig: &str) {
        let Some(state) = self.rigs.get(rig) else { return };
        let by_logical: BTreeMap<String, Option<String>> = state
            .rig
            .seats
            .iter()
            .map(|s| (s.logical.clone(), s.agent.clone()))
            .collect();
        let mut updates = Vec::new();
        for s in &state.rig.seats {
            let Some(agent) = &s.agent else { continue };
            let parent = state
                .rig
                .edges
                .iter()
                .find(|e| {
                    (e.kind == EdgeKind::DelegatesTo && e.to == s.logical)
                        || (e.kind == EdgeKind::SpawnedBy && e.from == s.logical)
                })
                .and_then(|e| {
                    by_logical
                        .get(if e.kind == EdgeKind::DelegatesTo {
                            &e.from
                        } else {
                            &e.to
                        })
                        .cloned()
                        .flatten()
                });
            updates.push((
                agent.clone(),
                s.address.clone(),
                parent,
                s.role.clone().or(s.label.clone()),
            ));
        }
        for (agent, address, parent, role) in updates {
            if let Some(a) = self.agents.get_mut(&agent) {
                a.seat = Some(address);
                a.rig = Some(rig.to_string());
                a.parent = parent;
                if a.role.is_none() {
                    a.role = role;
                }
                self.publish(&agent);
            }
        }
    }

    fn kill_seat(&mut self, seat: &Seat) {
        if let Some(agent) = &seat.agent
            && let Some(p) = self.panes.get_mut(agent)
        {
            let _ = p.kill();
        }
    }
}

/// What `up --plan` prints.
fn describe(plan: &RigPlan, runtimes: &RuntimeSettings) -> String {
    let mut out = format!(
        "rig {}  ({} seats, root {})\n",
        plan.name,
        plan.seats.len(),
        plan.root.display()
    );
    if let Some(s) = &plan.summary {
        out.push_str(&format!("{}\n", s.trim()));
    }
    out.push_str("\nseats, in launch order:\n");
    for s in &plan.seats {
        let installed = if on_path(&program(runtimes, s.runtime)) {
            ""
        } else {
            "   (not installed!)"
        };
        out.push_str(&format!(
            "  {:<28} {:<12} {:<14} {}{}\n",
            s.address,
            s.runtime.spec_name(),
            s.model.as_deref().unwrap_or("default model"),
            s.cwd.display(),
            installed
        ));
    }
    if !plan.edges.is_empty() {
        out.push_str("\nedges:\n");
        for e in &plan.edges {
            out.push_str(&format!("  {} {} {}\n", e.from, e.kind.label(), e.to));
        }
    }
    for w in &plan.warnings {
        out.push_str(&format!("warning: {w}\n"));
    }
    out
}

pub(crate) fn up(d: &Shared, spec: UpSpec) -> Response {
    // A rig ainet already knows, without a spec file of that name: restore it.
    let known = lock(d).rigs.contains_key(&spec.source) && !Path::new(&spec.source).exists();
    if known && !spec.plan {
        return restore(d, &spec.source, None, &spec.fresh);
    }
    let resolved = if Path::new(&spec.source).exists() {
        ainet_spec::load(Path::new(&spec.source))
    } else {
        let dirs = lock(d).spec_dirs();
        ainet_spec::load_named_from(&dirs, &spec.source)
    };
    let resolved = match resolved {
        Ok(r) => r,
        Err(e) => return err(format!("{e:#}")),
    };
    let mut plan = RigPlan::from_resolved(&resolved);
    if let Some(cwd) = &spec.cwd {
        for s in &mut plan.seats {
            s.cwd = cwd.clone();
        }
        plan.root = cwd.clone();
    }
    if let Some(name) = &spec.name {
        plan.rename(name);
    }
    let runtimes = lock(d).settings().runtimes.clone();
    if spec.plan {
        return Response::Text {
            text: describe(&plan, &runtimes),
        };
    }
    if let Some(missing) = plan
        .seats
        .iter()
        .map(|s| program(&runtimes, s.runtime))
        .find(|p| !on_path(p))
    {
        return err(format!(
            "{missing} is not installed, and this rig needs it (ainet up {} --plan shows the seats)",
            spec.source
        ));
    }
    let name = plan.name.clone();
    let addresses: Vec<String> = plan.seats.iter().map(|s| s.address.clone()).collect();
    {
        let mut g = lock(d);
        // Same rig again: keep the sessions of seats that are still there.
        let previous = g.rigs.get(&name).cloned();
        if let Some(prev) = &previous
            && prev
                .rig
                .seats
                .iter()
                .any(|s| matches!(s.status, SeatStatus::Running | SeatStatus::Launching))
        {
            return err(format!(
                "rig {name} is already running: use ainet grow to add seats, or ainet down {name} first"
            ));
        }
        let mut state = RigState::new(plan, g.typing_guard());
        if let Some(prev) = previous {
            for s in &mut state.rig.seats {
                s.agent = prev.rig.seat(&s.address).and_then(|p| p.agent.clone());
            }
        }
        g.rigs.insert(name.clone(), state);
        g.save_rig(&name);
    }
    launch_seats(d, &name, addresses, spec.fresh);
    status(d, &name)
}

pub(crate) fn status(d: &Shared, rig: &str) -> Response {
    match lock(d).rigs.get(rig) {
        Some(s) => Response::Rig { rig: s.rig.clone() },
        None => err(format!("no rig {rig} (ainet ps lists them)")),
    }
}

pub(crate) fn down(d: &Shared, rig: &str, snapshot: bool, delete: bool) -> Response {
    if snapshot && let Response::Error { message } = snapshot_create(d, rig, Some("down".into())) {
        return err(message);
    }
    let mut g = lock(d);
    let Some(state) = g.rigs.get(rig).cloned() else {
        return err(format!("no rig {rig}"));
    };
    for seat in &state.rig.seats {
        g.kill_seat(seat);
        if let Some(a) = &seat.agent {
            g.tmux.remove(a);
        }
    }
    if let Some(st) = g.rigs.get_mut(rig) {
        for s in &mut st.rig.seats {
            s.status = SeatStatus::Stopped;
        }
    }
    if delete {
        g.rigs.remove(rig);
        g.store.delete("rig", rig);
        g.emit(Event::RigRemoved {
            name: rig.to_string(),
        });
        return Response::Ok;
    }
    g.save_rig(rig);
    Response::Rig {
        rig: g.rigs[rig].rig.clone(),
    }
}

pub(crate) fn snapshot_create(d: &Shared, rig: &str, label: Option<String>) -> Response {
    let g = lock(d);
    let Some(state) = g.rigs.get(rig).cloned() else {
        return err(format!("no rig {rig}"));
    };
    let created = now_ms();
    let sessions: BTreeMap<String, String> = state
        .rig
        .seats
        .iter()
        .filter_map(|s| s.agent.clone().map(|a| (s.address.clone(), a)))
        .collect();
    let info = SnapshotInfo {
        id: format!("{rig}@{created}"),
        rig: rig.to_string(),
        label,
        created,
        seats: sessions.len(),
    };
    let record = SnapshotRecord {
        info: info.clone(),
        plan: state.plan.clone(),
        sessions,
    };
    g.store
        .put("snapshot", &format!("{rig}/{}", num_key(created as u64)), &record);
    g.store.log(
        created,
        None,
        "snapshot",
        &serde_json::json!({ "rig": rig, "id": info.id }),
    );
    if let Some(keep) = g.settings().rigs.keep_snapshots {
        g.prune_snapshots(rig, keep as usize);
    }
    Response::Snapshots {
        snapshots: vec![info],
    }
}

impl Daemon {
    /// Deletes all but the newest `keep` snapshots of a rig
    /// (`rigs.keep_snapshots`).
    fn prune_snapshots(&self, rig: &str, keep: usize) {
        let keys: Vec<String> = self
            .store
            .scan::<serde_json::Value>("snapshot", &format!("{rig}/"))
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for key in keys.iter().take(keys.len().saturating_sub(keep)) {
            self.store.delete("snapshot", key);
        }
    }
}

pub(crate) fn snapshot_list(d: &Shared, rig: &str) -> Response {
    let g = lock(d);
    let snapshots = g
        .store
        .scan::<SnapshotRecord>("snapshot", &format!("{rig}/"))
        .into_iter()
        .map(|(_, r)| r.info)
        .collect();
    Response::Snapshots { snapshots }
}

/// Brings stopped seats back: from a snapshot (the latest when none is
/// named), else from what ainet last knew.
pub(crate) fn restore(d: &Shared, rig: &str, snapshot: Option<&str>, fresh: &[String]) -> Response {
    restore_from(d, rig, Some(snapshot), fresh)
}

/// Brings a rig's stopped seats back as ainet last knew them, not from a
/// snapshot: what `daemon.restore_rigs` does at start.
pub(crate) fn restore_current(d: &Shared, rig: &str) -> Response {
    restore_from(d, rig, None, &[])
}

/// [`restore`]; `snapshot` is `None` to skip snapshots altogether.
fn restore_from(d: &Shared, rig: &str, snapshot: Option<Option<&str>>, fresh: &[String]) -> Response {
    let to_launch = {
        let mut g = lock(d);
        let records = match snapshot {
            Some(_) => g.store.scan::<SnapshotRecord>("snapshot", &format!("{rig}/")),
            None => Vec::new(),
        };
        let record = match snapshot.flatten() {
            Some(id) => match records.into_iter().find(|(_, r)| r.info.id == id) {
                Some((_, r)) => Some(r),
                None => return err(format!("no snapshot {id} for {rig}")),
            },
            None => records.into_iter().last().map(|(_, r)| r),
        };
        let current = g.rigs.get(rig).cloned();
        let mut state = match (current, record) {
            (Some(cur), None) => cur,
            (cur, Some(rec)) => {
                let mut st = RigState::new(rec.plan.clone(), g.typing_guard());
                for s in &mut st.rig.seats {
                    s.agent = rec.sessions.get(&s.address).cloned();
                    s.status = SeatStatus::Stopped;
                    // Seats running right now stay as they are.
                    if let Some(live) = cur.as_ref().and_then(|c| c.rig.seat(&s.address))
                        && matches!(live.status, SeatStatus::Running | SeatStatus::Launching)
                    {
                        *s = live.clone();
                    }
                }
                st
            }
            (None, None) => return err(format!("no rig or snapshot called {rig}")),
        };
        for s in &mut state.rig.seats {
            if !matches!(s.status, SeatStatus::Running | SeatStatus::Launching) {
                s.status = SeatStatus::Pending;
            }
        }
        let to_launch: Vec<String> = state
            .rig
            .seats
            .iter()
            .filter(|s| s.status == SeatStatus::Pending)
            .map(|s| s.address.clone())
            .collect();
        if let Some(missing) = state
            .plan
            .seats
            .iter()
            .filter(|s| to_launch.contains(&s.address))
            .map(|s| program(&g.settings().runtimes, s.runtime))
            .find(|p| !on_path(p))
        {
            return err(format!("{missing} is not installed, and this rig needs it"));
        }
        g.rigs.insert(rig.to_string(), state);
        g.save_rig(rig);
        to_launch
    };
    launch_seats(d, rig, to_launch, fresh.to_vec());
    status(d, rig)
}

pub(crate) fn grow(d: &Shared, rig: &str, seats: Vec<SeatSpec>, edges: Vec<Edge>) -> Response {
    let added = {
        let mut g = lock(d);
        let Some(state) = g.rigs.get(rig).cloned() else {
            return err(format!("no rig {rig}"));
        };
        let mut plans = Vec::new();
        for spec in &seats {
            let seat = match ainet_spec::seat_from_spec(rig, &state.plan.root, spec) {
                Ok(s) => SeatPlan::from(&s),
                Err(e) => return err(format!("{e:#}")),
            };
            if state.plan.seat(&seat.address).is_some()
                || plans.iter().any(|p: &SeatPlan| p.address == seat.address)
            {
                return err(format!("{} already exists", seat.address));
            }
            let program = program(&g.settings().runtimes, seat.runtime);
            if !on_path(&program) {
                return err(format!("{program} is not installed"));
            }
            plans.push(seat);
        }
        let typing_guard = g.typing_guard();
        let st = g.rigs.get_mut(rig).expect("checked above");
        for p in &plans {
            st.rig.seats.push(seat_record(rig, p, typing_guard));
            st.plan.seats.push(p.clone());
        }
        st.plan.edges.extend(edges.iter().cloned());
        st.rig.edges.extend(edges);
        g.save_rig(rig);
        plans.into_iter().map(|p| p.address).collect::<Vec<_>>()
    };
    launch_seats(d, rig, added, vec![]);
    status(d, rig)
}

fn remove_where(d: &Shared, rig: &str, pick: impl Fn(&Seat) -> bool) -> Response {
    let mut g = lock(d);
    let Some(state) = g.rigs.get(rig).cloned() else {
        return err(format!("no rig {rig}"));
    };
    let gone: Vec<Seat> = state.rig.seats.iter().filter(|s| pick(s)).cloned().collect();
    if gone.is_empty() {
        return err("no such seats");
    }
    for s in &gone {
        g.kill_seat(s);
    }
    let logicals: Vec<String> = gone.iter().map(|s| s.logical.clone()).collect();
    let st = g.rigs.get_mut(rig).expect("checked above");
    st.rig.seats.retain(|s| !pick(s));
    st.plan.seats.retain(|s| !logicals.contains(&s.logical));
    st.rig
        .edges
        .retain(|e| !logicals.contains(&e.from) && !logicals.contains(&e.to));
    st.plan
        .edges
        .retain(|e| !logicals.contains(&e.from) && !logicals.contains(&e.to));
    g.save_rig(rig);
    Response::Rig {
        rig: g.rigs[rig].rig.clone(),
    }
}

pub(crate) fn shrink(d: &Shared, rig: &str, pod: &str) -> Response {
    remove_where(d, rig, |s| s.pod == pod)
}

pub(crate) fn remove_seat(d: &Shared, key: &str) -> Response {
    let Some(seat) = lock(d).find_seat(key, None).cloned() else {
        return err(format!("no seat {key}"));
    };
    let address = seat.address.clone();
    remove_where(d, &seat.rig, move |s| s.address == address)
}

pub(crate) fn launch_seat(d: &Shared, key: &str, fresh: bool) -> Response {
    let seat = {
        let mut g = lock(d);
        let Some(seat) = g.find_seat(key, None).cloned() else {
            return err(format!("no seat {key}"));
        };
        if matches!(seat.status, SeatStatus::Running | SeatStatus::Launching) {
            if !fresh {
                return err(format!("{} is running; --fresh replaces it", seat.address));
            }
            g.kill_seat(&seat);
        }
        g.set_seat_status(&seat.rig, &seat.address, SeatStatus::Pending);
        seat
    };
    let fresh_list = if fresh { vec![seat.address.clone()] } else { vec![] };
    launch_seats(d, &seat.rig, vec![seat.address.clone()], fresh_list);
    status(d, &seat.rig)
}

pub(crate) fn create(d: &Shared, rig: &str, mut seat: SeatSpec) -> Response {
    if lock(d).rigs.contains_key(rig) {
        return err(format!("rig {rig} exists"));
    }
    if seat.pod.is_empty() {
        seat.pod = "main".into();
    }
    if seat.member.is_empty() {
        seat.member = "agent".into();
    }
    let root = seat.cwd.clone().unwrap_or_else(|| lock(d).default_cwd());
    let resolved = match ainet_spec::seat_from_spec(rig, &root, &seat) {
        Ok(s) => s,
        Err(e) => return err(format!("{e:#}")),
    };
    let plan = RigPlan {
        name: rig.to_string(),
        summary: None,
        root,
        spec_path: None,
        edges: vec![],
        seats: vec![SeatPlan::from(&resolved)],
        warnings: vec![],
    };
    let program = program(&lock(d).settings().runtimes, plan.seats[0].runtime);
    if !on_path(&program) {
        return err(format!("{program} is not installed"));
    }
    let address = plan.seats[0].address.clone();
    {
        let mut g = lock(d);
        let state = RigState::new(plan, g.typing_guard());
        g.rigs.insert(rig.to_string(), state);
        g.save_rig(rig);
    }
    launch_seats(d, rig, vec![address], vec![]);
    status(d, rig)
}

pub(crate) fn export(d: &Shared, rig: &str) -> Response {
    match lock(d).rigs.get(rig) {
        Some(s) => Response::Text {
            text: ainet_spec::to_yaml(&s.plan.to_resolved()),
        },
        None => err(format!("no rig {rig}")),
    }
}

pub(crate) fn import(d: &Shared, path: &Path) -> Response {
    let resolved = match ainet_spec::load(path) {
        Ok(r) => r,
        Err(e) => return err(format!("{e:#}")),
    };
    let plan = RigPlan::from_resolved(&resolved);
    let name = plan.name.clone();
    let mut g = lock(d);
    if g.rigs.contains_key(&name) {
        return err(format!("rig {name} exists"));
    }
    let mut state = RigState::new(plan, g.typing_guard());
    for s in &mut state.rig.seats {
        s.status = SeatStatus::Stopped;
    }
    g.rigs.insert(name.clone(), state);
    g.save_rig(&name);
    Response::Rig {
        rig: g.rigs[&name].rig.clone(),
    }
}

pub(crate) fn whoami(d: &Shared, key: &str) -> Response {
    let g = lock(d);
    let Some(id) = g.find_agent(key, None) else {
        return err(format!("no agent {key}"));
    };
    let agent = &g.agents[&id];
    let seat = agent.seat.as_deref().and_then(|s| g.find_seat(s, None)).cloned();
    let (mut delegates_to, mut delegated_by, mut peers, mut edges) = (vec![], vec![], vec![], vec![]);
    if let Some(s) = &seat
        && let Some(state) = g.rigs.get(&s.rig)
    {
        for e in &state.rig.edges {
            if e.from != s.logical && e.to != s.logical {
                continue;
            }
            edges.push(e.clone());
            if e.kind == EdgeKind::DelegatesTo {
                if e.from == s.logical {
                    delegates_to.push(e.to.clone());
                } else {
                    delegated_by.push(e.from.clone());
                }
            }
        }
        peers = state
            .rig
            .seats
            .iter()
            .filter(|o| o.address != s.address)
            .map(|o| o.logical.clone())
            .collect();
    }
    let me = agent.seat.clone().unwrap_or_else(|| agent.name.clone());
    let open_queue = g
        .queue
        .values()
        .filter(|q| q.state.is_open() && (q.destination == me || q.owner.as_deref() == Some(&me)))
        .count() as u32;
    Response::Whoami {
        whoami: Box::new(Whoami {
            agent: id.clone(),
            name: agent.name.clone(),
            seat,
            delegates_to,
            delegated_by,
            peers,
            edges,
            unread: agent.unread,
            open_queue,
        }),
    }
}

/// Nudges a seat stuck at a prompt, or relaunches it if it stopped.
pub(crate) fn seat_continue(d: &Shared, key: &str) -> Response {
    let seat = lock(d).find_seat(key, None).cloned();
    match seat {
        Some(s) if !matches!(s.status, SeatStatus::Running | SeatStatus::Launching) => {
            launch_seat(d, &s.address, false)
        }
        Some(s) => {
            let Some(agent) = s.agent else {
                return err("the seat has no agent");
            };
            match lock(d).write_raw(&agent, b"\r") {
                Ok(()) => Response::Ok,
                Err(e) => err(e),
            }
        }
        None => err(format!("no seat {key}")),
    }
}
