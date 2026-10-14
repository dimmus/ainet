//! Where ainet keeps its socket and state. XDG first, then `~/.local/state`.

use std::path::PathBuf;

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

/// The daemon's socket: `$AINET_SOCKET`, else `$XDG_RUNTIME_DIR/ainet.sock`,
/// else inside the state directory.
pub fn socket() -> PathBuf {
    if let Some(p) = std::env::var_os("AINET_SOCKET") {
        return PathBuf::from(p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("ainet.sock"),
        None => state_dir().join("ainet.sock"),
    }
}

pub fn database() -> PathBuf {
    state_dir().join("ainet.sqlite")
}

/// `$XDG_CONFIG_HOME/ainet/config.toml`.
pub fn config_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("ainet")
        .join("config.toml")
}

/// The Omarchy theme file; ainet follows it like Den does.
pub fn omarchy_colors() -> PathBuf {
    home().join(".local/state/omarchy/current/theme/colors.toml")
}
