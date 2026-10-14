//! Persistence: an append-only event log, and a small namespaced JSON store
//! for what must outlive the daemon (rigs and their seats, snapshots,
//! messages, chat, queue items, watchdogs). The live state is in memory; it
//! is written through here on every change and read back at start.

use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id     INTEGER PRIMARY KEY,
                ts     INTEGER NOT NULL,
                agent  TEXT,
                kind   TEXT NOT NULL,
                data   TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS events_agent ON events(agent, ts);
            CREATE TABLE IF NOT EXISTS kv (
                ns     TEXT NOT NULL,
                key    TEXT NOT NULL,
                value  TEXT NOT NULL,
                PRIMARY KEY (ns, key)
            );",
        )?;
        Ok(Store { db })
    }

    pub fn log(&self, ts: i64, agent: Option<&str>, kind: &str, data: &serde_json::Value) {
        let res = self.db.execute(
            "INSERT INTO events (ts, agent, kind, data) VALUES (?1, ?2, ?3, ?4)",
            params![ts, agent, kind, data.to_string()],
        );
        if let Err(e) = res {
            tracing::warn!("event log: {e}");
        }
    }

    pub fn count(&self) -> i64 {
        self.db
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Writes `value` under `ns/key`, replacing what was there.
    pub fn put<T: Serialize>(&self, ns: &str, key: &str, value: &T) {
        let json = match serde_json::to_string(value) {
            Ok(j) => j,
            Err(e) => return tracing::warn!("store {ns}/{key}: {e}"),
        };
        let res = self.db.execute(
            "INSERT INTO kv (ns, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(ns, key) DO UPDATE SET value = excluded.value",
            params![ns, key, json],
        );
        if let Err(e) = res {
            tracing::warn!("store {ns}/{key}: {e}");
        }
    }

    pub fn delete(&self, ns: &str, key: &str) {
        if let Err(e) = self
            .db
            .execute("DELETE FROM kv WHERE ns = ?1 AND key = ?2", params![ns, key])
        {
            tracing::warn!("store delete {ns}/{key}: {e}");
        }
    }

    pub fn get<T: DeserializeOwned>(&self, ns: &str, key: &str) -> Option<T> {
        let raw: Option<String> = self
            .db
            .query_row(
                "SELECT value FROM kv WHERE ns = ?1 AND key = ?2",
                params![ns, key],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        raw.and_then(|r| serde_json::from_str(&r).ok())
    }

    /// Every value in `ns` whose key starts with `prefix`, by key. Values that
    /// no longer parse (an older format) are skipped.
    pub fn scan<T: DeserializeOwned>(&self, ns: &str, prefix: &str) -> Vec<(String, T)> {
        let Ok(mut stmt) = self
            .db
            .prepare("SELECT key, value FROM kv WHERE ns = ?1 AND key >= ?2 ORDER BY key")
        else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![ns, prefix], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        let Ok(rows) = rows else { return Vec::new() };
        rows.filter_map(Result::ok)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter_map(|(k, v)| serde_json::from_str(&v).ok().map(|t| (k, t)))
            .collect()
    }

    /// The last `limit` values of `ns`, oldest first. Keys must sort in time
    /// order (zero-padded numbers do).
    pub fn tail<T: DeserializeOwned>(&self, ns: &str, limit: usize) -> Vec<T> {
        let Ok(mut stmt) = self
            .db
            .prepare("SELECT value FROM kv WHERE ns = ?1 ORDER BY key DESC LIMIT ?2")
        else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![ns, limit as i64], |r| r.get::<_, String>(0));
        let Ok(rows) = rows else { return Vec::new() };
        let mut out: Vec<T> = rows
            .filter_map(Result::ok)
            .filter_map(|v| serde_json::from_str(&v).ok())
            .collect();
        out.reverse();
        out
    }
}

/// A key that sorts numerically.
pub fn num_key(n: u64) -> String {
    format!("{n:012}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_events() {
        let s = Store::in_memory().unwrap();
        s.log(1, Some("a"), "Stop", &serde_json::json!({"x": 1}));
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn kv_round_trips_and_scans_by_prefix() {
        let s = Store::in_memory().unwrap();
        s.put("snap", "a/1", &1u32);
        s.put("snap", "a/2", &2u32);
        s.put("snap", "b/1", &3u32);
        s.put("snap", "a/2", &5u32);
        let a: Vec<(String, u32)> = s.scan("snap", "a/");
        assert_eq!(a, vec![("a/1".into(), 1), ("a/2".into(), 5)]);
        assert_eq!(s.get::<u32>("snap", "b/1"), Some(3));
        s.delete("snap", "b/1");
        assert_eq!(s.get::<u32>("snap", "b/1"), None);
        for i in 1..=5u64 {
            s.put("msg", &num_key(i), &i);
        }
        assert_eq!(s.tail::<u64>("msg", 2), vec![4, 5]);
    }
}
