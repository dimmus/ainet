//! A running rig written back out as an OpenRig 0.2 RigSpec.
//!
//! Seats are grouped into pods in the order the pods first appear; edges
//! within a pod are written unqualified under it, the rest at the top level
//! as `pod.member`. Working directories are written relative to the rig root
//! where they can be. A seat whose agent is one of the built-in library's
//! keeps it as `local:agents/<name>` (which ainet resolves from the library
//! when there is no such directory); other agents are omitted, so the
//! export carries topology and configuration but not their guidance.

use std::path::Path;

use ainet_core::{Edge, Runtime};
use serde::Serialize;

use crate::{ResolvedRig, ResolvedSeat, library};

#[derive(Serialize)]
struct OutRig<'a> {
    version: &'static str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    culture_file: Option<&'static str>,
    pods: Vec<OutPod<'a>>,
    edges: Vec<OutEdge>,
}

#[derive(Serialize)]
struct OutPod<'a> {
    id: &'a str,
    label: &'a str,
    members: Vec<OutMember<'a>>,
    edges: Vec<OutEdge>,
}

#[derive(Serialize)]
struct OutMember<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_ref: Option<String>,
    profile: &'static str,
    runtime: &'static str,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restore_policy: Option<&'a str>,
}

#[derive(Serialize)]
struct OutEdge {
    kind: &'static str,
    from: String,
    to: String,
}

fn member<'a>(seat: &'a ResolvedSeat, root: &Path) -> OutMember<'a> {
    let terminal = seat.runtime == Runtime::Shell;
    let agent_ref = if terminal {
        Some("builtin:terminal".to_string())
    } else {
        seat.agent_name
            .as_deref()
            .filter(|n| has_builtin_role(seat, n))
            .map(|n| format!("local:agents/{n}"))
    };
    let cwd = match seat.cwd.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => seat.cwd.to_string_lossy().into_owned(),
    };
    OutMember {
        id: &seat.member,
        agent_ref,
        profile: if terminal { "none" } else { "default" },
        runtime: seat.runtime.spec_name(),
        cwd,
        label: seat.label.as_deref(),
        model: seat.model.as_deref(),
        effort: seat.effort.as_deref(),
        role: seat.role.as_deref().filter(|_| !terminal),
        restore_policy: Some(seat.restore_policy.as_str()).filter(|p| *p != "resume_if_possible"),
    }
}

/// Whether the seat's agent is the built-in one of that name, not another
/// spec's agent that happens to share it.
fn has_builtin_role(seat: &ResolvedSeat, agent: &str) -> bool {
    library::builtin_role(agent).is_some_and(|role| seat.guidance.iter().any(|d| d.text == role))
}

/// The pod of a logical id, and the member within it.
fn split(logical: &str) -> (&str, &str) {
    logical.split_once('.').unwrap_or(("", logical))
}

fn out_edge(e: &Edge, local: bool) -> OutEdge {
    let name = |id: &str| {
        if local {
            split(id).1.to_string()
        } else {
            id.to_string()
        }
    };
    OutEdge {
        kind: e.kind.label(),
        from: name(&e.from),
        to: name(&e.to),
    }
}

/// The rig as RigSpec YAML.
pub fn to_yaml(rig: &ResolvedRig) -> String {
    let mut pods: Vec<OutPod> = Vec::new();
    for seat in &rig.seats {
        let m = member(seat, &rig.root);
        match pods.iter_mut().find(|p| p.id == seat.pod) {
            Some(pod) => pod.members.push(m),
            None => pods.push(OutPod {
                id: &seat.pod,
                label: &seat.pod,
                members: vec![m],
                edges: Vec::new(),
            }),
        }
    }
    let mut edges = Vec::new();
    for e in &rig.edges {
        let (from_pod, to_pod) = (split(&e.from).0, split(&e.to).0);
        match pods
            .iter_mut()
            .find(|p| p.id == from_pod)
            .filter(|_| from_pod == to_pod)
        {
            Some(pod) => pod.edges.push(out_edge(e, true)),
            None => edges.push(out_edge(e, false)),
        }
    }
    let culture = library::builtin_culture();
    let culture_file = rig
        .seats
        .iter()
        .any(|s| s.guidance.iter().any(|d| d.text == culture))
        .then_some("CULTURE.md");
    let out = OutRig {
        version: "0.2",
        name: &rig.name,
        summary: rig.summary.as_deref(),
        culture_file,
        pods,
        edges,
    };
    match serde_saphyr::to_string(&out) {
        Ok(yaml) => format!("# Exported by ainet: an OpenRig 0.2 RigSpec.\n{yaml}"),
        Err(e) => format!("# ainet could not export rig {}: {e}\n", rig.name),
    }
}
