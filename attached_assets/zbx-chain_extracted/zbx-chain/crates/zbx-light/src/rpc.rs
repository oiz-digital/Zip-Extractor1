//! Light client RPC: expose chain data to local applications.

use crate::{
    header_chain::LightHeader,
    spv::{AccountProof, StorageProof, TxProof},
    sync::LightSync,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde_json::{json, Value};

/// Light RPC server (minimal JSON-RPC subset).
pub struct LightRpc {
    sync: Arc<RwLock<LightSync>>,
}

impl LightRpc {
    pub fn new(sync: Arc<RwLock<LightSync>>) -> Self {
        Self { sync }
    }

    pub async fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "zbx_blockNumber" => {
                let s = self.sync.read().await;
                json!(format!("0x{:x}", s.header_chain().tip_number()))
            }
            "zbx_getHeaderByNumber" => {
                let num_str = params[0].as_str().unwrap_or("latest");
                let s = self.sync.read().await;
                if let Some(h) = s.header_chain().tip() {
                    json!({
                        "number": h.number,
                        "hash": format!("{:?}", h.hash),
                        "parentHash": format!("{:?}", h.parent_hash),
                        "stateRoot": format!("{:?}", h.state_root),
                        "timestamp": h.timestamp,
                        "finalized": h.finalized,
                    })
                } else {
                    json!(null)
                }
            }
            "zbx_syncing" => {
                json!({ "currentBlock": "0x0", "highestBlock": "0x0" })
            }
            _ => json!({ "error": "method not found" }),
        }
    }
}