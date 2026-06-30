//! SDK error types.

use thiserror::Error;

/// The error type for all SDK operations.
#[derive(Debug, Error)]
pub enum SdkError {
    // --- Transport ----------------------------------------------------------
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("connection refused: {0}")]
    ConnectionRefused(String),

    // --- JSON-RPC -----------------------------------------------------------
    #[error("JSON-RPC error {code}: {message}")]
    Rpc { code: i64, message: String },

    #[error("JSON-RPC parse error: {0}")]
    RpcParse(String),

    #[error("method not found: {0}")]
    MethodNotFound(String),

    // --- Serialization ------------------------------------------------------
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),

    #[error("ABI decode error: {0}")]
    Abi(String),

    #[error("RLP encode error: {0}")]
    Rlp(String),

    // --- Signing ------------------------------------------------------------
    #[error("invalid private key: {0}")]
    InvalidKey(String),

    #[error("signing error: {0}")]
    Signing(String),

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    // --- Transaction --------------------------------------------------------
    #[error("insufficient funds: need {need} have {have}")]
    InsufficientFunds { need: String, have: String },

    #[error("nonce too low: expected {expected} got {got}")]
    NonceTooLow { expected: u64, got: u64 },

    #[error("gas estimation failed: {0}")]
    GasEstimation(String),

    #[error("transaction reverted: {data}")]
    Reverted { data: String },

    #[error("transaction dropped from mempool")]
    TransactionDropped,

    // --- Contract -----------------------------------------------------------
    #[error("contract not deployed at {0:?}")]
    ContractNotDeployed(zbx_types::Address),

    #[error("function not found: {0}")]
    FunctionNotFound(String),

    // --- Timeout / Retry ----------------------------------------------------
    #[error("operation timed out after {secs}s")]
    Timeout { secs: u64 },

    #[error("max retries ({0}) exceeded")]
    MaxRetries(u32),

    // --- Misc ---------------------------------------------------------------
    #[error("URL parse error: {0}")]
    UrlParse(String),

    #[error("provider not connected")]
    NotConnected,

    #[error("{0}")]
    Other(String),
}

impl SdkError {
    pub fn rpc(code: i64, message: impl Into<String>) -> Self {
        Self::Rpc { code, message: message.into() }
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
    pub fn is_retryable(&self) -> bool {
        matches!(self,
            SdkError::Transport(_)
          | SdkError::WebSocket(_)
          | SdkError::ConnectionRefused(_)
          | SdkError::Timeout { .. }
        )
    }
}