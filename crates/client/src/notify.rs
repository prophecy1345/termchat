//! Sound and system notifications for new messages (ADR-008). Nothing from
//! the message, not even the sender's name, reaches another program.

use std::io::Write;

/// The system's notification sound; the terminal bell if no player is found.
pub fn sound() {
    if !termchat_engine::notify::sound() {
        emit("\x07");
    }
}

/// A desktop notification through the terminal, always the same text.
/// kitty speaks OSC 99; iTerm2, Ghostty, WezTerm and others OSC 9.
pub fn system() {
    let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some();
    match kitty {
        true => emit("\x1b]99;;termchat: new message\x1b\\"),
        false => emit("\x1b]9;termchat: new message\x07"),
    }
}

fn emit(s: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}
