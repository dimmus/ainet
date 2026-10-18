//! `/always` rules: an answer that allows a permission request and the
//! same again (`ainet_claude::always`). An agent's rules last for its
//! session, or are kept in the database across sessions and restarts with
//! `permissions.persist_always`; global rules from settings.json
//! (`permissions.global_rules`) cover every agent. Paths you add to
//! `permissions.sensitive_paths` are never covered, whatever the rule.

use ainet_claude::always::Rule;
use ainet_core::protocol::AlwaysRule;
use ainet_core::settings::GlobalRule;
use ainet_core::{Response, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Daemon, Shared, err, lock};

/// Where kept rules live in the store, one row per agent.
const NS: &str = "always";

/// One agent's rule and when it was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Kept {
    pub rule: Rule,
    pub created: i64,
}

/// A global rule from settings.json as the rule it stands for.
fn global(rule: &GlobalRule) -> Option<Rule> {
    // The two share their JSON shape.
    serde_json::to_value(rule)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
}

impl Daemon {
    /// The rule that covers a request from agent `id`, if one does: its
    /// own first, then a global one.
    pub(crate) fn always_covering(&self, id: &str, tool: &str, input: &Value, cwd: &str) -> Option<Rule> {
        let p = &self.settings().permissions;
        let extra = &p.sensitive_paths;
        let own = self.always.get(id).into_iter().flatten().map(|k| k.rule.clone());
        let globals = p.global_rules.iter().filter_map(global);
        own.chain(globals).find(|r| r.matches_in(tool, input, cwd, extra))
    }

    /// Makes the rule an `/always` answer asks for, or says why there is
    /// none.
    pub(crate) fn always_add(&mut self, agent: &str, tool: &str, input: &Value, cwd: &str) -> String {
        let extra = self.settings().permissions.sensitive_paths.clone();
        match ainet_claude::always::derive_in(tool, input, cwd, &extra) {
            Ok(rule) => {
                let kept = self.settings().permissions.persist_always;
                let note = format!(
                    "always allowing {} {}",
                    rule.scope(),
                    if kept { "from now on" } else { "for this session" }
                );
                let rules = self.always.entry(agent.to_string()).or_default();
                if !rules.iter().any(|k| k.rule == rule) {
                    rules.push(Kept {
                        rule,
                        created: now_ms(),
                    });
                }
                self.always_save(agent);
                note
            }
            Err(refusal) => format!("allowed once; no always rule: {refusal:?}"),
        }
    }

    /// A session ended: its rules go, unless they are kept.
    pub(crate) fn always_session_end(&mut self, agent: &str) {
        if !self.settings().permissions.persist_always {
            self.always.remove(agent);
        }
    }

    /// Writes an agent's rules to the store when they are kept.
    fn always_save(&self, agent: &str) {
        if !self.settings().permissions.persist_always {
            return;
        }
        match self.always.get(agent).filter(|r| !r.is_empty()) {
            Some(rules) => self.store.put(NS, agent, rules),
            None => self.store.delete(NS, agent),
        }
    }

    /// At start: the kept rules, when they are kept; otherwise any left
    /// from before are forgotten, so turning keeping back on later does
    /// not bring back rules you no longer see.
    pub(crate) fn always_load(&mut self) {
        if self.settings().permissions.persist_always {
            for (agent, rules) in self.store.scan::<Vec<Kept>>(NS, "") {
                self.always.insert(agent, rules);
            }
        } else {
            self.always_forget_kept();
        }
    }

    /// Settings changed: keeping turned on saves the rules of sessions
    /// running now; turned off, the database forgets them, and so does
    /// memory for sessions that ended (kept only because keeping was on:
    /// a seat resumed later would still be allowed). Sessions running
    /// now keep theirs until they end.
    pub(crate) fn always_settings_changed(&mut self) {
        if self.settings().permissions.persist_always {
            let agents: Vec<String> = self.always.keys().cloned().collect();
            for a in agents {
                self.always_save(&a);
            }
        } else {
            self.always_forget_kept();
            let agents = &self.agents;
            self.always
                .retain(|id, _| agents.get(id).is_some_and(|a| a.state.is_live()));
        }
    }

