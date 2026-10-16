# Security policy

termchat protects private conversations, so security reports are the most
welcome contribution there is. Thank you for looking.

## Reporting a vulnerability

**Please do not open a public issue.** Report privately through GitHub:
[Security → Report a vulnerability](https://github.com/prophecy1345/termchat/security/advisories/new).
Only the maintainer sees the report.

Helpful to include:
- what an attacker can do (read messages, impersonate someone, crash the relay,
  learn metadata, …) and who the attacker is (relay operator, network, contact,
  stranger);
- the version (`termchat --version`) or commit;
- steps or a proof of concept.

## What to expect

termchat is a personal hobby project with one maintainer, so these are goals,
not guarantees:

- an acknowledgement within **7 days**;
- an assessment and a plan within **14 days**;
- a fix released as soon as possible, usually within **90 days**, coordinated
  with you before anything is published;
- credit in the release notes and the advisory, if you want it.

## Scope

In scope: everything in this repository: the client (`crates/client`), the
cryptographic core and wire format (`crates/core`, `crates/proto`), the relay
(`crates/relay`), the install scripts, the release workflow and the website.

Especially interesting:
- anything that breaks the guarantees in the
  [threat model](docs/threat-model.md) or deviates from the
  [protocol](docs/protocol.md);
- key binding, session handling and state storage (`crates/core`);
- parsing of untrusted input from the network or from other users;
- ways to make a terminal display misleading content (escape sequences, bidi);
- the duress password and wiping;
- denial of service against the public relay beyond what its rate limits allow.

Out of scope, because the threat model already says so:
- metadata the relay is documented to see (who writes to whom and when, size
  buckets);
- attacks that need malware on the user's device or a compromised OS;
- traffic correlation by a global observer;
- bugs in vodozemac itself: please report those to
  [matrix-org/vodozemac](https://github.com/matrix-org/vodozemac/security).

The public relay at `relay.termchat.app` is a live service used by real people.
Please do not run load tests or attacks against it; run your own relay
(`termchat-relay`, see [self-host.md](docs/self-host.md)) for testing.

## Supported versions

Only the latest release gets fixes. termchat has not had an external security
audit yet.
