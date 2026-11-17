#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::{Body, Inner};

fuzz_target!(|data: &[u8]| {
    if let Ok(inner) = Inner::from_padded(data) {
        match inner.body {
            // A kind from a newer version (ADR-025): read, never sent.
            Body::Unknown => assert!(inner.to_padded().is_err()),
            // Anything else accepted must re-pad to the same bytes.
            _ => assert_eq!(inner.to_padded().ok().as_deref(), Some(data)),
        }
    }
});
