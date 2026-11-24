#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_engine::picture::{MAX_SHOWN, decode, is_image};

// ADR-044: anything at all goes in; out comes an error or a picture within
// the limits, never a panic, a hang or a huge allocation.
fuzz_target!(|data: &[u8]| {
    if let Ok(p) = decode(data) {
        assert!(is_image(data));
        assert!(p.width <= MAX_SHOWN && p.height <= MAX_SHOWN);
        assert_eq!(p.rgba.len(), p.width as usize * p.height as usize * 4);
    }
});
