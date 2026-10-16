//! Where ainet keeps its socket, state and settings. XDG first, then
//! `~/.local/state` and `~/.config`.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `$XDG_STATE_HOME/ainet`: the database and logs.
pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local").join("state"))
        .join("ainet")
}

/// The daemon's socket: `$AINET_SOCKET`, else `daemon.socket` in
/// settings.json, else `$XDG_RUNTIME_DIR/ainet.sock`, else inside the
/// state directory.
pub fn socket() -> PathBuf {
    if let Some(p) = std::env::var_os("AINET_SOCKET") {
        return PathBuf::from(p);
    }
    // A settings file that does not read is reported where it is used; here
    // it only means the default.
    if let Ok(Some(loaded)) = crate::settings::load(&settings_file())
        && let Some(p) = loaded.settings.daemon.socket
    {
        return expand(&p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("ainet.sock"),
        None => state_dir().join("ainet.sock"),
    }
}

pub fn database() -> PathBuf {
    state_dir().join("ainet.sqlite")
}

/// `$XDG_CONFIG_HOME/ainet`, else `~/.config/ainet`: settings.json, the
/// desktop app's keymap and your own specs. (The `config.toml` once
/// planned here was never read, and is gone.)
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("ainet")
}

/// `$XDG_CONFIG_HOME/ainet/settings.json`: the settings of ainetd, the
/// console and the desktop app ([`crate::settings`]).
pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// `$XDG_CONFIG_HOME/ainet/specs`: your own rig specs.
pub fn specs_dir() -> PathBuf {
    config_dir().join("specs")
}

/// `ainetd.log` in the state directory: where an autostarted daemon
/// writes its log unless settings.json says otherwise.
pub fn daemon_log() -> PathBuf {
    state_dir().join("ainetd.log")
}

/// A path from settings.json with a leading `~` made your home.
pub fn expand(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// The Omarchy theme file; ainet follows it like Den does.
pub fn omarchy_colors() -> PathBuf {
    home().join(".local/state/omarchy/current/theme/colors.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_live_in_the_config_dir() {
        assert!(settings_file().ends_with("ainet/settings.json"));
        assert!(specs_dir().ends_with("ainet/specs"));
        assert!(daemon_log().ends_with("ainet/ainetd.log"));
        assert_eq!(expand(Path::new("~/x")), home().join("x"));
        assert_eq!(expand(Path::new("/a/~")), PathBuf::from("/a/~"));
    }
}
