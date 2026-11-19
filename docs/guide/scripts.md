---
title: "termchat.app for scripts and servers: send, recv, cron, systemd"
description: "Send end-to-end encrypted messages from a shell script, cron job, CI run or server to your phone with termchat send, and read answers with termchat recv, without a bot account."
---

# Scripts and servers

Русская версия: [scripts.ru.md](scripts.ru.md).

`termchat` with no arguments opens the chat screen. With a command it does one thing without a screen and exits, so a script, a cron job or a server can send you an end-to-end encrypted message, or read your answer: "the backup failed" on your phone, without a Telegram bot or a third party reading it.

## Commands

| Command | What it does |
|---|---|
| `init --nickname NAME [--relay URL] [--onion URL] [--tor]` | makes the identity |
| `invite` | prints a one-time invite |
| `add INVITE` | asks a contact; they accept in their app |
| `contacts` | name, fingerprint and state, one contact per line |
| `send CONTACT [TEXT]` | sends TEXT, or stdin when TEXT is left out; waits until the relay has it |
| `send-file CONTACT PATH` | a document of up to 1 MiB |
| `recv [--from C] [--wait S] [--follow] [--json] [--save-dir DIR]` | prints new messages and changes, each once; files are saved only into `--save-dir` |
| `daemon` | keeps the identity open, so the commands above need no password and answer at once (Linux, macOS; see below) |

Every command also takes `--password-file PATH` or `--password-stdin`, and `--timeout SECS` (60 by default). The state folder is `TERMCHAT_HOME`, or `~/.termchat`.

- `CONTACT` is the name the app shows, or the start of the fingerprint. If more than one contact matches, nothing is sent.
- **The password never comes from the environment**: every child process inherits it. Use a file that belongs to you and has mode `600`, stdin, or the keyboard. Without any of them termchat asks at the keyboard, and a script has none: it ends with "no terminal to type the password".
- There is no "send to all": one command, one contact.
- The owner accepts contact requests in the app. The CLI and daemon have no command to accept a request.
- Text from the other side is cleaned of control characters before it reaches your terminal or log. Files are saved only into the folder you name, never opened.

Exit codes: 0 done, 1 password or state, 2 arguments, 3 contact not found or more than one, 4 the relay did not take it in time (it goes with the next command), 5 cannot write to that contact. A command exits only once the relay took everything it sent, within `--timeout`.

## Setting up a server

Steps for Linux with systemd, run as root. Give the server **an identity of its own**, never your main one: whoever gets root on that server can read everything that identity says and hears, and write as it.

```sh
# 1. termchat for every user of the server
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | TERMCHAT_INSTALL_DIR=/usr/local/bin sh

# 2. a system user that owns only this identity, its folder, and a password only it can read
useradd --system --home-dir /var/lib/termchat-bot --shell /usr/sbin/nologin termchat-bot
install -d -o termchat-bot -g termchat-bot -m 700 /var/lib/termchat-bot /etc/termchat-bot
head -c 30 /dev/urandom | base64 | sudo -u termchat-bot sh -c 'umask 077; cat > /etc/termchat-bot/pw'

# a short name for the next steps
bot() { sudo -u termchat-bot env TERMCHAT_HOME=/var/lib/termchat-bot termchat "$@" --password-file /etc/termchat-bot/pw; }

# 3. the identity; the nickname is what you will see in your app
bot init --nickname backup-box

# 4. your invite: Ctrl+O in the terminal, Invite in the desktop app, Your invite on Android
bot add 'termchat:...'
```

5. Accept the request from `backup-box` in your app.
6. See under which name the server knows you, and send a test:

```sh
bot contacts
bot send alice "hello from $(hostname)"
```

## From systemd or cron

A service or a cron job does not see what you set in your shell: give it `TERMCHAT_HOME` and the password file itself.

```ini
# /etc/systemd/system/disk-report.service
[Service]
Type=oneshot
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
ExecStart=/bin/sh -c 'df -h / | /usr/local/bin/termchat send alice --password-file /etc/termchat-bot/pw'
```

Add a `.timer` for a schedule. With cron, `crontab -u termchat-bot -e`:

```
0 9 * * * TERMCHAT_HOME=/var/lib/termchat-bot /usr/local/bin/termchat send alice "good morning" --password-file /etc/termchat-bot/pw
```

systemd's `LoadCredential=` does not fit: the copy it makes belongs to root, and termchat accepts only a password file of its own user.

One identity runs one command at a time: a second one at the same moment ends with code 1 and says the identity is open elsewhere. A daemon lifts that limit.

## A daemon, for a bot that answers

Each command above unlocks the identity and connects to the relay anew: over Tor that is 10–30 seconds a command. `termchat daemon` does it once and stays (ADR-029). While it runs, `send`, `send-file`, `recv`, `contacts`, `invite` and `add` for the same `TERMCHAT_HOME` go through it: no password, an answer within a second, and several at a time. When no daemon runs, they work as before.

