use thiserror::Error;
use serde::{Deserialize, Serialize};

/// JSON-RPC error codes (Ethereum standard + Zebvix extensions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum RpcErrorCode {
    ParseError      = -32700,
    InvalidRequest  = -32600,
    MethodNotFound  = -32601,
    InvalidParams   = -32602,
    InternalError   = -32603,
    // Ethereum application layer
    ExecutionError  = 3,
    // Zebvix-specific
    BlockNotFound   = -39001,
    TxNotFound      = -39002,
    NotSynced       = -39003,
}

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("method not found: {0}")]
    MethodNotFound(String),

    #[error("invalid params: {0}")]
    InvalidParams(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("block not found")]
    BlockNotFound,

    #[error("transaction not found: {0}")]
    TxNotFound(String),

    #[error("node not synced")]
    NotSynced,

    /// EVM execution error (eth_call / eth_estimateGas reverts).
    /// Maps to Ethereum application-layer error code 3.
    #[error("execution error: {0}")]
    Execution(String),
}

/// JSON-RPC 2.0 response error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl From<RpcError> for JsonRpcError {
    fn from(e: RpcError) -> Self {
        let (code, msg) = match &e {
            RpcError::Parse(m)          => (RpcErrorCode::ParseError as i32, m.clone()),
            RpcError::InvalidRequest(m) => (RpcErrorCode::InvalidRequest as i32, m.clone()),
            RpcError::MethodNotFound(m) => (RpcErrorCode::MethodNotFound as i32, m.clone()),
            RpcError::InvalidParams(m)  => (RpcErrorCode::InvalidParams as i32, m.clone()),
            RpcError::Internal(m)       => (RpcErrorCode::InternalError as i32, m.clone()),
            RpcError::BlockNotFound     => (RpcErrorCode::BlockNotFound as i32, "block not found".into()),
            RpcError::TxNotFound(m)     => (RpcErrorCode::TxNotFound as i32, m.clone()),
            RpcError::NotSynced         => (RpcErrorCode::NotSynced as i32, "node not synced".into()),
            RpcError::Execution(m)      => (RpcErrorCode::ExecutionError as i32, m.clone()),
        };
        JsonRpcError { code, message: msg, data: None }
    }
}