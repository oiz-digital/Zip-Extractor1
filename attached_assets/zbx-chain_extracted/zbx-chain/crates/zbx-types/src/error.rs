use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZbxError {
    #[error("invalid hex string: {0}")]
    InvalidHex(String),

    #[error("invalid length: expected {expected} bytes, got {got}")]
    InvalidLength { expected: usize, got: usize },

    #[error("RLP decode error: {0}")]
    RlpDecode(String),

    #[error("signature error: {0}")]
    Signature(String),

    #[error("invalid address: {0}")]
    InvalidAddress(String),

    #[error("arithmetic overflow")]
    Overflow,

    #[error("invalid transaction: {0}")]
    InvalidTransaction(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("consensus error: {0}")]
    Consensus(String),
}