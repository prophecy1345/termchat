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
| `groups [--json]` | group ID and name, one per line |
| `send CONTACT [TEXT] [--expires 5m]` | sends TEXT, or stdin when TEXT is left out; waits until the relay has it |
| `send-file CONTACT PATH` | a document of up to 1 MiB |
| `recv [--from C] [--wait S] [--follow] [--json] [--save-dir DIR]` | prints new messages and changes, each once; files are saved only into `--save-dir` |
| `watch send WATCHER --every SECS` | sends hidden heartbeats to a watcher host (requires a daemon) |
| `watch monitor SERVER --notify OWNER --after SECS` | messages OWNER when SERVER's heartbeats stop (requires a daemon) |
| `run (--to C \| --group NAME \| --group-id ID) [--name L] [--lines N] [--failures-only] -- COMMAND` | runs COMMAND, then reports how it ended |
| `ask CONTACT QUESTION [--timeout SECS or Nm]` | waits for a verified contact to approve or decline (requires a daemon) |
| `hook --token-file PATH --route NAME=contact:C \| group:G [...]` | takes monitoring webhooks and passes them on (requires a daemon; see below) |
| `page --to C [--then C ...] [--key K] [--every 5m] [--escalate 15m] [--give-up 2h] TEXT` | pages a verified contact until someone answers `ack CODE`, then the next one (requires a daemon) |
| `resolve --key K` | ends a page and tells everyone paged that it is resolved |
| `daemon` | keeps the identity open, so the commands above need no password and answer at once (Linux, macOS; see below) |

Commands that open the identity take `--password-file PATH` or `--password-stdin` and `--timeout SECS` (60 by default). The state folder is `TERMCHAT_HOME`, or `~/.termchat`.

- `CONTACT` is the name the app shows, or the start of the fingerprint. If more than one contact matches, nothing is sent.
- **The password never comes from the environment**: every child process inherits it. Use a file that belongs to you and has mode `600`, stdin, or the keyboard. Without any of them termchat asks at the keyboard, and a script has none: it ends with "no terminal to type the password".
- There is no "send to all": one command, one contact.
- The owner accepts contact requests in the app. The CLI and daemon have no command to accept a request.
- Text from the other side is cleaned of control characters before it reaches your terminal or log. Files are saved only into the folder you name, never opened.

**A message that disappears.** `send --expires` gives one direct message a timer of its own: `30s`, `5m`, `1h` or `1d`, from 10 seconds to 7 days. Your copy disappears that long after sending, and theirs that long after it reaches their device, as with the chat timer in the apps. Pass a secret on stdin, so it stays out of shell history and the process list:

```sh
printf '%s\n' "$DB_PASSWORD" | termchat send alice --expires 5m
```

The timer limits how long the text stays in both chats. It does not stop a screenshot, a notification on screen or a copy. Group messages follow the group's timer, so `--expires` works only for a contact.

`send` and `send-file` wait for the relay to accept the message within `--timeout`. They return `0` on delivery to the relay, `1` for a password or state error, `2` for invalid arguments, `3` for a missing or ambiguous contact, `4` if the message is still queued, and `5` if it cannot be sent.

## Private groups

Create groups, accept invitations and manage membership in the app. A server can participate with its own identity. Use `groups` to find its groups and copy an ID when names are duplicated or may change.

```sh
termchat groups --json
printf '%s\n' 'Backup finished' | termchat send --group 'Operations'
termchat recv --group-id 2b14b109d9b97317785a7bdb7075ae53 --follow --json
```

Use the exact displayed name with `--group` or the full 32-digit hexadecimal ID with `--group-id`. These commands use the daemon for the same `TERMCHAT_HOME` when it is running. Otherwise the CLI opens the identity itself.

Group `send` takes one text argument or reads stdin. It returns `0` when the relay accepts that text, `1` for an internal or state error, `2` for invalid arguments, `3` for a missing or ambiguous group, `4` on timeout and `5` for a final refusal or loss of access. A queued text may arrive after a timeout. An in-flight send may reach the relay before a membership change reaches the sender.

