//! The new-message sound (ADR-008). Nothing from the message, not even the
//! sender's name, reaches another program.

use std::process::{Command, Stdio};
use std::time::Duration;

/// At most one sound this often, so a burst of messages is not noise.
pub const SOUND_GAP: Duration = Duration::from_secs(2);

/// Plays the system's notification sound in the background. The commands are
/// fixed: no outside input. False if no player could be started.
pub fn sound() -> bool {
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
            return true;
        }
    }
    false
}
