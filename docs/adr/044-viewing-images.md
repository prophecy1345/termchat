# ADR-044: viewing a received image on the desktop and Android, on a tap

Status: **accepted** (owner, 2026-11-24). Русская версия: [044-viewing-images.ru.md](044-viewing-images.ru.md). **Changes rule 9** and the "Showing images" entry rejected in [ADR-017](017-documents.md).

## Context

The owner asked (2026-11-24) to look at a received picture inside the app, full size, as Telegram does, on the desktop and Android. The terminal is left out.

ADR-017 rejected showing images: **image decoders are a classic attack surface**. Zero-click attacks on messengers came through them (the libwebp bug CVE-2023-4863; FORCEDENTRY through an image parser). Anyone who can message you can feed your decoder. Earlier the same day the owner turned down image previews for that reason.

This ADR allows viewing under conditions that remove the zero-click part and keep the decoder out of native code.

## Decision

### 1. Only on a tap, only a document already here

- A received document that is **whole and still held** (not saved or discarded, ADR-017 and ADR-043) gets a **View** button if its first bytes are a PNG, JPEG, GIF or WebP signature. That check reads a few bytes and decodes nothing. The file name is ignored.
- Nothing is decoded until the person taps **View**. No thumbnails in the chat, no previews in notifications, no decoding on arrival, no auto-play.
- Only from accepted contacts, in chats and groups. A pending request or a blocked contact cannot get there anyway (ADR-017, section 5).
- Our own sent files are not viewable: the app does not keep their bytes after sending.
- Once the document is saved or discarded, its bytes are gone from termchat.app and View goes with them. The saved file lives outside the app, as before.

### 2. One decoder, in Rust, in the engine

- Decoding happens in `engine` with the **`image` crate** (PNG, JPEG, GIF, WebP only), on the desktop **and on Android**: the app passes the bytes through FFI and gets pixels back. Android's `BitmapFactory` and `ImageDecoder` are **not used**: they are the native code those attacks went through.
- Limits, checked before and during the decode (`image::Limits`):
  - input up to 25 MiB (the document limit);
  - at most 8 192 pixels per side and 40 megapixels;
  - at most 256 MiB allocated by the decoder.
- An image over a limit, or one that fails to decode, shows "this image cannot be shown"; the document stays savable.
- GIF: the first frame only, no animation. EXIF orientation is applied, so phone photos are upright; nothing else from EXIF is read or shown.
- The result is scaled down to at most 2 560 pixels on the longest side before it reaches the app. That is enough for any screen and keeps the memory a phone needs bounded.
- Decoding runs off the interface thread (a worker thread on the desktop, a background coroutine on Android), so a slow image cannot freeze the app.

### 3. The viewer

- **Desktop:** a window over the chat, the image fitted to it; zoom with the wheel or the trackpad, drag to move, Esc closes. Buttons: Save…, Close.
- **Android:** a full-screen view; pinch to zoom, drag to move, back closes. Buttons: Save, Close. The app's screenshot block (`FLAG_SECURE`) covers it.
- Pixels live only in memory while the viewer is open. They are never written to disk, a cache or the clipboard. On the desktop the buffer is wiped when the viewer closes. On Android the bitmap is recycled on close; until the garbage collector reuses that memory the pixels may stay in RAM.

### 4. Settings

None. The View button is the decision, made per image.

### 5. Rule 9 becomes

"Text, and documents as opaque bytes (ADR-017). On the desktop and Android a received image may be viewed on an explicit tap, decoded only by the engine's Rust decoder with limits (ADR-044). No previews, thumbnails, auto-opening, link previews or message formatting."

## Consequences and honest limits

- **One-click, not zero-click.** A hostile image still reaches the decoder if you tap View. The decoder is memory-safe Rust, which rules out the buffer overflows behind the attacks above, but its dependencies contain some `unsafe` code and a bug there cannot be excluded. `cargo deny` and `cargo vet` cover them, and our own fuzz target runs the decode function with its limits.
- **Denial of service** is bounded by the limits: an image built to need too much memory or too many pixels is refused, not decoded.
- **On screen.** Whoever looks at the screen sees the picture.
- **No new metadata** for the relay: viewing happens on the device.
- **The terminal** shows no images.

## Rejected

- **System decoders** (`BitmapFactory`, `ImageDecoder`, the desktop OS): native code, the path of known zero-click attacks.
- **Thumbnails or previews in the chat**: decoding without a tap; turned down by the owner on 2026-11-24.
- **Decoding in a separate sandboxed process**: platform-specific and heavy; the Rust decoder with limits covers the main risk. Can be added later if the decoder's track record calls for it.
- **Animated GIF and video**: more decoder, more surface, little gain.
- **A setting to turn viewing on**: the per-image tap is already an explicit choice; a setting adds no protection.

## Dependencies

- **`image`**, default features off, `png`, `jpeg`, `gif`, `webp` only, in `engine`. Justified in the commit, checked with `cargo deny` and `cargo vet`.
- A fuzz target for the decode function with its limits.
