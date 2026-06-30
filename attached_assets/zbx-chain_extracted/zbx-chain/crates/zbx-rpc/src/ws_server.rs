//! WebSocket JSON-RPC server for ZBX Chain.
//!
//! Runs on a separate TCP port (default 8546) alongside the HTTP server.
//! Clients connect via `ws://host:8546` and send standard JSON-RPC 2.0
//! requests.  Subscription events are pushed as `eth_subscription`
//! notifications.
//!
//! Supported subscriptions (eth_subscribe):
//!   - "newHeads"               — new block headers as they are sealed
//!   - "newPendingTransactions" — hashes of newly-accepted mempool txs
//!   - "logs"                   — EVM log events matching an optional filter
//!
//! # Wire protocol
//!
//! ```text
//! Client → Server:
//!   { "jsonrpc":"2.0", "id":1, "method":"eth_subscribe", "params":["newHeads"] }
//!
//! Server → Client (subscription confirmation):
//!   { "jsonrpc":"2.0", "id":1, "result":"0x0000000000000001" }
//!
//! Server → Client (push notification):
//!   { "jsonrpc":"2.0", "method":"eth_subscription",
//!     "params":{ "subscription":"0x0000000000000001", "result": { ...header... } } }
//! ```

use crate::{
    error::{JsonRpcError, RpcError},
    state::RpcState,
    types::{JsonRpcRequest, JsonRpcResponse},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{debug, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Subscription ID counter — monotonically increasing, hex-formatted.
// ─────────────────────────────────────────────────────────────────────────────

static SUB_COUNTER: AtomicU64 = AtomicU64::new(1);

fn new_sub_id() -> String {
    let n = SUB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("0x{n:016x}")
}

// ─────────────────────────────────────────────────────────────────────────────
// Subscription type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum SubType {
    NewHeads,
    NewPendingTransactions,
    Logs,
}

// ─────────────────────────────────────────────────────────────────────────────
// WsServer
// ─────────────────────────────────────────────────────────────────────────────

/// WebSocket server that handles `eth_subscribe` / `eth_unsubscribe` and
/// pushes real-time chain events to connected clients.
pub struct WsServer {
    pub ws_port:   u16,
    pub bind_addr: String,
    state:         RpcState,
}

impl WsServer {
    /// Create a new WebSocket server backed by the given `RpcState`.
    pub fn new(state: RpcState, ws_port: u16) -> Self {
        Self {
            ws_port,
            bind_addr: "0.0.0.0".to_string(),
            state,
        }
    }

    /// Bind and run the WebSocket server — loops forever accepting connections.
    /// Spawn with `tokio::spawn(ws_server.run())`.
    pub async fn run(self) -> std::io::Result<()> {
        let addr = format!("{}:{}", self.bind_addr, self.ws_port);
        let listener = TcpListener::bind(&addr).await?;
        info!(addr = addr, "WebSocket JSON-RPC server listening");

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let state = self.state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, state).await {
                            debug!(peer = %peer, "WS connection closed: {e}");
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "WebSocket accept error — retrying");
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-connection handler
// ─────────────────────────────────────────────────────────────────────────────

async fn handle_connection(
    stream: TcpStream,
    state: RpcState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Perform the WebSocket HTTP-Upgrade handshake.
    let ws_stream = accept_async(stream).await?;
    let (mut tx, mut rx) = ws_stream.split();

    // Per-connection subscription registry: sub_id → event type.
    let mut subs: HashMap<String, SubType> = HashMap::new();
    // SEC-2026-05-09 Pass-15 (CRIT-03): per-connection subscription
    // cap. Pre-fix `subs` was an unbounded HashMap — a single
    // misbehaving WebSocket client could call `eth_subscribe` in a
    // tight loop until the node OOM'd (each entry holds the sub-id
    // string + a SubType filter potentially carrying topics/addresses).
    // Cap fixes the worst-case at 64 KiB / connection.
    const MAX_SUBS_PER_CONN: usize = 1024;

    // Subscribe to node-level broadcast channels.
    let mut head_rx    = state.new_head_tx.subscribe();
    let mut pending_rx = state.new_pending_tx.subscribe();

    loop {
        tokio::select! {
            // ── Inbound JSON-RPC message from client ─────────────────────
            msg = rx.next() => {
                let msg = match msg {
                    Some(Ok(m))  => m,
                    _            => break,  // connection closed or I/O error
                };

                let text = match msg {
                    Message::Text(t)  => t,
                    Message::Close(_) => break,
                    Message::Ping(d)  => {
                        let _ = tx.send(Message::Pong(d)).await;
                        continue;
                    }
                    _ => continue,
                };

                // Parse JSON-RPC 2.0 request.
                let req: JsonRpcRequest = match serde_json::from_str(&text) {
                    Ok(r)  => r,
                    Err(_) => {
                        let err_resp = JsonRpcResponse::error(
                            Value::Null,
                            JsonRpcError::from(RpcError::Parse("invalid JSON-RPC".into())),
                        );
                        let _ = tx.send(Message::Text(serde_json::to_string(&err_resp)?)).await;
                        continue;
                    }
                };

                // SEC-2026-05-09 Pass-15 (CRIT-03): refuse new subs
                // past the cap. Unsubscribe is always allowed so a
                // client at the cap can recover slots.
                if req.method == "eth_subscribe" && subs.len() >= MAX_SUBS_PER_CONN {
                    let err_resp = JsonRpcResponse::error(
                        req.id,
                        JsonRpcError::from(RpcError::InvalidParams(format!(
                            "subscription cap reached ({MAX_SUBS_PER_CONN}); unsubscribe first"
                        ))),
                    );
                    let _ = tx.send(Message::Text(serde_json::to_string(&err_resp)?)).await;
                    continue;
                }

                // Dispatch eth_subscribe / eth_unsubscribe.
                let resp = match dispatch_ws_method(&req.method, &req.params, &mut subs) {
                    Ok(v)  => JsonRpcResponse::success(req.id, v),
                    Err(e) => JsonRpcResponse::error(req.id, JsonRpcError::from(e)),
                };

                let _ = tx.send(Message::Text(serde_json::to_string(&resp)?)).await;
            }

            // ── New block head from consensus layer ───────────────────────
            result = head_rx.recv() => {
                let head = match result {
                    Ok(h)  => h,
                    Err(_) => continue,   // lagged or channel closed — skip
                };
                let push = subs.iter()
                    .filter(|(_, st)| **st == SubType::NewHeads)
                    .map(|(id, _)| {
                        json!({
                            "jsonrpc": "2.0",
                            "method":  "eth_subscription",
                            "params":  { "subscription": id, "result": head }
                        })
                    })
                    .collect::<Vec<_>>();

                for p in push {
                    let _ = tx.send(Message::Text(p.to_string())).await;
                }
            }

            // ── Pending transaction accepted by mempool ───────────────────
            result = pending_rx.recv() => {
                let hash = match result {
                    Ok(h)  => h,
                    Err(_) => continue,
                };
                let push = subs.iter()
                    .filter(|(_, st)| **st == SubType::NewPendingTransactions)
                    .map(|(id, _)| {
                        json!({
                            "jsonrpc": "2.0",
                            "method":  "eth_subscription",
                            "params":  { "subscription": id, "result": hash }
                        })
                    })
                    .collect::<Vec<_>>();

                for p in push {
                    let _ = tx.send(Message::Text(p.to_string())).await;
                }
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Method dispatcher (WebSocket-only methods)
// ─────────────────────────────────────────────────────────────────────────────

fn dispatch_ws_method(
    method: &str,
    params: &Value,
    subs: &mut HashMap<String, SubType>,
) -> Result<Value, RpcError> {
    match method {
        "eth_subscribe" => {
            let event = params
                .get(0)
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::InvalidParams("missing event name".into()))?;

            let sub_type = match event {
                "newHeads"               => SubType::NewHeads,
                "newPendingTransactions" => SubType::NewPendingTransactions,
                "logs"                   => SubType::Logs,
                other => {
                    return Err(RpcError::InvalidParams(
                        format!("unknown subscription event: {other}"),
                    ));
                }
            };

            let id = new_sub_id();
            subs.insert(id.clone(), sub_type);
            Ok(json!(id))
        }

        "eth_unsubscribe" => {
            let id = params
                .get(0)
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::InvalidParams("missing subscription id".into()))?;
            let removed = subs.remove(id).is_some();
            Ok(json!(removed))
        }

        other => Err(RpcError::MethodNotFound(other.to_string())),
    }
}
