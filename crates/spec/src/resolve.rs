//! From RigSpec YAML to a [`ResolvedRig`]: checks the spec by OpenRig's
//! rules, loads each member's AgentSpec, layers startup files and actions
//! into guidance and text to type, and sorts the seats into launch order.
//!
//! Problems are collected rather than returned one at a time, so `validate`
//! can list them all; `parse` and `load` fail if there is any error.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ainet_core::settings::PermissionMode;
use ainet_core::{Edge, EdgeKind, Runtime};

use crate::files;
use crate::raw::{self, RawAction, RawAgent, RawEdge, RawFile, RawMember, RawRig, RawStartup, Text, text};
use crate::{Doc, ResolvedRig, ResolvedSeat};

/// Where a spec's paths resolve.
pub struct Origin<'a> {
    /// Member working directories resolve here.
    pub root: &'a Path,
    /// Culture, startup files and `local:` agent refs resolve here. The same
    /// as `root` for a spec on disk; the library for a built-in.
    pub dir: &'a Path,
    pub spec_path: Option<&'a Path>,
}

pub struct Outcome {
    pub rig: Option<ResolvedRig>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

const RESTORE_POLICIES: [&str; 3] = ["resume_if_possible", "relaunch_fresh", "checkpoint_only"];
const HINTS: [&str; 4] = ["auto", "guidance_merge", "skill_install", "send_text"];
const UNSUPPORTED_RUNTIMES: [&str; 3] = ["pi", "omp", "stub"];

/// Keys OpenRig knows that ainet reads but does not act on.
const IGNORED_RIG_KEYS: [&str; 5] = [
    "services",
    "permission_policy",
    "non_interruptive",
    "managed_blocks",
    "workspace",
];
const QUIET_RIG_KEYS: [&str; 1] = ["docs"];
const IGNORED_POD_KEYS: [&str; 1] = ["continuity_policy"];
const IGNORED_MEMBER_KEYS: [&str; 6] = [
    "permission_policy",
    "codex_config_profile",
    "compaction_strategy",
    "mechanic",
    "session_source",
    "starter_ref",
];

/// One member, from a RigSpec or a [`SeatSpec`](ainet_core::protocol::SeatSpec).
#[derive(Default)]
pub struct MemberIn {
    pub id: String,
    pub agent_ref: Option<String>,
    pub profile: Option<String>,
    pub runtime: Option<String>,
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub role: Option<String>,
    pub restore_policy: Option<String>,
    pub startup: Option<RawStartup>,
    pub guidance: Option<String>,
    pub first_message: Option<String>,
    /// OpenRig's `permission_policy`, the member's over the rig's.
    pub permission_policy: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

fn owned(v: &Option<Text>) -> Option<String> {
    text(v).map(str::to_string)
}

impl MemberIn {
    fn from_raw(m: &RawMember, id: String) -> Self {
        Self {
            id,
            agent_ref: owned(&m.agent_ref),
            profile: owned(&m.profile),
            runtime: owned(&m.runtime),
            cwd: owned(&m.cwd),
            label: owned(&m.label),
            model: owned(&m.model),
            effort: owned(&m.effort),
            role: owned(&m.role),
            restore_policy: owned(&m.restore_policy),
            startup: m.startup.clone(),
            permission_policy: raw::loose(&m.extra, "permission_policy").map(str::to_string),
            ..Self::default()
        }
    }
}

/// A startup block and the directory its file paths resolve in.
struct Layer {
    /// How messages name it: `rig startup`, `pod dev startup`, …
    what: String,
    dir: PathBuf,
    startup: RawStartup,
}

/// A loaded AgentSpec with its imports.
struct Agent {
    name: String,
    dir: PathBuf,
    raw: RawAgent,
    imports: Vec<(String, PathBuf, RawAgent)>,
}

/// What a seat receives, built up layer by layer.
#[derive(Default)]
struct Delivery {
    guidance: Vec<Doc>,
    send_text: Vec<String>,
    restore_send_text: Vec<String>,
    skills: Vec<PathBuf>,
    actions: Vec<String>,
    restore_actions: Vec<String>,
}

pub struct Resolver<'a> {
    origin: Origin<'a>,
    rig: String,
    errors: Vec<String>,
    warnings: Vec<String>,
    agents: HashMap<PathBuf, Option<Rc<Agent>>>,
    /// The rig's `permission_policy`, for members without their own.
    policy: Option<String>,
}

fn push_unique(list: &mut Vec<String>, msg: String) {
    if !list.contains(&msg) {
        list.push(msg);
    }
}

/// Parses and resolves a RigSpec.
pub fn resolve(yaml: &str, origin: Origin<'_>) -> Outcome {
    let mut r = Resolver::new(origin, String::new());
    let raw: RawRig = match serde_saphyr::from_str(yaml) {
        Ok(raw) => raw,
        Err(e) => {
            r.error(format!("not a valid RigSpec: {e}"));
            return r.finish(None);
        }
    };
    let rig = r.rig(&raw);
    r.finish(rig)
}

/// Resolves one seat outside a spec: `grow`, `add`, `create`.
pub fn seat(rig: &str, root: &Path, member: MemberIn, pod: &str) -> Outcome {
    let mut r = Resolver::new(
        Origin {
            root,
            dir: root,
            spec_path: None,
        },
        rig.to_string(),
    );
    r.check_id("rig name", rig, true);
    r.check_id("pod id", pod, false);
    r.check_id("member id", &member.id, false);
    let seat = r.seat(pod, &member, &[]);
    let rig = seat.map(|s| ResolvedRig {
        name: rig.to_string(),
        summary: None,
        root: root.to_path_buf(),
        spec_path: None,
        seats: vec![s],
        edges: Vec::new(),
        warnings: Vec::new(),
    });
    r.finish(rig)
}

impl<'a> Resolver<'a> {
    fn new(origin: Origin<'a>, rig: String) -> Self {
        Self {
            origin,
            rig,
            errors: Vec::new(),
            warnings: Vec::new(),
            agents: HashMap::new(),
            policy: None,
        }
    }

