//! Wire format: envelopes, inner messages, padding, size limits.
//!
//! See `docs/protocol.md`. Everything here parses untrusted input, so every
//! decoder checks sizes and rejects trailing bytes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// Anonymous queues (ADR-023). Version 1 had accounts and is not supported.
pub const VERSION: u16 = 2;
/// Padded plaintext sizes (protocol.md, section 8).
pub const BUCKETS: [usize; 3] = [512, 4096, 16384];
pub const MAX_TEXT_BYTES: usize = 16_000;
/// Self-chosen display name, only ever sent inside E2EE.
pub const MAX_NICKNAME_BYTES: usize = 64;
/// Free text attached to a contact request.
pub const MAX_NOTE_BYTES: usize = 256;
/// A document sent in chunks (ADR-017).
pub const MAX_FILE_BYTES: u32 = 1 << 20;
/// Data per `Body::File` chunk: fits the largest bucket with a full header.
pub const FILE_CHUNK: usize = 16_000;
pub const MAX_FILE_NAME_BYTES: usize = 255;
/// Olm ciphertext of the largest bucket plus pre-key overhead.
pub const MAX_BODY_BYTES: usize = 20 * 1024;
/// One WebSocket frame (protocol.md, section 11).
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// A UnifiedPush endpoint URL (ADR-012).
pub const MAX_PUSH_URL_BYTES: usize = 512;
/// HTTP header on the relay's upgrade response: its version (ADR-010).
pub const VERSION_HEADER: &str = "x-termchat-version";
/// One-time keys per `PublishKeys`: vodozemac keeps 50 on the server.
pub const MAX_KEYS_PER_PUBLISH: usize = 100;
/// How long the relay keeps an undelivered envelope (ADR-005). Clients keep an
/// old fallback key one day longer (ADR-002).
pub const QUEUE_TTL_DAYS: u64 = 14;

pub type Ed25519Key = [u8; 32];
pub type Curve25519Key = [u8; 32];
pub type Signature = [u8; 64];
/// A queue address (ADR-023): `recv_id` to read it, `send_id` to write to it.
pub type QueueId = [u8; 16];
/// The queue owner's secret; the relay keeps only its SHA-256.
pub type QueueToken = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("malformed message")]
    Malformed,
    #[error("message too large")]
    TooLarge,
    #[error("unsupported protocol version")]
    UnsupportedVersion,
}

/// What the relay forwards from client to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    /// 0 = pre-key, 1 = normal (vodozemac `OlmMessage::to_parts`).
    pub olm_type: u8,
    pub body: Vec<u8>,
}

impl Envelope {
    pub fn encode(&self) -> Vec<u8> {
        // Serializing an owned struct into a Vec cannot fail.
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        // Bound the input before postcard allocates for `body`.
        if bytes.len() > MAX_BODY_BYTES + 64 {
            return Err(Error::TooLarge);
        }
        let env: Self = decode_exact(bytes)?;
        env.check()?;
        Ok(env)
    }

    fn check(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

/// Client to relay (protocol.md, sections 3-5). Nothing here names a person:
/// the relay knows queues only (ADR-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientFrame {
    /// First frame on every connection. Stays variant 0, so a version 1
    /// client still gets a readable `UnsupportedVersion`.
    Hello {
        version: u16,
    },
    CreateQueue,
    DeleteQueue {
        recv_id: QueueId,
        token: QueueToken,
    },
    Subscribe {
        recv_id: QueueId,
        token: QueueToken,
    },
    /// Prekeys of the queue; a contact queue carries one-time keys only.
    PublishKeys {
        recv_id: QueueId,
        token: QueueToken,
        one_time: Vec<SignedKey>,
        fallback: Option<SignedKey>,
    },
    ClaimKey {
        send_id: QueueId,
    },
    /// No authentication: knowing `send_id` is the right to write.
    Send {
        client_id: u64,
        send_id: QueueId,
        envelope: Envelope,
    },
    Ack {
        recv_id: QueueId,
        relay_id: u64,
    },
    /// The queue owner chose to disclose `text` to the relay operator (ADR-006).
    Report {
        recv_id: QueueId,
        token: QueueToken,
        text: String,
    },
    /// Where to wake the owner when a message waits in the queue; `None` forgets it (ADR-012).
    SetPush {
        recv_id: QueueId,
        token: QueueToken,
        endpoint: Option<String>,
    },
}

/// Relay to client. `Error` stays variant 7, as in version 1, so an old
/// client can read why it was turned away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerFrame {
    QueueCreated {
        recv_id: QueueId,
        send_id: QueueId,
        token: QueueToken,
    },
    Subscribed {
        recv_id: QueueId,
    },
    ClaimedKey {
        send_id: QueueId,
        key: SignedKey,
    },
    /// Sent to the queue's subscriber. `fallback_used`: the relay handed out
    /// the fallback key; rotate it (ADR-002).
    KeyCount {
        recv_id: QueueId,
        one_time_left: u32,
        fallback_used: bool,
    },
    Accepted {
        client_id: u64,
    },
    /// Same answer whatever the relay did.
    Reported,
    Deliver {
        recv_id: QueueId,
        relay_id: u64,
        envelope: Envelope,
    },
    Error {
        client_id: Option<u64>,
        code: ErrorCode,
    },
    QueueDeleted {
        recv_id: QueueId,
    },
    /// `SetPush` was stored (ADR-012).
    PushSet,
}

