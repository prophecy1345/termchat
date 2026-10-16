# Logo and favicon prompt (for ChatGPT image generation)

Paste the prompt below as is. Ask for the mark first, then the wordmark, then
the favicon versions of the chosen mark.

---

Design a logo for **termchat**, a private, end-to-end encrypted, text-only
messenger that runs in the computer terminal. No accounts, no phone numbers:
a person is just a key pair and a nickname. The server only ever sees
ciphertext.

**The idea to show:** a terminal prompt meeting a sealed message. Combine the
terminal cursor `>_` with either a speech bubble or a padlock, so that it reads
as "a conversation, locked, in a terminal". Optional subtle detail: half of the
shape made of small monospace characters (like ciphertext) resolving into a
clean solid half (plaintext), as if the message is being decrypted.

**Style:**
- Flat, geometric, vector-like, built from simple strokes and rounded
  rectangles; consistent stroke weight; no gradients except one optional soft
  gradient from mauve to pink.
- Must stay recognisable at 16×16 pixels: few shapes, strong silhouette, no
  thin details.
- Mood: calm, trustworthy, a little retro (1980s terminal, synthwave) but not
  neon, not glowing, not loud.

**Colours (Catppuccin Mocha):**
- background `#1e1e2e` (dark) or transparent;
- main colour mauve `#cba6f7`, optional gradient to pink `#f5c2e7`;
- small accent teal `#94e2d5` (for example the cursor block `_`);
- text colour `#cdd6f4`.

**Deliverables:**
1. The mark alone, square 1024×1024, centred, on `#1e1e2e` with rounded corners
   (radius about 22%), and the same mark on a transparent background.
2. A horizontal wordmark: the mark plus the word `termchat` in lowercase, in a
   clean monospace typeface (like JetBrains Mono or SF Mono), text in `#cdd6f4`.
3. A favicon version of the mark simplified for 32×32 and 16×16: at most two
   colours on `#1e1e2e`, no text.
4. A single-colour version (mauve on dark, and dark on white) for print and
   monochrome use.

**Avoid:** photorealism, 3D, glossy effects, bright neon glow, shields, globes,
paper planes, anything that resembles the logos of Signal, Telegram, WhatsApp,
Matrix/Element or Tor; text inside the favicon; more than three colours.

---

The result is in `docs/logo/` and `site/public/` (favicon, logo, apple-touch-icon).
