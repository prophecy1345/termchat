#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_proto::Invite;

// Pasted by the user from anywhere.
fuzz_target!(|data: &str| {
    if let Ok(invite) = Invite::decode(data) {
        assert_eq!(Invite::decode(&invite.encode()).as_ref(), Ok(&invite));
    }
});
