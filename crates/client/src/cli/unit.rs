//! `termchat systemd-unit`: a template unit that tells a contact or a group
//! when a systemd unit fails (PLAN D3.5). It prints text only: it opens no
//! identity, reads no password and writes no file.

use std::path::Path;

use super::group::Selector;
use super::{Fail, fail};

/// The unit for `to` (a contact) or `group`, run by `user` with state in
/// `home`, the password read from `password_file` if given, else through a
/// running daemon (ADR-029).
pub(super) fn unit(
    to: Option<&str>,
    group: Option<&Selector>,
    exe: &Path,
    home: &Path,
    user: Option<&str>,
    password_file: Option<&Path>,
) -> Result<String, Fail> {
    let (who, target) = match (to, group) {
        (Some(to), None) => (to.to_string(), quoted(to)?),
        (None, Some(Selector::Name(name))) => (
            format!("the group {name}"),
            format!("--group {}", quoted(name)?),
        ),
        (None, Some(Selector::Id(id))) => {
            let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
            (format!("group {hex}"), format!("--group-id {hex}"))
        }
        _ => return fail(2, "systemd-unit takes one contact or one --group"),
    };
    let exe = path(exe)?;
    let home = path(home)?;
    let password = match password_file {
        Some(p) => format!(" --password-file {}", path(p)?),
        None => String::new(),
    };
    let user = match user {
        Some(u)
            if !u.is_empty()
                && u.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) =>
        {
            format!("User={u}\n")
        }
        _ => "# User=the account that owns TERMCHAT_HOME\n".into(),
    };
    let daemon = if password_file.is_none() {
        "# No --password-file: this works while `termchat daemon` runs for\n# this TERMCHAT_HOME (ADR-029).\n"
    } else {
        ""
    };
    Ok(format!(
        "# termchat-failed@.service: tells {who} over termchat.app when a unit fails.
# Save as /etc/systemd/system/termchat-failed@.service, run `systemctl
# daemon-reload`, then add to the [Unit] section of any unit to watch:
#   OnFailure=termchat-failed@%n.service
{daemon}[Unit]
Description=Tell {who} over termchat.app that %I failed

[Service]
Type=oneshot
{user}Environment=TERMCHAT_HOME={home}
ExecStart={exe} send {target} \"%I failed on %H\"{password}
"
    ))
}

/// Text systemd reads as one argument as is: no quote, backslash, `%`
/// (a specifier), `$` (a variable), control or line break.
fn quoted(s: &str) -> Result<String, Fail> {
    let bad = |c: char| matches!(c, '"' | '\\' | '%' | '$' | '\'') || c.is_control();
    if s.trim().is_empty() || s.chars().any(bad) {
        return fail(
            2,
            "that name cannot go into a systemd unit; rename the contact or group",
        );
    }
    Ok(format!("\"{s}\""))
}

/// An absolute path systemd takes unquoted.
fn path(p: &Path) -> Result<String, Fail> {
    let s = p.to_str().unwrap_or_default();
    let ok = p.is_absolute()
        && !s.is_empty()
        && !s.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '"' | '\\' | '%' | '$' | '\'' | ';')
        });
    if !ok {
        return fail(
            2,
            format!(
                "{} cannot go into a systemd unit: use an absolute path without spaces",
                p.display()
            ),
        );
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(to: Option<&str>, group: Option<&Selector>, pw: Option<&str>) -> Result<String, Fail> {
        unit(
            to,
            group,
            Path::new("/usr/local/bin/termchat"),
            Path::new("/var/lib/termchat-bot"),
            Some("termchat-bot"),
            pw.map(Path::new),
        )
    }

    #[test]
    fn a_unit_for_a_contact_and_a_group() {
        let text = make(Some("alice"), None, Some("/etc/termchat-bot/pw"))
            .ok()
            .unwrap_or_default();
        assert!(text.contains(
            "ExecStart=/usr/local/bin/termchat send \"alice\" \"%I failed on %H\" --password-file /etc/termchat-bot/pw"
        ));
        assert!(text.contains("User=termchat-bot\n"));
        assert!(text.contains("Environment=TERMCHAT_HOME=/var/lib/termchat-bot\n"));
        assert!(text.contains("OnFailure=termchat-failed@%n.service"));
        let ops = Selector::Name("ops team".into());
        let text = make(None, Some(&ops), None).ok().unwrap_or_default();
        assert!(text.contains("send --group \"ops team\" \"%I failed on %H\"\n"));
        assert!(
            text.contains("termchat daemon"),
            "without a password file it says it needs the daemon"
        );
    }

    #[test]
    fn nothing_that_would_break_out_of_the_unit() {
        for name in ["a\"b", "a\\b", "50%", "$HOME", "x\ny", " ", "it's"] {
            assert!(make(Some(name), None, None).is_err(), "{name:?}");
        }
        assert!(make(Some("alice"), None, Some("relative/pw")).is_err());
        assert!(make(Some("alice"), None, Some("/etc/my pw")).is_err());
        assert!(make(Some("alice"), Some(&Selector::Name("g".into())), None).is_err());
        assert!(make(None, None, None).is_err());
        let odd_user = unit(
            Some("alice"),
            None,
            Path::new("/usr/local/bin/termchat"),
            Path::new("/var/lib/t"),
            Some("root\nExecStart=/bin/sh"),
            None,
        )
        .ok()
        .unwrap_or_default();
        assert!(!odd_user.contains("ExecStart=/bin/sh"));
    }
}
