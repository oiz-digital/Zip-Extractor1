//! State snapshot restoration — fast sync from snapshot.

use std::path::{Path, PathBuf};
use std::fs;
use std::time::Instant;
use std::collections::HashMap;

use crate::manager::{SnapshotMeta, AccountSnapshot, SnapshotError};

/// Restore progress tracking
#[derive(Debug, Clone)]
pub struct RestoreProgress {
    pub total_chunks: u32,
    pub chunks_done: u32,
    pub accounts_restored: u64,
    pub elapsed_secs: u64,
    pub errors: Vec<String>,
    pub complete: bool,
}

/// Snapshot restorer
pub struct SnapshotRestorer {
    pub snapshot_dir: PathBuf,
    pub state_writer: Box<dyn StateWriter>,
    pub verify_checksum: bool,
    pub parallel_chunks: usize,
}

/// Trait for writing restored state to target DB
pub trait StateWriter: Send {
    fn write_account(&mut self, account: &AccountSnapshot) -> Result<(), String>;
    fn commit(&mut self) -> Result<[u8; 32], String>; // returns state root
    fn rollback(&mut self);
}

impl SnapshotRestorer {
    pub fn new(snapshot_dir: PathBuf, state_writer: Box<dyn StateWriter>) -> Self {
        Self {
            snapshot_dir,
            state_writer,
            verify_checksum: true,
            parallel_chunks: 4,
        }
    }

    /// Restore state from snapshot
    pub fn restore(&mut self, meta: &SnapshotMeta) -> Result<RestoreProgress, SnapshotError> {
        let start = Instant::now();
        let snap_path = self.snapshot_dir.join(format!("snapshot_{}", meta.block_number));
        if !snap_path.exists() {
            return Err(SnapshotError::NotFound(meta.block_number));
        }

        let mut progress = RestoreProgress {
            total_chunks: meta.total_chunks,
            chunks_done: 0,
            accounts_restored: 0,
            elapsed_secs: 0,
            errors: Vec::new(),
            complete: false,
        };

        tracing::info!(
            block = meta.block_number,
            total_accounts = meta.total_accounts,
            total_chunks = meta.total_chunks,
            "Starting snapshot restore"
        );

        for chunk_idx in 0..meta.total_chunks {
            let chunk_path = snap_path.join(format!("chunk_{:05}.bin", chunk_idx));
            if !chunk_path.exists() {
                let err = format!("Missing chunk: {}", chunk_idx);
                progress.errors.push(err.clone());
                tracing::error!(chunk = chunk_idx, "Chunk file missing");
                continue;
            }

            match self.restore_chunk(&chunk_path) {
                Ok(count) => {
                    progress.accounts_restored += count;
                    progress.chunks_done += 1;
                    tracing::debug!(chunk = chunk_idx, accounts = count, "Chunk restored");
                }
                Err(e) => {
                    progress.errors.push(e.to_string());
                    tracing::error!(chunk = chunk_idx, error = %e, "Chunk restore failed");
                }
            }
        }

        // Commit and verify state root
        match self.state_writer.commit() {
            Ok(state_root) => {
                if self.verify_checksum && state_root != meta.state_root {
                    self.state_writer.rollback();
                    return Err(SnapshotError::ChecksumMismatch);
                }
                tracing::info!(block = meta.block_number, accounts = progress.accounts_restored, "Snapshot restore complete");
            }
            Err(e) => {
                self.state_writer.rollback();
                return Err(SnapshotError::Io(e));
            }
        }

        progress.elapsed_secs = start.elapsed().as_secs();
        progress.complete = progress.errors.is_empty();
        Ok(progress)
    }

    fn restore_chunk(&mut self, path: &Path) -> Result<u64, SnapshotError> {
        let data = fs::read(path).map_err(|e| SnapshotError::Io(e.to_string()))?;
        let accounts: Vec<AccountSnapshot> = bincode::deserialize(&data)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        let count = accounts.len() as u64;
        for account in &accounts {
            self.state_writer.write_account(account)
                .map_err(|e| SnapshotError::Io(e))?;
        }
        Ok(count)
    }

    /// Verify snapshot integrity
    pub fn verify(&self, meta: &SnapshotMeta) -> Result<bool, SnapshotError> {
        let snap_path = self.snapshot_dir.join(format!("snapshot_{}", meta.block_number));
        if !snap_path.exists() { return Err(SnapshotError::NotFound(meta.block_number)); }
        for i in 0..meta.total_chunks {
            let chunk_path = snap_path.join(format!("chunk_{:05}.bin", i));
            if !chunk_path.exists() { return Ok(false); }
        }
        Ok(true)
    }
}