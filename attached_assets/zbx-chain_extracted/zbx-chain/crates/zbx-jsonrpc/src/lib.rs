//! zbx-jsonrpc: JSON-RPC 2.0 server with HTTP and WebSocket transport.
//!
//! # ⚠ DEPRECATION NOTICE (M53-03)
//!
//! **This crate (`zbx-jsonrpc`) is superseded by `zbx-rpc`.**
//!
//! `zbx-rpc` provides a fully audited, production-ready JSON-RPC 2.0 server
//! with improved middleware, type-safe method routing, and native support for
//! the full `eth_*` + `zbx_*` namespace. The node binary (`zbx-node`) has
//! been updated to depend only on `zbx-rpc`.
//!
//! This crate is **not actively maintained** and will be removed in the next
//! major release. New code must not depend on it directly.
//!
//! **Migrate to:** `zbx-rpc` — see `docs/rpc/MIGRATION.md`.
//!
//! ---
//!
//! JSON-RPC 2.0 server (legacy):
//!
//! - Full JSON-RPC 2.0 (single requests, batch requests, notifications)
//! - HTTP transport (POST to /rpc or /)
//! - WebSocket transport with pub/sub support (eth_subscribe / eth_unsubscribe)
//! - Middleware stack: CORS, rate limiting, request tracing, auth
//! - Method router with async handlers
//!
//! # Usage (legacy — prefer zbx-rpc for new code)
//!
//! ```rust
//! let router = RpcRouter::new()
//!     .method("eth_blockNumber", handlers::block_number)
//!     .method("eth_getBalance",  handlers::get_balance);
//!
//! let server = JsonRpcServer::new("0.0.0.0:8545", router).await?;
//! server.start().await?;
//! ```

pub mod error;
pub mod request;
pub mod response;
pub mod router;
pub mod http;
pub mod ws;
pub mod pubsub;

pub use error::RpcError;
pub use request::{JsonRpcRequest, JsonRpcId};
pub use response::JsonRpcResponse;
pub use router::{RpcRouter, RpcHandler};
pub use http::HttpTransport;
pub use ws::WsTransport;
