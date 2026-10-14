//! Reading spec files from disk or from the library built into the binary,
//! and the path rules a RigSpec follows.
//!
//! The built-in library lives under a virtual root, [`BUILTIN`]. Paths below
//! it are looked up in the embedded file map; everything else is the real
//! filesystem. Spec paths are normalised lexically (`.` and `..` folded) so
//! both kinds join and compare the same way.

use std::path::{Component, Path, PathBuf};

use crate::library;

/// The virtual directory the embedded library appears under.
pub const BUILTIN: &str = "/ainet-builtin";

pub fn builtin_root() -> PathBuf {
    PathBuf::from(BUILTIN)
}

/// Whether a path is on the real filesystem rather than in the library.
pub fn is_real(path: &Path) -> bool {
    !path.starts_with(BUILTIN)
}

fn embedded_key(path: &Path) -> Option<String> {
    let rel = path.strip_prefix(BUILTIN).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// A text file's contents, or `None` if it is missing, a directory or unreadable.
pub fn read(path: &Path) -> Option<String> {
    match embedded_key(path) {
        Some(key) => library::embedded(&key).map(str::to_string),
        None => std::fs::read_to_string(path).ok(),
    }
}

/// Whether a file or directory exists.
pub fn exists(path: &Path) -> bool {
    match embedded_key(path) {
        Some(key) => library::embedded_exists(&key),
        None => path.exists(),
    }
}

/// Folds `.` and `..` without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// An absolute, normalised path; relative paths are taken from the current directory.
pub fn absolute(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    normalize(&abs)
}

/// OpenRig's "safe relative path": not empty, not absolute, no `..`.
pub fn unsafe_reason(path: &str) -> Option<&'static str> {
    let p = Path::new(path);
    if path.trim().is_empty() {
        Some("is empty")
    } else if p.is_absolute() || path.starts_with('~') {
        Some("must be relative to the spec's directory")
    } else if p.components().any(|c| c == Component::ParentDir) {
        Some("must not contain `..`")
    } else {
        None
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Expands a leading `~` or `~/` and `$VAR` / `${VAR}`. Unset variables stay
/// as written.
pub fn expand(raw: &str) -> String {
    let tilde = match (raw, home()) {
        ("~", Some(h)) => h.to_string_lossy().into_owned(),
        (r, Some(h)) if r.starts_with("~/") => format!("{}{}", h.to_string_lossy(), &r[1..]),
        (r, _) => r.to_string(),
    };
    expand_vars(&tilde)
}

fn expand_vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, consumed) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after
                .char_indices()
                .find(|&(i, c)| !(c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())))
                .map_or(after.len(), |(i, _)| i);
            (&after[..end], end)
        };
        match std::env::var(name).ok().filter(|_| !name.is_empty()) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[at..at + 1 + consumed]),
        }
        rest = &after[consumed..];
    }
    out.push_str(rest);
    out
}

/// A member's working directory, by the RigSpec rules: relative to the rig
/// root; used as written if that exists, else with `~` and `$VAR` expanded.
pub fn resolve_cwd(raw: &str, root: &Path) -> PathBuf {
    let raw = if raw.trim().is_empty() { "." } else { raw.trim() };
    let direct = root.join(raw);
    if direct.exists() {
        return absolute(&direct);
    }
    absolute(&root.join(expand(raw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_lexically() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(normalize(Path::new("a/../..")), PathBuf::from(".."));
    }

    #[test]
    fn safe_paths() {
        assert!(unsafe_reason("guidance/x.md").is_none());
        assert!(unsafe_reason("../x.md").is_some());
        assert!(unsafe_reason("/etc/passwd").is_some());
        assert!(unsafe_reason("a/../../b").is_some());
    }

    #[test]
    fn expands_variables() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(expand("$HOME/x"), format!("{home}/x"));
        assert_eq!(expand("${HOME}/x"), format!("{home}/x"));
        assert_eq!(expand("~/x"), format!("{home}/x"));
        assert_eq!(expand("$AINET_SURELY_UNSET_VAR/x"), "$AINET_SURELY_UNSET_VAR/x");
        assert_eq!(expand("a$"), "a$");
        assert_eq!(expand("${unterminated"), "${unterminated");
    }
}
