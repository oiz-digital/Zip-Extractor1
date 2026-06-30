use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("SSZ decode error: {0}")]
    SszDecode(String),
    #[error("Borsh decode error: {0}")]
    BorshDecode(String),
    #[error("SCALE decode error: {0}")]
    ScaleDecode(String),
    #[error("RLP decode error: {0}")]
    RlpDecode(String),
    #[error("buffer too short: need {need}, got {got}")]
    BufferTooShort { need: usize, got: usize },
    #[error("invalid enum variant: {0}")]
    InvalidVariant(u8),
    #[error("length limit exceeded: max {max}, got {got}")]
    LengthExceeded { max: usize, got: usize },
}