```ini
# /etc/systemd/system/termchat-daemon.service
[Unit]
Description=termchat daemon
After=network-online.target
Wants=network-online.target

[Service]
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
ExecStart=/usr/local/bin/termchat daemon --password-file /etc/termchat-bot/pw
Restart=on-failure
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/termchat-bot
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

A bot that answers "status" and "disk" from your phone (`jq` reads the JSON):

```sh
export TERMCHAT_HOME=/var/lib/termchat-bot
termchat recv --from alice --follow --json | while read -r line; do
  case "$(printf '%s' "$line" | jq -r .text)" in
    status) termchat send alice "$(uptime)" ;;
    disk)   termchat send alice "$(df -h / | tail -1)" ;;
  esac
done
```

- The daemon listens on `daemon.sock` in the state folder, mode `600`, and lets in only its own user. Nothing listens on the network.
- **While it runs, the password and the keys are in its memory, and any process of that user can send through it.** Give it a server identity under a user of its own, never your main identity.
- A document you send is read by the command, not the daemon; a `recv --save-dir` folder must exist before you ask.
- It stops on SIGTERM or Ctrl+C: up to 5 s for the relay to take what was just sent, then it saves. On Windows there is no daemon.

## Get a message when a server goes quiet

Set up separate identities on two hosts by following steps 1–3 under “Setting up a server”. This example calls the monitored server `backup-box`, the watcher `watcher`, and the alert recipient `alice`. Use the `bot` helper from that section on each host.

1. Run `bot invite` on `watcher` and copy the entire `termchat:…` line. Run `bot add 'termchat:…'` on `backup-box`, substituting the line you copied.
2. If the daemon is already running, stop it on each host while you set up the contacts. Use `sudo systemctl stop termchat-daemon` if you installed the example unit. Open `termchat` without a subcommand, using the same user and `TERMCHAT_HOME` as the service.

   ```sh
   sudo -u termchat-bot env TERMCHAT_HOME=/var/lib/termchat-bot termchat
   ```

   Enter the service identity's password on each host.

3. In the `watcher` interface, select the request from `backup-box` and press **Ctrl+Y**. After acceptance, each identity appears in the other's contacts. On each host, select that contact and press **Ctrl+F** to open the safety number. Compare the numbers and press **v** on both hosts if they match.
4. On `watcher`, press **Ctrl+A**, paste the owner's invite, and press **Enter**. The owner accepts the request in their app. Compare the safety number with the owner and mark the owner as verified on `watcher`. [Accepting requests and comparing safety numbers](contacts.md)

Close both terminal interfaces and start `termchat daemon` for each identity. Run the watch commands as the same user and with the same `TERMCHAT_HOME` as the service. Use the names shown by `bot contacts` on the respective host.

```sh
# On the host you are watching, using the server identity's TERMCHAT_HOME
termchat watch send watcher --every 300

# On the second host, using the watcher identity's TERMCHAT_HOME
termchat watch monitor backup-box --notify alice --after 900
```

The example sends a heartbeat every five minutes and alerts after 15 minutes of silence. Use seconds for both options and leave room for reconnects. Both commands run in the foreground, so use your service manager to restart them when the daemon stops or the socket closes. The heartbeats stay out of chat history and `recv`. The alert is an ordinary direct message.

Keep the watcher awake and the clocks synchronized. A relay outage can cause an alert or prevent delivery. The daemon logs locally when it queues an alert and when it cannot confirm delivery. A fresh heartbeat lets the watcher report the next outage.

## More examples

```sh
# the end of a long command, on your laptop (again an identity of its own)
make release; termchat send alice "release build: exit $?" --password-file ~/.termchat-bot.pw

# text from a pipe
df -h / | termchat send alice --password-file /etc/termchat-bot/pw

# wait up to 10 minutes for an answer, as JSON lines
termchat recv --from alice --wait 600 --json --password-file /etc/termchat-bot/pw
```

## What recv prints

Plain: `alice: text`, `alice edited: text`, `alice deleted a message`, `alice reacted like to: text`.

With `--json`, one object per line:

```json
{"at":1794913363,"fingerprint":"78125 66938 20371 20023 95579 06120","from":"alice·78125","id":"17037012887881307594","kind":"react","mine":true,"reaction":"love"}
{"at":1794913393,"fingerprint":"78125 66938 20371 20023 95579 06120","from":"alice·78125","id":"5136533931879411279","kind":"message","reply_to":{"id":"5235206626092912747","mine":true},"text":"which disk? / or /var?"}
```

- `kind`: `message`, `edit`, `delete` or `react`.
- `id`: the message, as a string (JSON readers lose digits past 2^53). For `react`, `mine` says whether it is your message; `reaction` is one of `like`, `love`, `laugh`, `wow`, `sad`, `thanks`, `dislike`, or `null` when it was taken back.
- `reply_to`: what a message answers; `mine` there says whether that was your message.
- `at`: when the message itself arrived.

A message edited before `recv` took it comes once, as it is now; one deleted before that does not come at all.
