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

/// Keys must not end up in a core file after a crash.
pub fn no_core_dumps() {
    #[cfg(unix)]
    {
        use rustix::process::{Resource, Rlimit, setrlimit};
        let _ = setrlimit(
            Resource::Core,
            Rlimit {
                current: Some(0),
                maximum: Some(0),
            },
        );
    }
}
