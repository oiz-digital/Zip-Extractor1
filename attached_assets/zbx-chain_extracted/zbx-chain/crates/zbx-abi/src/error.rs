use thiserror::Error;

#[derive(Debug, Error)]
pub enum AbiError {
    #[error("unsupported type: {0}")]
    UnsupportedType(String),

    #[error("encode error: {0}")]
    Encode(String),

    #[error("decode error: unexpected end of data")]
    UnexpectedEnd,

    #[error("decode error: bad bool value")]
    BadBool,

    #[error("decode error: integer overflow")]
    Overflow,

    #[error("invalid UTF-8 in string")]
    InvalidUtf8,

    #[error("buffer too short: need {need}, have {have}")]
    BufferTooShort { need: usize, have: usize },

    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch { expected: String, got: String },

    #[error("json parse error: {0}")]
    JsonParse(String),

    #[error("bad selector: expected 4 bytes")]
    BadSelector,
}