/// protocol.md, section 12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// Variant 0, as in version 1 (see `ServerFrame`).
    UnsupportedVersion,
    Malformed,
    TooLarge,
    RateLimited,
    NoKeys,
    /// No such queue, or a wrong token: one answer, so a token cannot probe
    /// which queues exist.
    UnknownQueue,
    QueueFull,
    Internal,
}

impl From<Error> for ErrorCode {
    fn from(e: Error) -> Self {
        match e {
            Error::Malformed => Self::Malformed,
            Error::TooLarge => Self::TooLarge,
            Error::UnsupportedVersion => Self::UnsupportedVersion,
        }
    }
}

impl ClientFrame {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::TooLarge);
        }
        let frame: Self = decode_exact(bytes)?;
        match &frame {
            Self::Send { envelope, .. } => envelope.check()?,
            Self::PublishKeys { one_time, .. } if one_time.len() > MAX_KEYS_PER_PUBLISH => {
                return Err(Error::TooLarge);
            }
            Self::Report { text, .. } if text.len() > MAX_TEXT_BYTES => {
                return Err(Error::TooLarge);
            }
            Self::SetPush {
                endpoint: Some(url),
                ..
            } if url.len() > MAX_PUSH_URL_BYTES => return Err(Error::TooLarge),
            _ => {}
        }
        Ok(frame)
    }
}

impl ServerFrame {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::TooLarge);
        }
        let frame: Self = decode_exact(bytes)?;
        if let Self::Deliver { envelope, .. } = &frame {
            envelope.check()?;
        }
        Ok(frame)
    }
}

/// Self-certifying identity: the Ed25519 key signs the Curve25519 key
/// (`"termchat-id-v1" || curve25519`). Sent in invites and contact requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub ed25519: Ed25519Key,
    pub curve25519: Curve25519Key,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

/// What a user hands out to be added: `termchat:<base64url(postcard)>` (protocol.md, section 9).
/// Versions 1 and 2 named only the identity; they are refused (ADR-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub version: u16,
    pub relay_url: String,
    /// The same relay as a Tor onion service, if it has one (ADR-004).
    pub onion_url: Option<String>,
    pub identity: Identity,
    /// `send_id` of the invite queue, where strangers write.
    pub queue: QueueId,
    /// Echoed in `ContactRequestV2` (ADR-013).
    pub code: InviteCode,
}

pub type InviteCode = [u8; 16];

pub const INVITE_VERSION: u16 = 3;
const INVITE_PREFIX: &str = "termchat:";
const MAX_RELAY_URL: usize = 256;

fn check_urls(relay_url: &str, onion_url: Option<&str>) -> Result<(), Error> {
    if relay_url.len() > MAX_RELAY_URL || onion_url.is_some_and(|u| u.len() > MAX_RELAY_URL) {
        return Err(Error::TooLarge);
    }
    Ok(())
}

impl Invite {
    pub fn encode(&self) -> String {
        format!(
            "{INVITE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(postcard::to_allocvec(self).unwrap_or_default())
        )
    }

    /// Tolerates surrounding whitespace from copy-paste.
    pub fn decode(s: &str) -> Result<Self, Error> {
        let b64 = s
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or(Error::Malformed)?;
        if b64.len() > 8 * MAX_RELAY_URL {
            return Err(Error::TooLarge);
        }
        let bytes = URL_SAFE_NO_PAD.decode(b64).map_err(|_| Error::Malformed)?;
        let (version, _): (u16, _) =
            postcard::take_from_bytes(&bytes).map_err(|_| Error::Malformed)?;
        if version != INVITE_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let invite: Self = decode_exact(&bytes)?;
        check_urls(&invite.relay_url, invite.onion_url.as_deref())?;
        Ok(invite)
    }
}

