//! Terminal client (TUI): a shell over `termchat-engine` (ADR-011).

pub mod alert;
pub mod app;
pub mod cli;
pub mod form;
pub mod input;
pub mod notify;
pub mod theme;
pub mod ui;

pub use termchat_engine::{data, password, *};
