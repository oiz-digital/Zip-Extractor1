use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum RlpError {
    #[error("unexpected end of input at byte {0}")]
    UnexpectedEnd(usize),

    #[error("invalid leading byte: 0x{0:02x}")]
    InvalidLeadingByte(u8),

    #[error("non-canonical (unnecessary leading zeroes)")]
    NonCanonical,

    #[error("decoded length {got} does not match expected {expected}")]
    LengthMismatch { expected: usize, got: usize },

    #[error("list item count mismatch")]
    ItemCountMismatch,

    #[error("integer overflow in length field")]
    Overflow,

    #[error("expected string, got list")]
    ExpectedString,

    #[error("expected list, got string")]
    ExpectedList,

    #[error("custom: {0}")]
    Custom(String),
}