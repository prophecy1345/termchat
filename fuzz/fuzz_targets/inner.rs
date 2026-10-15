#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::Inner;

fuzz_target!(|data: &[u8]| {
    if let Ok(inner) = Inner::from_padded(data) {
        // Anything accepted must re-pad to the same bytes.
        assert_eq!(inner.to_padded().ok().as_deref(), Some(data));
    }
});
