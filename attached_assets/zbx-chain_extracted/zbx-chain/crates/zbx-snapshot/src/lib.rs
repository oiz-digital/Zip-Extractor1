//! State snapshot system — fast-sync from a trusted snapshot.

pub mod manager;
pub mod restore;
pub mod chunk;

pub use manager::{SnapshotManager, SnapshotMeta, AccountSnapshot, SnapshotError};
pub use restore::SnapshotRestorer;
pub use chunk::SnapshotChunk;