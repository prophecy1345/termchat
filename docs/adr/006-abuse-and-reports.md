# ADR-006: abuse, blocking and reports

Status: accepted, option A, implemented. Date: 2026-10-15. Русская версия: [006-abuse-and-reports.ru.md](006-abuse-and-reports.ru.md).

## Context

termchat exists for private conversation, not for crime: trafficking in people,
weapons or drugs, planning violence, pushing someone to suicide. The question is
how to hinder such use without breaking everyone else's privacy.

## Rejected: a topic filter on messages

Messages are not scanned, neither on the relay nor in the client. Why:

1. **The relay sees only ciphertext;** only the client could check text. The
   client is open source (AGPL): a build without the filter takes minutes. A
   filter would stop only the law-abiding.
2. **Evasion is trivial:** code words, typos, transliteration, another language.
   Word lists and ML classifiers lose to rephrasing.
3. **False positives hurt the vulnerable:** someone writing to a friend about
   suicidal thoughts, a victim asking for help, a journalist, a doctor. Blocking
   the message of a person in crisis does direct harm.
4. **A scanning mechanism is a backdoor:** its topic list can be ordered to grow
   to politics. That contradicts the threat model.
5. **Ready-made tools do not fit:** PhotoDNA and Thorn Safer work on images;
   cloud moderation (Perspective, OpenAI Moderation) needs the text sent out.
   Apple dropped on-device scanning in 2022, and the EU "Chat Control" proposal
   has been blocked for years for the same reasons.

## Accepted: a design without reach

Trafficking and recruitment need reach and search. We do not provide them
(rule 15 in `CONTRIBUTING.md`):
- no user search and no nickname directory; contact only by invite;
- no public channels, broadcasts or public groups; groups, if they come, only
  small and by invite;
- strangers land in "requests"; the relay limits new recipients per day;
- text only.

## Accepted and done: blocking (M4)

Ctrl+B in the client. Messages from a blocked key are acknowledged to the relay
and thrown away **without decryption**, and the user cannot write to it.
Blocking is local: the relay does not know (otherwise it would learn another
piece of the social graph).

## Proposed: reporting to the relay operator

A recipient may choose to forward a message and the sender's key to the relay
operator. The operator may ban the key on their relay. No scanning: only the
person who was written to, and who decided to report, reads anything.

### Option A (accepted): a report without cryptographic proof

- `ClientFrame::Report { sender, text }`: plaintext the recipient chose to
  disclose. The client warns: "the relay operator will read this message".
- The relay checks plausibility against metadata it already has: the sender
  really did send envelopes to this recipient in the last N days.
- A key is banned automatically only after reports from **K different**
  recipients (proposed: K = 3) who really corresponded with it. Fewer reports go
  to the operator's log for manual review.
- A ban: the relay refuses the key at `Auth`. A new key runs into the per-IP
  registration limit.
- The report log is a separate file on the relay, for the operator only, deleted
  after review. It holds keys and the disclosed text, no IP addresses.
- Risk: a recipient can make up text. Defence: the threshold K and the metadata
  check. A ban on one relay does not stop anyone from moving to another.

### Option B: cryptographic proof (message franking)

Facebook Messenger adds a commitment to every message, which the server signs
when forwarding, so a recipient can prove that this sender sent this text.
vodozemac has nothing like it, and building such a scheme from primitives on top
of Olm means **writing our own crypto protocol**, which rule 1 forbids. Revisit
if a vetted library appears.

## Accepted and done: help in a crisis

The client's help (`?`) has a section with the international directory of
helplines (findahelpline.com), emergency numbers and hotlines. Text only, no
analysis of messages.

## Accepted: honest rules

The README describes the purpose, forbids illegal use, and says what a relay
operator can and cannot hand over (public keys, times, the ciphertext queue; no
contents, no IP history).

## Decisions on option A

- **K = 3**, automatic bans on (`Limits::report_threshold`). Refined in ADR-007:
  a report counts only with at least 3 envelopes from the sender to the reporter,
  and a reporter key younger than 7 days weighs 0.5.
- **Plausibility check:** a report counts only if the sender sent envelopes to
  the reporter at least once while the relay has been running. There is no window
  N: this data lives in memory and is lost on restart (ADR-005 keeps it that way).
- At most 10 reports per day from one key. The `Reported` answer is the same on
  every path, so it does not reveal whether the report counted or the key was
  banned.
- **Log** (`--reports`, `termchat-reports.log` by default, mode `0600`): one JSON
  line per report: time, sender key, whether it was banned, the disclosed text.
  No reporter key, no IP.
- **Client:** Ctrl+X shows exactly the text that will go to the operator (the last
  5 messages received), with a warning. On `y` the report is sent and the contact
  is blocked.
- **Ban:** the relay drops the key's connection and refuses it at `Auth`
  (`ErrorCode::Banned`). Bans are stored on disk (ADR-005).
