//! `ainet`: one binary for the TUI, the daemon, the hook, the scripting
//! commands and the MCP server agents talk through. Symlinked as `rig`, it
//! answers to OpenRig's command names.

mod cli;
mod commands;
mod conn;
mod hook;
mod mcp;
mod output;
mod settings;

use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches};

/// The name we were run as (`ainet`, or `rig` through a symlink), so help
/// and errors use it.
fn invoked_as() -> &'static str {
    let name = std::env::args_os()
        .next()
        .and_then(|a| {
            std::path::Path::new(&a)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "ainet".into());
    match name.as_str() {
        "ainet" => "ainet",
        "rig" => "rig",
        _ => Box::leak(name.into_boxed_str()),
    }
}

fn main() -> ExitCode {
    let name = invoked_as();
    let matches = cli::Cli::command().name(name).bin_name(name).get_matches();
    let cli = match cli::Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    match commands::run(cli, ainet_core::paths::socket()) {
        Ok(code) => code,
        Err(e) => match e.downcast_ref::<commands::Exit>() {
            Some(commands::Exit(code, message)) => {
                if !message.is_empty() {
                    eprintln!("{name}: {message}");
                }
                ExitCode::from(*code)
            }
            None => {
                eprintln!("{name}: {e:#}");
                ExitCode::FAILURE
            }
        },
    }
}
