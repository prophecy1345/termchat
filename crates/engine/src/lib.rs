//! Everything a termchat client does besides drawing: the relay transport, the
//! app data and its file, password rules (ADR-011). The TUI and the Android
//! app are shells over this crate.

pub mod data;
pub mod engine;
pub mod notify;
pub mod password;
mod relay;
pub mod settings;
pub mod themes;

pub use engine::{Engine, Happened, RelayEvent, Setup, Unlocked, clean};
pub use relay::*;

/// Idle time before a client locks itself.
pub const AUTOLOCK: std::time::Duration = std::time::Duration::from_secs(5 * 60);
