//! JSON-RPC request and response types.

use serde::{Deserialize, Serialize};
use crate::error::JsonRpcError;

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        JsonRpcResponse { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }

    pub fn error(id: serde_json::Value, err: JsonRpcError) -> Self {
        JsonRpcResponse { jsonrpc: "2.0".into(), id, result: None, error: Some(err) }
    }
}

/// Block tag parameter (latest / earliest / pending / hex number).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BlockTag {
    Name(String),
    Number(u64),
}

impl BlockTag {
    pub fn is_latest(&self) -> bool {
        matches!(self, BlockTag::Name(s) if s == "latest")
    }
}