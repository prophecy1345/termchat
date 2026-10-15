//! Wire format: envelopes, inner messages, padding, size limits.
//!
//! See `docs/protocol.md`. Everything here parses untrusted input, so every
//! decoder checks sizes and rejects trailing bytes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
/// Padded plaintext sizes (protocol.md, section 8).
pub const BUCKETS: [usize; 3] = [512, 4096, 16384];
pub const MAX_TEXT_BYTES: usize = 16_000;
/// Self-chosen display name, only ever sent inside E2EE.
pub const MAX_NICKNAME_BYTES: usize = 64;
/// Olm ciphertext of the largest bucket plus pre-key overhead.
pub const MAX_BODY_BYTES: usize = 20 * 1024;
/// One WebSocket frame (protocol.md, section 11).
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// One-time keys per `PublishKeys`: vodozemac keeps 50 on the server.
pub const MAX_KEYS_PER_PUBLISH: usize = 100;
/// How long the relay keeps an undelivered envelope (ADR-005). Clients keep an
/// old fallback key one day longer (ADR-002).
pub const QUEUE_TTL_DAYS: u64 = 14;

pub type Ed25519Key = [u8; 32];
pub type Curve25519Key = [u8; 32];
pub type Signature = [u8; 64];

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
    pub recipient: Ed25519Key,
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

/// Client to relay (protocol.md, sections 3-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientFrame {
    Hello {
        version: u16,
    },
    /// `signature` over `"termchat-auth-v1" || relay_host || nonce`.
    Auth {
        identity: Identity,
        #[serde(with = "serde_big_array")]
        signature: Signature,
    },
    PublishKeys {
        one_time: Vec<SignedKey>,
        fallback: Option<SignedKey>,
    },
    ClaimKey {
        target: Ed25519Key,
    },
    Send {
        client_id: u64,
        envelope: Envelope,
    },
    Ack {
        relay_id: u64,
    },
    /// The recipient chose to disclose `text` from `sender` to the relay operator (ADR-006).
    Report {
        sender: Ed25519Key,
        text: String,
    },
}

/// Relay to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerFrame {
    Challenge {
        nonce: [u8; 32],
    },
    AuthOk,
    ClaimedKey {
        target: Ed25519Key,
        key: SignedKey,
    },
    /// `fallback_used`: the relay handed out the fallback key; rotate it (ADR-002).
    KeyCount {
        one_time_left: u32,
        fallback_used: bool,
    },
    Accepted {
        client_id: u64,
    },
    /// Same answer whatever the relay did, so reports cannot probe bans.
    Reported,
    Deliver {
        relay_id: u64,
        sender: Ed25519Key,
        envelope: Envelope,
    },
    Error {
        client_id: Option<u64>,
        code: ErrorCode,
    },
}

/// protocol.md, section 12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    UnsupportedVersion,
    Malformed,
    TooLarge,
    NotAuthenticated,
    BadSignature,
    ChallengeExpired,
    RateLimited,
    NoKeys,
    UnknownRecipient,
    QueueFull,
    /// Banned on this relay after reports (ADR-006).
    Banned,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub version: u16,
    pub relay_url: String,
    /// The same relay as a Tor onion service, if it has one (ADR-004).
    pub onion_url: Option<String>,
    pub identity: Identity,
}

const INVITE_PREFIX: &str = "termchat:";
const MAX_RELAY_URL: usize = 256;

impl Invite {
    pub fn encode(&self) -> String {
        let bytes = postcard::to_allocvec(self).unwrap_or_default();
        format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
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
        let invite: Self = decode_exact(&bytes)?;
        if invite.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if invite.relay_url.len() > MAX_RELAY_URL
            || invite
                .onion_url
                .as_ref()
                .is_some_and(|u| u.len() > MAX_RELAY_URL)
        {
            return Err(Error::TooLarge);
        }
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
        let inner: Self = decode_exact(&bytes[..end])?;
        inner.check_limits()?;
        Ok(inner)
    }

    fn check_limits(&self) -> Result<(), Error> {
        match &self.body {
            Body::Text { text, .. } if text.len() > MAX_TEXT_BYTES => Err(Error::TooLarge),
            Body::ContactRequest { nickname, .. } | Body::ContactAccept { nickname } => {
                check_nickname(nickname)
            }
            _ => Ok(()),
        }
    }
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
        let key = SignedKey {
            key: [4; 32],
            signature,
        };
        let bytes = postcard::to_allocvec(&key).unwrap();
        assert_eq!(bytes.len(), 96);
        assert_eq!(postcard::from_bytes::<SignedKey>(&bytes).unwrap(), key);
    }

    #[test]
    fn frames_roundtrip_and_check_limits() {
        let env = Envelope {
            version: VERSION,
            recipient: [3; 32],
            olm_type: 0,
            body: vec![1; 10],
        };
        let send = ClientFrame::Send {
            client_id: 7,
            envelope: env.clone(),
        };
        assert_eq!(ClientFrame::decode(&send.encode()).unwrap(), send);
        let deliver = ServerFrame::Deliver {
            relay_id: 1,
            sender: [4; 32],
            envelope: env.clone(),
        };
        assert_eq!(ServerFrame::decode(&deliver.encode()).unwrap(), deliver);

        let old = ClientFrame::Send {
            client_id: 7,
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
    fn invite_roundtrip() {
        let invite = Invite {
            version: VERSION,
            relay_url: "wss://relay.example.org/".into(),
            onion_url: Some(
                "ws://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/".into(),
            ),
            identity: Identity {
                ed25519: [1; 32],
                curve25519: [2; 32],
                signature: [3; 64],
            },
        };
        let s = invite.encode();
        assert!(s.starts_with("termchat:") && s.len() < 350, "{s}");
        assert_eq!(Invite::decode(&format!("  {s}\n")).unwrap(), invite);
        assert_eq!(Invite::decode("termchat:!!!"), Err(Error::Malformed));
        assert_eq!(Invite::decode(&s[1..]), Err(Error::Malformed));
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
            recipient: [3; 32],
            olm_type: 1,
            body: vec![9; 100],
        };
        assert_eq!(Envelope::decode(&env.encode()).unwrap(), env);

        let mut bytes = env.encode();
        bytes.push(0);
        assert_eq!(Envelope::decode(&bytes), Err(Error::Malformed));

        // Overlong varint for version 1.
        let mut overlong = vec![0x81, 0x00];
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
