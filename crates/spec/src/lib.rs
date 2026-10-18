//! OpenRig RigSpecs for ainet: so that the specs people already wrote for
//! OpenRig run here unchanged where they can.
//!
//! A RigSpec (`rig.yaml`, version 0.2) describes a team: pods of members,
//! each member pointing at an AgentSpec (`agent.yaml`) for its role, plus
//! edges between members. This crate parses both, checks them by OpenRig's
//! rules, and resolves them into a flat launch plan: one [`ResolvedSeat`]
//! per member, in launch order, carrying the guidance for its system prompt
//! and the text to type once it is ready. It also ships a small library of
//! built-in rigs, lists the user's own, and writes a running rig back out as
//! a RigSpec.
//!
//! Fields ainet does not act on (services, permission policies, plugins…)
//! are accepted and reported as warnings; runtimes ainet cannot run (`pi`,
//! `omp`, `stub`) are errors.

mod export;
mod files;
mod library;
mod order;
mod raw;
mod resolve;

use std::fmt;
use std::path::{Path, PathBuf};

use ainet_core::protocol::SeatSpec;
use anyhow::{Context, bail};

pub use export::to_yaml;
pub use library::{library, library_from, load_named, load_named_from, spec_yaml, spec_yaml_from};
pub use order::launch_order;

/// A piece of guidance for an agent's system prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub title: String,
    pub text: String,
}

/// One member of a rig, ready to launch.
#[derive(Clone, Debug)]
pub struct ResolvedSeat {
    pub pod: String,
    pub member: String,
    /// `pod.member`.
    pub logical: String,
    /// `pod-member@rig`: OpenRig's canonical session name.
    pub address: String,
    pub label: Option<String>,
    pub role: Option<String>,
    pub runtime: ainet_core::Runtime,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Absolute: relative to the rig root, with `~` and `$VAR` expanded.
    pub cwd: PathBuf,
    /// `resume_if_possible` (the default), `relaunch_fresh` or `checkpoint_only`.
    pub restore_policy: String,
    /// The AgentSpec's `name`, if the member has one.
    pub agent_name: Option<String>,
    /// For the system prompt, in layering order: culture file, rig startup,
    /// pod startup, the agent's guidance resources and startup files, the
    /// member's startup.
    pub guidance: Vec<Doc>,
    /// Typed into the terminal once it is ready, on a fresh start: startup
    /// files delivered as text, then `send_text` and `slash_command` actions.
    pub send_text: Vec<String>,
    /// The same, for a restore.
    pub restore_send_text: Vec<String>,
    /// Skill directories (or files) to install for the seat.
    pub skills: Vec<PathBuf>,
    /// OpenRig's `permission_policy` (the member's, else the rig's): read
    /// and reported; ainetd applies it only when settings.json says to.
    pub permission_policy: Option<String>,
    /// Claude Code's permission mode, from a seat added outside a spec.
    pub permission_mode: Option<ainet_core::settings::PermissionMode>,
    /// More arguments for the agent CLI, from a seat added outside a spec.
    pub args: Vec<String>,
    /// More environment variables, from a seat added outside a spec.
    pub env: std::collections::BTreeMap<String, String>,
}

/// A rig resolved from a spec.
#[derive(Clone, Debug)]
pub struct ResolvedRig {
    pub name: String,
    pub summary: Option<String>,
    /// Where member working directories resolve.
    pub root: PathBuf,
    pub spec_path: Option<PathBuf>,
    /// In launch order.
    pub seats: Vec<ResolvedSeat>,
    /// Every edge, by logical id (`pod.member`).
    pub edges: Vec<ainet_core::Edge>,
    /// What ainet accepted but will not act on, and optional files it could not find.
    pub warnings: Vec<String>,
}

/// What [`validate`] found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for e in &self.errors {
            writeln!(f, "error: {e}")?;
        }
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }
        let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        match (self.errors.len(), self.warnings.len()) {
            (0, 0) => write!(f, "valid"),
            (0, w) => write!(f, "valid, {}", plural(w, "warning")),
            (e, _) => write!(f, "invalid: {}", plural(e, "error")),
        }
    }
}

