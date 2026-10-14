//! The identity the agent-badge mod gives each Claude Code session, and the
//! usage figures it records, read from Claude Code's plugin store.
//!
//! Ported from Den's `internal/badge` and its bundled `mods/agent-badge`
//! (`hooks/register.tsx`), both MIT.
//!
//! Where and what (agent-badge 1.1+): Claude Code keeps plugin stores as
//! `<config dir>/plugins/store/agent-badge_*.json`, one JSON object per file.
//! agent-badge writes:
//!
//! - `badge:<session id>`: `{"role": "coding agent", "task": "fix the login
//!   redirect", "history": ["earlier task", …]}`, labelled by a small model
//!   from the user's prompts, peers' briefs and the agent's own reports;
//! - `usage:<session id>`: `{"context": <% of window>, "filled": <tokens>,
//!   "window": <tokens>, "fiveHour": <% used>, "fiveHourResets": "<ISO
//!   time>", "sevenDay": <% used>, "sevenDayResets": "<ISO time>", "usd":
//!   <session cost>, "now": <ms, rounded to the minute>}`, the figures Claude
//!   Code reports to the mod itself (better than a transcript's guess).
//!
//! The critter name ("Mossy Otter") is not stored: it is generated from the
//! session id, bit for bit as agent-badge does ([`name`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::util;

const ADJECTIVES: [&str; 20] = [
    "Mossy", "Fizzy", "Sleepy", "Brave", "Tiny", "Cosmic", "Fuzzy", "Zippy", "Wobbly", "Sunny", "Misty",
    "Plucky", "Snug", "Bouncy", "Quiet", "Spicy", "Velvet", "Pebble", "Comet", "Maple",
];

const CRITTERS: [&str; 20] = [
    "Otter", "Moth", "Newt", "Fox", "Owl", "Axolotl", "Panda", "Gecko", "Quokka", "Hedgehog", "Puffin",
    "Wombat", "Ferret", "Koala", "Lemur", "Capybara", "Badger", "Narwhal", "Sprite", "Tanuki",
];

/// The critter name agent-badge shows for a session id.
pub fn name(session_id: &str) -> String {
    let mut r = Mulberry32::new(fnv1a(session_id) ^ 0x9e37_79b9);
    let a = ADJECTIVES[(r.next() * ADJECTIVES.len() as f64).floor() as usize % ADJECTIVES.len()];
    let c = CRITTERS[(r.next() * CRITTERS.len() as f64).floor() as usize % CRITTERS.len()];
    format!("{a} {c}")
}

/// 32-bit FNV-1a over UTF-16 code units, as agent-badge hashes.
fn fnv1a(text: &str) -> u32 {
    let mut acc: u32 = 0x811c_9dc5;
    for u in text.encode_utf16() {
        acc ^= u as u32;
        acc = acc.wrapping_mul(0x0100_0193);
    }
    acc
}

/// The mulberry32 variant from agent-badge, bit for bit.
struct Mulberry32(u32);

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Mulberry32(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

/// What agent-badge labelled a session with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Labels {
    pub role: String,
    pub task: String,
    pub history: Vec<String>,
}

/// What agent-badge records about a session's context, rate limits and
/// spend. Absent figures are `None`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Context used, percent.
    pub context: Option<f64>,
    /// Context used, tokens.
    pub filled: Option<f64>,
    /// The context window, tokens.
    pub window: Option<f64>,
    /// Five-hour limit used, percent.
    #[serde(rename = "fiveHour")]
    pub five_hour: Option<f64>,
    /// When the five-hour limit resets (ISO 8601).
    #[serde(rename = "fiveHourResets")]
    pub five_hour_resets: Option<String>,
    /// Weekly limit used, percent.
    #[serde(rename = "sevenDay")]
    pub seven_day: Option<f64>,
    #[serde(rename = "sevenDayResets")]
    pub seven_day_resets: Option<String>,
    /// The session's cost as Claude Code reports it.
    pub usd: Option<f64>,
    /// When recorded, Unix ms rounded down to the minute.
    pub now: Option<i64>,
}

impl Usage {
    /// The five-hour reset as Unix ms.
    pub fn five_hour_resets_ms(&self) -> Option<i64> {
        self.five_hour_resets.as_deref().and_then(util::parse_rfc3339_ms)
    }

    /// The weekly reset as Unix ms.
    pub fn seven_day_resets_ms(&self) -> Option<i64> {
        self.seven_day_resets.as_deref().and_then(util::parse_rfc3339_ms)
    }