    fn error(&mut self, msg: String) {
        push_unique(&mut self.errors, msg);
    }

    fn warn(&mut self, msg: String) {
        push_unique(&mut self.warnings, msg);
    }

    fn finish(self, rig: Option<ResolvedRig>) -> Outcome {
        let rig = rig.map(|mut rig| {
            rig.warnings = self.warnings.clone();
            rig
        });
        Outcome {
            rig,
            errors: self.errors,
            warnings: self.warnings,
        }
    }

    fn extra_keys(&mut self, at: &str, extra: &raw::Extra, ignored: &[&str], quiet: &[&str]) {
        for key in extra.keys() {
            if quiet.contains(&key.as_str()) {
            } else if ignored.contains(&key.as_str()) {
                self.warn(format!("{at}: `{key}` is accepted but ainet ignores it"));
            } else {
                self.warn(format!(
                    "{at}: unknown key `{key}` (ignored; OpenRig would refuse it)"
                ));
            }
        }
    }

    /// Pod and member ids: no `.` or `@`, and safe in a session name.
    fn check_id(&mut self, what: &str, id: &str, rig_name: bool) -> bool {
        let allowed =
            |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || (rig_name && "@.".contains(c));
        let problem = if id.is_empty() {
            Some("is missing".to_string())
        } else if !rig_name && (id.contains('.') || id.contains('@')) {
            Some("must not contain `.` or `@`".to_string())
        } else if !id.chars().all(allowed) {
            Some("may use only letters, digits, `-` and `_`".to_string())
        } else {
            None
        };
        match problem {
            Some(p) => {
                self.error(format!("{what} {id:?} {p}"));
                false
            }
            None => true,
        }
    }

    // ---- the rig --------------------------------------------------------

