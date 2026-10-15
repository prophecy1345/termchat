#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::ClientFrame;

// What the relay parses from every connection.
fuzz_target!(|data: &[u8]| {
    if let Ok(frame) = ClientFrame::decode(data) {
        assert_eq!(frame.encode(), data);
    }
});
