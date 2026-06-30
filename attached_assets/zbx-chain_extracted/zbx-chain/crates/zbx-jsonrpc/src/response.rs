//! JSON-RPC 2.0 response types.

use crate::{request::JsonRpcId, error::RpcErrorObject};
use serde::{Deserialize, Serialize};

/// A single JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result:  Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error:   Option<RpcErrorObject>,
    pub id:      Option<JsonRpcId>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<JsonRpcId>, result: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0".into(), result: Some(result), error: None, id }
    }

    pub fn error(id: Option<JsonRpcId>, err: RpcErrorObject) -> Self {
        Self { jsonrpc: "2.0".into(), result: None, error: Some(err), id }
    }
}

/// A pub/sub notification.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method:  String,
    pub params:  NotificationParams,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationParams {
    pub subscription: String,
    pub result:       serde_json::Value,
}

impl JsonRpcNotification {
    pub fn new(method: &str, subscription: &str, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method:  method.to_string(),
            params:  NotificationParams {
                subscription: subscription.to_string(),
                result,
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::JsonRpcId;
    use serde_json::json;

    #[test]
    fn success_response_fields() {
        let r = JsonRpcResponse::success(Some(JsonRpcId::Number(1)), json!({"block": 100}));
        assert_eq!(r.jsonrpc, "2.0");
        assert!(r.result.is_some());
        assert!(r.error.is_none());
    }

    #[test]
    fn error_response_fields() {
        use crate::error::RpcErrorObject;
        let e = RpcErrorObject::method_not_found("eth_fake");
        let r = JsonRpcResponse::error(Some(JsonRpcId::Number(1)), e);
        assert!(r.result.is_none());
        assert!(r.error.is_some());
    }

    #[test]
    fn notification_has_correct_fields() {
        let n = JsonRpcNotification::new("eth_subscription", "sub-001", json!("0xdeadbeef"));
        assert_eq!(n.jsonrpc, "2.0");
        assert_eq!(n.method, "eth_subscription");
        assert_eq!(n.params.subscription, "sub-001");
    }

    #[test]
    fn success_serializes_without_error_field() {
        let r = JsonRpcResponse::success(None, json!(42));
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("\"error\""));
    }
}
