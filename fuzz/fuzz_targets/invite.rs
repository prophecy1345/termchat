#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::Invite;

// Pasted by the user from anywhere: an invite or a device's link code.
fuzz_target!(|data: &str| {
    if let Ok(invite) = Invite::decode(data) {
        assert_eq!(Invite::decode(&invite.encode()).as_ref(), Ok(&invite));
    }
    if let Ok(link) = Invite::decode_link(data) {
        assert_eq!(Invite::decode_link(&link.encode_link()).as_ref(), Ok(&link));
    }
});
