//! zbx-da — Data Availability layer for Zebvix Chain.
//!
//! Supports:
//! - Blob transactions (EIP-4844 compatible format)
//! - KZG polynomial commitments for blob data
//! - Data availability sampling (DAS) for light clients
//! - Blob pruning after finality window
//! - DA proof generation and verification

pub mod blob;
pub mod commitment;
pub mod sampling;
pub mod pruner;
pub mod store;
pub mod error;

pub use blob::{Blob, BlobTransaction, BlobSidecar};
pub use commitment::{KzgCommitment, KzgProof, KzgSettings};
pub use sampling::{DaSampler, SampleResult, ChunkProof, ChunkFetcher};
pub use pruner::BlobPruner;
pub use store::BlobStore;
pub use error::DaError;

/// Maximum blob size: 128 KB (4096 field elements × 32 bytes each)
pub const BLOB_SIZE: usize = 131_072;

/// Max blobs per block (Zebvix Chain supports up to 8 blobs per block)
pub const MAX_BLOBS_PER_BLOCK: usize = 8;

/// Blob data target per block in bytes (~512 KB)
pub const TARGET_BLOB_DATA_PER_BLOCK: usize = BLOB_SIZE * 4;

/// Blob finality window: blobs pruned after 30 days (~518,400 blocks @ 5s)
pub const BLOB_PRUNE_BLOCKS: u64 = 518_400;

// ── High-level runner (ZEP-003 node wiring) ───────────────────────────────────
pub mod service;
pub use service::DaService;