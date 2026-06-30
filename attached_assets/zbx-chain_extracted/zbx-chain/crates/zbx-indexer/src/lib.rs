//! zbx-indexer — transaction and event indexer for Zebvix Chain.
//!
//! Indexes:
//! - All transactions (by hash, sender, recipient, block)
//! - All EVM events/logs (by contract, topic, block range)
//! - Token transfers (ERC-20 Transfer events)
//! - Contract deployments (CREATE / CREATE2)
//! - Internal transactions (call traces)
//!
//! Storage backend: SQLite (embedded) for single-node; PostgreSQL for
//! production deployments. Exposes a GraphQL-compatible REST API.

pub mod schema;
pub mod indexer;
pub mod query;
pub mod server;
pub mod tvl;

pub use indexer::{Indexer, IndexerConfig};
pub use query::{QueryEngine, TxFilter, LogFilter};
pub use tvl::{TvlClient, TvlSnapshot, snapshot_loop, insert_snapshot};

// ── High-level runner (indexer node wiring) ───────────────────────────────────
pub mod service;
pub use service::{IndexerService, IndexerServiceConfig};