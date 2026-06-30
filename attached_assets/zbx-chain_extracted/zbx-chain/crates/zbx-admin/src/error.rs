//! Admin crate errors.

use thiserror::Error;
use zbx_types::address::Address;

#[derive(Debug, Error)]
pub enum AdminError {
    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("permission denied: operation '{0}' requires role '{1}'")]
    PermissionDenied(String, String),

    #[error("validator not found: {0:?}")]
    ValidatorNotFound(Address),

    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("operation timed out after {0}s")]
    Timeout(u64),

    #[error("node is syncing — operation unavailable")]
    NodeSyncing,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}