Group `recv` takes either `--wait SECS` or `--follow`. Without either, it prints the texts that have already arrived and exits. An empty timed read exits with `0`. Loss of access ends a wait with `5`. Plain `recv` reads direct chats. Group reads reject `--from` and `--save-dir`.

JSON lists contain `id` and `name`. JSON message records contain `group_id`, `group`, `from`, `fingerprint`, `at` (Unix seconds when received), `id`, `text` and `reply_to` when present. Both message IDs are strings. Group reads include incoming texts and replies received after the upgrade. A text edited before handoff appears in its current form. The app shows earlier history, documents and subsequent changes. Own, deleted and expired texts are excluded.

The daemon hands each batch to the readers already waiting on that group. Each reader can buffer up to 64 records and exits with code `1` on overflow. The read marker is saved before output, so a broken pipe or crash can interrupt output after a text has been taken.

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

Each command above unlocks the identity and connects to the relay anew: over Tor that is 10–30 seconds a command. `termchat daemon` does it once and stays (ADR-029). While it runs, `send`, `send-file`, `recv`, `groups`, `contacts`, `invite` and `add` for the same `TERMCHAT_HOME` go through it without a password, and several can run at a time, and `run` checks and reports through it. When no daemon runs, they work as before. `watch` and `ask` work only through a daemon.

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

The example sends a heartbeat every five minutes and alerts after 15 minutes of silence. Use seconds for both options: `--every` takes at least 10 and `--after` at least 30. Set `--after` to at least twice `--every` plus a minute for reconnects. A threshold at or below the send interval makes the watcher send an alert after nearly every heartbeat. Both commands run in the foreground, so use your service manager to restart them when the daemon stops or the socket closes. The heartbeats stay out of chat history and `recv`. The alert is an ordinary direct message, such as “backup-box has been silent for 15 minutes”.

A unit for the sender, next to the daemon unit from the section above. The unit for `watch monitor` on the watcher host differs only in `ExecStart`.

```ini
# /etc/systemd/system/termchat-watch.service
[Unit]
Description=termchat heartbeat to the watcher
After=termchat-daemon.service
BindsTo=termchat-daemon.service

[Service]
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
ExecStart=/usr/local/bin/termchat watch send watcher --every 300
Restart=always
RestartSec=10

[Install]
WantedBy=termchat-daemon.service
```

Enable it with `systemctl enable --now termchat-watch`. It starts and stops with the daemon. Until the daemon's socket appears, the command exits and systemd starts it again after 10 seconds.

Keep the watcher awake and the clocks synchronized. A relay outage can cause an alert or prevent delivery. The daemon logs locally when it queues an alert and when it cannot confirm delivery. A fresh heartbeat lets the watcher report the next outage.

## Ask for approval before a script step

Set up a server identity and add the approver as under “Setting up a server”. Mark the approver as verified on the server with steps 2 and 3 of the previous section: stop the daemon, open `termchat` as the service user, press **Ctrl+F** on the contact, [compare safety numbers](contacts.md) and press **v**. Then start `termchat daemon` again. Run `ask` as the same user with the same `TERMCHAT_HOME` on Linux or macOS.

```sh
if termchat ask alice "Deploy build 42 to staging?" --timeout 10m; then
    ./deploy-staging build-42
fi
```

On their primary device, Alice sees an ordinary chat message with two possible replies, for example `yes f4bc2801e9029d71` and `no f4bc2801e9029d71`. She sends one of those lines. Letter case and spaces at the ends do not matter, but extra words do. The server waits for a reply with the current code from her verified primary device. A reply from her linked desktop or laptop does not count. Only one request per contact can wait at a time. The timeout defaults to 10 minutes, and `600` and `10m` mean the same thing.

If the question begins with `--`, put it after the `--` separator and put any options before that separator.

The command writes nothing to stdout. It returns `0` for approval, `1` for refusal and `4` for timeout. Other errors use `2` for invalid input, `3` for an unknown or ambiguous contact, `5` for an ineligible contact, a pending request or failed send, and `6` for an unavailable daemon or internal error. A delayed question may still arrive after the request closes. A reply to it has no effect.

