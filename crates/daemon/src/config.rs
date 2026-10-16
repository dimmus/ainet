//! The settings in force: read from settings.json at start, followed while
//! ainetd runs, and applied live (each use reads them), with environment
//! variables winning over the file. A file that does not read leaves the
//! last good settings in force and is reported in the log.

use std::path::PathBuf;
use std::time::Duration;

use ainet_core::settings::{self, Follower, Loaded, Settings, Source};

use crate::{Daemon, Shared, lock};

/// How often settings.json is checked for changes.
const POLL: Duration = Duration::from_secs(1);

/// The settings in force and where they came from.
pub(crate) struct Config {
    /// File, then environment, over the defaults.
    pub settings: Settings,
    /// The last good file's contents.
    pub loaded: Loaded,
    /// Why the file's current text is not in force.
    pub error: Option<String>,
    /// Environment variables that win over the file: (setting, var, value).
    pub env: Vec<(String, String, String)>,
    pub file: Option<PathBuf>,
}

impl Config {
    pub(crate) fn new(follower: &Follower) -> Config {
        let env = settings::env_overrides();
        Config {
            settings: follower.settings().clone().with_env(&env),
            loaded: follower.loaded.clone(),
            error: follower.error.clone(),
            env,
            file: follower.path().map(PathBuf::from),
        }
    }

    /// One line on what is set and from where, for the log.
    fn describe(&self) -> String {
        let all = settings::effective(&self.settings, &self.loaded.raw, &self.env);
        let set: Vec<String> = all
            .iter()
            .filter_map(|e| match &e.source {
                Source::Default => None,
                Source::File => Some(e.key.clone()),
                Source::Env { var } => Some(format!("{} (from {var})", e.key)),
            })
            .collect();
        let file = self
            .file
            .as_ref()
            .map_or_else(|| "no settings file".to_string(), |f| f.display().to_string());
        if set.is_empty() {
            format!("settings: {file}: all defaults")
        } else {
            format!("settings: {file}: {}", set.join(", "))
        }
    }

    /// Says in the log what is in force and what is wrong.
    pub(crate) fn report(&self) {
        tracing::info!("{}", self.describe());
        if !self.loaded.unknown.is_empty() {
            tracing::warn!(
                "settings: ignored unknown keys {}",
                self.loaded.unknown.join(", ")
            );
        }
        if let Some(e) = &self.error {
            tracing::warn!("settings: {e}; the last good settings stay in force");
        }
    }
}

impl Daemon {
    /// The settings in force.
    pub(crate) fn settings(&self) -> &Settings {
        &self.config.settings
    }
}

/// Follows settings.json until the daemon stops: each change is read on a
/// blocking thread, never under the daemon's lock, then put in force.
pub(crate) async fn follow(d: Shared, mut follower: Follower) {
    loop {
        tokio::time::sleep(POLL).await;
        let polled = tokio::task::spawn_blocking(move || {
            let changed = follower.poll();
            (follower, changed)
        })
        .await;
        let Ok((back, changed)) = polled else { return };
        follower = back;
        if changed {
            let config = Config::new(&follower);
            config.report();
            let mut g = lock(&d);
            g.config = config;
            g.settings_changed();
        }
    }
}
