# ADR-024: scriptable mode — messages from the shell and from servers

Status: **accepted** (maintainer, 2026-11-16). Русская версия: [024-scriptable-mode.ru.md](024-scriptable-mode.ru.md).

## Context

termchat.app is made of interactive apps today: the TUI, the desktop app and Android. But the terminal has another side: scripts, cron, CI and servers. Typical jobs:

- "the backup failed", from a server to a phone;
- "the build is done", after a long command;
- reading a person's answer in a script ("yes, restart it").

People use Telegram bots, ntfy or email for this. Nearly all of them leave the text readable by a third party, and a bot needs an account with its owner's phone number. End-to-end encryption from a script, without infrastructure of your own, barely exists.

termchat.app has everything for it except a command-line way in: an engine without an interface, anonymous queues (ADR-023), Tor, and separate identities through `TERMCHAT_HOME`.

## Decision

### 1. Subcommands of the same binary

`termchat` with no arguments still starts the TUI. With a subcommand it runs without an interface and exits:

```
termchat init --nickname NAME [--relay URL] [--onion URL] [--tor]
termchat invite                       # a one-time invite on stdout
termchat add INVITE                   # add a contact from an invite
termchat contacts                     # name, fingerprint, state; one line per contact
termchat send CONTACT [TEXT]          # TEXT, or stdin when TEXT is not given
termchat send-file CONTACT PATH
termchat recv [--from CONTACT] [--wait SECS] [--follow] [--json] [--save-dir DIR]
```

- The state folder is as today: `TERMCHAT_HOME` or `~/.termchat`. A server gets an identity of its own, in its own folder.
- `CONTACT` is the name the app shows (nickname or local name) or the start of the fingerprint. If more than one contact matches, it is an error and nothing is sent.
- A send always goes to one contact. There is no "send to all contacts" (rule 15: no broadcasts).
- There is no command to accept a contact request. A link is made like this: the server's identity adds the owner's invite (`termchat add`), and the owner accepts the request on the phone. So a server identity can never let a stranger in by itself.

### 2. The password without an interface

The state file is sealed with a password (ADR-003), and a script has nobody to type it. Sources, in order of preference:

- `--password-file PATH`: the file must belong to the current user and have mode `0600`, or it is an error. systemd's `LoadCredential=` does not fit: checked on 2026-11-17, its copy belongs to root with mode `0440`; a service passes the file of its own user instead.
- `--password-stdin`: the first line of stdin. The text for `send` then comes from the argument.
- Typed at the terminal without echo, when stdin is a terminal and no other source is given.

There is no environment variable for the password: every child process inherits the environment, and any process of the same user can read it.

The duress password works here too (ADR-003): if it comes from the file, the data is erased silently and the command exits as with a wrong password.

### 3. Delivery and exit codes

`send` connects, sends and waits for the relay's `Accepted` (up to 60 seconds by default, `--timeout SECS`).

| Code | Meaning |
|---|---|
| 0 | the message is on the relay |
| 1 | wrong password, no state, input error |
| 2 | bad arguments |
| 3 | no such contact, or more than one matches |
| 4 | the relay did not take the message in time; it stays queued and goes with the next run of any command |
| 5 | cannot send: the contact left, is blocked, or needs a new invite (ADR-023) |

Over Tor the first connection takes 10–30 seconds. That is fine for occasional notices. There is no daemon mode that keeps the connection open (decision 1 below).

### 4. What `recv` prints

- By default it prints the unread messages, one record each, and marks them read. `--wait SECS` waits for at least one; `--follow` does not exit and prints new ones as they come.
- Text from the contact is cleaned before printing, as in the TUI: no control or bidi characters (`clean_text`). Otherwise a contact could send an escape sequence into someone's terminal or log.
- `--json`: one object per line (`{"from": ..., "fingerprint": ..., "at": ..., "text": ...}`), the text escaped by JSON. For parsing in scripts.
- Files (ADR-017) are saved only with an explicit `--save-dir`, under a safe name (`safe_name`), never over an existing file, and never opened. Without `--save-dir` only a line "file NAME, SIZE arrived" is printed.
- Contact requests are not printed: their texts stay hidden until accepted, as in the apps (ADR-007).

### 5. Alongside an open app

One process holds the state file (`data::lock`). If the TUI or another command already has this identity open, the command exits at once with code 1 and a message instead of waiting. So scripts get an identity of their own, not the main one.

### 6. What changes in the code

- `client`: parsing subcommands and a mode without the TUI. Everything else is the existing `Engine` (`setup`, `unlock`, `invite`, `add`, `send`, `send_file`, `on_relay`, `tick`).
- `engine`: finding a contact by name or fingerprint, and "unread since" for `recv`. The protocol, the relay and `core` do not change.
- Docs: a README section with examples (cron, systemd, the end of a long command), and a line in the threat model.

## Threat model

- **An identity on a server is as strong as the server.** The password sits next to the state file, so whoever gets root on the server reads all of that identity's conversations and can write as it. This is said plainly: a server gets an identity of its own that talks only to its owner, never the main one.
- **The owner's contacts are not exposed.** The server identity is a separate key with separate queues (ADR-023); it is no way into the owner's main identity.
- **The relay sees what it sees for people:** a queue, timing, size. That a script writes, not a person, may show in the timing (cron at exactly 03:00).
- **Output is data from the contact.** Cleaning the text and never opening files protect the terminal, and the script that reads the output.

## Not a new cryptographic protocol (rule 1)

The same Olm sessions, the same queues and frames. Only the way the client starts changes.

## Rejected

- **The password in an environment variable:** see section 2.
- **An HTTP API or webhook on the device:** an open port is one more attack surface and a way in for strangers. A script runs `termchat` directly.
- **Sending to all contacts or a group:** that is a broadcast (rule 15).
- **Accepting requests from the command line:** a server identity must not let strangers in; a person makes the link from their own device.
- **A separate binary for scripts:** one more release artifact; subcommands are enough.

## Maintainer's decisions (2026-11-16)

1. **No daemon mode.** No long-lived process with the password in memory; every command connects on its own. Come back to it if people ask. (Revisited: [ADR-029](029-daemon.md) adds one, 2026-11-18.)
2. **Typing the password is supported** in the first version: at the terminal without echo, when stdin is a terminal and no other source is given.
3. **The duress password from a file erases the data**, silently, as in the apps (ADR-003).
4. **A script contact is not marked** in the desktop app or on Android: it is an ordinary contact with the nickname it chose.