    fn rig(&mut self, raw: &RawRig) -> Option<ResolvedRig> {
        let name = text(&raw.name).unwrap_or_default().to_string();
        if self.check_id("rig name", &name, true) {
            self.rig = name.clone();
        }
        if text(&raw.version).is_none() {
            self.warn("no `version`: OpenRig needs version: \"0.2\"".into());
        }
        self.extra_keys("rig", &raw.extra, &IGNORED_RIG_KEYS, &QUIET_RIG_KEYS);
        self.policy = raw::loose(&raw.extra, "permission_policy").map(str::to_string);

        let dir = self.origin.dir.to_path_buf();
        let mut layers = Vec::new();
        if let Some(culture) = text(&raw.culture_file) {
            let file = RawFile {
                path: Some(Text(culture.into())),
                ..RawFile::default()
            };
            let startup = RawStartup {
                files: Some(vec![file]),
                actions: None,
            };
            self.check_startup("culture_file", &startup);
            let dir = self.culture_dir(culture);
            layers.push(Layer {
                what: "culture_file".into(),
                dir,
                startup,
            });
        }
        if let Some(startup) = &raw.startup {
            self.check_startup("rig startup", startup);
            layers.push(Layer {
                what: "rig startup".into(),
                dir: dir.clone(),
                startup: startup.clone(),
            });
        }

        let pods = raw.pods.as_deref().unwrap_or_default();
        if pods.is_empty() {
            self.error("`pods` must list at least one pod".into());
        }
        let mut seats = Vec::new();
        let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for pod in pods {
            let id = text(&pod.id).unwrap_or_default().to_string();
            let ok = self.check_id("pod id", &id, false);
            if ok && members.contains_key(&id) {
                self.error(format!("pod id {id:?} is used twice"));
                continue;
            }
            self.extra_keys(&format!("pod {id}"), &pod.extra, &IGNORED_POD_KEYS, &[]);
            let mut pod_layers: Vec<&Layer> = layers.iter().collect();
            let pod_layer = pod.startup.as_ref().map(|startup| {
                self.check_startup(&format!("pod {id} startup"), startup);
                Layer {
                    what: format!("pod {id} startup"),
                    dir: dir.clone(),
                    startup: startup.clone(),
                }
            });
            pod_layers.extend(pod_layer.as_ref());

            let mut ids: Vec<String> = Vec::new();
            for m in pod.members.as_deref().unwrap_or_default() {
                let mid = text(&m.id).unwrap_or_default().to_string();
                let where_ = format!("member {id}.{mid}");
                if !self.check_id(&format!("{where_}: id"), &mid, false) {
                    continue;
                }
                if ids.contains(&mid) {
                    self.error(format!("{where_}: member id is used twice in pod {id}"));
                    continue;
                }
                ids.push(mid.clone());
                self.extra_keys(&where_, &m.extra, &IGNORED_MEMBER_KEYS, &[]);
                if ok && let Some(seat) = self.seat(&id, &MemberIn::from_raw(m, mid), &pod_layers) {
                    seats.push(seat);
                }
            }
            if ok {
                members.insert(id, ids);
            }
        }

        let mut edges = Vec::new();
        for pod in pods {
            let id = text(&pod.id).unwrap_or_default();
            let Some(ids) = members.get(id) else { continue };
            for e in pod.edges.as_deref().unwrap_or_default() {
                if let Some(edge) = self.pod_edge(id, ids, e) {
                    edges.push(edge);
                }
            }
        }
        for e in raw.edges.as_deref().unwrap_or_default() {
            if let Some(edge) = self.cross_edge(&members, e) {
                edges.push(edge);
            }
        }

        if !self.errors.is_empty() {
            return None;
        }
        let seats = match crate::launch_order(&seats, &edges) {
            Ok(order) => order.into_iter().map(|i| seats[i].clone()).collect(),
            Err(e) => {
                self.error(e.to_string());
                return None;
            }
        };
        Some(ResolvedRig {
            name,
            summary: owned(&raw.summary),
            root: self.origin.root.to_path_buf(),
            spec_path: self.origin.spec_path.map(Path::to_path_buf),
            seats,
            edges,
            warnings: Vec::new(),
        })
    }

    /// Where the culture file resolves: the spec's directory, or the
    /// library when it is not there but the library has one by that name
    /// (an exported or copied built-in).
    fn culture_dir(&mut self, culture: &str) -> PathBuf {
        let dir = self.origin.dir.to_path_buf();
        let builtin = files::builtin_root();
        if files::unsafe_reason(culture).is_none()
            && !files::exists(&dir.join(culture))
            && files::exists(&builtin.join(culture))
        {
            if files::is_real(&dir) {
                self.warn(format!(
                    "culture_file {culture} is not in {}; using ainet's built-in one",
                    dir.display()
                ));
            }
            return builtin;
        }
        dir
    }

    // ---- edges ----------------------------------------------------------

    fn edge_kind(&mut self, at: &str, e: &RawEdge) -> Option<EdgeKind> {
        self.extra_keys(at, &e.extra, &[], &[]);
        let kind = text(&e.kind).unwrap_or_default();
        let found = [
            EdgeKind::DelegatesTo,
            EdgeKind::SpawnedBy,
            EdgeKind::CanObserve,
            EdgeKind::CollaboratesWith,
            EdgeKind::EscalatesTo,
        ]
        .into_iter()
        .find(|k| k.label() == kind);
        if found.is_none() {
            self.error(format!(
                "{at}: edge kind {kind:?} is not one of delegates_to, spawned_by, can_observe, collaborates_with, escalates_to"
            ));
        }
        found
    }

    fn pod_edge(&mut self, pod: &str, ids: &[String], e: &RawEdge) -> Option<Edge> {
        let (from, to) = (text(&e.from).unwrap_or_default(), text(&e.to).unwrap_or_default());
        let at = format!("pod {pod} edge {from} -> {to}");
        let kind = self.edge_kind(&at, e);
        let mut ok = true;
        for end in [from, to] {
            if end.contains('.') {
                self.error(format!(
                    "{at}: pod-local edges use member ids, not `pod.member` ({end:?})"
                ));
                ok = false;
            } else if !ids.iter().any(|m| m == end) {
                self.error(format!("{at}: {end:?} is not a member of pod {pod}"));
                ok = false;
            }
        }
        let kind = kind.filter(|_| ok)?;
        Some(Edge {
            kind,
            from: format!("{pod}.{from}"),
            to: format!("{pod}.{to}"),
        })
    }

