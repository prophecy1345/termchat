#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::Envelope;

fuzz_target!(|data: &[u8]| {
    if let Ok(env) = Envelope::decode(data) {
        // Anything accepted must re-encode to the same bytes.
        assert_eq!(env.encode(), data);
    }
});
