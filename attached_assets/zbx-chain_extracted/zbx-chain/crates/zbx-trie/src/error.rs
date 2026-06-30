use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrieError {
    #[error("node not found in database: {0}")]
    MissingNode(String),

    #[error("RLP decode error: {0}")]
    RlpDecode(String),

    #[error("RLP encode error: {0}")]
    RlpEncode(String),

    #[error("invalid proof")]
    InvalidProof,

    #[error("key not found")]
    KeyNotFound,

    #[error("database error: {0}")]
    Database(String),

    #[error("inconsistent trie state")]
    Inconsistent,
}