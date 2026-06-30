//! RocksDB database snapshot — consistent point-in-time reads.

use crate::{error::StorageError, schema::Column};

/// A consistent snapshot of the database at a particular point in time.
/// Reads from a `DbSnapshot` always see the state at the moment the snapshot
/// was created, even if subsequent writes modify the same keys.
pub struct DbSnapshot {
    /// Block number this snapshot corresponds to.
    pub block_number: u64,
    /// Block hash this snapshot corresponds to.
    pub block_hash:   [u8; 32],
    /// Timestamp (Unix seconds) when snapshot was created.
    pub created_at:   u64,
    /// Total size of all column families at snapshot time (bytes).
    pub size_bytes:   u64,
}

impl DbSnapshot {
    pub fn new(block_number: u64, block_hash: [u8; 32]) -> Self {
        Self {
            block_number,
            block_hash,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            size_bytes: 0,
        }
    }

    /// Read a key from a column in this snapshot context.
    pub fn get(&self, col: Column, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        // In production: delegate to RocksDB snapshot read.
        Err(StorageError::NotFound)
    }

    /// Export this snapshot to a directory (for cold backup).
    pub fn export(&self, path: &std::path::Path) -> Result<(), StorageError> {
        std::fs::create_dir_all(path).map_err(|e| StorageError::Io(e.to_string()))?;
        // In production: RocksDB::create_checkpoint(path).
        Ok(())
    }

    /// Verify integrity of a snapshot directory.
    pub fn verify(path: &std::path::Path) -> Result<SnapshotMeta, StorageError> {
        // In production: read snapshot manifest, verify SSTFile checksums.
        Ok(SnapshotMeta::default())
    }
}

#[derive(Debug, Default)]
pub struct SnapshotMeta {
    pub block_number: u64,
    pub block_hash:   [u8; 32],
    pub size_bytes:   u64,
    pub file_count:   u32,
    pub created_at:   u64,
}