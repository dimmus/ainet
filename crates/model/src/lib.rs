//! What a front end knows about the fleet, without drawing any of it.
//!
//! The TUI (ratatui) and the desktop GUI (GPUI) both keep a [`Store`] fed
//! by a [`Link`]: a snapshot and then events from ainetd, or the pretend
//! fleet of [`Demo`]. On top of it sit the things both front ends must agree
//! on: how the Fleet tree is ordered, how the Comms stream interleaves
//! messages and chat, how `/commands` and `@Name` lines turn into requests,
//! and how daemon replies read as text.

pub mod command;
pub mod daemon;
pub mod demo;
pub mod describe;
pub mod journal;
pub mod link;
pub mod map;
pub mod palette;
pub mod popup;
pub mod sample;
pub mod store;
pub mod terminal;

pub use command::{AgentCommand, Command, Context, QueueCommand};
pub use demo::Demo;
pub use link::{Backend, Link, Listing, Msg, Sink};
pub use palette::Palette;
pub use store::{Change, Entry, RigCounts, Row, Store, Tally};
pub use terminal::Pump;
