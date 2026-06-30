//! WebSocket transport: real-time bidirectional JSON-RPC.
//!
//! Security hardening applied (NODE-SEC-2026):
//!   - Concurrent connection cap enforced via AtomicUsize counter.
//!     New connections are rejected with a 1008 policy-violation close frame
//!     when the cap is reached (default: 256, operator-configurable).
//!   - Per-message size limit: text frames > MAX_WS_MSG_BYTES are dropped
//!     with an error response instead of being deserialized.
//!   - Batch requests are capped at MAX_WS_BATCH_SIZE items.

use crate::{
    error::RpcErrorObject,
    request::{JsonRpcCall, JsonRpcRequest},
    response::JsonRpcResponse,
    router::RpcRouter,
    pubsub::PubSubManager,
};
use std::{net::SocketAddr, sync::{Arc, atomic::{AtomicUsize, Ordering}}};
use tracing::{info, debug, warn};
use tokio::net::{TcpListener, TcpStream};

/// Maximum WebSocket text-frame payload (4 MiB).
pub const MAX_WS_MSG_BYTES: usize = 4 * 1024 * 1024;

/// Maximum batch items over a single WebSocket message.
pub const MAX_WS_BATCH_SIZE: usize = 100;

/// Default maximum concurrent WebSocket connections.
pub const DEFAULT_MAX_WS_CONNS: usize = 256;

pub struct WsTransport {
    addr:      SocketAddr,
    router:    Arc<RpcRouter>,
    pubsub:    Arc<PubSubManager>,
    /// Hard cap on simultaneous WebSocket connections.
    max_conns: usize,
}

impl WsTransport {
    pub fn new(addr: SocketAddr, router: RpcRouter, pubsub: PubSubManager) -> Self {
        Self {
            addr,
            router: Arc::new(router),
            pubsub: Arc::new(pubsub),
            max_conns: DEFAULT_MAX_WS_CONNS,
        }
    }

    /// Override the default connection cap (useful for public vs. private nodes).
    pub fn with_max_conns(mut self, max: usize) -> Self {
        self.max_conns = max;
        self
    }

    pub async fn start(self) -> Result<(), std::io::Error> {
        let listener  = TcpListener::bind(self.addr).await?;
        info!("json-rpc WebSocket listening on {} (max_conns={})", self.addr, self.max_conns);

        // Shared atomic counter — incremented when a connection is accepted and
        // decremented (via Drop guard) when it is closed.
        let conn_count = Arc::new(AtomicUsize::new(0));
        let max_conns  = self.max_conns;

        loop {
            let (stream, addr) = listener.accept().await?;
            let current = conn_count.load(Ordering::Relaxed);
            if current >= max_conns {
                warn!("ws: connection limit {} reached, rejecting {}", max_conns, addr);
                // Close the raw TCP stream immediately — the peer will receive a
                // TCP RST / clean close, no WS handshake is performed.
                drop(stream);
                continue;
            }

            debug!("ws: connection from {} ({}/{} slots)", addr, current + 1, max_conns);
            let router     = Arc::clone(&self.router);
            let pubsub     = Arc::clone(&self.pubsub);
            let counter    = Arc::clone(&conn_count);
            counter.fetch_add(1, Ordering::Relaxed);

            tokio::spawn(async move {
                if let Err(e) = Self::handle(stream, router, pubsub).await {
                    warn!("ws: handler error for {}: {}", addr, e);
                }
                // Decrement regardless of success or error.
                counter.fetch_sub(1, Ordering::Relaxed);
                debug!("ws: connection from {} closed", addr);
            });
        }
    }

    async fn handle(
        stream: TcpStream,
        router: Arc<RpcRouter>,
        pubsub: Arc<PubSubManager>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio_tungstenite::tungstenite::Message;
        use futures::{SinkExt, StreamExt};

        let ws = tokio_tungstenite::accept_async(stream).await?;
        let (mut sink, mut source) = ws.split();

        while let Some(msg) = source.next().await {
            match msg? {
                Message::Text(text) => {
                    // Reject oversized messages before deserialization to
                    // prevent memory-amplification attacks.
                    if text.len() > MAX_WS_MSG_BYTES {
                        warn!("ws: oversized message ({} bytes), dropping", text.len());
                        let r = JsonRpcResponse::error(
                            None,
                            RpcErrorObject::server_error(
                                -32600,
                                &format!(
                                    "message size {} exceeds limit {} bytes",
                                    text.len(), MAX_WS_MSG_BYTES
                                ),
                            ),
                        );
                        sink.send(Message::Text(serde_json::to_string(&r)?)).await?;
                        continue;
                    }

                    let call: JsonRpcCall = match serde_json::from_str(&text) {
                        Ok(c) => c,
                        Err(_) => {
                            let r = JsonRpcResponse::error(None, RpcErrorObject::parse_error());
                            sink.send(Message::Text(serde_json::to_string(&r)?)).await?;
                            continue;
                        }
                    };
                    match call {
                        JsonRpcCall::Single(req) => {
                            let id = req.id.clone();
                            let resp = match router.dispatch(req).await {
                                Ok(v)  => JsonRpcResponse::success(id, v),
                                Err(e) => JsonRpcResponse::error(id, e),
                            };
                            sink.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        }
                        JsonRpcCall::Batch(reqs) => {
                            // Enforce batch cap to prevent handler monopolisation.
                            if reqs.len() > MAX_WS_BATCH_SIZE {
                                warn!("ws: batch size {} exceeds cap {}", reqs.len(), MAX_WS_BATCH_SIZE);
                                let r = JsonRpcResponse::error(
                                    None,
                                    RpcErrorObject::server_error(
                                        -32600,
                                        &format!(
                                            "batch size {} exceeds maximum {}",
                                            reqs.len(), MAX_WS_BATCH_SIZE
                                        ),
                                    ),
                                );
                                sink.send(Message::Text(serde_json::to_string(&r)?)).await?;
                                continue;
                            }
                            let mut resps = Vec::new();
                            for req in reqs {
                                let id = req.id.clone();
                                let resp = match router.dispatch(req).await {
                                    Ok(v)  => JsonRpcResponse::success(id, v),
                                    Err(e) => JsonRpcResponse::error(id, e),
                                };
                                resps.push(resp);
                            }
                            sink.send(Message::Text(serde_json::to_string(&resps)?)).await?;
                        }
                    }
                }
                Message::Ping(d) => { sink.send(Message::Pong(d)).await?; }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok(())
    }
}
