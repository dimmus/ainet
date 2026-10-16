//! How agents read as text, the same in every front end: how long ago,
//! where, and the one-line summary under an agent in the Fleet.

use std::path::Path;

use ainet_core::{Agent, now_ms};

/// How long ago `since_ms` was: `42s`, `5m`, `2h 3m`, `4d`.
pub fn age(since_ms: i64) -> String {
    age_at(since_ms, now_ms())
}

/// [`age`] as of `now_ms`.
pub fn age_at(since_ms: i64, now_ms: i64) -> String {
    let s = ((now_ms - since_ms) / 1000).max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h {}m", s / 3600, s / 60 % 60),
        _ => format!("{}d", s / 86_400),
    }
}

/// A path with the home directory written `~`.
pub fn short_path(p: &Path) -> String {
    short_path_in(p, std::env::var("HOME").ok().as_deref())
}

fn short_path_in(p: &Path, home: Option<&str>) -> String {
    let s = p.display().to_string();
    match home {
        Some(home) if !home.is_empty() && s.starts_with(home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

/// The line under an agent in the Fleet: how long in its state, runtime,
/// model and how full its context is.
pub fn meta(a: &Agent) -> String {
    let mut parts = vec![age(a.state_since), a.runtime.label().to_string()];
    if let Some(m) = &a.model {
        parts.push(m.clone());
    }
    if let Some(ctx) = a.context {
        parts.push(format!("ctx {:.0}%", ctx * 100.0));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_in_the_largest_sensible_unit() {
        let now = 10_000_000_000;
        let ago = |s: i64| age_at(now - s * 1000, now);
        assert_eq!(ago(0), "0s");
        assert_eq!(ago(59), "59s");
        assert_eq!(ago(61), "1m");
        assert_eq!(ago(3 * 3600 + 5 * 60), "3h 5m");
        assert_eq!(ago(2 * 86_400), "2d");
        assert_eq!(age_at(now + 5000, now), "0s", "the future is now");
    }

    #[test]
    fn home_is_written_as_a_tilde() {
        let p = Path::new("/home/me/code/app");
        assert_eq!(short_path_in(p, Some("/home/me")), "~/code/app");
        assert_eq!(short_path_in(p, Some("/home/you")), "/home/me/code/app");
        assert_eq!(short_path_in(p, Some("")), "/home/me/code/app");
        assert_eq!(short_path_in(p, None), "/home/me/code/app");
    }
}
