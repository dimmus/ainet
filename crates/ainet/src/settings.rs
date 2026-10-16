//! settings.json as the console reads it: once per run, and only by the
//! commands that need it (never the hook or the MCP server, which must
//! print nothing of their own). A file that does not read is reported on
//! stderr and the defaults are used.

use std::path::PathBuf;
use std::sync::OnceLock;

use ainet_core::paths;
use ainet_core::settings::Settings;

/// The settings, read on first use.
pub fn get() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| match ainet_core::settings::load(&paths::settings_file()) {
        Ok(loaded) => loaded.map(|l| l.settings).unwrap_or_default(),
        Err(e) => {
            eprintln!("ainet: {e}; using the default settings");
            Settings::default()
        }
    })
}

/// Where a new agent or one-seat rig runs when no directory is given:
/// `rigs.default_cwd`, else here.
pub fn default_cwd(settings: &Settings) -> std::io::Result<PathBuf> {
    match &settings.rigs.default_cwd {
        Some(p) => Ok(paths::expand(p)),
        None => std::env::current_dir(),
    }
}

/// Whether `ainet down` snapshots first: never with `--no-snapshot`,
/// else as `rigs.snapshot_on_down` says (yes by default).
pub fn snapshot_on_down(settings: &Settings, no_snapshot: bool) -> bool {
    !no_snapshot && settings.rigs.snapshot_on_down
}

/// The daemon's log filter: `$AINET_LOG`, else `daemon.log.level`.
pub fn log_filter(settings: &Settings, env: Option<String>) -> String {
    env.filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| settings.daemon.log.level.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_settings_is_what_the_console_always_did() {
        let s = Settings::default();
        assert!(snapshot_on_down(&s, false));
        assert!(!snapshot_on_down(&s, true));
        assert_eq!(default_cwd(&s).unwrap(), std::env::current_dir().unwrap());
        assert_eq!(log_filter(&s, None), "info");
        assert_eq!(log_filter(&s, Some("debug".into())), "debug");
    }

    #[test]
    fn settings_change_the_defaults_and_flags_still_win() {
        let mut s = Settings::default();
        s.rigs.snapshot_on_down = false;
        s.rigs.default_cwd = Some("/srv/work".into());
        s.daemon.log.level = "warn".into();
        assert!(!snapshot_on_down(&s, false));
        assert_eq!(default_cwd(&s).unwrap(), PathBuf::from("/srv/work"));
        assert_eq!(log_filter(&s, None), "warn");
        assert_eq!(log_filter(&s, Some("trace".into())), "trace", "AINET_LOG wins");
        assert_eq!(log_filter(&s, Some(" ".into())), "warn");
    }
}