Anyone who can send as Alice's primary device, including someone with her unlocked phone, can approve a pending request. The question and reply may appear in chat history and notifications, and the question may appear in shell history or CI logs. Keep secrets out of the question.

## Report how a command ended

`termchat run` wraps a command. When the command ends, it sends one message with the result, how long it took, the host and the last lines of output (ADR-035).

```sh
termchat run --to alice --name nightly-backup --failures-only -- /usr/local/bin/backup.sh --full
```

```
nightly-backup failed with exit 1 after 42m 10s on backup-box
rsync: write failed on "/mnt/backup/db.tar": No space left on device (28)
```

- Pick one recipient: `--to CONTACT`, `--group NAME` or `--group-id ID`. termchat checks it before the command starts. A wrong password, an unknown contact or a group the server cannot use stops `run`, and the command does not start.
- Everything after `--` is the command. termchat starts it without a shell. For a pipeline, write `sh -c '…'` yourself.
- The report names the job with `--name`, or with the program's file name. Arguments and environment never go into it.
- `--lines N` keeps 0 to 50 lines, 10 by default. Each line is cut at 500 bytes. `--failures-only` sends nothing when the command exits with `0`.
- The command's output still reaches your terminal or log unchanged. It keeps stdin and the environment.
- Without a daemon, `run` needs `--password-file` and opens the identity only to check and to send, not while the command runs. `--password-stdin` is not accepted, because stdin belongs to the command.
- `run` exits with the command's status: its code, `128 + N` when signal N ended it, or `127` when it could not start. Ctrl+C and SIGTERM go to the command, and `run` still reports. When the command succeeded but the report was not sent, `run` returns the send code (`1`, `4` or `5`). It always explains a send problem on stderr.

The output can contain secrets: a token, a password, a private path. The report lands in the chat history and notifications, and on every device of a group. For such commands, use `--lines 0` or `--failures-only`.

## Alerts from your monitoring

