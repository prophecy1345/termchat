---
title: "termchat.app troubleshooting: connecting, passwords, copying, layout"
description: "What to do when termchat.app is stuck connecting, says wrong password or SAVE FAILED, cannot copy an invite, looks broken, or plays no sound."
---

# If something goes wrong

Русская версия: [trouble.ru.md](trouble.ru.md).

| Problem | What to do |
|---|---|
| Stuck on `○ connecting…` | Check your internet. With Tor the first connection can take up to a minute, through bridges a few minutes. If it never connects, Tor may be blocked where you are: add [bridges](tor.md). Or turn Tor off for a moment to see whether the relay is reachable at all. |
| `wrong password` | Passwords are case-sensitive and depend on the keyboard layout. There is no recovery; restore from a backup if you have one. |
| `SAVE FAILED` in the status bar | The disk is full or not writable. Messages are acknowledged only once they are saved, so nothing is lost; free some space. |
| "termchat.app is already open in another window" (in a script: "this identity is open in another termchat") | The terminal client, the desktop app and the scriptable mode share one account, one at a time. Quit the other one, or wait for the other command to end. |
| A contact shows `↻` or "needs a new invite" | They were added before version 0.7.0. Exchange invites again. |
| "their app is older" | Answers and reactions need 0.9 on both sides; ask them to update. |
| **c** does not copy the invite | Your terminal does not let programs use the clipboard (OSC 52; macOS Terminal does not). Select the text with the mouse while holding **Shift**. |
| The layout looks broken | Make the window larger, at least 80×24, and use a font with box-drawing characters. In an old terminal without true colour pick the `terminal` theme. |
| Selecting text with the mouse does not work | Hold **Shift** while dragging, or turn the mouse off in settings → interface. |
| No sound | macOS plays a system sound; Linux needs `canberra-gtk-play` or `paplay`; otherwise the terminal bell is used, which your terminal may mute. |
| `termchat add` from a script ends with code 4 | The request goes with the next command; run any command again, or `termchat contacts`. |

Something else? Open an issue at [github.com/prophecy1345/termchat/issues](https://github.com/prophecy1345/termchat/issues); for a question or an idea, start a thread in [Discussions](https://github.com/prophecy1345/termchat/discussions). Both are public: never paste your password, backup or invite there, and report a vulnerability privately as [SECURITY.md](https://github.com/prophecy1345/termchat/blob/main/SECURITY.md) says.