    /// Context tokens and window, when recorded: `filled`, else `context`%
    /// of `window`. Den prefers these over the transcript's guess.
    pub fn context_tokens(&self) -> Option<(i64, i64)> {
        let window = self.window.filter(|w| *w > 0.0)?;
        let tokens = match (self.filled, self.context) {
            (Some(f), _) => f,
            (None, Some(c)) => c * window / 100.0,
            _ => return None,
        };
        Some((tokens as i64, window as i64))
    }
}

/// Everything agent-badge stored for one session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Badge {
    pub labels: Option<Labels>,
    pub usage: Option<Usage>,
}

/// Where Claude Code keeps plugin stores: `<config dir>/plugins/store`.
pub fn store_dir() -> PathBuf {
    util::config_dir().join("plugins").join("store")
}

/// Every agent-badge entry by session id. A missing store is empty: the mod
/// may not be installed.
pub fn load() -> HashMap<String, Badge> {
    load_from(&store_dir())
}

/// [`load`] from an explicit store directory.
pub fn load_from(dir: &Path) -> HashMap<String, Badge> {
    let mut out: HashMap<String, Badge> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("agent-badge_") && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    for f in files {
        let Ok(raw) = std::fs::read(&f) else { continue };
        let Ok(store) = serde_json::from_slice::<HashMap<String, Value>>(&raw) else {
            continue;
        };
        for (key, v) in store {
            if let Some(id) = key.strip_prefix("badge:") {
                if let Ok(l) = serde_json::from_value::<Labels>(v) {
                    out.entry(id.to_string()).or_default().labels = Some(l);
                }
            } else if let Some(id) = key.strip_prefix("usage:")
                && let Ok(u) = serde_json::from_value::<Usage>(v)
            {
                out.entry(id.to_string()).or_default().usage = Some(u);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn name_matches_agent_badge() {
        // Values produced by agent-badge's own critterName() for these ids.
        for (id, want) in [
            ("11111111-2222-4333-8444-555555555501", "Wobbly Hedgehog"),
            ("11111111-2222-4333-8444-555555555502", "Spicy Tanuki"),
            ("11111111-2222-4333-8444-555555555503", "Plucky Axolotl"),
            ("11111111-2222-4333-8444-555555555504", "Fizzy Tanuki"),
        ] {
            assert_eq!(name(id), want, "{id}");
        }
    }

    #[test]
    fn loads_the_store() {
        let tmp = TempDir::new();
        std::fs::write(
            tmp.join("agent-badge_abc.json"),
            r#"{"badge:s1":{"role":"coding agent","task":"fix the login redirect","history":["set up ci"]},
                "usage:s1":{"context":42.5,"filled":85000,"window":200000,"fiveHour":12,"fiveHourResets":"2026-10-14T18:00:00Z","sevenDay":30,"sevenDayResets":"2026-10-20T00:00:00.000Z","usd":1.25,"now":1791453600000},
                "usage:s2":{"context":10,"window":1000000},
                "other:s3":{},
                "badge:bad":"not an object"}"#,
        )
        .unwrap();
        std::fs::write(tmp.join("other-mod_x.json"), r#"{"badge:zz":{"role":"x"}}"#).unwrap();
        std::fs::write(tmp.join("agent-badge_broken.json"), "{").unwrap();
        let all = load_from(tmp.path());
        assert_eq!(all.len(), 2, "{all:?}");
        let s1 = &all["s1"];
        let l = s1.labels.as_ref().unwrap();
        assert_eq!(
            (l.role.as_str(), l.task.as_str()),
            ("coding agent", "fix the login redirect")
        );
        assert_eq!(l.history, vec!["set up ci"]);
        let u = s1.usage.as_ref().unwrap();
        assert_eq!(u.five_hour, Some(12.0));
        assert_eq!(
            u.five_hour_resets_ms(),
            util::parse_rfc3339_ms("2026-10-14T18:00:00Z")
        );
        assert!(u.seven_day_resets_ms().is_some());
        assert_eq!(u.usd, Some(1.25));
        assert_eq!(u.context_tokens(), Some((85_000, 200_000)));
        assert_eq!(
            all["s2"].usage.as_ref().unwrap().context_tokens(),
            Some((100_000, 1_000_000))
        );
        assert!(all["s2"].labels.is_none());
        assert!(load_from(&tmp.join("missing")).is_empty());
    }
}