/// The spec file for a path: the file itself, or `rig.yaml` in a directory.
fn spec_file(path: &Path) -> PathBuf {
    if path.is_dir() {
        let yml = path.join("rig.yml");
        if !path.join("rig.yaml").exists() && yml.exists() {
            yml
        } else {
            path.join("rig.yaml")
        }
    } else {
        path.to_path_buf()
    }
}

/// Reads a spec file and returns it with its absolute path and directory.
fn read_spec(path: &Path) -> anyhow::Result<(String, PathBuf, PathBuf)> {
    let file = files::absolute(&spec_file(path));
    let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let dir = file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    Ok((text, file, dir))
}

/// Turns a resolver outcome into a rig, or one error listing every problem.
fn finish(out: resolve::Outcome, what: &str) -> anyhow::Result<ResolvedRig> {
    if !out.errors.is_empty() {
        bail!("{what}:\n  {}", out.errors.join("\n  "));
    }
    out.rig.with_context(|| format!("{what}: nothing to launch"))
}

/// Loads a RigSpec: a `rig.yaml`, or a directory containing one.
pub fn load(path: &Path) -> anyhow::Result<ResolvedRig> {
    let (text, file, dir) = read_spec(path)?;
    parse(&text, &dir, Some(&file))
}

/// Parses RigSpec YAML whose relative paths resolve against `root`.
pub fn parse(yaml: &str, root: &Path, spec_path: Option<&Path>) -> anyhow::Result<ResolvedRig> {
    let root = files::absolute(root);
    let what = spec_path.map_or_else(|| "rig spec".to_string(), |p| p.display().to_string());
    finish(
        resolve::resolve(
            yaml,
            resolve::Origin {
                root: &root,
                dir: &root,
                spec_path,
            },
        ),
        &what,
    )
}

/// Checks a spec without launching anything. Takes a path, or the name of a
/// library spec when no such path exists.
pub fn validate(path: &Path) -> Report {
    validate_from(&[ainet_core::paths::specs_dir()], path)
}

/// [`validate`] with the user's specs in `dirs`.
pub fn validate_from(dirs: &[PathBuf], path: &Path) -> Report {
    if !path.exists()
        && let Some(name) = path.to_str().filter(|n| !n.contains('/'))
        && let Some(report) = library::validate_named(dirs, name)
    {
        return report;
    }
    match read_spec(path) {
        Ok((text, file, dir)) => {
            let out = resolve::resolve(
                &text,
                resolve::Origin {
                    root: &dir,
                    dir: &dir,
                    spec_path: Some(&file),
                },
            );
            Report {
                errors: out.errors,
                warnings: out.warnings,
            }
        }
        Err(e) => Report {
            errors: vec![format!("{e:#}")],
            warnings: Vec::new(),
        },
    }
}

/// One seat outside a spec, for growing a rig or creating a one-seat rig.
/// Its `agent_ref` resolves against `root`, like a member's.
pub fn seat_from_spec(rig: &str, root: &Path, spec: &SeatSpec) -> anyhow::Result<ResolvedSeat> {
    let root = files::absolute(root);
    let member = resolve::MemberIn {
        id: spec.member.clone(),
        agent_ref: spec.agent_ref.clone(),
        runtime: spec.runtime.map(|r| r.spec_name().to_string()),
        cwd: spec.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
        label: spec.label.clone(),
        model: spec.model.clone(),
        role: spec.role.clone(),
        guidance: spec.guidance.clone(),
        first_message: spec.first_message.clone(),
        effort: spec.effort.map(|e| e.as_str().to_string()),
        permission_mode: spec.permission_mode,
        args: spec.args.clone(),
        env: spec.env.clone(),
        ..resolve::MemberIn::default()
    };
    let what = format!("seat {}.{}", spec.pod, spec.member);
    let rig = finish(resolve::seat(rig, &root, member, &spec.pod), &what)?;
    rig.seats
        .into_iter()
        .next()
        .with_context(|| format!("{what}: not resolved"))
}
