# ADR-034 — Tolerant `ask` replies and minimum `watch` intervals

Status — **proposed**, awaiting the maintainer. [Русская версия](034-ask-and-watch-input.ru.md).

Amends [ADR-032](032-ask.md) and [ADR-031](031-watch.md).

## Context

A review of 0.18–0.20 found two input rules that fail quietly.

1. ADR-032 accepts only `yes CODE` or `no CODE` byte for byte. Neither the apps nor the engine trims a message before sending. A trailing space or newline from a phone keyboard or a paste, or a capitalised `Yes`, is ignored. The person sees no error and the script waits until its timeout (exit 4). That looks like silence, not a refusal.
2. ADR-031 accepts any positive interval. `watch send --every 1` makes both service identities rewrite their encrypted state file every second and sends a relay message every second. Nothing on the server or watcher needs pulses that often.

## Decision

### 1. `ask` replies

The daemon normalises a candidate reply before comparing it:

- strip ASCII whitespace (space, tab, CR, LF) at both ends;
- split on one run of ASCII spaces or tabs into exactly two words;
- compare the first word with `yes` or `no` ignoring ASCII case;
- compare the second word with the current code ignoring ASCII case.

Everything else in ADR-032 stays: the reply must come from the pinned contact's primary device through its direct queue, as an original `Body::Text` or `Body::Message`, saved before the decision and before the deadline. Extra words, other decision words (`y`, `ok`) and other codes are still ignored. The code keeps 64 bits from the OS. Case-insensitive hex adds no guesses, because the code is 16 lowercase hex digits and an uppercase copy names the same value.

### 2. `watch` intervals

- `watch send --every` takes at least 10 seconds.
- `watch monitor --after` takes at least 30 seconds.

Smaller values exit with code 2. The guide already asks for `--after` of at least twice `--every` plus a minute. The two values live on different hosts, so the daemon cannot compare them.

## Threat model

Normalising does not widen who can approve: sender, device, queue, code and deadline checks are unchanged. A person who sees the question can still approve only from the pinned primary device, as before. The interval minimums change no message format and no relay behaviour.

## Alternatives considered

- **Keep exact matching and document it louder.** The failure would still be silent and would still end in a timeout.
- **Answer a near-miss with a hint message.** That adds chat traffic the script did not ask for and tells anyone reading the chat which replies come close.
- **No minimum, warn in the docs only.** A one-second typo would still wear storage on both hosts.

## Acceptance criteria

`yes CODE`, `Yes CODE `, `YES code\n` and `yes\tCODE` approve. `no CODE` in any case declines. `yes CODE now`, `y CODE`, `yes OTHERCODE`, the same text from a linked device and the same text after the deadline do nothing. `watch send --every 9` and `watch monitor --after 29` exit with code 2. `--every 10` and `--after 30` start.
