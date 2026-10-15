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
pub mod link;
pub mod palette;
pub mod popup;
pub mod store;

pub use command::{Command, Context};
pub use demo::Demo;
pub use link::{Backend, Link, Msg, Sink};
pub use palette::Palette;
pub use store::{Change, Entry, Row, Store, Tally};
