//! zbx-storage: Persistent chain storage for Zebvix.
//!
//! Column families:
//! - blocks      : block_hash → RLP(Block)
//! - headers     : block_number → block_hash
//! - transactions: tx_hash → (block_hash, tx_index, SignedTx)
//! - receipts    : tx_hash → TransactionReceipt
//! - state       : keccak(addr) → RLP(AccountState)
//! - storage     : keccak(addr || slot) → value
//! - code        : code_hash → bytecode
//! - metadata    : named keys (finalized_height, genesis_hash, …)

pub mod batch;
pub mod db;
pub mod error;
pub mod pruner;
pub mod schema;

pub use db::ZbxDb;
pub use error::StorageError;
pub use pruner::{PruneStats, PrunerConfig};
pub use schema::Column;