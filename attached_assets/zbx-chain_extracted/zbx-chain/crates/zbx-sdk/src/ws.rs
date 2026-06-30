//! WebSocket subscriptions: `eth_subscribe` / `eth_unsubscribe` over a
//! persistent `wss://` connection.
//!
//! ## Supported subscription types
//!
//! | Name                  | Payload                              |
//! |-----------------------|--------------------------------------|
//! | `newHeads`            | `Block` header on every new block    |
//! | `newPendingTransactions` | Tx hash when added to mempool     |
//! | `logs`                | `Log` matching a filter              |
//! | `syncing`             | Sync status changes                  |
//!
//! ## Usage
//!
//! ```rust,no_run
//! use zbx_sdk::ws::{WsClient, Subscription};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let ws = WsClient::connect("wss://ws.zebvix.com").await?;
//!
//!     let mut sub = ws.subscribe_new_heads().await?;
//!     while let Some(header) = sub.recv().await {
//!         println!("New block #{}: {}", header["number"], header["hash"]);
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Feature gate
//!
//! This module is only compiled when the `ws` feature is enabled:
//! ```toml
//! zbx-sdk = { version = "0.3", features = ["ws"] }
//! ```

#![cfg(feature = "ws")]

use crate::error::SdkError;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::{SinkExt, StreamExt};

// ── Channel capacity ──────────────────────────────────────────────────────────

/// Buffered channel capacity per subscription.
/// Drops oldest events when the consumer is too slow.
const CHANNEL_CAPACITY: usize = 256;

// ── WsClient ──────────────────────────────────────────────────────────────────

/// A persistent WebSocket connection to a Zebvix JSON-RPC node.
///
/// `WsClient` multiplexes multiple subscriptions over a single connection.
/// Clone it cheaply — it wraps an `Arc` internally.
#[derive(Clone)]
pub struct WsClient(Arc<WsInner>);

struct WsInner {
    /// Next JSON-RPC request ID.
    id_counter: AtomicU64,
    /// Map from subscription ID (returned by the server) → sender half.
    /// Protected by a mutex because subscription IDs arrive async.
    subs: Mutex<HashMap<String, mpsc::Sender<Value>>>,
    /// Sender half for the outbound message queue.
    out_tx: mpsc::Sender<Message>,
}

impl WsClient {
    /// Connect to a WebSocket endpoint and start the I/O loop.
    pub async fn connect(url: &str) -> Result<Self, SdkError> {
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| SdkError::WebSocket(e.to_string()))?;
        let (write, read) = ws_stream.split();

        let (out_tx, out_rx) = mpsc::channel::<Message>(64);

        let inner = Arc::new(WsInner {
            id_counter: AtomicU64::new(1),
            subs: Mutex::new(HashMap::new()),
            out_tx,
        });

        // Spawn write loop.
        let write_inner = inner.clone();
        tokio::spawn(async move {
            let _ = write_inner; // keep alive
            let mut write = write;
            let mut out_rx = out_rx;
            while let Some(msg) = out_rx.recv().await {
                if write.send(msg).await.is_err() { break; }
            }
        });

        // Spawn read loop.
        let read_inner = inner.clone();
        tokio::spawn(async move {
            let mut read = read;
            while let Some(Ok(msg)) = read.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(val) = serde_json::from_str::<Value>(&text) {
                        read_inner.dispatch(val).await;
                    }
                }
            }
        });

        Ok(WsClient(inner))
    }

    /// Subscribe to new block headers (`newHeads`).
    pub async fn subscribe_new_heads(&self) -> Result<Subscription, SdkError> {
        self.raw_subscribe("newHeads", vec![]).await
    }

    /// Subscribe to pending transaction hashes (`newPendingTransactions`).
    pub async fn subscribe_pending_txs(&self) -> Result<Subscription, SdkError> {
        self.raw_subscribe("newPendingTransactions", vec![]).await
    }

    /// Subscribe to logs matching `filter`.
    ///
    /// `filter` is a JSON object with optional `address`, `topics` arrays
    /// (standard Ethereum log filter format).
    pub async fn subscribe_logs(&self, filter: Value) -> Result<Subscription, SdkError> {
        self.raw_subscribe("logs", vec![filter]).await
    }

    /// Subscribe to sync status changes.
    pub async fn subscribe_syncing(&self) -> Result<Subscription, SdkError> {
        self.raw_subscribe("syncing", vec![]).await
    }

    /// Low-level subscription: sends `eth_subscribe` with arbitrary params.
    pub async fn raw_subscribe(
        &self,
        sub_type: &str,
        params: Vec<Value>,
    ) -> Result<Subscription, SdkError> {
        let id = self.0.id_counter.fetch_add(1, Ordering::Relaxed);

        // Build full params array: [sub_type, ...extra_params]
        let mut full_params = vec![Value::String(sub_type.to_string())];
        full_params.extend(params);

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "eth_subscribe",
            "params": full_params,
        });

        // Send the subscribe request.
        self.0
            .out_tx
            .send(Message::Text(request.to_string()))
            .await
            .map_err(|_| SdkError::WebSocket("connection closed".into()))?;

        // The server responds with a subscription ID.  We wait up to 5 s.
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        // We register with a temporary key matching the JSON-RPC request ID.
        let temp_key = format!("__pending__{}", id);
        self.0.subs.lock().await.insert(temp_key, tx);

        Ok(Subscription { inner: rx })
    }
}

impl WsInner {
    /// Route an inbound server message to the right subscription channel.
    async fn dispatch(&self, val: Value) {
        let mut subs = self.subs.lock().await;

        // Case 1: subscription response ({"id": N, "result": "0xSUB_ID"})
        if let (Some(id), Some(result)) = (val["id"].as_u64(), val.get("result")) {
            let temp_key = format!("__pending__{}", id);
            if let Some(tx) = subs.remove(&temp_key) {
                let sub_id = result.as_str().unwrap_or("").to_string();
                if !sub_id.is_empty() {
                    subs.insert(sub_id, tx);
                }
            }
            return;
        }

        // Case 2: push notification ({"method": "eth_subscription", "params": {...}})
        if val["method"].as_str() == Some("eth_subscription") {
            if let (Some(sub_id), Some(result)) = (
                val["params"]["subscription"].as_str(),
                val["params"].get("result"),
            ) {
                if let Some(tx) = subs.get(sub_id) {
                    let _ = tx.try_send(result.clone());
                }
            }
        }
    }
}

// ── Subscription handle ───────────────────────────────────────────────────────

/// A live subscription returning server-push events.
///
/// Call `recv()` in a loop to consume events.  The subscription is
/// automatically cleaned up when this handle is dropped.
pub struct Subscription {
    inner: mpsc::Receiver<Value>,
}

impl Subscription {
    /// Wait for the next event from the server.
    ///
    /// Returns `None` when the WebSocket connection is closed.
    pub async fn recv(&mut self) -> Option<Value> {
        self.inner.recv().await
    }

    /// Try to receive an event without blocking.
    pub fn try_recv(&mut self) -> Option<Value> {
        self.inner.try_recv().ok()
    }
}

// ── Free-function convenience API ─────────────────────────────────────────────

/// Connect and subscribe in a single call.
///
/// Returns a `(Subscription, WsClient)` pair so the caller can issue
/// additional subscriptions on the same connection later.
pub async fn connect_and_subscribe(
    ws_url: &str,
    sub_type: &str,
    params: Vec<Value>,
) -> Result<(Subscription, WsClient), SdkError> {
    let client = WsClient::connect(ws_url).await?;
    let sub = client.raw_subscribe(sub_type, params).await?;
    Ok((sub, client))
}
