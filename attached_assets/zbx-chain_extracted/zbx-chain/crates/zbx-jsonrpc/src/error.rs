use serde::{Deserialize, Serialize};
use thiserror::Error;

/// JSON-RPC 2.0 error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Invalid JSON.
    ParseError     = -32700,
    /// Request is not a valid RPC call.
    InvalidRequest = -32600,
    /// Method does not exist.
    MethodNotFound = -32601,
    /// Invalid method parameters.
    InvalidParams  = -32602,
    /// Internal JSON-RPC error.
    InternalError  = -32603,
    /// Server error range: -32099 to -32000.
    ServerError    = -32000,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcErrorObject {
    pub code:    i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data:    Option<serde_json::Value>,
}

impl std::fmt::Display for RpcErrorObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl RpcErrorObject {
    pub fn parse_error()      -> Self { Self { code: -32700, message: "Parse error".into(), data: None } }
    pub fn invalid_request()  -> Self { Self { code: -32600, message: "Invalid Request".into(), data: None } }
    pub fn method_not_found(m: &str) -> Self {
        Self { code: -32601, message: format!("Method not found: {}", m), data: None }
    }
    pub fn invalid_params(msg: &str) -> Self {
        Self { code: -32602, message: format!("Invalid params: {}", msg), data: None }
    }
    pub fn internal_error(msg: &str) -> Self {
        Self { code: -32603, message: format!("Internal error: {}", msg), data: None }
    }
    pub fn server_error(code: i64, msg: &str) -> Self {
        Self { code, message: msg.to_string(), data: None }
    }
    pub fn execution_reverted(reason: &str) -> Self {
        Self {
            code: 3,
            message: "execution reverted".to_string(),
            data: Some(serde_json::Value::String(format!("0x{}", hex::encode(reason)))),
        }
    }
}

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("{0}")]
    Rpc(RpcErrorObject),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        assert_eq!(RpcErrorObject::parse_error().code, -32700);
        assert_eq!(RpcErrorObject::invalid_request().code, -32600);
        assert_eq!(RpcErrorObject::method_not_found("x").code, -32601);
        assert_eq!(RpcErrorObject::invalid_params("x").code, -32602);
        assert_eq!(RpcErrorObject::internal_error("x").code, -32603);
    }

    #[test]
    fn method_not_found_includes_name() {
        let e = RpcErrorObject::method_not_found("zbx_fake");
        assert!(e.message.contains("zbx_fake"));
    }

    #[test]
    fn server_error_custom_code() {
        let e = RpcErrorObject::server_error(-32050, "rate limited");
        assert_eq!(e.code, -32050);
        assert_eq!(e.message, "rate limited");
    }

    #[test]
    fn execution_reverted_has_code_3() {
        let e = RpcErrorObject::execution_reverted("out of gas");
        assert_eq!(e.code, 3);
        assert!(e.data.is_some());
    }

    #[test]
    fn display_includes_code_and_message() {
        let e = RpcErrorObject::parse_error();
        let s = format!("{}", e);
        assert!(s.contains("-32700"));
    }

    #[test]
    fn no_data_field_is_none() {
        let e = RpcErrorObject::parse_error();
        assert!(e.data.is_none());
    }
}
