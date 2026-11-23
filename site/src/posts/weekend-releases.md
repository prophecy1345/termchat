---
title: "Drafts, QR safety numbers and your own pager: termchat.app 0.19 to 0.25"
description: "A weekend of termchat.app releases: everyday chat features in every app, and for servers approvals, command reports, monitoring webhooks and on-call paging, end to end encrypted."
date: 2026-11-22
lang: en
---

Between Friday and Sunday termchat.app went from 0.19 to 0.25. Half of it is for everyone who chats; the other half is for people who run servers and want those servers to talk to them without a bot platform in between.

## For everyone

- **Safety numbers by QR code (0.23).** The desktop app and Android show a QR code next to the 60 digits, the terminal draws one, and Android scans the code on the other screen. Only an exact match marks the contact verified. [Safety numbers](/docs/safety/)
- **Drafts (0.23).** Half-written text stays with its chat when you switch chats or lock, encrypted like everything else.
- **Mute, pin, archive (0.23).** Mute a chat or group for an hour, a day or always; pin chats to the top; archive them under their own heading.
- **Forward and export (0.23).** Forward a message or a document to another chat or group, with no mark of where it came from. Export a chat to a text file, only on an explicit command and with a warning that the file is not encrypted.
- **Faster start without Tor (0.24).** Without Tor every chat connects right after you unlock instead of one by one over half a minute: the relay sees one IP address for all of them anyway. With Tor the slow start stays, because there it hides which chats are yours.
- **Android share and desktop shortcuts (0.25).** Share text or a file from another Android app into a chat. On the desktop, Cmd+K (Ctrl+K) jumps to a chat by name, F1 lists the keys, and the unread count shows on the Dock icon or in the window title.
- The apps now draw their own fine pixel icons instead of emoji and system glyphs, and there is a new theme, thoroughbred.

## For servers and scripts

Each of these runs from the command line, best with `termchat daemon` keeping the server's identity open. The server gets an identity of its own, and you verify it once, like any contact. [Scripts guide](/docs/scripts/)

- **Ask before a risky step (0.19).** `termchat ask alice "Deploy build 42?"` waits for Alice to answer `yes CODE` from her phone. Only her verified main device counts.
- **Groups from scripts (0.20).** List private groups, send to one and read replies by name.
- **Report how a command ended (0.21).** `termchat run --to alice --failures-only -- backup.sh` sends one message with the exit status, the time it took and the last lines of output.
- **A message that disappears (0.22).** `termchat send alice --expires 5m` for a secret that should not stay in either chat.
- **Alerts from your monitoring (0.25).** `termchat hook` takes webhooks from Alertmanager, Grafana, Uptime Kuma or Gatus on localhost, with a token, and passes each alert on as an encrypted message to a verified contact or a group. No Telegram bot, no Slack.
- **On-call paging (0.25).** `termchat page` keeps reminding Alice until she answers `ack CODE`, passes the page to Bob after a while, and tells everyone who acknowledged it or that it resolved. A webhook route can page too: a firing alert opens a page, the resolved one closes it.

An acknowledgement only stops the reminders: nothing a contact sends can run a command on the server. What an alert says lands in the chat history, so keep secrets out of it. [How paging works](https://github.com/prophecy1345/termchat/blob/main/docs/adr/040-paging.md)
