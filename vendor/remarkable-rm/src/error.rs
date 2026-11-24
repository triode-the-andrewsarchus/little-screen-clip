use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("file truncated: needed {needed} bytes, got {got}")]
    Truncated { needed: usize, got: usize },

    #[error("invalid magic header: not a v6 .rm file")]
    BadMagic,

    #[error("unsupported file version: {0}")]
    UnsupportedVersion(u8),

    #[error("unknown tag type {tag_type:#x} at index {index}")]
    UnknownTagType { index: u32, tag_type: u8 },

    #[error(
        "unexpected tag: expected (index={expected_index}, type={expected_type:#x}), \
         got (index={got_index}, type={got_type:#x})"
    )]
    UnexpectedTag {
        expected_index: u32,
        expected_type: u8,
        got_index: u32,
        got_type: u8,
    },

    #[error("varuint overflow: encoded value exceeds u64 range")]
    VarUIntOverflow,

    #[error("subblock length mismatch: declared {declared}, consumed {consumed}")]
    SubBlockLengthMismatch { declared: u32, consumed: u32 },

    #[error("invalid pen value: {0}")]
    InvalidPen(u32),

    #[error("invalid pen color: {0}")]
    InvalidPenColor(u32),

    #[error("invalid paragraph style: {0}")]
    InvalidParagraphStyle(u8),

    #[error("invalid inline format code: {0}")]
    InvalidInlineFormat(u32),

    #[error("invalid text CRDT: {0}")]
    InvalidText(String),

    #[error("invalid UTF-8 in string field")]
    InvalidUtf8,

    #[error("invalid block: {0}")]
    InvalidBlock(&'static str),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
