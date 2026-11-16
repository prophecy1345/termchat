---
title: "Encrypted messages from your servers, without a bot account: termchat.app 0.8 for scripts and cron"
description: "termchat.app 0.8 runs without its screen: termchat send and recv let a cron job, a CI run or the end of a long command write to your phone, end to end encrypted, with no Telegram bot and no third party reading it (ADR-024)."
date: 2026-11-16
lang: en
---

termchat.app started as a messenger for the terminal. But the terminal has another side: scripts, cron jobs, CI and servers that need to tell you something. Version 0.8 lets them.

## The problem

"The backup failed." "The build is done." "Disk at 95 %." Most people send these through a Telegram bot, ntfy or email. That works, but:

- the text sits readable on someone else's server;
- a Telegram bot needs an account tied to a phone number;
- answering back from your phone takes more plumbing still.

End-to-end encryption from a shell script, without running infrastructure of your own, barely exists.

## What 0.8 adds

`termchat` with no arguments still opens the chat screen. With a command, it does one thing and exits:

```sh
termchat send alice "backup failed on $(hostname)"
make release; termchat send alice "release build: exit $?"
df -h / | termchat send alice
termchat recv --from alice --wait 600 --json
```

A message from a script travels exactly like one from a person: Olm encryption, an anonymous queue on the relay, Tor if you turn it on. The relay cannot tell the two apart.

## Setting up a server

Give the server an identity of its own:

```sh
export TERMCHAT_HOME=/var/lib/backup-bot
install -m 600 /dev/null "$TERMCHAT_HOME.pw" && head -c 30 /dev/urandom | base64 > "$TERMCHAT_HOME.pw"
termchat init --nickname backup-box --password-file "$TERMCHAT_HOME.pw"
termchat add 'termchat:...' --password-file "$TERMCHAT_HOME.pw"   # your invite
```

Accept its request in your app, and from then on any script on that server can write to you. With systemd, `LoadCredential=` keeps the password out of the unit file. Our own relay server now sends its owner a morning report and a message when a site goes down, this way.

## Decisions worth explaining

- **The password never comes from the environment.** Only from a file that must be yours and `chmod 600`, from stdin, or typed at a terminal. Every child process inherits the environment.
- **One contact per send.** There is no "send to everyone": that would be a broadcast, and termchat.app has none by design.
- **A server cannot accept requests.** You make the link from your own device, so a server identity can never let a stranger in.
- **Output is cleaned.** Control characters from the other side never reach your terminal or log, and files are saved only into a folder you name, never opened.
- **No background daemon.** Each command connects and exits; there is no long-running process holding the password. Over Tor that costs 10 to 30 seconds per command, which is fine for alerts.
- **Exit codes a script can use:** 0 on the relay, 3 contact not found or ambiguous, 4 not taken in time (it goes with the next command), 5 cannot write to that contact.

## Be honest about the server

A server identity is exactly as strong as the server. Its password sits next to its state, so whoever gets root there reads that identity's messages and can write as it. That is why it should always be a separate identity that talks only to you, never your main one: it is a different key with different queues, so it is no way into your real contacts.

The full design is in ADR-024, and the commands are in the README under "Scripts and servers".
