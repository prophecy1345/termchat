//! Wire format: envelopes, inner messages, padding, size limits.
//!
//! See `docs/protocol.md`. Everything here parses untrusted input, so every
//! decoder checks sizes and rejects trailing bytes.

use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
/// Padded plaintext sizes (protocol.md, section 8).
pub const BUCKETS: [usize; 3] = [512, 4096, 16384];
pub const MAX_TEXT_BYTES: usize = 16_000;
/// Olm ciphertext of the largest bucket plus pre-key overhead.
pub const MAX_BODY_BYTES: usize = 20 * 1024;

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
        if env.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if env.body.len() > MAX_BODY_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(env)
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
    ContactRequest {
        identity: Identity,
        note: String,
    },
    ContactAccept,
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
            _ => Ok(()),
        }
    }
}

/// Accepts only the canonical encoding: postcard tolerates overlong varints,
/// so the value must re-encode to exactly the input (found by fuzzing).
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
                note: "hi".into(),
            },
            ..text(0)
        };
        assert_eq!(
            Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
            inner
        );
        let key = SignedKey {
            key: [4; 32],
            signature,
        };
        let bytes = postcard::to_allocvec(&key).unwrap();
        assert_eq!(bytes.len(), 96);
        assert_eq!(postcard::from_bytes::<SignedKey>(&bytes).unwrap(), key);
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