/// One-time or fallback key, signed over `"termchat-otk-v1" || key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedKey {
    pub key: Curve25519Key,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

/// Plaintext inside the Olm ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inner {
    pub sender: Ed25519Key,
    pub recipient: Ed25519Key,
    /// Per session and direction, starts at 1.
    pub seq: u64,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    /// First message to someone who does not know us yet.
    /// The nickname is the only thing we learn about a person; no real names,
    /// phones or emails exist anywhere in the protocol.
    ContactRequest {
        identity: Identity,
        nickname: String,
        note: String,
    },
    ContactAccept {
        nickname: String,
    },
    Text {
        sent_at: u64,
        expires_in: Option<u32>,
        text: String,
    },
    /// The sender wiped their account and will not read anything (ADR-009).
    Goodbye,
    /// `ContactRequest` sent from a version 2 invite, echoing its code (ADR-013).
    ContactRequestV2 {
        identity: Identity,
        nickname: String,
        note: String,
        code: InviteCode,
    },
    /// One chunk of a document, opaque bytes (ADR-017). Name and size repeat
    /// in every chunk; chunk `index` holds bytes from `index * FILE_CHUNK`.
    File {
        id: u32,
        name: String,
        size: u32,
        index: u16,
        expires_in: Option<u32>,
        data: Vec<u8>,
    },
    /// Write to me here from now on (ADR-023). Sent only inside Olm, so only
    /// the contact learns the address.
    QueueOffer {
        relay_url: String,
        onion_url: Option<String>,
        send_id: QueueId,
    },
    /// A text that can be replied to, edited, deleted and reacted to
    /// (ADR-025). `id` is random and names the message among the sender's own.
    Message {
        id: u64,
        sent_at: u64,
        expires_in: Option<u32>,
        reply_to: Option<Ref>,
        text: String,
    },
    /// New text for the sender's own `Message` `id` (ADR-025).
    Edit {
        id: u64,
        text: String,
    },
    /// The sender's own `Message` `id` is to be wiped (ADR-025).
    Delete {
        id: u64,
    },
    /// One of `REACTIONS`, or `None` to take one's reaction back (ADR-025).
    React {
        target: Ref,
        reaction: Option<u8>,
    },
    /// The kinds of message the sender reads: `UNDERSTANDS` (ADR-025).
    Understands {
        level: u16,
    },
    /// A kind this version does not know, from a newer client (ADR-025): its
    /// `seq` counts, nothing is shown. Never on the wire; keep it last, and
    /// put new kinds above it.
    #[serde(skip)]
    Unknown,
}

/// Kinds of `Body` on the wire: every variant but `Unknown`.
const KNOWN_KINDS: u32 = 12;

/// What `Understands` says this version reads: 1 = ADR-025.
pub const UNDERSTANDS: u16 = 1;

/// Reactions are indices into a fixed set the apps draw (ADR-025):
/// like, love, laugh, wow, sad, thanks, dislike.
pub const REACTIONS: u8 = 7;

/// A message in a chat, seen from whoever sends the `Ref`: `theirs` is
/// false for their own message, true for the recipient's (ADR-025).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    pub id: u64,
    pub theirs: bool,
}

/// The part of `Inner` before the body, to read past a body of unknown kind.
#[derive(Deserialize)]
struct InnerHead {
    sender: Ed25519Key,
    recipient: Ed25519Key,
    seq: u64,
}

impl Inner {
    /// Serialize and pad to the smallest bucket (ISO/IEC 7816-4: 0x80, then zeros).
    pub fn to_padded(&self) -> Result<Vec<u8>, Error> {
        self.check_limits()?;
        let mut buf = postcard::to_allocvec(self).map_err(|_| Error::Malformed)?;
        let bucket = *BUCKETS
            .iter()
            .find(|&&b| b > buf.len())
            .ok_or(Error::TooLarge)?;
        buf.push(0x80);
        buf.resize(bucket, 0);
        Ok(buf)
    }

