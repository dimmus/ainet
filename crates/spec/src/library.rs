//! The spec library: rigs built into the binary, and the user's own in
//! `$XDG_CONFIG_HOME/ainet/specs/` (`<name>/rig.yaml` or `<name>.yaml`).
//!
//! Built-in rigs, their agents and the shared culture file are embedded and
//! read through a virtual directory (see [`files::BUILTIN`]), so they work
//! with nothing on disk. Their members still run in the directory you launch
//! them from. A user spec with a built-in's name takes its place.

use std::path::{Path, PathBuf};

use ainet_core::protocol::SpecEntry;
use anyhow::{Context, bail};

use crate::files;
use crate::raw::{RawRig, text};
use crate::resolve::{self, Origin};
use crate::{Report, ResolvedRig};

macro_rules! agent {
    ($name:literal) => {
        [
            (
                concat!("agents/", $name, "/agent.yaml"),
                include_str!(concat!("../library/agents/", $name, "/agent.yaml")),
            ),
            (
                concat!("agents/", $name, "/guidance/role.md"),
                include_str!(concat!("../library/agents/", $name, "/guidance/role.md")),
            ),
        ]
    };
}

const CULTURE: &str = include_str!("../library/CULTURE.md");

const AGENTS: &[[(&str, &str); 2]] = &[
    agent!("orchestrator"),
    agent!("advisor"),
    agent!("implementer"),
    agent!("qa"),
    agent!("designer"),
    agent!("reviewer"),
    agent!("review-lead"),
    agent!("research-lead"),
    agent!("analyst"),
    agent!("synthesizer"),
    agent!("product-lead"),
    agent!("researcher"),
    agent!("prototyper"),
    agent!("operator"),
];

const RIGS: &[(&str, &str)] = &[
    ("starter", include_str!("../library/rigs/starter.yaml")),
    ("workshop", include_str!("../library/rigs/workshop.yaml")),
    ("factory", include_str!("../library/rigs/factory.yaml")),
    ("code-review", include_str!("../library/rigs/code-review.yaml")),
    ("research", include_str!("../library/rigs/research.yaml")),
    ("pm", include_str!("../library/rigs/pm.yaml")),
    ("operator", include_str!("../library/rigs/operator.yaml")),
];

fn embedded_files() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once(("CULTURE.md", CULTURE)).chain(AGENTS.iter().flatten().copied())
}

/// An embedded file by its path in the library.
pub(crate) fn embedded(key: &str) -> Option<&'static str> {
    embedded_files().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Whether an embedded file or directory exists.
pub(crate) fn embedded_exists(key: &str) -> bool {
    let key = key.trim_end_matches('/');
    key.is_empty()
        || embedded_files().any(|(k, _)| k == key || k.strip_prefix(key).is_some_and(|r| r.starts_with('/')))
}

fn builtin(name: &str) -> Option<&'static str> {
    RIGS.iter().find(|(n, _)| *n == name).map(|(_, y)| *y)
}

/// `$XDG_CONFIG_HOME/ainet/specs`.
fn user_dir() -> PathBuf {
    let config = ainet_core::paths::config_file();
    config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("specs")
}

/// The user's specs in `dir`, by name, sorted.
fn user_specs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut specs: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            if path.is_dir() {
                let file = path.join("rig.yaml");
                file.is_file()
                    .then_some((path.file_name()?.to_string_lossy().into_owned(), file))
            } else if matches!(path.extension().and_then(|x| x.to_str()), Some("yaml" | "yml")) {
                Some((name, path))
            } else {
                None
            }
        })
        .collect();
    specs.sort();
    specs.dedup_by(|a, b| a.0 == b.0);
    specs
}

fn user_spec(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    user_specs(dir)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p)
}

fn entry(name: &str, yaml: &str, source: String) -> SpecEntry {
    let raw: Option<RawRig> = serde_saphyr::from_str(yaml).ok();
    let summary = raw
        .as_ref()
        .and_then(|r| text(&r.summary))
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "));
    let seats = raw
        .as_ref()
        .map(|r| {
            r.pods
                .iter()
                .flatten()
                .map(|p| p.members.as_ref().map_or(0, Vec::len))
                .sum()
        })
        .unwrap_or(0);
    SpecEntry {
        name: name.to_string(),
        summary,
        seats,
        source,
    }
}

fn library_in(dir: &Path) -> Vec<SpecEntry> {
    let users = user_specs(dir);
    let mut out: Vec<SpecEntry> = RIGS
        .iter()
        .filter(|(name, _)| !users.iter().any(|(n, _)| n == name))
        .map(|(name, yaml)| entry(name, yaml, "builtin".into()))
        .collect();
    for (name, path) in users {
        let yaml = std::fs::read_to_string(&path).unwrap_or_default();
        out.push(entry(&name, &yaml, path.display().to_string()));
    }
    out
}

