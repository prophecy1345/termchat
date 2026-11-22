# termchat.app: project memory

## Positioning: everything is a contact

Everything we talk to or work through in termchat is a **contact**: a person, a
bot, an AI model, a script, a daemon, your own other device. One model, one UI,
one E2E channel (key pair + nickname, no accounts).

- No special "AI mode", "bot API" or "integration" screens: an AI is a contact
  you add, like anyone else (see `deploy/ai-bot/termchat-ai-bot`).
- The clients stay free of AI; nothing reaches a model unless you write to a
  contact that is one.
- Use this wording in docs, README and the site: "bots, AI and scripts are just
  contacts". Don't introduce a separate concept for them.
