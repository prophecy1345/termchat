#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::ServerFrame;

// What the client parses from the relay, which is only half trusted.
fuzz_target!(|data: &[u8]| {
    if let Ok(frame) = ServerFrame::decode(data) {
        assert_eq!(frame.encode(), data);
    }
});
