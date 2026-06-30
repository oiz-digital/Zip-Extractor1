//! Method router: maps method names to async handler functions.

use crate::{
    error::{RpcError, RpcErrorObject},
    request::JsonRpcRequest,
};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

/// Alias for an async RPC handler's return type.
pub type HandlerFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, RpcErrorObject>> + Send>>;

pub type RpcHandler = Arc<dyn Fn(JsonRpcRequest) -> HandlerFuture + Send + Sync>;

/// Method router: registry of RPC method handlers.
#[derive(Clone)]
pub struct RpcRouter {
    methods: HashMap<String, RpcHandler>,
}

impl RpcRouter {
    pub fn new() -> Self {
        Self { methods: HashMap::new() }
    }

    /// Register a method handler.
    pub fn method<F, Fut>(mut self, name: &str, handler: F) -> Self
    where
        F: Fn(JsonRpcRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, RpcErrorObject>> + Send + 'static,
    {
        let handler: RpcHandler = Arc::new(move |req| Box::pin(handler(req)));
        self.methods.insert(name.to_string(), handler);
        self
    }

    /// Dispatch a single request.
    pub async fn dispatch(
        &self,
        req: JsonRpcRequest,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        match self.methods.get(&req.method) {
            Some(handler) => handler(req).await,
            None => Err(RpcErrorObject::method_not_found(&req.method)),
        }
    }

    /// List all registered methods.
    pub fn method_names(&self) -> Vec<&str> {
        self.methods.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for RpcRouter {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{error::RpcErrorObject, request::{JsonRpcId, JsonRpcRequest}};
    use serde_json::json;

    fn make_req(method: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method:  method.to_string(),
            params:  json!(null),
            id:      Some(JsonRpcId::Number(1)),
        }
    }

    fn make_router() -> RpcRouter {
        RpcRouter::new().method("ping", |_req| async move {
            Ok(json!("pong"))
        })
    }

    #[tokio::test]
    async fn registered_method_dispatches() {
        let router = make_router();
        let result = router.dispatch(make_req("ping")).await.unwrap();
        assert_eq!(result, json!("pong"));
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let router = make_router();
        let err = router.dispatch(make_req("unknown")).await.unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn method_names_lists_registered() {
        let router = make_router();
        let names = router.method_names();
        assert!(names.contains(&"ping"));
    }

    #[test]
    fn default_router_is_empty() {
        let router = RpcRouter::default();
        assert!(router.method_names().is_empty());
    }
}
