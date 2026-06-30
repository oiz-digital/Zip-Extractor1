//! Batch RPC: send multiple calls in a single HTTP request.

use crate::{error::SdkError, provider::Provider};
use serde_json::{json, Value};
use reqwest::Client;
use std::sync::atomic::{AtomicU64, Ordering};

/// Build and execute a batch of JSON-RPC calls.
///
/// ```rust,no_run
/// use zbx_sdk::batch::BatchRequest;
///
/// let mut batch = BatchRequest::new(provider.clone());
/// let block_num_id = batch.add("eth_blockNumber", json!([]));
/// let chain_id_id  = batch.add("eth_chainId",     json!([]));
/// let results = batch.send().await?;
/// ```
pub struct BatchRequest {
    url:      String,
    client:   Client,
    calls:    Vec<(u64, &'static str, Value)>,
    next_id:  AtomicU64,
}

impl BatchRequest {
    pub fn new_raw(url: impl Into<String>) -> Self {
        Self {
            url:     url.into(),
            client:  Client::new(),
            calls:   Vec::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Add a call. Returns the request ID used to match the response.
    pub fn add(&mut self, method: &'static str, params: Value) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.calls.push((id, method, params));
        id
    }

    /// Send all queued calls in one HTTP request. Returns id → result map.
    pub async fn send(self) -> Result<std::collections::HashMap<u64, Value>, SdkError> {
        let payload: Vec<Value> = self.calls.iter().map(|(id, method, params)| {
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        }).collect();

        let responses: Vec<Value> = self.client
            .post(&self.url)
            .json(&payload)
            .send().await?
            .json().await?;

        let mut map = std::collections::HashMap::new();
        for resp in responses {
            if let Some(id) = resp["id"].as_u64() {
                if let Some(err) = resp.get("error") {
                    let code = err["code"].as_i64().unwrap_or(-1);
                    let msg  = err["message"].as_str().unwrap_or("").to_string();
                    return Err(SdkError::rpc(code, msg));
                }
                map.insert(id, resp["result"].clone());
            }
        }
        Ok(map)
    }
}