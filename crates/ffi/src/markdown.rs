use termchat_engine::markdown::{self, Block, Span};

#[derive(uniffi::Enum)]
pub enum MarkdownBlock {
    Line {
        spans: Vec<MarkdownSpan>,
    },
    Quote {
        spans: Vec<MarkdownSpan>,
    },
    Bullet {
        spans: Vec<MarkdownSpan>,
    },
    Numbered {
        number: String,
        spans: Vec<MarkdownSpan>,
    },
    Code {
        text: String,
    },
}

#[derive(uniffi::Enum)]
pub enum MarkdownSpan {
    Plain { text: String },
    Bold { text: String },
    Italic { text: String },
    Strike { text: String },
    Code { text: String },
    Link { label: String, address: String },
}

#[uniffi::export]
pub fn parse_markdown(text: String) -> Option<Vec<MarkdownBlock>> {
    Some(
        markdown::parse(&text)?
            .into_iter()
            .map(MarkdownBlock::from)
            .collect(),
    )
}

impl From<Block> for MarkdownBlock {
    fn from(block: Block) -> Self {
        fn convert(spans: Vec<Span>) -> Vec<MarkdownSpan> {
            spans.into_iter().map(MarkdownSpan::from).collect()
        }
        match block {
            Block::Line { spans } => Self::Line {
                spans: convert(spans),
            },
            Block::Quote { spans } => Self::Quote {
                spans: convert(spans),
            },
            Block::Bullet { spans } => Self::Bullet {
                spans: convert(spans),
            },
            Block::Numbered { number, spans } => Self::Numbered {
                number,
                spans: convert(spans),
            },
            Block::Code { text } => Self::Code { text },
        }
    }
}

impl From<Span> for MarkdownSpan {
    fn from(span: Span) -> Self {
        match span {
            Span::Plain { text } => Self::Plain { text },
            Span::Bold { text } => Self::Bold { text },
            Span::Italic { text } => Self::Italic { text },
            Span::Strike { text } => Self::Strike { text },
            Span::Code { text } => Self::Code { text },
            Span::Link { label, address } => Self::Link { label, address },
        }
    }
}
