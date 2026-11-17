#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::{GroupCiphertext, GroupInner};

// ADR-027: the two parsers of a group message.
fuzz_target!(|data: &[u8]| {
    if let Ok(c) = GroupCiphertext::decode(data) {
        // Anything accepted must encode back to the same bytes.
        assert_eq!(c.encode(), data);
    }
    if let Ok(inner) = GroupInner::from_padded(data) {
        assert_eq!(inner.to_padded().ok().as_deref(), Some(data));
    }
});
