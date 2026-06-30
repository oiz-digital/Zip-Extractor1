//! GraphQL API error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphqlError {
    #[error("block not found: {0}")]
    BlockNotFound(String),

    #[error("transaction not found: {0}")]
    TxNotFound(String),

    #[error("account not found: {0}")]
    AccountNotFound(String),

    #[error("validator not found: {0}")]
    ValidatorNotFound(String),

    #[error("invalid address '{0}': {1}")]
    InvalidAddress(String, String),

    #[error("invalid hash '{0}': {1}")]
    InvalidHash(String, String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("internal error: {0}")]
    Internal(String),
}