    fn cross_edge(&mut self, members: &BTreeMap<String, Vec<String>>, e: &RawEdge) -> Option<Edge> {
        let (from, to) = (text(&e.from).unwrap_or_default(), text(&e.to).unwrap_or_default());
        let at = format!("edge {from} -> {to}");
        let kind = self.edge_kind(&at, e);
        let mut pods = Vec::new();
        for end in [from, to] {
            match end.split_once('.') {
                Some((p, m)) if members.get(p).is_some_and(|ids| ids.iter().any(|i| i == m)) => pods.push(p),
                Some(_) => self.error(format!("{at}: no seat {end:?}")),
                None => self.error(format!("{at}: cross-pod edges use `pod.member` ids ({end:?})")),
            }
        }
        if pods.len() == 2 && pods[0] == pods[1] {
            self.error(format!(
                "{at}: both ends are in pod {}; use a pod-local edge",
                pods[0]
            ));
            return None;
        }
        let kind = kind.filter(|_| pods.len() == 2)?;
        Some(Edge {
            kind,
            from: from.to_string(),
            to: to.to_string(),
        })
    }

    // ---- startup blocks -------------------------------------------------

    fn check_startup(&mut self, at: &str, s: &RawStartup) {
        for f in s.files() {
            let path = text(&f.path).unwrap_or_default();
            if let Some(why) = files::unsafe_reason(path) {
                self.error(format!("{at}: file path {path:?} {why}"));
            }
            let hint = text(&f.delivery_hint).unwrap_or("auto");
            if !HINTS.contains(&hint) {
                self.error(format!(
                    "{at}: file {path}: delivery_hint {hint:?} is not one of {}",
                    HINTS.join(", ")
                ));
            }
            self.check_applies_on(&format!("{at}: file {path}"), &f.applies_on);
        }
        for a in s.actions() {
            self.check_action(at, a);
        }
    }

    fn check_applies_on(&mut self, at: &str, applies: &Option<Vec<Text>>) {
        for v in raw::list(applies) {
            if v != "fresh_start" && v != "restore" {
                self.error(format!("{at}: applies_on {v:?} is not fresh_start or restore"));
            }
        }
    }

    fn check_action(&mut self, at: &str, a: &RawAction) {
        let kind = text(&a.kind).unwrap_or_default();
        let value = text(&a.value).unwrap_or_default();
        let at = format!("{at}: action {kind} {value:?}");
        match kind {
            "send_text" | "slash_command" => {}
            "startup_proof" => {
                if value != "authenticated" && value != "none" {
                    self.error(format!(
                        "{at}: a startup_proof value is `authenticated` or `none`"
                    ));
                }
            }
            "shell" => self.error(format!("{at}: shell actions are not supported")),
            other => self.error(format!(
                "{at}: type {other:?} is not slash_command, send_text or startup_proof"
            )),
        }
        if value.is_empty() {
            self.error(format!("{at}: `value` is required"));
        }
        if let Some(phase) = text(&a.phase).filter(|p| *p != "after_files" && *p != "after_ready") {
            self.error(format!("{at}: phase {phase:?} is not after_files or after_ready"));
        }
        match a.idempotent {
            None => self.error(format!("{at}: `idempotent` is required")),
            Some(false) if applies(&a.applies_on).1 => {
                self.error(format!("{at}: a non-idempotent action must not apply on restore"))
            }
            _ => {}
        }
        self.check_applies_on(&at, &a.applies_on);
    }

    // ---- seats ----------------------------------------------------------

