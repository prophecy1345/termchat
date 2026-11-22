//! ADR-039: any webhook body, in any format, gives text within the limits or nothing.
#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat::alert::{Format, MAX_CHARS, render};

fuzz_target!(|data: &[u8]| {
    let labels = ["job".to_string(), "severity".to_string()];
    for format in [Format::Auto, Format::Alertmanager, Format::Kuma, Format::Text] {
        if let Some(text) = render(format, data, &labels) {
            assert!(text.chars().count() <= MAX_CHARS);
            assert!(!text.trim().is_empty());
            assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
        }
    }
});