    fn always_forget_kept(&self) {
        for (agent, _) in self.store.scan::<Value>(NS, "") {
            self.store.delete(NS, &agent);
        }
    }
}

/// The rules in force: one agent's, or everyone's and the global ones.
pub(crate) fn list(d: &Shared, agent: Option<String>) -> Response {
    let g = lock(d);
    let kept = g.settings().permissions.persist_always;
    let one = match agent.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        Some(key) => match g
            .find_agent(key, None)
            .or_else(|| g.always.contains_key(key).then(|| key.to_string()))
        {
            Some(id) => Some(id),
            None => return err(format!("no agent {key}")),
        },
        None => None,
    };
    let mut rules: Vec<AlwaysRule> = g
        .always
        .iter()
        .filter(|(a, _)| one.as_ref().is_none_or(|o| o == *a))
        .flat_map(|(a, rules)| {
            rules.iter().map(move |k| AlwaysRule {
                agent: Some(a.clone()),
                key: k.rule.key(),
                scope: k.rule.scope(),
                created: k.created,
                kept,
            })
        })
        .collect();
    rules.sort_by_key(|r| r.created);
    rules.extend(
        g.settings()
            .permissions
            .global_rules
            .iter()
            .filter_map(global)
            .map(|r| AlwaysRule {
                agent: None,
                key: r.key(),
                scope: r.scope(),
                created: 0,
                kept: true,
            }),
    );
    Response::AlwaysRules { rules }
}

/// Revokes one of an agent's rules by key, or all of them.
pub(crate) fn revoke(d: &Shared, agent: &str, rule: Option<String>) -> Response {
    let mut g = lock(d);
    let key = agent.trim();
    let id = g
        .find_agent(key, None)
        .filter(|id| g.always.contains_key(id))
        .or_else(|| g.always.contains_key(key).then(|| key.to_string()));
    let gone = match (&id, &rule) {
        (None, _) => 0,
        (Some(id), None) => g.always.remove(id).map_or(0, |r| r.len()),
        (Some(id), Some(wanted)) => {
            let rules = g.always.entry(id.clone()).or_default();
            let before = rules.len();
            rules.retain(|k| k.rule.key() != *wanted);
            let gone = before - rules.len();
            if rules.is_empty() {
                g.always.remove(id);
            }
            gone
        }
    };
    let Some(id) = id.filter(|_| gone > 0) else {
        let is_global = rule.as_ref().is_some_and(|wanted| {
            g.settings()
                .permissions
                .global_rules
                .iter()
                .filter_map(global)
                .any(|r| r.key() == *wanted)
        });
        return err(match rule {
            _ if is_global => {
                "that is a global rule: remove it from permissions.global_rules in settings.json".to_string()
            }
            Some(r) => format!("{key} has no /always rule {r}"),
            None => format!("{key} has no /always rules"),
        });
    };
    g.always_save(&id);
    Response::Text {
        text: format!("revoked {gone} /always rule{}", if gone == 1 { "" } else { "s" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_rules_are_always_rules() {
        let globals: Vec<GlobalRule> = serde_json::from_str(
            r#"[{"command": "cargo test"}, {"tool": "WebSearch"}, {"fetch_host": "docs.rs"},
                {"edits_under": {"tool": "Edit", "dir": "/w"}}]"#,
        )
        .unwrap();
        let rules: Vec<Rule> = globals.iter().filter_map(global).collect();
        assert_eq!(
            rules,
            vec![
                Rule::Command("cargo test".into()),
                Rule::Tool("WebSearch".into()),
                Rule::FetchHost("docs.rs".into()),
                Rule::EditsUnder {
                    tool: "Edit".into(),
                    dir: "/w".into()
                },
            ]
        );
    }
}