    /// Resolves one member. `layers` are the culture, rig and pod layers
    /// that apply to it, outermost first.
    fn seat(&mut self, pod: &str, m: &MemberIn, layers: &[&Layer]) -> Option<ResolvedSeat> {
        let logical = format!("{pod}.{}", m.id);
        let at = format!("seat {logical}");
        if let Some(s) = &m.startup {
            self.check_startup(&format!("{at} startup"), s);
        }

        let terminal_ref = m.agent_ref.as_deref() == Some("builtin:terminal");
        let none_profile = m.profile.as_deref() == Some("none");
        let terminal = m
            .runtime
            .as_deref()
            .is_some_and(|r| r.parse() == Ok(Runtime::Shell));
        if terminal || terminal_ref || none_profile {
            let triple = (terminal || m.runtime.is_none())
                && (terminal_ref || m.agent_ref.is_none())
                && (none_profile || m.profile.is_none());
            if !triple {
                self.error(format!(
                    "{at}: a terminal node needs runtime: terminal, agent_ref: builtin:terminal and profile: none together"
                ));
                return None;
            }
            return self.terminal_seat(pod, m, logical);
        }

        let profile_name = m.profile.as_deref().unwrap_or("default");
        let agent = match m.agent_ref.as_deref() {
            Some(r) => Some(self.agent(&at, r)?),
            None => None,
        };
        let prof = agent
            .as_ref()
            .and_then(|a| a.raw.profiles.as_ref().and_then(|p| p.get(profile_name)));
        if let Some(a) = &agent
            && prof.is_none()
            && profile_name != "default"
        {
            self.error(format!("{at}: agent {} has no profile {profile_name:?}", a.name));
            return None;
        }

        let defaults = agent.as_ref().and_then(|a| a.raw.defaults.as_ref());
        let prefs = prof.and_then(|p| p.preferences.as_ref());
        let pick = |member: &Option<String>, pref: Option<&Option<Text>>, def: Option<&Option<Text>>| {
            member
                .clone()
                .or_else(|| pref.and_then(owned))
                .or_else(|| def.and_then(owned))
        };
        let runtime_name = pick(
            &m.runtime,
            prefs.map(|p| &p.runtime),
            defaults.map(|d| &d.runtime),
        )
        .unwrap_or_else(|| "claude-code".into());
        let runtime = self.runtime(&at, &runtime_name)?;
        if runtime == Runtime::Shell {
            self.error(format!(
                "{at}: runtime terminal needs agent_ref: builtin:terminal and profile: none"
            ));
            return None;
        }
        let model = pick(&m.model, prefs.map(|p| &p.model), defaults.map(|d| &d.model));
        let effort = pick(&m.effort, prefs.map(|p| &p.effort), defaults.map(|d| &d.effort));

        let restore_policy = self.restore_policy(
            &at,
            [
                defaults
                    .and_then(|d| d.lifecycle.as_ref())
                    .and_then(|l| text(&l.restore_policy)),
                prof.and_then(|p| p.lifecycle.as_ref())
                    .and_then(|l| text(&l.restore_policy)),
                m.restore_policy.as_deref(),
            ],
        );

        let mut out = Delivery::default();
        for layer in layers {
            self.deliver(&layer.what, &layer.dir, &layer.startup, &mut out);
        }
        if let Some(a) = &agent {
            self.agent_layers(&at, a, prof, &mut out);
        }
        if let Some(s) = &m.startup {
            self.deliver(&format!("{at} startup"), self.origin.dir, s, &mut out);
        }
        if let Some(g) = m.guidance.as_deref().filter(|g| !g.trim().is_empty()) {
            out.guidance.push(Doc {
                title: format!("{logical} guidance"),
                text: g.to_string(),
            });
        }
        if let Some(first) = m.first_message.as_deref().filter(|f| !f.trim().is_empty()) {
            out.actions.push(first.to_string());
        }

        let cwd = self.cwd(&at, m.cwd.as_deref());
        let Delivery {
            guidance,
            mut send_text,
            mut restore_send_text,
            skills,
            actions,
            restore_actions,
        } = out;
        send_text.extend(actions);
        restore_send_text.extend(restore_actions);
        Some(ResolvedSeat {
            pod: pod.to_string(),
            member: m.id.clone(),
            address: format!("{pod}-{}@{}", m.id, self.rig),
            logical,
            label: m.label.clone(),
            role: m.role.clone(),
            runtime,
            model,
            effort,
            cwd,
            restore_policy,
            agent_name: agent.as_ref().map(|a| a.name.clone()),
            guidance,
            send_text,
            restore_send_text,
            skills,
            permission_policy: m.permission_policy.clone().or_else(|| self.policy.clone()),
            permission_mode: m.permission_mode,
            args: m.args.clone(),
            env: m.env.clone(),
        })
    }

    /// A terminal node: no agent, no guidance; only its own startup is typed.
    fn terminal_seat(&mut self, pod: &str, m: &MemberIn, logical: String) -> Option<ResolvedSeat> {
        let at = format!("seat {logical}");
        if m.role.is_some() {
            self.warn(format!("{at}: `role` is not used on a terminal node"));
        }
        let mut out = Delivery::default();
        if let Some(s) = &m.startup {
            self.deliver(&format!("{at} startup"), self.origin.dir, s, &mut out);
        }
        if let Some(first) = m.first_message.as_deref().filter(|f| !f.trim().is_empty()) {
            out.actions.push(first.to_string());
        }
        let restore_policy = self.restore_policy(&at, [None, None, m.restore_policy.as_deref()]);
        let cwd = self.cwd(&at, m.cwd.as_deref());
        let Delivery {
            mut send_text,
            mut restore_send_text,
            actions,
            restore_actions,
            ..
        } = out;
        send_text.extend(actions);
        restore_send_text.extend(restore_actions);
        Some(ResolvedSeat {
            pod: pod.to_string(),
            member: m.id.clone(),
            address: format!("{pod}-{}@{}", m.id, self.rig),
            logical,
            label: m.label.clone(),
            role: None,
            runtime: Runtime::Shell,
            model: None,
            effort: None,
            cwd,
            restore_policy,
            agent_name: None,
            guidance: Vec::new(),
            send_text,
            restore_send_text,
            skills: Vec::new(),
            permission_policy: None,
            permission_mode: None,
            args: m.args.clone(),
            env: m.env.clone(),
        })
    }

