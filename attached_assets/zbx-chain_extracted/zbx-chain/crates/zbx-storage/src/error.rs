use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(String),

    #[error("block not found at height {0}")]
    BlockNotFound(u64),

    #[error("block hash not found: {0}")]
    HashNotFound(String),

    #[error("transaction not found: {0}")]
    TxNotFound(String),

    #[error("encode error: {0}")]
    Encode(String),

    #[error("decode error: {0}")]
    Decode(String),

    #[error("schema version mismatch: expected {expected}, got {got}")]
    SchemaMismatch { expected: u32, got: u32 },
}