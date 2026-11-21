# ADR-036 — CI notifications through a server, not an identity in CI

Status — **accepted by the maintainer**. [Русская версия](036-ci-over-ssh.ru.md).

## Context

A build or deploy in GitHub Actions or another CI should be able to message a person, or wait for their approval, through termchat. The obvious design keeps a termchat identity in the repository's secrets and restores it on every run. That design is unsafe.

A secret is a fixed snapshot. Every run would start from the same Olm session state and encrypt its message with the same message key as the run before. Two different texts under one key and one IV break confidentiality, and the recipient sees the same `seq` again. termchat backups leave sessions out for this reason. Keeping state in a CI cache instead needs rollback protection for an evicted cache and for two runs at once, and anyone who reads the CI secrets reads that identity's messages.

## Decision

CI holds no termchat identity. It reaches a server that runs one, over SSH, with a key that can do exactly one thing.

1. **A CI identity on a server.** Set up a separate system user and identity for CI, as under “Setting up a server”, with `/bin/sh` as the shell, because sshd runs forced commands through the user's shell. Run `termchat daemon` for it.
2. **One key, one command.** The CI public key goes into that user's `authorized_keys` with a forced command and every forwarding turned off:

   ```
   command="TERMCHAT_HOME=/var/lib/termchat-ci /usr/local/bin/termchat send alice",restrict ssh-ed25519 AAAA… ci@example/repo
   ```

   sshd ignores the command the client asks for and runs this one. The text comes from stdin. The recipient is fixed on the server, not chosen by CI.
3. **Approvals the same way.** A second key may be bound to `termchat ask alice "Deploy example/repo to production?" --timeout 30m`. The question is fixed on the server. ssh returns the remote exit status, so the CI step fails unless the person approves.
4. **The CI step.** The guide gives a copyable step. It writes the private key from a secret and the server's host key from a variable, connects with `StrictHostKeyChecking=yes`, and pipes the message on stdin. Values from the `github` context go through environment variables, never straight into the script, because branch names and commit messages are attacker-controlled text.
5. **No action to install.** The step is plain shell in the user's workflow. A published action would be one more thing to trust, pin and update for a dozen lines.

## Threat model

- A leaked CI key lets someone send texts to that one recipient as the CI identity, or start the fixed question. It does not read messages, choose recipients or run commands. Rotate it by replacing the `authorized_keys` line.
- The CI identity lives on the server under its own user. Root on that server reads it, as for any server identity (ADR-024).
- The message contains what the workflow puts into it. Build logs or secrets piped into it reach the chat history.
- `ask` through CI keeps ADR-032: only the verified contact's primary device approves, and anyone with that unlocked device can.
- Pinning the host key stops a network attacker from posing as the server and collecting messages or answering `ask`.

## Alternatives considered

- **Identity in CI secrets.** Repeats Olm message keys on every run; rejected.
- **State in the CI cache with rollback protection.** Needs engine changes and still exposes the identity's messages to anyone who reads CI secrets.
- **A fresh identity per run.** Every run would need a new contact request and acceptance.
- **A webhook on the server.** Needs an HTTP listener, TLS and authentication, which SSH already provides.

## Acceptance criteria

Against a test sshd: the forced command runs whatever command the client asks for; text on stdin arrives as the message; the remote exit status (`0`, `4`, `1` for a declined `ask`) reaches the CI step; port, agent and X11 forwarding and a PTY are refused; an unknown host key stops the connection. The guide's step passes `github` values only through the environment.