    fn runtime(&mut self, at: &str, name: &str) -> Option<Runtime> {
        let lower = name.trim().to_ascii_lowercase();
        if UNSUPPORTED_RUNTIMES.contains(&lower.as_str()) {
            self.error(format!(
                "{at}: runtime {lower} is not supported by ainet (use claude-code, codex or terminal)"
            ));
            return None;
        }
        match name.parse::<Runtime>() {
            Ok(r) => Some(r),
            Err(e) => {
                self.error(format!("{at}: {e}"));
                None
            }
        }
    }

    /// The narrowest of the given policies: each layer may only narrow.
    fn restore_policy(&mut self, at: &str, layers: [Option<&str>; 3]) -> String {
        let mut current = 0;
        for (i, policy) in layers.into_iter().enumerate() {
            let Some(policy) = policy else { continue };
            match RESTORE_POLICIES.iter().position(|p| *p == policy) {
                Some(rank) if rank >= current => current = rank,
                Some(_) if i == 2 => self.warn(format!(
                    "{at}: restore_policy {policy} cannot widen the agent's {}; keeping that",
                    RESTORE_POLICIES[current]
                )),
                Some(_) => {}
                None => self.error(format!(
                    "{at}: restore_policy {policy:?} is not one of {}",
                    RESTORE_POLICIES.join(", ")
                )),
            }
        }
        RESTORE_POLICIES[current].to_string()
    }

    fn cwd(&mut self, at: &str, raw: Option<&str>) -> PathBuf {
        let cwd = files::resolve_cwd(raw.unwrap_or("."), self.origin.root);
        if !cwd.is_dir() {
            self.warn(format!("{at}: cwd {} does not exist", cwd.display()));
        }
        cwd
    }

    // ---- agent specs ----------------------------------------------------

    /// Loads the AgentSpec an `agent_ref` names. A `local:` ref that is not on
    /// disk falls back to the built-in library, so specs exported by ainet
    /// and copies of built-ins keep their agents.
    fn agent(&mut self, at: &str, reference: &str) -> Option<Rc<Agent>> {
        let dir = self.agent_dir(at, reference, self.origin.dir.to_path_buf())?;
        if let Some(cached) = self.agents.get(&dir) {
            return cached.clone();
        }
        let mut found = files::read(&dir.join("agent.yaml")).map(|t| (dir.clone(), t));
        if found.is_none()
            && let Some(rel) = reference.strip_prefix("local:")
        {
            let builtin = files::normalize(&files::builtin_root().join(rel.trim()));
            found = files::read(&builtin.join("agent.yaml")).map(|t| (builtin, t));
            if found.is_some() && files::is_real(&dir) {
                self.warn(format!(
                    "{at}: {reference} is not in {}; using ainet's built-in agent",
                    dir.display()
                ));
            }
        }
        let agent = match found {
            None => {
                self.error(format!(
                    "{at}: agent_ref {reference}: no agent.yaml in {}",
                    dir.display()
                ));
                None
            }
            Some((real_dir, text)) => self.parse_agent(at, reference, real_dir, &text).map(Rc::new),
        };
        self.agents.insert(dir, agent.clone());
        agent
    }

    fn agent_dir(&mut self, at: &str, reference: &str, base: PathBuf) -> Option<PathBuf> {
        let reference = reference.trim();
        if let Some(rel) = reference.strip_prefix("local:") {
            if Path::new(rel.trim()).is_absolute() {
                self.error(format!(
                    "{at}: {reference}: a local: ref is relative; use path: for an absolute path"
                ));
                return None;
            }
            Some(files::normalize(&base.join(rel.trim())))
        } else if let Some(abs) = reference.strip_prefix("path:") {
            if !Path::new(abs.trim()).is_absolute() {
                self.error(format!(
                    "{at}: {reference}: a path: ref is absolute; use local: for a relative path"
                ));
                return None;
            }
            Some(files::normalize(Path::new(abs.trim())))
        } else {
            self.error(format!(
                "{at}: agent_ref {reference:?} must start with local: or path: (or be builtin:terminal)"
            ));
            None
        }
    }

