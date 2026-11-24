---
title: "Encrypted server alerts without the noise, now in Docker: termchat.app 0.31"
description: "termchat.app 0.31 folds repeated monitoring alerts into one end-to-end encrypted message and ships a 15 MB Docker image of the command-line client for Compose sidecars and Kubernetes CronJobs. No Slack, no Telegram bot, no SaaS in between."
date: 2026-11-24
lang: en
---

termchat.app can already pass alerts from Alertmanager, Grafana, Uptime Kuma or Gatus to your phone as end-to-end encrypted messages, with `termchat hook` on the server and no Slack or Telegram bot in between. Version 0.31 makes that quieter and easier to run.

## One alert, not twelve

A disk that fills up does not fire once. Monitoring repeats the alert, and a phone that buzzes twelve times for the same problem teaches you to ignore it.

From 0.31, `termchat hook` remembers what it sent on each route:

- the **same alert on the same route within 10 minutes** is not sent again; the hook answers `200` and counts it;
- the next time that alert goes out, one line says how many repeats were held back: *the same alert came 11 more times after it was last sent*;
- a different alert, a different route or the resolved notice goes out at once;
- a send that failed is not counted: the next alert tries again.

Nothing about this leaves the server: the counting happens in the hook, before encryption, and the relay sees fewer messages, not more. On-call paging routes (`termchat page`) keep their own rules: a firing alert keeps paging until someone acknowledges it.

## A 15 MB image for sidecars and CronJobs

Servers increasingly run as containers, and a messenger client should fit there too. `deploy/cli.Dockerfile` builds an image of the `termchat` command alone:

- a static binary on an empty base, **15 MB**, running as an unprivileged user;
- root certificates for `wss://` included, nothing else;
- the identity lives in a `/data` volume, never in the image;
- built with the same pinned Rust toolchain as the relay image.

```sh
docker build -f deploy/cli.Dockerfile -t termchat .
docker run --rm -i -v termchat:/data termchat init --nickname build-box --password-stdin < pw
docker run --rm -i -v termchat:/data termchat send alice "deploy done" --password-stdin < pw
```

Run it as a sidecar next to your services with `termchat daemon` or `termchat hook`, or as a Kubernetes CronJob that reports how a backup ended. Pass the password as a secret file, never as an environment variable. The server is a contact like any other: added with an invite, verified once by its safety number, and unable to reach anyone who did not add it.

## Also in 0.31

Creating an incognito invite now says that a new identity is being made and clears that note once the invite opens; on Android the button waits meanwhile instead of seeming to do nothing.

Everything about scripts, the daemon, webhooks, paging and Docker: [Scripts and servers](/docs/scripts/).