/// Built-in specs (source `builtin`) and the user's own (source: the file).
pub fn library() -> Vec<SpecEntry> {
    library_in(&user_dir())
}

/// Resolves a built-in: files from the library, working directories from
/// the current directory.
fn resolve_builtin(name: &str, yaml: &str) -> anyhow::Result<resolve::Outcome> {
    let cwd =
        std::env::current_dir().with_context(|| format!("built-in spec {name}: no current directory"))?;
    let root = files::absolute(&cwd);
    let dir = files::builtin_root();
    Ok(resolve::resolve(
        yaml,
        Origin {
            root: &root,
            dir: &dir,
            spec_path: None,
        },
    ))
}

fn load_named_in(dir: &Path, name: &str) -> anyhow::Result<ResolvedRig> {
    if let Some(path) = user_spec(dir, name) {
        return crate::load(&path);
    }
    let Some(yaml) = builtin(name) else {
        let names: Vec<String> = library_in(dir).into_iter().map(|e| e.name).collect();
        bail!(
            "no spec named {name:?} in the library (have: {})",
            names.join(", ")
        );
    };
    crate::finish(resolve_builtin(name, yaml)?, &format!("built-in spec {name}"))
}

/// A library spec by name: the user's `<name>/rig.yaml` or `<name>.yaml`,
/// else a built-in. Built-ins run in the current directory.
pub fn load_named(name: &str) -> anyhow::Result<ResolvedRig> {
    load_named_in(&user_dir(), name)
}

/// The YAML of a library spec, for `specs show`.
pub fn spec_yaml(name: &str) -> anyhow::Result<String> {
    if let Some(path) = user_spec(&user_dir(), name) {
        return std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()));
    }
    builtin(name)
        .map(str::to_string)
        .with_context(|| format!("no spec named {name:?} in the library"))
}

/// Validates a library spec by name, if there is one.
pub(crate) fn validate_named(name: &str) -> Option<Report> {
    if let Some(path) = user_spec(&user_dir(), name) {
        return Some(crate::validate(&path));
    }
    let yaml = builtin(name)?;
    Some(match resolve_builtin(name, yaml) {
        Ok(out) => Report {
            errors: out.errors,
            warnings: out.warnings,
        },
        Err(e) => Report {
            errors: vec![format!("{e:#}")],
            warnings: Vec::new(),
        },
    })
}

/// The role guidance of a built-in agent.
pub(crate) fn builtin_role(name: &str) -> Option<&'static str> {
    embedded(&format!("agents/{name}/guidance/role.md"))
}

/// The culture file the built-in rigs share.
pub(crate) fn builtin_culture() -> &'static str {
    CULTURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_specs_shadow_builtins() {
        let dir = std::env::temp_dir().join(format!("ainet-spec-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mine")).unwrap();
        let yaml = "version: \"0.2\"\nname: mine\nsummary: Mine\npods:\n  - id: dev\n    label: Dev\n    members:\n      - id: solo\n        runtime: claude-code\n        cwd: \".\"\n";
        std::fs::write(dir.join("mine/rig.yaml"), yaml).unwrap();
        std::fs::write(
            dir.join("starter.yaml"),
            yaml.replace("name: mine", "name: starter"),
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "not a spec").unwrap();

        let lib = library_in(&dir);
        let starters: Vec<_> = lib.iter().filter(|e| e.name == "starter").collect();
        assert_eq!(starters.len(), 1);
        assert!(starters[0].source.ends_with("starter.yaml"));
        let mine = lib.iter().find(|e| e.name == "mine").unwrap();
        assert_eq!((mine.seats, mine.summary.as_deref()), (1, Some("Mine")));
        assert!(lib.iter().any(|e| e.name == "factory" && e.source == "builtin"));

        let rig = load_named_in(&dir, "mine").unwrap();
        assert_eq!(rig.seats[0].address, "dev-solo@mine");
        assert_eq!(rig.root, files::absolute(&dir.join("mine")));
        assert_eq!(load_named_in(&dir, "starter").unwrap().seats.len(), 1);
        assert!(load_named_in(&dir, "nope").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn embedded_directories_exist() {
        assert!(embedded_exists("agents/implementer"));
        assert!(embedded_exists("agents"));
        assert!(!embedded_exists("agents/impl"));
        assert!(builtin_role("reviewer").is_some());
    }
}
