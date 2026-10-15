//! The command line: `ainet-gui [--demo] [--socket PATH] [--view VIEW]`.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "ainet-gui",
    version,
    about = "The ainet desktop app: a control room for teams of coding agents"
)]
pub struct Args {
    /// Show a pretend fleet instead of connecting to ainetd.
    #[arg(long)]
    pub demo: bool,
    /// ainetd's socket (default: $AINET_SOCKET, else $XDG_RUNTIME_DIR/ainet.sock).
    #[arg(long, value_name = "PATH")]
    pub socket: Option<PathBuf>,
    /// The view to open first: fleet, terminals, rig:NAME, pod:RIG/POD,
    /// comms, inbox, map, review, or a rig's name.
    #[arg(long, value_name = "VIEW", value_parser = View::parse)]
    pub view: Option<View>,
}

impl Args {
    /// The socket to use: the one given, else the console's default.
    pub fn socket(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(ainet_core::paths::socket)
    }
}

/// What the center of the window shows first. The names are the TUI's
/// (`AINET_VIEW`, `ainet terminal open`), so both front ends open the same
/// places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Fleet,
    /// Terminals, of one rig or of all of them.
    Terminals {
        rig: Option<String>,
    },
    Comms,
    Inbox,
    Map,
    Review,
    /// A rig by name; it shows that rig's terminals if such a rig exists.
    Named(String),
}

impl View {
    pub fn parse(s: &str) -> Result<View, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty view".into());
        }
        let (kind, arg) = s.split_once(':').unwrap_or((s, ""));
        let rig = |a: &str| Some(a.to_string()).filter(|a| !a.is_empty());
        Ok(match kind {
            "fleet" => View::Fleet,
            "terminals" | "terminal" | "grid" => View::Terminals { rig: None },
            "rig" | "saved" => View::Terminals { rig: rig(arg) },
            "pod" => View::Terminals {
                rig: arg.split('/').next().and_then(rig),
            },
            "comms" | "chat" | "queue" => View::Comms,
            "inbox" | "attention" | "feed" => View::Inbox,
            "map" | "graph" => View::Map,
            "review" => View::Review,
            _ => View::Named(s.to_string()),
        })
    }

    /// A short title for the view.
    pub fn title(&self) -> String {
        match self {
            View::Fleet => "Fleet".into(),
            View::Terminals { rig: None } => "Terminals".into(),
            View::Terminals { rig: Some(r) } | View::Named(r) => format!("Terminals · {r}"),
            View::Comms => "Comms".into(),
            View::Inbox => "Inbox".into(),
            View::Map => "Map".into(),
            View::Review => "Review".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Args {
        Args::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn clap_definition_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn bare_follows_the_daemon() {
        let args = parse(&["ainet-gui"]);
        assert!(!args.demo);
        assert!(args.socket.is_none() && args.view.is_none());
    }

    #[test]
    fn flags() {
        let args = parse(&[
            "ainet-gui",
            "--demo",
            "--socket",
            "/tmp/a.sock",
            "--view",
            "rig:docs",
        ]);
        assert!(args.demo);
        assert_eq!(args.socket(), PathBuf::from("/tmp/a.sock"));
        assert_eq!(
            args.view,
            Some(View::Terminals {
                rig: Some("docs".into())
            })
        );
    }

    #[test]
    fn views_use_the_tui_names() {
        let v = |s| View::parse(s).unwrap();
        assert_eq!(v("terminals"), View::Terminals { rig: None });
        assert_eq!(v("rig:"), View::Terminals { rig: None });
        assert_eq!(
            v("pod:docs/dev"),
            View::Terminals {
                rig: Some("docs".into())
            }
        );
        assert_eq!(v("chat"), View::Comms);
        assert_eq!(v("attention"), View::Inbox);
        assert_eq!(v("graph"), View::Map);
        assert_eq!(v("docs"), View::Named("docs".into()));
        assert!(View::parse("  ").is_err());
        assert!(Args::try_parse_from(["ainet-gui", "--view", ""]).is_err());
    }

    #[test]
    fn unknown_flags_are_errors() {
        assert!(Args::try_parse_from(["ainet-gui", "--nope"]).is_err());
    }
}
