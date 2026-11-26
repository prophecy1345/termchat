//! ADR-048: display-only Markdown shared by the three clients.

#![forbid(unsafe_code)]

use crate::engine::clean_text;
use termchat_proto::MAX_TEXT_BYTES;

#[derive(Debug, PartialEq, Eq)]
pub enum Block {
    Line { spans: Vec<Span> },
    Quote { spans: Vec<Span> },
    Bullet { spans: Vec<Span> },
    Numbered { number: String, spans: Vec<Span> },
    Code { text: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Span {
    Plain { text: String },
    Bold { text: String },
    Italic { text: String },
    Strike { text: String },
    Code { text: String },
    Link { label: String, address: String },
}

const FENCE: &str = "```";

/// Parse a display copy of the message. On `None`, show it via `clean_text`.
pub fn parse(text: &str) -> Option<Vec<Block>> {
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    let text = clean_text(text);
    let mut fences = text.split('\n').filter(|line| *line == FENCE).count();
    let mut lines = text.split('\n');
    let mut blocks = Vec::new();
    while let Some(line) = lines.next() {
        if line == FENCE && fences >= 2 {
            fences -= 2;
            let code = lines.by_ref().take_while(|line| *line != FENCE);
            blocks.push(Block::Code {
                text: code.collect::<Vec<_>>().join("\n"),
            });
        } else {
            blocks.push(block(line));
        }
    }
    Some(blocks)
}

fn list_prefix(line: &str) -> Option<(&str, &str)> {
    if let Some(body) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(("", body));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    Some((&line[..digits], line[digits..].strip_prefix(". ")?))
}

fn block(line: &str) -> Block {
    if let Some(body) = line.strip_prefix("> ") {
        return Block::Quote {
            spans: inline(body),
        };
    }
    if let Some((number, body)) = list_prefix(line)
        && list_prefix(body).is_none()
    {
        let spans = inline(body);
        return if number.is_empty() {
            Block::Bullet { spans }
        } else {
            Block::Numbered {
                number: number.to_owned(),
                spans,
            }
        };
    }
    Block::Line {
        spans: inline(line),
    }
}

#[derive(Clone, Copy)]
enum Kind {
    // The two italic markers have separate closing positions.
    Mark(Mark),
    Escape,
    Label,
    Image,
    EndLabel,
    Paren,
    EndParen,
    BareAddress,
}

#[derive(Clone, Copy)]
enum Mark {
    Bold,
    Star,
    Underscore,
    Strike,
    Code,
}

struct Token {
    kind: Kind,
    start: usize,
    end: usize,
    close: Option<usize>,
}

fn tokens(line: &str) -> Vec<Token> {
    let mut chars = line.char_indices().peekable();
    let mut tokens = Vec::new();
    while let Some((start, ch)) = chars.next() {
        let rest = &line[start..];
        let kind = match ch {
            '*' | '_' | '~' | '`' => {
                let mut count = 1;
                while chars.next_if(|&(_, next)| next == ch).is_some() {
                    count += 1;
                }
                let mark = match (ch, count) {
                    ('*', 2) => Mark::Bold,
                    ('*', 1) => Mark::Star,
                    ('_', 1) => {
                        let before = line[..start].chars().next_back();
                        let after = chars.peek().map(|&(_, next)| next);
                        if before.is_some_and(char::is_alphanumeric)
                            && after.is_some_and(char::is_alphanumeric)
                        {
                            continue;
                        }
                        Mark::Underscore
                    }
                    ('~', 2) => Mark::Strike,
                    ('`', 1) => Mark::Code,
                    _ => continue,
                };
                Kind::Mark(mark)
            }
            // As in CommonMark, so `C:\Users` and `\d+` keep their backslash.
            '\\' if chars
                .next_if(|&(_, next)| next.is_ascii_punctuation())
                .is_some() =>
            {
                Kind::Escape
            }
            '!' if rest.starts_with("![") => {
                chars.next();
                Kind::Image
            }
            '[' => Kind::Label,
            ']' => Kind::EndLabel,
            '(' => Kind::Paren,
            ')' => Kind::EndParen,
            ':' if rest.starts_with("://")
                && line[..start].ends_with(|ch: char| ch.is_ascii_alphanumeric()) =>
            {
                Kind::BareAddress
            }
            '<' if rest.starts_with("<!--") => {
                chars.nth(2);
                while let Some((at, ch)) = chars.next() {
                    if ch == '-' && line[at..].starts_with("-->") {
                        chars.nth(1);
                        break;
                    }
                }
                continue;
            }
            '<' if rest.as_bytes().get(1).is_some_and(|&next| {
                next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?')
            }) =>
            {
                // Leave the entire literal tag together, including any quoted >.
                let mut quote = None;
                for (_, ch) in chars.by_ref() {
                    match quote {
                        Some(q) if ch == q => quote = None,
                        None if matches!(ch, '\'' | '"') => quote = Some(ch),
                        None if ch == '>' => break,
                        _ => {}
                    }
                }
                continue;
            }
            _ => continue,
        };
        tokens.push(Token {
            kind,
            start,
            end: chars.peek().map_or(line.len(), |&(next, _)| next),
            close: None,
        });
    }
    tokens
}

fn pair(tokens: &mut [Token]) {
    let mut marks = [None; 5];
    let mut labels = Vec::new();
    let mut parens = Vec::new();
    // Record closing positions once, including on lines with many failed openers.
    for (i, token) in tokens.iter_mut().enumerate().rev() {
        token.close = match token.kind {
            Kind::Mark(mark) => marks[mark as usize].replace(i),
            Kind::Label | Kind::Image => labels.pop(),
            Kind::EndLabel => {
                labels.push(i);
                None
            }
            Kind::Paren => parens.pop(),
            Kind::EndParen => {
                parens.push(i);
                None
            }
            Kind::Escape | Kind::BareAddress => None,
        };
    }
}

fn unescape(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::with_capacity(text.len());
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && let Some(next) = chars.next_if(char::is_ascii_punctuation)
        {
            out.push(next);
        } else {
            out.push(ch);
        }
    }
    out
}

fn inline(line: &str) -> Vec<Span> {
    let mut tokens = tokens(line);
    pair(&mut tokens);
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut start = 0;
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        i += 1;
        let (span, end) = match (token.kind, token.close) {
            (Kind::Escape, _) => {
                plain.push_str(&line[start..token.start]);
                plain.push_str(&line[token.start + 1..token.end]);
                start = token.end;
                continue;
            }
            (Kind::Mark(mark), Some(end)) if token.end < tokens[end].start => {
                let text = &line[token.end..tokens[end].start];
                let span = match mark {
                    Mark::Bold => Span::Bold {
                        text: unescape(text),
                    },
                    Mark::Star | Mark::Underscore => Span::Italic {
                        text: unescape(text),
                    },
                    Mark::Strike => Span::Strike {
                        text: unescape(text),
                    },
                    Mark::Code => Span::Code {
                        text: text.to_owned(),
                    },
                };
                (span, end)
            }
            (Kind::Label | Kind::Image, Some(end)) => {
                i = end + 1;
                let Some(open) = tokens.get(i).filter(|open| {
                    matches!(open.kind, Kind::Paren) && open.start == tokens[end].end
                }) else {
                    continue;
                };
                let Some(close) = open.close else {
                    break;
                };
                i = close + 1;
                let address = &line[open.end..tokens[close].start];
                if matches!(token.kind, Kind::Image)
                    || token.end == tokens[end].start
                    || address.is_empty()
                    || address.chars().any(char::is_whitespace)
                {
                    continue;
                }
                let span = Span::Link {
                    label: unescape(&line[token.end..tokens[end].start]),
                    address: unescape(address),
                };
                (span, close)
            }
            (Kind::BareAddress, _) => {
                let end = line[token.start..]
                    .find(char::is_whitespace)
                    .map_or(line.len(), |offset| token.start + offset);
                while tokens.get(i).is_some_and(|next| next.start < end) {
                    i += 1;
                }
                continue;
            }
            _ => continue,
        };
        plain.push_str(&line[start..token.start]);
        if !plain.is_empty() {
            spans.push(Span::Plain {
                text: std::mem::take(&mut plain),
            });
        }
        spans.push(span);
        i = end + 1;
        start = tokens[end].end;
    }
    plain.push_str(&line[start..]);
    if !plain.is_empty() {
        spans.push(Span::Plain { text: plain });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Span {
        Span::Plain { text: text.into() }
    }

    #[test]
    fn inline_styles_and_full_link_address() {
        let input = r"**жирный** *star* _under_ ~~strike~~ `**x**\y` [сайт](https://example.org/wiki/A_(B)?x=1#part)";
        assert_eq!(
            parse(input),
            Some(vec![Block::Line {
                spans: vec![
                    Span::Bold {
                        text: "жирный".into()
                    },
                    plain(" "),
                    Span::Italic {
                        text: "star".into()
                    },
                    plain(" "),
                    Span::Italic {
                        text: "under".into()
                    },
                    plain(" "),
                    Span::Strike {
                        text: "strike".into()
                    },
                    plain(" "),
                    Span::Code {
                        text: r"**x**\y".into()
                    },
                    plain(" "),
                    Span::Link {
                        label: "сайт".into(),
                        address: "https://example.org/wiki/A_(B)?x=1#part".into(),
                    },
                ],
            }])
        );
        assert_eq!(
            inline(r"[a\]b](file:///a\)b)"),
            vec![Span::Link {
                label: "a]b".into(),
                address: "file:///a)b".into()
            }]
        );
    }

    #[test]
    fn blocks_and_line_breaks() {
        let input = "> **quote**\n- one\n* two\n007. numbered\n```\n- `raw`\n\nlast\n```\n";
        assert_eq!(
            parse(input),
            Some(vec![
                Block::Quote {
                    spans: vec![Span::Bold {
                        text: "quote".into()
                    }]
                },
                Block::Bullet {
                    spans: vec![plain("one")]
                },
                Block::Bullet {
                    spans: vec![plain("two")]
                },
                Block::Numbered {
                    number: "007".into(),
                    spans: vec![plain("numbered")]
                },
                Block::Code {
                    text: "- `raw`\n\nlast".into()
                },
                Block::Line { spans: vec![] },
            ])
        );
        assert_eq!(
            parse("```\n**still text with Markdown**"),
            Some(vec![
                Block::Line {
                    spans: vec![plain("```")]
                },
                Block::Line {
                    spans: vec![Span::Bold {
                        text: "still text with Markdown".into()
                    }]
                },
            ])
        );
    }

    #[test]
    fn unsupported_and_unmatched_markup_is_literal() {
        for input in [
            "**unclosed",
            "~~unclosed",
            "[unclosed](example.org/**literal**",
            "[empty]()",
            "[space](not a **URL**)",
            "![**remote**](https://example.org/image.webp)",
            "<img alt='**remote**' src='https://example.org/image.webp'>",
            "<img alt='> [example](https://example.org) **text**'>",
            "<!-- a > **literal** -->",
            "<!-- [example](https://example.org)",
            "https://example.org/*raw*",
            "snake_case_with_underscores",
            "часть_один_два",
            "# heading",
            "| table |",
            "[^footnote]",
            "||spoiler||",
            "  - nested",
            "- - nested",
        ] {
            assert_eq!(
                parse(input),
                Some(vec![Block::Line {
                    spans: vec![plain(input)]
                }]),
                "{input}"
            );
        }
        assert_eq!(
            inline(r"\*escaped\* \🪶 \\ C:\Users\me **\d+\***"),
            vec![
                plain(r"*escaped* \🪶 \ C:\Users\me "),
                Span::Bold {
                    text: r"\d+*".into()
                },
            ]
        );
        assert_eq!(
            inline("**one**_two_"),
            vec![
                Span::Bold { text: "one".into() },
                Span::Italic { text: "two".into() },
            ]
        );
    }

    #[test]
    fn sanitizes_output_and_limits_input_bytes() {
        assert_eq!(
            parse("**a\u{1b}b** [x\u{202e}](https://example.org/\u{2066}y)"),
            Some(vec![Block::Line {
                spans: vec![
                    Span::Bold { text: "ab".into() },
                    plain(" "),
                    Span::Link {
                        label: "x".into(),
                        address: "https://example.org/y".into()
                    },
                ],
            }])
        );
        let mut input = "я".repeat(MAX_TEXT_BYTES / 2);
        assert!(parse(&input).is_some());
        input.push('\u{1b}');
        assert!(parse(&input).is_none());
    }

    #[test]
    fn long_runs_and_failed_link_openers() {
        for input in [
            "*".repeat(MAX_TEXT_BYTES),
            "[x](".repeat(MAX_TEXT_BYTES / 4),
        ] {
            assert_eq!(
                parse(&input),
                Some(vec![Block::Line {
                    spans: vec![plain(&input)]
                }])
            );
        }
    }
}
