//! zbx-sync: Block and state synchronisation for Zebvix Chain.
//!
//! # Sync Modes
//!
//! ## Fast-Sync (Block-by-Block)
//! Downloads and verifies each block header + body in order from genesis.
//! Safe but slow for initial chain download.
//!
//! ## Snap-Sync (State Snapshot)
//! Downloads a recent finalized state snapshot in parallel chunks, then
//! catches up with live block processing. Modelled after Ethereum's
//! snap-sync (EIP-2124).
//!
//! ## Live Sync (Consensus-Driven)
//! Once caught up, blocks are received via the consensus protocol
//! (zbx-consensus HotStuff BFT). No separate sync loop needed.
//!
//! # Architecture
//!
//! ```
//! SyncManager
//!   ├─ PivotSelector      — choose a recent finalized block as snap pivot
//!   ├─ FastSyncer         — sequential block download & verify
//!   ├─ SnapSyncer         — parallel state trie chunk download
//!   └─ LiveSyncer         — live block ingestion from consensus
//! ```

pub mod error;
pub mod fast_sync;
pub mod snap_sync;
pub mod pivot;
pub mod manager;
pub mod coordinator;
pub mod merkle;
pub mod manifest;
pub mod producer;

pub use error::SyncError;
pub use manager::{SyncManager, SyncMode, SyncStatus};
pub use coordinator::{SyncCoordinator, SyncPeer, SnapshotMeta, FastSyncOutcome, COORD_SAFE_PIVOT};

// ── High-level runner (sync node wiring) ─────────────────────────────────────
pub mod service;
pub use service::SyncService;