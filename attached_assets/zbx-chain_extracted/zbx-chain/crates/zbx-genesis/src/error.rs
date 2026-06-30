use thiserror::Error;

#[derive(Debug, Error)]
pub enum GenesisError {
    #[error("invalid genesis: {0}")]
    Invalid(String),
    #[error("allocation overflow for address {addr}: balance {balance}")]
    AllocationOverflow { addr: String, balance: String },
    #[error("duplicate allocation for address {0}")]
    DuplicateAllocation(String),
    #[error("state root mismatch: expected={expected}, got={got}")]
    StateRootMismatch { expected: String, got: String },
    #[error("genesis hash mismatch: expected={expected}, got={got}")]
    GenesisHashMismatch { expected: String, got: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}