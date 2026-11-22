# ADR-040: paging, acknowledgement and escalation

Status: **accepted** (maintainer, 2026-11-22). Русская версия: [040-paging.ru.md](040-paging.ru.md). Builds on [ADR-032](032-ask.md) (codes, pinned verified contacts) and [ADR-039](039-alert-webhooks.md).

## Context

An alert that nobody reads is no alert. On-call tools (PagerDuty, Opsgenie) repeat a page until someone acknowledges it, pass it to the next person if nobody does, and say when the problem is gone. termchat has the parts: verified contacts, one-time codes from `ask`, a daemon that keeps state. This ADR puts them together, without a SaaS and without the incident text leaving end-to-end encryption.

## Decision

### 1. `termchat page`

```
termchat page --to alice --then bob --every 5m --escalate 15m --key disk-web1 \
    "Disk 95% full on web-1"
```

- Sends the text to the first contact, with a line: `Reply "ack 3f9c2e81a07d6b54" to acknowledge.` The code is 16 lowercase hex digits from the OS, as in ADR-032.
- Repeats a short reminder every `--every` (default 5 minutes, at least 1) until acknowledged.
- After `--escalate` (default 15 minutes) without an acknowledgement, pages the next contact in `--then` (several allowed, in order) with a new code, and tells the earlier ones "passed to bob". Earlier contacts stay paged: any of them can still acknowledge.
- `ack <code>` from any paged contact's primary device ends the page; the others get "acknowledged by alice". The command exits `0`. If the whole chain runs out (`--give-up`, default 2 hours), it exits `4` and tells everyone "nobody acknowledged".
- Every contact in the chain must be accepted and fingerprint-verified; their primary keys are pinned at start (ADR-032). Matching of the reply follows ADR-032 exactly: original direct text, authenticated sender and queue, exact form, after persistence.
- Pages go to contacts only, never to groups: a group has no single person to acknowledge, and group members are not pinned and verified one by one.

### 2. Keys and resolution

- `--key` names the incident. A second `page` with a key already open does not page again: it adds "still firing (2)" to the open one. So a flapping check does not wake the same person twice.
- `termchat resolve --key disk-web1` ends the page without an acknowledgement and tells everyone paged "resolved after 14m". Resolving after an acknowledgement sends the same line.
- From the webhook receiver (ADR-039), a route can page: `--route oncall=page:alice,bob:15m`. A firing Alertmanager alert opens a page keyed by its `fingerprint`; the matching resolved alert resolves it.

### 3. Lifetime

Pages live in the daemon's memory, as `ask` requests do. A daemon restart drops them; the docs say so, and `page` exits `6` when its daemon goes away. At most 64 open pages per daemon; more are refused with `5`. Reminders are not stored in the chat history as separate notes, they are ordinary messages.

| Exit status | Meaning |
| --- | --- |
| `0` | Acknowledged. |
| `2` | Invalid arguments or unsupported platform. |
| `3` | A contact not found or ambiguous. |
| `4` | Nobody acknowledged before `--give-up`, or resolved without acknowledgement. |
| `5` | A contact ineligible, too many pages, or a permanent send failure. |
| `6` | Daemon unavailable or lost. |

## Threat model

- **Whoever holds an unlocked primary device of a paged contact can acknowledge**, as with `ask`. An acknowledgement only stops reminders; it authorises nothing on the server.
- **Reminders are messages:** the relay sees their rhythm, as it sees `watch` heartbeats. The text stays encrypted.
- **No command reaches the server** through a page: `ack` only ends the page. This keeps the D2 rule: a stolen phone is not a console.
- **A flood:** keys merge repeats; the 64-page cap and ADR-039's rate limit bound what a broken alert source can do.

## Rejected

- **Acknowledging in a group:** who acknowledged would be unclear, and group members are not pinned.
- **Storing pages on disk:** a restart that silently re-pages old incidents is worse than one that drops them and says so.
- **Phone calls or SMS escalation:** other products, and phone numbers (rule 14).

## What changes

- `client`: `page` and `resolve` commands, daemon state for pages (reusing the `ask` matcher), `page:` routes in `hook`. Docs: "On-call paging" in the scripting guide (EN/RU), threat model.
- `core`, `proto`, `relay`, `engine`'s protocol: no change.