    pub fn from_padded(bytes: &[u8]) -> Result<Self, Error> {
        if !BUCKETS.contains(&bytes.len()) {
            return Err(Error::Malformed);
        }
        let end = bytes
            .iter()
            .rposition(|&b| b != 0)
            .ok_or(Error::Malformed)?;
        if bytes[end] != 0x80 {
            return Err(Error::Malformed);
        }
        let bytes = &bytes[..end];
        let inner: Self = match decode_exact(bytes) {
            Ok(inner) => inner,
            Err(e) => return Self::unknown_kind(bytes).ok_or(e),
        };
        inner.check_limits()?;
        Ok(inner)
    }

    /// A body of a kind newer than this version, after a valid head.
    fn unknown_kind(bytes: &[u8]) -> Option<Self> {
        let (head, rest) = postcard::take_from_bytes::<InnerHead>(bytes).ok()?;
        let (kind, _) = postcard::take_from_bytes::<u32>(rest).ok()?;
        (kind >= KNOWN_KINDS).then_some(Self {
            sender: head.sender,
            recipient: head.recipient,
            seq: head.seq,
            body: Body::Unknown,
        })
    }

    fn check_limits(&self) -> Result<(), Error> {
        match &self.body {
            Body::Text { text, .. } | Body::Message { text, .. } | Body::Edit { text, .. }
                if text.len() > MAX_TEXT_BYTES =>
            {
                Err(Error::TooLarge)
            }
            Body::Unknown => Err(Error::Malformed),
            Body::ContactRequest { note, .. } | Body::ContactRequestV2 { note, .. }
                if note.len() > MAX_NOTE_BYTES =>
            {
                Err(Error::TooLarge)
            }
            Body::ContactRequest { nickname, .. }
            | Body::ContactRequestV2 { nickname, .. }
            | Body::ContactAccept { nickname } => check_nickname(nickname),
            Body::QueueOffer {
                relay_url,
                onion_url,
                ..
            } => check_urls(relay_url, onion_url.as_deref()),
            Body::File {
                name,
                size,
                index,
                data,
                ..
            } => {
                check_file_name(name)?;
                if *size == 0 || *size > MAX_FILE_BYTES {
                    return Err(Error::TooLarge);
                }
                if data.len() != chunk_len(*size, *index).ok_or(Error::Malformed)? {
                    return Err(Error::Malformed);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Number of chunks a file of `size` bytes takes.
pub fn file_chunks(size: u32) -> usize {
    (size as usize).div_ceil(FILE_CHUNK)
}

/// Exact data length of chunk `index`, or `None` past the last chunk.
pub fn chunk_len(size: u32, index: u16) -> Option<usize> {
    let start = usize::from(index) * FILE_CHUNK;
    (start < size as usize).then(|| (size as usize - start).min(FILE_CHUNK))
}

/// Control characters drive a terminal; bidi overrides reorder what is
/// around them. Neither belongs in anything a peer gets shown.
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A name only, never a path: the receiver joins it to a folder it picked.
pub fn check_file_name(name: &str) -> Result<(), Error> {
    if name.len() > MAX_FILE_NAME_BYTES {
        return Err(Error::TooLarge);
    }
    if name.trim().is_empty()
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| is_unsafe_char(c) || matches!(c, '/' | '\\'))
    {
        return Err(Error::Malformed);
    }
    Ok(())
}

/// Accepts only the canonical encoding: postcard tolerates overlong varints,
/// so the value must re-encode to exactly the input (found by fuzzing).
/// Control characters would let a peer drive the recipient's terminal.
pub fn check_nickname(nickname: &str) -> Result<(), Error> {
    if nickname.len() > MAX_NICKNAME_BYTES {
        return Err(Error::TooLarge);
    }
    if nickname.trim().is_empty() || nickname.chars().any(char::is_control) {
        return Err(Error::Malformed);
    }
    Ok(())
}

fn decode_exact<'a, T: Serialize + Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, Error> {
    let value: T = postcard::from_bytes(bytes).map_err(|_| Error::Malformed)?;
    match postcard::to_allocvec(&value) {
        Ok(canonical) if canonical == bytes => Ok(value),
        _ => Err(Error::Malformed),
    }
}

/// serde supports arrays only up to 32 elements; signatures are 64 bytes.
mod serde_big_array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        let mut halves = ([0u8; 32], [0u8; 32]);
        halves.0.copy_from_slice(&v[..32]);
        halves.1.copy_from_slice(&v[32..]);
        halves.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let (a, b) = <([u8; 32], [u8; 32])>::deserialize(d)?;
        let mut out = [0; 64];
        out[..32].copy_from_slice(&a);
        out[32..].copy_from_slice(&b);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(len: usize) -> Inner {
        Inner {
            sender: [1; 32],
            recipient: [2; 32],
            seq: 1,
            body: Body::Text {
                sent_at: 0,
                expires_in: None,
                text: "a".repeat(len),
            },
        }
    }

    fn with(body: Body) -> Inner {
        Inner {
            sender: [1; 32],
            recipient: [2; 32],
            seq: 7,
            body,
        }
    }

    /// Pads raw `Inner` bytes the way `to_padded` does.
    fn pad(mut buf: Vec<u8>) -> Vec<u8> {
        let bucket = *BUCKETS.iter().find(|&&b| b > buf.len()).unwrap();
        buf.push(0x80);
        buf.resize(bucket, 0);
        buf
    }

    #[test]
    fn adr_025_kinds_roundtrip() {
        let target = Ref {
            id: u64::MAX,
            theirs: true,
        };
        for body in [
            Body::Message {
                id: u64::MAX,
                sent_at: u64::MAX,
                expires_in: Some(u32::MAX),
                reply_to: Some(target),
                text: "a".repeat(MAX_TEXT_BYTES),
            },
            Body::Edit {
                id: 3,
                text: "fixed".into(),
            },
            Body::Delete { id: 3 },
            Body::React {
                target,
                reaction: Some(REACTIONS - 1),
            },
            Body::React {
                target,
                reaction: None,
            },
            Body::Understands { level: UNDERSTANDS },
        ] {
            let inner = with(body);
            assert_eq!(
                Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
                inner
            );
        }
    }

    #[test]
    fn adr_025_texts_keep_the_limit() {
        for body in [
            Body::Message {
                id: 1,
                sent_at: 0,
                expires_in: None,
                reply_to: None,
                text: "a".repeat(MAX_TEXT_BYTES + 1),
            },
            Body::Edit {
                id: 1,
                text: "a".repeat(MAX_TEXT_BYTES + 1),
            },
        ] {
            assert!(matches!(with(body).to_padded(), Err(Error::TooLarge)));
        }
    }

    #[test]
    fn known_kinds_counts_every_kind_on_the_wire() {
        // The last kind on the wire is `KNOWN_KINDS - 1`, right after the head.
        let raw = postcard::to_allocvec(&with(Body::Understands { level: 1 })).unwrap();
        let head = postcard::to_allocvec(&(&[1u8; 32], &[2u8; 32], 7u64)).unwrap();
        assert_eq!(u32::from(raw[head.len()]), KNOWN_KINDS - 1);
    }

    #[test]
    fn a_newer_kind_is_unknown_with_its_seq() {
        let mut raw = postcard::to_allocvec(&(&[1u8; 32], &[2u8; 32], 7u64)).unwrap();
        raw.extend(postcard::to_allocvec(&KNOWN_KINDS).unwrap());
        raw.extend(b"whatever a newer version puts here");
        assert_eq!(
            Inner::from_padded(&pad(raw.clone())).unwrap(),
            with(Body::Unknown)
        );
        // A kind number past one byte as well.
        raw.truncate(73);
        raw.extend(postcard::to_allocvec(&100_000u32).unwrap());
        assert_eq!(Inner::from_padded(&pad(raw)).unwrap(), with(Body::Unknown));
    }

    #[test]
    fn a_broken_known_kind_stays_malformed() {
        let mut raw = postcard::to_allocvec(&with(Body::Delete { id: 1 })).unwrap();
        raw.extend(b"trailing");
        assert!(matches!(
            Inner::from_padded(&pad(raw)),
            Err(Error::Malformed)
        ));
        // A head cut short is no unknown kind either.
        assert!(matches!(
            Inner::from_padded(&pad(vec![1; 40])),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn unknown_never_goes_on_the_wire() {
        assert!(with(Body::Unknown).to_padded().is_err());
    }

    #[test]
    fn padding_roundtrip_hits_every_bucket() {
        for (len, bucket) in [(0, 512), (1000, 4096), (MAX_TEXT_BYTES, 16384)] {
            let padded = text(len).to_padded().unwrap();
            assert_eq!(padded.len(), bucket);
            assert_eq!(Inner::from_padded(&padded).unwrap(), text(len));
        }
    }

    #[test]
    fn contact_request_roundtrip() {
        let identity = Identity {
            ed25519: [1; 32],
            curve25519: [2; 32],
            signature: [3; 64],
        };
        let mut signature = [0; 64];
        signature[63] = 9;
        let inner = Inner {
            body: Body::ContactRequest {
                identity: Identity {
                    signature,
                    ..identity
                },
                nickname: "Ёжик 🦔".into(),
                note: "hi".into(),
            },
            ..text(0)
        };
        assert_eq!(
            Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
            inner
        );
        for (bad, err) in [
            ("evil\u{1b}[2J", Error::Malformed),
            ("  ", Error::Malformed),
            (&*"я".repeat(33), Error::TooLarge),
        ] {
            let inner = Inner {
                body: Body::ContactAccept {
                    nickname: bad.into(),
                },
                ..text(0)
            };
            assert_eq!(inner.to_padded(), Err(err));
        }
        let long_note = Inner {
            body: Body::ContactRequest {
                identity: Identity {
                    signature,
                    ..identity
                },
                nickname: "x".into(),
                note: "n".repeat(MAX_NOTE_BYTES + 1),
            },
            ..text(0)
        };
        assert_eq!(long_note.to_padded(), Err(Error::TooLarge));
        let mut v2 = Inner {
            body: Body::ContactRequestV2 {
                identity: identity.clone(),
                nickname: "x".into(),
                note: "hi".into(),
                code: [5; 16],
            },
            ..text(0)
        };
        assert_eq!(Inner::from_padded(&v2.to_padded().unwrap()).unwrap(), v2);
        if let Body::ContactRequestV2 { note, .. } = &mut v2.body {
            *note = "n".repeat(MAX_NOTE_BYTES + 1);
        }
        assert_eq!(v2.to_padded(), Err(Error::TooLarge));
        let bye = Inner {
            body: Body::Goodbye,
            ..text(0)
        };
        let padded = bye.to_padded().unwrap();
        assert_eq!(padded.len(), 512);
        assert_eq!(Inner::from_padded(&padded).unwrap(), bye);
        let key = SignedKey {
            key: [4; 32],
            signature,
        };
        let bytes = postcard::to_allocvec(&key).unwrap();
        assert_eq!(bytes.len(), 96);
        assert_eq!(postcard::from_bytes::<SignedKey>(&bytes).unwrap(), key);
    }

    fn chunk(name: &str, size: u32, index: u16, len: usize) -> Inner {
        Inner {
            seq: u64::MAX,
            body: Body::File {
                id: u32::MAX,
                name: name.into(),
                size,
                index,
                expires_in: Some(u32::MAX),
                data: vec![7; len],
            },
            ..text(0)
        }
    }

    #[test]
    fn file_chunks_roundtrip_and_check_limits() {
        // The largest header still fits the largest bucket.
        let full = chunk(
            &"n".repeat(MAX_FILE_NAME_BYTES),
            MAX_FILE_BYTES,
            0,
            FILE_CHUNK,
        );
        let padded = full.to_padded().unwrap();
        assert_eq!(padded.len(), 16384);
        assert_eq!(Inner::from_padded(&padded).unwrap(), full);
        let last = file_chunks(MAX_FILE_BYTES) - 1;
        assert_eq!(file_chunks(MAX_FILE_BYTES), 66);
        let tail = chunk(
            "a.pdf",
            MAX_FILE_BYTES,
            last as u16,
            (1 << 20) - last * FILE_CHUNK,
        );
        assert_eq!(
            Inner::from_padded(&tail.to_padded().unwrap()).unwrap(),
            tail
        );
        assert_eq!(file_chunks(1), 1);
        assert_eq!(chunk_len(FILE_CHUNK as u32, 1), None);

        for (bad, err) in [
            (chunk("a", 0, 0, 0), Error::TooLarge),
            (
                chunk("a", MAX_FILE_BYTES + 1, 0, FILE_CHUNK),
                Error::TooLarge,
            ),
            (chunk("a", 10, 1, 0), Error::Malformed),
            (chunk("a", 10, 0, 9), Error::Malformed),
            (chunk("a", 10, 0, 11), Error::Malformed),
            (chunk("a", 20_000, 0, 4000), Error::Malformed),
            (chunk("a", 20_000, 1, FILE_CHUNK), Error::Malformed),
            (
                chunk(&"n".repeat(MAX_FILE_NAME_BYTES + 1), 1, 0, 1),
                Error::TooLarge,
            ),
        ] {
            assert_eq!(bad.to_padded(), Err(err));
        }
        for name in [
            "",
            " ",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "a\0b",
            "evil\u{1b}[2J",
            "x\u{202E}fdp.exe",
        ] {
            assert_eq!(
                chunk(name, 1, 0, 1).to_padded(),
                Err(Error::Malformed),
                "{name:?}"
            );
        }
        assert!(chunk("отчёт v2.final.pdf", 1, 0, 1).to_padded().is_ok());
    }

    #[test]
    fn file_chunk_rejected_on_the_wire() {
        // Same check on decode: a peer that skips `to_padded` gets nowhere.
        let bad = chunk("../x", 1, 0, 1);
        let mut buf = postcard::to_allocvec(&bad).unwrap();
        buf.push(0x80);
        buf.resize(512, 0);
        assert_eq!(Inner::from_padded(&buf), Err(Error::Malformed));
    }

    #[test]
    fn frames_roundtrip_and_check_limits() {
        let env = Envelope {
            version: VERSION,
            olm_type: 0,
            body: vec![1; 10],
        };
        let send = ClientFrame::Send {
            client_id: 7,
            send_id: [5; 16],
            envelope: env.clone(),
        };
        assert_eq!(ClientFrame::decode(&send.encode()).unwrap(), send);
        let deliver = ServerFrame::Deliver {
            recv_id: [4; 16],
            relay_id: 1,
            envelope: env.clone(),
        };
        assert_eq!(ServerFrame::decode(&deliver.encode()).unwrap(), deliver);
        let create = ClientFrame::CreateQueue;
        assert_eq!(ClientFrame::decode(&create.encode()).unwrap(), create);
        let mut trailing = create.encode();
        trailing.push(0);
        assert_eq!(ClientFrame::decode(&trailing), Err(Error::Malformed));
        let created = ServerFrame::QueueCreated {
            recv_id: [1; 16],
            send_id: [2; 16],
            token: [3; 32],
        };
        assert_eq!(ServerFrame::decode(&created.encode()).unwrap(), created);
        let push = ClientFrame::SetPush {
            recv_id: [1; 16],
            token: [3; 32],
            endpoint: Some("https://ntfy.sh/upAbc".into()),
        };
        assert_eq!(ClientFrame::decode(&push.encode()).unwrap(), push);
        let long = ClientFrame::SetPush {
            recv_id: [1; 16],
            token: [3; 32],
            endpoint: Some("x".repeat(MAX_PUSH_URL_BYTES + 1)),
        };
        assert_eq!(ClientFrame::decode(&long.encode()), Err(Error::TooLarge));
        let report = ClientFrame::Report {
            recv_id: [1; 16],
            token: [3; 32],
            text: "x".repeat(MAX_TEXT_BYTES + 1),
        };
        assert_eq!(ClientFrame::decode(&report.encode()), Err(Error::TooLarge));
        assert_eq!(
            ServerFrame::decode(&ServerFrame::PushSet.encode()).unwrap(),
            ServerFrame::PushSet
        );

        let old = ClientFrame::Send {
            client_id: 7,
            send_id: [5; 16],
            envelope: Envelope { version: 9, ..env },
        };
        assert_eq!(
            ClientFrame::decode(&old.encode()),
            Err(Error::UnsupportedVersion)
        );
        let key = SignedKey {
            key: [0; 32],
            signature: [0; 64],
        };
        let many = ClientFrame::PublishKeys {
            recv_id: [1; 16],
            token: [3; 32],
            one_time: vec![key; MAX_KEYS_PER_PUBLISH + 1],
            fallback: None,
        };
        assert_eq!(ClientFrame::decode(&many.encode()), Err(Error::TooLarge));
        assert_eq!(
            ClientFrame::decode(&vec![0; MAX_FRAME_BYTES + 1]),
            Err(Error::TooLarge)
        );
    }

    #[test]
    fn version_1_client_reads_the_refusal() {
        // A version 1 client opens with `Hello { version: 1 }` and can decode
        // only its own `ServerFrame`: `Error` must keep variant 7 and
        // `UnsupportedVersion` variant 0, or it sees garbage instead of
        // "please update".
        assert_eq!(
            ClientFrame::Hello { version: 1 }.encode(),
            [0, 1],
            "Hello is variant 0"
        );
        let refusal = ServerFrame::Error {
            client_id: None,
            code: ErrorCode::UnsupportedVersion,
        };
        assert_eq!(refusal.encode(), [7, 0, 0]);
    }

    #[test]
    fn invite_roundtrip_and_old_versions_refused() {
        let invite = Invite {
            version: INVITE_VERSION,
            relay_url: "wss://relay.example.org/".into(),
            onion_url: Some(
                "ws://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/".into(),
            ),
            identity: Identity {
                ed25519: [1; 32],
                curve25519: [2; 32],
                signature: [3; 64],
            },
            queue: [4; 16],
            code: [9; 16],
        };
        let s = invite.encode();
        assert!(s.starts_with("termchat:") && s.len() < 400, "{s}");
        assert_eq!(Invite::decode(&format!("  {s}\n")).unwrap(), invite);
        assert_eq!(Invite::decode("termchat:!!!"), Err(Error::Malformed));
        assert_eq!(Invite::decode(&s[1..]), Err(Error::Malformed));

        let raw = URL_SAFE_NO_PAD.decode(&s[INVITE_PREFIX.len()..]).unwrap();
        let with = |b: &[u8]| format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(b));
        for v in [1, 2, 4] {
            let mut old = raw.clone();
            old[0] = v;
            assert_eq!(
                Invite::decode(&with(&old)),
                Err(Error::UnsupportedVersion),
                "version {v}"
            );
        }
        let mut trailing = raw.clone();
        trailing.push(0);
        assert_eq!(Invite::decode(&with(&trailing)), Err(Error::Malformed));
        let long = Invite {
            relay_url: "x".repeat(MAX_RELAY_URL + 1),
            ..invite
        };
        assert_eq!(Invite::decode(&long.encode()), Err(Error::TooLarge));
    }

    #[test]
    fn queue_offer_roundtrip_and_url_limits() {
        let offer = Inner {
            body: Body::QueueOffer {
                relay_url: "wss://relay.example.org/".into(),
                onion_url: None,
                send_id: [8; 16],
            },
            ..text(0)
        };
        let padded = offer.to_padded().unwrap();
        assert_eq!(padded.len(), 512);
        assert_eq!(Inner::from_padded(&padded).unwrap(), offer);
        for (relay_url, onion_url) in [
            ("x".repeat(MAX_RELAY_URL + 1), None),
            ("x".into(), Some("y".repeat(MAX_RELAY_URL + 1))),
        ] {
            let bad = Inner {
                body: Body::QueueOffer {
                    relay_url,
                    onion_url,
                    send_id: [8; 16],
                },
                ..text(0)
            };
            assert_eq!(bad.to_padded(), Err(Error::TooLarge));
            // The same check on decode, for a peer that skips `to_padded`.
            let mut buf = postcard::to_allocvec(&bad).unwrap();
            buf.push(0x80);
            buf.resize(4096, 0);
            assert_eq!(Inner::from_padded(&buf), Err(Error::TooLarge));
        }
    }

    #[test]
    fn text_over_limit_is_rejected() {
        assert_eq!(text(MAX_TEXT_BYTES + 1).to_padded(), Err(Error::TooLarge));
    }

    #[test]
    fn bad_padding_is_rejected() {
        let mut padded = text(5).to_padded().unwrap();
        assert_eq!(Inner::from_padded(&padded[..511]), Err(Error::Malformed));
        let last = padded.len() - 1;
        padded[last] = 1;
        assert_eq!(Inner::from_padded(&padded), Err(Error::Malformed));
        assert_eq!(Inner::from_padded(&[0; 512]), Err(Error::Malformed));
    }

    #[test]
    fn envelope_checks_version_size_and_trailing_bytes() {
        let env = Envelope {
            version: VERSION,
            olm_type: 1,
            body: vec![9; 100],
        };
        assert_eq!(Envelope::decode(&env.encode()).unwrap(), env);

        let mut bytes = env.encode();
        bytes.push(0);
        assert_eq!(Envelope::decode(&bytes), Err(Error::Malformed));

        // Overlong varint for the version.
        let mut overlong = vec![0x82, 0x00];
        overlong.extend_from_slice(&env.encode()[1..]);
        assert_eq!(Envelope::decode(&overlong), Err(Error::Malformed));

        let old = Envelope {
            version: 0,
            ..env.clone()
        };
        assert_eq!(
            Envelope::decode(&old.encode()),
            Err(Error::UnsupportedVersion)
        );

        let big = Envelope {
            body: vec![0; MAX_BODY_BYTES + 1],
            ..env
        };
        assert_eq!(Envelope::decode(&big.encode()), Err(Error::TooLarge));
    }
}