    fn parse_agent(&mut self, at: &str, reference: &str, dir: PathBuf, yaml: &str) -> Option<Agent> {
        let file = dir.join("agent.yaml");
        let raw: RawAgent = match serde_saphyr::from_str(yaml) {
            Ok(raw) => raw,
            Err(e) => {
                self.error(format!(
                    "{at}: agent_ref {reference}: {} is not a valid AgentSpec: {e}",
                    file.display()
                ));
                return None;
            }
        };
        let name = owned(&raw.name).unwrap_or_else(|| {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "agent".into())
        });
        let label = format!("agent {name}");
        if let Some(startup) = &raw.startup {
            self.check_startup(&format!("{label} startup"), startup);
        }
        for (pname, p) in raw.profiles.iter().flatten() {
            if let Some(startup) = &p.startup {
                self.check_startup(&format!("{label} profile {pname} startup"), startup);
            }
            let uses = p.uses.as_ref();
            let unused: Vec<&str> = [
                ("plugins", uses.map(|u| &u.plugins)),
                ("subagents", uses.map(|u| &u.subagents)),
                ("runtime_resources", uses.map(|u| &u.runtime_resources)),
            ]
            .into_iter()
            .filter(|(_, v)| v.is_some_and(|v| !raw::list(v).is_empty()))
            .map(|(k, _)| k)
            .collect();
            if !unused.is_empty() {
                self.warn(format!(
                    "{label}: profile {pname} uses {}, which ainet ignores",
                    unused.join(", ")
                ));
            }
        }
        let resources = raw.resources.as_ref();
        for r in resources
            .into_iter()
            .flat_map(|r| r.skills.iter().chain(r.guidance.iter()).flatten())
        {
            let path = text(&r.path).unwrap_or_default();
            if let Some(why) = files::unsafe_reason(path) {
                self.error(format!("{label}: resource path {path:?} {why}"));
            }
        }

        let mut imports = Vec::new();
        for imp in raw.imports.iter().flatten() {
            let Some(r) = text(&imp.reference) else { continue };
            let Some(idir) = self.agent_dir(&label, r, dir.clone()) else {
                continue;
            };
            let parsed = files::read(&idir.join("agent.yaml"))
                .and_then(|t| serde_saphyr::from_str::<RawAgent>(&t).ok());
            match parsed {
                Some(iraw) => {
                    let iname = owned(&iraw.name).unwrap_or_default();
                    imports.push((iname, idir, iraw));
                }
                None => self.warn(format!(
                    "{label}: import {r} has no readable agent.yaml; its resources are skipped"
                )),
            }
        }
        Some(Agent {
            name,
            dir,
            raw,
            imports,
        })
    }

    /// The agent's guidance resources, then its own and its profile's
    /// startup files, into `out`.
    fn agent_layers(&mut self, at: &str, a: &Agent, prof: Option<&raw::RawProfile>, out: &mut Delivery) {
        let label = format!("agent {}", a.name);
        let startups: Vec<&RawStartup> = a
            .raw
            .startup
            .iter()
            .chain(prof.and_then(|p| p.startup.as_ref()))
            .collect();
        let startup_paths: Vec<PathBuf> = startups
            .iter()
            .flat_map(|s| s.files())
            .filter_map(|f| text(&f.path))
            .map(|p| files::normalize(&a.dir.join(p)))
            .collect();

        // Guidance the profile selects, then any other the agent declares,
        // unless it is already delivered as a startup file.
        let uses = prof.and_then(|p| p.uses.as_ref());
        let mut ids: Vec<String> = uses
            .map(|u| raw::list(&u.guidance))
            .unwrap_or_default()
            .into_iter()
            .map(String::from)
            .collect();
        let own = a.raw.resources.as_ref().and_then(|r| r.guidance.as_ref());
        for r in own.into_iter().flatten() {
            if let Some(id) = text(&r.id).filter(|id| !ids.iter().any(|i| i == id)) {
                ids.push(id.to_string());
            }
        }
        for id in ids.iter().filter(|id| id.as_str() != "rig-role") {
            match find_resource(a, Kind::Guidance, id) {
                Some(path) if startup_paths.contains(&path) => {}
                Some(path) => match files::read(&path) {
                    Some(text) => out.guidance.push(Doc {
                        title: format!("{label}: {id}"),
                        text,
                    }),
                    None => self.warn(format!("{label}: guidance {id} not found at {}", path.display())),
                },
                None => self.warn(format!("{label}: guidance {id:?} is not a declared resource")),
            }
        }

        for s in startups {
            self.deliver(&format!("{label} startup"), &a.dir, s, out);
        }

        for id in uses.map(|u| raw::list(&u.skills)).unwrap_or_default() {
            let path = find_resource(a, Kind::Skill, id).or_else(|| {
                Some(files::normalize(&self.origin.dir.join("skills").join(id))).filter(|p| files::exists(p))
            });
            match path {
                Some(p) if files::exists(&p) => {
                    if files::is_real(&p) && !out.skills.contains(&p) {
                        out.skills.push(p);
                    }
                }
                _ => self.warn(format!(
                    "{at}: skill {id:?} of {label} was not found; it is skipped"
                )),
            }
        }
    }