`termchat hook` takes webhooks from Alertmanager, Grafana, Uptime Kuma, Gatus or anything that can POST, and passes each one on as an encrypted message to a contact or a private group (ADR-039). It holds no password and no keys: it hands every message to the [daemon](#a-daemon-for-a-bot-that-answers), so start that first.

```sh
termchat hook --new-token /etc/termchat/hook-token     # once: 32 random bytes, mode 600
termchat hook --token-file /etc/termchat/hook-token \
    --route oncall=contact:alice --route ops=group:Ops --labels job,severity
```

- A route is `NAME=contact:CONTACT`, `NAME=group:GROUP`, `NAME=group-id:ID` or `NAME=page:CONTACT,CONTACT[:15m]` (see [On-call paging](#on-call-paging)), with an optional format at the end: `:alertmanager`, `:kuma`, `:text`, or `:auto` (the default, which tells them apart). `POST /oncall` then goes to alice, `POST /ops` to the group. A request cannot pick a recipient.
- A contact must be accepted and have a verified safety number; `hook` checks every route when it starts and refuses to run otherwise.
- Every request needs `Authorization: Bearer <token>`. Without it: `401`.
- It listens on `127.0.0.1:9099`. `--listen ADDR` takes another address; anything other than loopback sends the token in the clear, so put a TLS proxy (Caddy, nginx) in front.
- An Alertmanager or Grafana alert becomes `FIRING (2)` and one line per alert: its name, `instance`, and the `summary` or `description` annotation. Other labels stay out unless `--labels` names them: they often hold internal addresses.
- Answers: `200` once the relay has the message, `503` when the daemon or relay is not there (Alertmanager retries), `400` for a body that is not the route's format, `413` over 1 MiB, `429` over 30 messages a minute per route.

Alertmanager, in `alertmanager.yml`:

```yaml
receivers:
  - name: termchat
    webhook_configs:
      - url: http://127.0.0.1:9099/oncall
        http_config:
          authorization:
            credentials_file: /etc/termchat/hook-token
```

Grafana: a contact point of type Webhook with the URL above and the token as "Authorization header" credentials (scheme `Bearer`). Uptime Kuma: a Webhook notification with "Request Body" set to "Preset - application/json" and the header `{"Authorization": "Bearer <token>"}`.

A unit next to the daemon's:

```ini
[Unit]
Description=termchat webhooks
After=termchat-daemon.service
Requires=termchat-daemon.service

[Service]
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
ExecStart=/usr/local/bin/termchat hook --token-file /etc/termchat/hook-token --route oncall=contact:alice
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

What an alert says lands in the chat history on every device that gets it. Keep secrets out of annotations. Whoever holds the token can write to the routes and nothing more: no reading, no other recipients, no commands to the server. To change it, replace the file and restart `hook`.

## On-call paging

`termchat page` keeps paging until a person answers, then tells everyone (ADR-040). It needs the daemon.

```sh
termchat page --to alice --then bob --every 5m --escalate 15m --key disk-web1 \
    "Disk 95% full on web-1"
```

- alice gets the text with a line `Reply "ack 3f9c2e81a07d6b54" to acknowledge.` She answers with exactly that, from her main device, in the app or with `termchat send`.
- Until then she gets a reminder every `--every` (default 5m, at least 1m). After `--escalate` (default 15m) bob is paged too, with a code of his own, and alice hears "Passed to bob". Either of them can acknowledge; the others then hear who did.
- `page` exits `0` when acknowledged, `4` when nobody answered before `--give-up` (default 2h, at most 7d) or the page was resolved first, `3` for an unknown contact, `5` when a contact is not verified, `6` without a daemon.
- `--key` names the incident. A second `page` with an open key pages nobody again: it sends "Still firing (2)". `termchat resolve --key disk-web1` ends it and sends "Resolved after 14m".
- Every contact in the chain must be accepted and verified. Pages go to contacts only, never to groups. They live in the daemon's memory: a daemon restart drops them.

From the webhook receiver, a route pages instead of sending: `--route oncall=page:alice,bob:15m`. A firing Alertmanager alert opens a page keyed by its fingerprint, the same alert resolved resolves it; an Uptime Kuma monitor going down pages, coming up resolves.

An acknowledgement only stops the reminders. It authorizes nothing on the server, and nothing a contact sends can run a command there.

## Your own AI bot

termchat has no AI inside, and nothing you write reaches a model unless you send it there. If you want one, run it as a contact of its own: [`deploy/ai-bot/termchat-ai-bot`](https://github.com/prophecy1345/termchat/blob/main/deploy/ai-bot/termchat-ai-bot) answers each message with a language model, next to a [daemon](#a-daemon-for-a-bot-that-answers) on a machine of yours.

It talks to any OpenAI-compatible chat API. By default that is [Ollama](https://ollama.com) on the same machine, so the whole path stays yours:

```sh
ollama pull llama3.2
AI_ALLOW="12345 67890 11121 31415 16171 81920" termchat-ai-bot
```

- `AI_ALLOW` lists the fingerprints that may use the bot, comma-separated; it ignores everyone else.
- `AI_URL` (default `http://127.0.0.1:11434/v1`), `AI_MODEL` (default `llama3.2`), `AI_KEY_FILE` (a `0600` file) or `AI_KEY` for an API that needs a key, `AI_SYSTEM` for the system prompt.
- Each message is answered on its own: the bot keeps no conversation history.
- It needs `jq` and `curl`, and answers through `termchat send`, so the daemon must run.

A unit next to the daemon's:

```ini
[Unit]
Description=termchat AI bot
After=termchat-daemon.service
Requires=termchat-daemon.service

[Service]
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
Environment=AI_ALLOW=12345 67890 11121 31415 16171 81920
ExecStart=/usr/local/bin/termchat-ai-bot
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

With a remote API instead of a local model, everything you write to the bot goes to that provider, in the clear for them: the encryption ends at your server. Do not add the bot to a group: it would pass on everything the group says. Keep the API key in a `0600` file, not in the unit.

## From GitHub Actions or another CI

Keep no termchat identity in CI secrets (ADR-036). A secret is a fixed copy of the identity, so every run would start from the same encryption state and reuse message keys. Instead, CI connects over SSH to a server that runs a CI identity, with a key that can only send to one contact.

```sh
# on the server, as root: a user with a shell, because sshd runs the forced command through it
useradd --system --create-home --home-dir /var/lib/termchat-ci --shell /bin/sh termchat-ci
usermod -p '*' termchat-ci          # no password login, but not a locked account
chmod 700 /var/lib/termchat-ci
install -d -o termchat-ci -g termchat-ci -m 700 /etc/termchat-ci /var/lib/termchat-ci/.ssh
head -c 30 /dev/urandom | base64 | sudo -u termchat-ci sh -c 'umask 077; cat > /etc/termchat-ci/pw'
ci() { sudo -u termchat-ci env TERMCHAT_HOME=/var/lib/termchat-ci termchat "$@" --password-file /etc/termchat-ci/pw; }
ci init --nickname ci-example
ci add 'termchat:...'               # your invite; accept the request in your app

# on your computer: a key for this repository only
ssh-keygen -t ed25519 -N '' -f termchat-ci -C 'ci example/repo'

# on the server: the key may only send to alice
printf 'command="TERMCHAT_HOME=/var/lib/termchat-ci /usr/local/bin/termchat send alice",restrict %s\n' \
  "$(cat termchat-ci.pub)" >> /var/lib/termchat-ci/.ssh/authorized_keys
chown termchat-ci: /var/lib/termchat-ci/.ssh/authorized_keys; chmod 600 /var/lib/termchat-ci/.ssh/authorized_keys
```

Run `termchat daemon` for this user as under “A daemon, for a bot that answers”, with `User=termchat-ci` and its paths. Check that `ci contacts` shows `alice` as `ok`. If the server has `AllowUsers` in `sshd_config`, add `termchat-ci` there.

In the repository settings, store the private key `termchat-ci` as the secret `TERMCHAT_SSH_KEY`. Store the server's host key as the variable `TERMCHAT_HOST_KEY`, as one line such as `ci.example.org ssh-ed25519 AAAA…`. Take it from `/etc/ssh/ssh_host_ed25519_key.pub` on the server itself, not from the network. Then add this step at the end of a job:

```yaml
      - name: Report to termchat
        if: always()
        env:
          KEY: ${{ secrets.TERMCHAT_SSH_KEY }}
          HOST_KEY: ${{ vars.TERMCHAT_HOST_KEY }}
          TEXT: "${{ github.repository }} ${{ github.workflow }} #${{ github.run_number }}: ${{ job.status }} ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}"
        run: |
          umask 077
          printf '%s\n' "$KEY" > "$RUNNER_TEMP/termchat-key"
          printf '%s\n' "$HOST_KEY" > "$RUNNER_TEMP/termchat-host"
          printf '%s\n' "$TEXT" | ssh -T -i "$RUNNER_TEMP/termchat-key" -o IdentitiesOnly=yes \
            -o BatchMode=yes -o StrictHostKeyChecking=yes \
            -o UserKnownHostsFile="$RUNNER_TEMP/termchat-host" termchat-ci@ci.example.org
```

- sshd runs only the forced command, whatever the client asks for. The text arrives on stdin, and the recipient is fixed on the server.
- The step fails with the `send` exit code, for example `4` when the message is still queued.
- `restrict` turns off port, agent and X11 forwarding and the terminal. A leaked key can only send texts to `alice` as `ci-example`. Replace its line in `authorized_keys` to revoke it.
- Values from `github` go through `env`, never straight into `run`: a branch name or commit message is text anyone with push access can choose.

**An approval before a deploy.** Generate a second key and bind it to a fixed question. Mark `alice` as verified for the CI identity first, as in “Ask for approval before a script step”.

```sh
printf 'command="TERMCHAT_HOME=/var/lib/termchat-ci /usr/local/bin/termchat ask alice \\"Deploy example/repo to production?\\" --timeout 30m",restrict %s\n' \
  "$(cat termchat-deploy.pub)" >> /var/lib/termchat-ci/.ssh/authorized_keys
```

Use the same step with that key, `< /dev/null` instead of the text, and `timeout-minutes: 35`. ssh returns the remote exit status, so the job continues only after `yes`: `1` means she declined and `4` that time ran out.

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
