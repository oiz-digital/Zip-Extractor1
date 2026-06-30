//! zbx-rpc: JSON-RPC 2.0 server for Zebvix.
//!
//! Implements the standard Ethereum JSON-RPC specification (eth_*) plus
//! native Zebvix extensions (zbx_*) for staking, bridge, and governance.
//!
//! HTTP endpoint: http://0.0.0.0:8545  (TLS terminated by nginx → https://rpc.zbx.io)
//! WebSocket endpoint: ws://0.0.0.0:8546
//!   Supports eth_subscribe / eth_unsubscribe for newHeads and
//!   newPendingTransactions.  Production exposure is via wss:// through nginx.

pub mod error;
pub mod eth_api;
pub mod methods;
pub mod middleware;
pub mod server;
pub mod state;
pub mod tx_decode;
pub mod types;
pub mod ws_server;
pub mod zbx_api;

pub use error::RpcError;
pub use methods::{dispatch, is_simulation, is_mutation, all_method_names, MethodClass};
pub use server::RpcServer;
pub use state::RpcState;
pub use ws_server::WsServer;
