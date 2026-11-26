#![no_main]

use libfuzzer_sys::fuzz_target;
use termchat_engine::markdown::{Block, Span, parse};
use termchat_proto::{MAX_TEXT_BYTES, is_unsafe_char};

fn checked(text: &str) -> usize {
    assert!(text.chars().all(|ch| ch == '\n' || !is_unsafe_char(ch)));
    text.len()
}

fuzz_target!(|text: &str| {
    let result = parse(text);
    assert_eq!(result.is_some(), text.len() <= MAX_TEXT_BYTES);
    let Some(blocks) = result else {
        return;
    };
    assert!(blocks.len() <= text.len() + 1);
    let mut bytes = 0;
    for block in blocks {
        let spans = match block {
            Block::Line { spans } | Block::Quote { spans } | Block::Bullet { spans } => spans,
            Block::Numbered { number, spans } => {
                bytes += checked(&number);
                spans
            }
            Block::Code { text } => {
                bytes += checked(&text);
                continue;
            }
        };
        for span in spans {
            bytes += match span {
                Span::Plain { text }
                | Span::Bold { text }
                | Span::Italic { text }
                | Span::Strike { text }
                | Span::Code { text } => checked(&text),
                Span::Link { label, address } => {
                    assert!(!address.is_empty());
                    checked(&label) + checked(&address)
                }
            };
        }
    }
    assert!(bytes <= text.len());
});
