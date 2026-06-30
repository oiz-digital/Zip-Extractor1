//! JSON-RPC 2.0 request types.

use serde::{Deserialize, Serialize};

/// JSON-RPC request ID (null, number, or string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Null,
    Number(i64),
    String(String),
}

/// A single JSON-RPC 2.0 request or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method:  String,
    #[serde(default)]
    pub params:  serde_json::Value,
    /// Absent for notifications.
    pub id:      Option<JsonRpcId>,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    pub fn validate(&self) -> bool {
        self.jsonrpc == "2.0" && !self.method.is_empty()
    }
}

/// Either a single request or a batch.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcCall {
    Single(JsonRpcRequest),
    Batch(Vec<JsonRpcRequest>),
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_req(method: &str, id: Option<JsonRpcId>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method:  method.to_string(),
            params:  json!(null),
            id,
        }
    }

    #[test]
    fn validate_passes_good_request() {
        let r = make_req("eth_blockNumber", Some(JsonRpcId::Number(1)));
        assert!(r.validate());
    }

    #[test]
    fn validate_fails_wrong_version() {
        let mut r = make_req("eth_blockNumber", Some(JsonRpcId::Number(1)));
        r.jsonrpc = "1.0".into();
        assert!(!r.validate());
    }

    #[test]
    fn validate_fails_empty_method() {
        let r = make_req("", Some(JsonRpcId::Number(1)));
        assert!(!r.validate());
    }

    #[test]
    fn notification_has_no_id() {
        let r = make_req("eth_subscribe", None);
        assert!(r.is_notification());
    }

    #[test]
    fn request_with_id_is_not_notification() {
        let r = make_req("eth_blockNumber", Some(JsonRpcId::Number(42)));
        assert!(!r.is_notification());
    }

    #[test]
    fn id_types_serialize_correctly() {
        assert_eq!(
            serde_json::to_value(JsonRpcId::Number(1)).unwrap(),
            json!(1)
        );
        assert_eq!(
            serde_json::to_value(JsonRpcId::String("abc".into())).unwrap(),
            json!("abc")
        );
    }
}