    /// Delivers one startup block's files and collects its actions.
    fn deliver(&mut self, what: &str, dir: &Path, s: &RawStartup, out: &mut Delivery) {
        for f in s.files() {
            self.deliver_file(what, dir, f, out);
        }
        for a in s.actions() {
            let kind = text(&a.kind).unwrap_or_default();
            let Some(value) = text(&a.value) else { continue };
            if kind != "send_text" && kind != "slash_command" {
                continue;
            }
            let (fresh, restore) = applies(&a.applies_on);
            if fresh {
                out.actions.push(value.to_string());
            }
            if restore {
                out.restore_actions.push(value.to_string());
            }
        }
    }

    fn deliver_file(&mut self, what: &str, dir: &Path, f: &RawFile, out: &mut Delivery) {
        let Some(rel) = text(&f.path).filter(|p| files::unsafe_reason(p).is_none()) else {
            return;
        };
        let path = files::normalize(&dir.join(rel));
        let content = files::read(&path);
        if content.is_none() && !files::exists(&path) {
            let msg = format!("{what}: file {rel} not found at {}", path.display());
            if f.required.unwrap_or(true) {
                self.error(msg);
            } else {
                self.warn(msg);
            }
            return;
        }
        let hint = match text(&f.delivery_hint).unwrap_or("auto") {
            "auto" => auto_hint(&path, content.as_deref()),
            other => other,
        };
        let (fresh, restore) = applies(&f.applies_on);
        match (hint, content) {
            ("guidance_merge", Some(text)) => out.guidance.push(Doc {
                title: format!("{what}: {rel}"),
                text,
            }),
            ("send_text", Some(text)) => {
                if fresh {
                    out.send_text.push(text.clone());
                }
                if restore {
                    out.restore_send_text.push(text);
                }
            }
            ("skill_install", _) => {
                let skill = if path.file_name().is_some_and(|n| n == "SKILL.md") {
                    path.parent().map(Path::to_path_buf).unwrap_or(path)
                } else {
                    path
                };
                if files::is_real(&skill) && !out.skills.contains(&skill) {
                    out.skills.push(skill);
                }
            }
            (_, None) => self.warn(format!(
                "{what}: {rel} is a directory; only skill_install takes directories"
            )),
            _ => {}
        }
    }
}

/// `applies_on` as (fresh start, restore); both when absent.
fn applies(v: &Option<Vec<Text>>) -> (bool, bool) {
    match v {
        None => (true, true),
        Some(_) => {
            let items = raw::list(v);
            (items.contains(&"fresh_start"), items.contains(&"restore"))
        }
    }
}

/// OpenRig's `auto` hint: a skill file is installed, Markdown is merged into
/// guidance, anything else is typed in.
fn auto_hint(path: &Path, content: Option<&str>) -> &'static str {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name == "skill.md"
        || content.is_some_and(|c| c.trim_start().starts_with("# SKILL"))
        || content.is_none()
    {
        "skill_install"
    } else if name.ends_with(".md") || name.ends_with(".markdown") {
        "guidance_merge"
    } else {
        "send_text"
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Skill,
    Guidance,
}

fn resources(raw: &RawAgent, kind: Kind) -> &[raw::RawResource] {
    let r = raw.resources.as_ref();
    let list = match kind {
        Kind::Skill => r.and_then(|r| r.skills.as_deref()),
        Kind::Guidance => r.and_then(|r| r.guidance.as_deref()),
    };
    list.unwrap_or_default()
}

fn lookup(raw: &RawAgent, dir: &Path, kind: Kind, id: &str) -> Option<PathBuf> {
    let r = resources(raw, kind).iter().find(|r| text(&r.id) == Some(id))?;
    let path = text(&r.path).filter(|p| files::unsafe_reason(p).is_none())?;
    Some(files::normalize(&dir.join(path)))
}

/// A `uses` reference: `ns:id` in the import named `ns`, or `id` in the
/// agent's own resources and then its imports.
fn find_resource(a: &Agent, kind: Kind, id: &str) -> Option<PathBuf> {
    if let Some((ns, rid)) = id.split_once(':') {
        let (_, dir, raw) = a.imports.iter().find(|(name, _, _)| name == ns)?;
        return lookup(raw, dir, kind, rid);
    }
    lookup(&a.raw, &a.dir, kind, id).or_else(|| {
        a.imports
            .iter()
            .find_map(|(_, dir, raw)| lookup(raw, dir, kind, id))
    })
}
