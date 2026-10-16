//! Sound and system notifications for new messages (ADR-008). Nothing from
//! the message, not even the sender's name, reaches another program.

use std::io::Write;
use std::process::{Command, Stdio};

/// Plays the system's notification sound in the background; the terminal
/// bell if no player is found. The commands are fixed: no outside input.
pub fn sound() {
    let players: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("afplay", &["/System/Library/Sounds/Glass.aiff"])]
    } else if cfg!(windows) {
        &[(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[System.Media.SystemSounds]::Asterisk.Play(); Start-Sleep -Milliseconds 800",
            ],
        )]
    } else {
        &[
            ("canberra-gtk-play", &["-i", "message-new-instant"]),
            (
                "paplay",
                &["/usr/share/sounds/freedesktop/stereo/message-new-instant.oga"],
            ),
        ]
    };
    for (program, args) in players {
        let child = Command::new(program)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            // Reaped in the background so it never lingers as a zombie.
            std::thread::spawn(move || child.wait());
            return;
        }
    }
    emit("\x07");
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
