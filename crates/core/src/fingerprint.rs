//! Key fingerprints for manual verification (protocol.md, section 10).

use sha2::{Digest, Sha512};
use termchat_proto::Ed25519Key;

/// 30 digits in 6 groups. The same key always gives the same fingerprint.
pub fn fingerprint(key: &Ed25519Key) -> String {
    let hash = Sha512::new()
        .chain_update(b"termchat-fp-v1")
        .chain_update(key)
        .finalize();
    hash[..30]
        .chunks(5)
        .map(|c| {
            let n = c.iter().fold(0u64, |acc, &b| acc << 8 | u64::from(b));
            format!("{:05}", n % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 60 digits: both fingerprints, ordered by key. Identical for both sides.
pub fn safety_number(a: &Ed25519Key, b: &Ed25519Key) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    format!("{} {}", fingerprint(lo), fingerprint(hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_symmetry() {
        let fp = fingerprint(&[7; 32]);
        assert_eq!(fp.len(), 6 * 5 + 5);
        assert!(
            fp.split(' ')
                .all(|g| g.len() == 5 && g.bytes().all(|b| b.is_ascii_digit()))
        );
        assert_eq!(
            safety_number(&[1; 32], &[2; 32]),
            safety_number(&[2; 32], &[1; 32])
        );
        assert_ne!(fingerprint(&[1; 32]), fingerprint(&[2; 32]));
    }
}
