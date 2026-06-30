//! zbx-mempool: Transaction pool for Zebvix.
//!
//! Maintains two sub-pools:
//! - pending: transactions ready for inclusion (correct nonce, sufficient balance).
//! - queued: transactions with gaps (future nonce) awaiting predecessors.
//!
//! Transactions are ordered by effective tip (max_priority_fee_per_gas).
//! Hard limits: 5 000 pending + 2 000 queued slots per node.

pub mod error;
pub mod nonce_tracker;
pub mod pool;

pub use error::MempoolError;
pub use pool::{MempoolConfig, TransactionPool};