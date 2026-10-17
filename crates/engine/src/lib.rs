//! Everything a termchat client does besides drawing: the relay transport, the
//! app data and its file, password rules (ADR-011). The TUI and the Android
//! app are shells over this crate.

pub mod data;
pub mod engine;
pub mod password;
mod relay;
pub mod themes;

pub use engine::{Engine, Happened, Setup, Unlocked, clean};
pub use relay::*;
