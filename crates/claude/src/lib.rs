//! Claude Code, fully: what ainet needs to know about a running Claude Code,
//! including the undocumented behaviours Den relies on.
//!
//! Most of this crate is a port of Den (<https://github.com/GlacierHubAB/den>,
//! MIT, Copyright (c) 2026 Martin Granström). Den validated these behaviours
//! against Claude Code 2.1.289 (sessions files from 2.1.289+, the inbox socket
//! from 2.1.224+). Every module says which Den package it comes from.
//!
//! - [`sessions`]: live sessions from `~/.claude/sessions/<pid>.json`, with
//!   liveness and process identity checks through `/proc`.
//! - [`peer`]: messages into a running session through its inbox socket.
//! - [`hooks`]: the hook payloads, the decisions a `PermissionRequest` hook
//!   prints, and installing the hooks into `settings.json`.
//! - [`always`]: Den's `/always` rules.
//! - [`question`]: `AskUserQuestion` calls and their answers.
//! - [`transcript`]: reading session transcripts from the end.
//! - [`cost`]: tokens to an estimate in dollars.
//! - [`badge`]: the agent-badge mod's plugin store.
//! - [`launch`]: `claude` command lines.
//! - [`closer`]: closing a session safely.
//! - [`sanitize`]: making text from outside safe to print.
//!
//! Functions that read the environment (`CLAUDE_CONFIG_DIR`, `HOME`) have
//! variants that take the directory, for tests and for the daemon.

pub mod always;
pub mod badge;
pub mod closer;
pub mod cost;
pub mod hooks;
pub mod launch;
pub mod peer;
pub mod question;
pub mod sanitize;
pub mod sessions;
pub mod transcript;

mod util;

pub use util::{config_dir, config_dir_from, home_dir};
