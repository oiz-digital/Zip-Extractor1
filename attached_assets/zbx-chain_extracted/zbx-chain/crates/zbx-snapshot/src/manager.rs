//! State snapshot manager — creates and manages state snapshots for fast sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use std::time::Instant;

/// Snapshot metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotMeta {
    pub version: u32,
    pub block_number: u64,
    pub state_root: [u8; 32],
    pub created_at: u64,
    pub total_accounts: u64,
    pub total_chunks: u32,
    pub checksum: [u8; 32],
    pub zbx_version: String,
}

/// Snapshot manager
pub struct SnapshotManager {
    pub base_dir: PathBuf,
    pub snapshots: Vec<SnapshotMeta>,
    pub config: SnapshotConfig,
    pub in_progress: Option<SnapshotInProgress>,
}

#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    pub max_snapshots: usize,
    pub chunk_size: usize,         // accounts per chunk
    pub enable_compression: bool,
    pub snapshot_interval: u64,    // blocks between snapshots
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            max_snapshots: 3,
            chunk_size: 100_000,
            enable_compression: true,
            snapshot_interval: 100_000,
        }
    }
}

#[derive(Debug)]
pub struct SnapshotInProgress {
    pub block_number: u64,
    pub started_at: Instant,
    pub chunks_written: u32,
    pub accounts_written: u64,
    pub path: PathBuf,
}

impl SnapshotManager {
    pub fn new(base_dir: PathBuf, config: SnapshotConfig) -> Result<Self, SnapshotError> {
        fs::create_dir_all(&base_dir).map_err(|e| SnapshotError::Io(e.to_string()))?;
        let snapshots = Self::load_snapshots(&base_dir)?;
        Ok(Self { base_dir, snapshots, config, in_progress: None })
    }

    /// Begin a new snapshot
    pub fn begin_snapshot(&mut self, block_number: u64) -> Result<(), SnapshotError> {
        if self.in_progress.is_some() {
            return Err(SnapshotError::SnapshotInProgress);
        }
        let path = self.base_dir.join(format!("snapshot_{}", block_number));
        fs::create_dir_all(&path).map_err(|e| SnapshotError::Io(e.to_string()))?;
        self.in_progress = Some(SnapshotInProgress {
            block_number, started_at: Instant::now(),
            chunks_written: 0, accounts_written: 0, path,
        });
        tracing::info!(block = block_number, "Snapshot started");
        Ok(())
    }

    /// Write a chunk of accounts
    pub fn write_chunk(&mut self, accounts: &[AccountSnapshot]) -> Result<(), SnapshotError> {
        let progress = self.in_progress.as_mut().ok_or(SnapshotError::NoSnapshotInProgress)?;
        let chunk_path = progress.path.join(format!("chunk_{:05}.bin", progress.chunks_written));
        let data = bincode::serialize(accounts).map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        let final_data = if self.config.enable_compression {
            Self::compress(&data)?
        } else { data };
        // Open + write + fsync so the chunk survives a crash mid-snapshot. The
        // previous `fs::write` call returned before the page cache flushed,
        // which on a hard crash could leave a snapshot whose meta.json points
        // at a chunk that doesn't actually exist on disk.
        // See AUDIT_2026-04-30.md H-05.
        Self::write_durable(&chunk_path, &final_data)?;
        progress.chunks_written += 1;
        progress.accounts_written += accounts.len() as u64;
        Ok(())
    }

    /// Write `data` to `path` and fsync the file. On Linux this guarantees the
    /// bytes are on stable storage before we return.
    fn write_durable(path: &std::path::Path, data: &[u8]) -> Result<(), SnapshotError> {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .write(true).create(true).truncate(true).open(path)
            .map_err(|e| SnapshotError::Io(e.to_string()))?;
        f.write_all(data).map_err(|e| SnapshotError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| SnapshotError::Io(e.to_string()))?;
        Ok(())
    }

    /// Finalize snapshot
    pub fn finalize_snapshot(&mut self, state_root: [u8; 32], checksum: [u8; 32]) -> Result<SnapshotMeta, SnapshotError> {
        let progress = self.in_progress.take().ok_or(SnapshotError::NoSnapshotInProgress)?;
        let meta = SnapshotMeta {
            version: 1,
            block_number: progress.block_number,
            state_root,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            total_accounts: progress.accounts_written,
            total_chunks: progress.chunks_written,
            checksum,
            zbx_version: env!("CARGO_PKG_VERSION").into(),
        };
        let meta_path = progress.path.join("meta.json");
        let meta_json = serde_json::to_string_pretty(&meta).map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        // meta.json is the manifest pointer — must be fsynced LAST so a crash
        // never produces a finalised snapshot whose chunks haven't fully
        // landed. See AUDIT_2026-04-30.md H-05.
        Self::write_durable(&meta_path, meta_json.as_bytes())?;
        let elapsed = progress.started_at.elapsed();
        tracing::info!(
            block = progress.block_number,
            accounts = progress.accounts_written,
            chunks = progress.chunks_written,
            elapsed_secs = elapsed.as_secs(),
            "Snapshot finalized"
        );
        // Rotate old snapshots
        self.snapshots.push(meta.clone());
        self.snapshots.sort_by_key(|s| s.block_number);
        while self.snapshots.len() > self.config.max_snapshots {
            let old = self.snapshots.remove(0);
            let old_path = self.base_dir.join(format!("snapshot_{}", old.block_number));
            let _ = fs::remove_dir_all(&old_path);
        }
        Ok(meta)
    }

    /// Get the latest available snapshot
    pub fn latest_snapshot(&self) -> Option<&SnapshotMeta> {
        self.snapshots.last()
    }

    /// Get snapshot by block number
    pub fn get_snapshot(&self, block_number: u64) -> Option<&SnapshotMeta> {
        self.snapshots.iter().find(|s| s.block_number == block_number)
    }

    /// Compress chunk data with LZ4 (lz4_flex — pure-Rust, no C build).
    ///
    /// The output format is `u32_le(decompressed_len) || lz4_block_data`.
    /// `decompress()` reads the prepended length to allocate the output buffer.
    fn compress(data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        let compressed = lz4_flex::compress_prepend_size(data);
        Ok(compressed)
    }

    /// Decompress an LZ4 block written by `compress()`.
    pub fn decompress(data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        lz4_flex::decompress_size_prepended(data)
            .map_err(|e| SnapshotError::Serialization(format!("lz4 decompress: {e}")))
    }

    fn load_snapshots(base_dir: &Path) -> Result<Vec<SnapshotMeta>, SnapshotError> {
        let mut metas = Vec::new();
        if !base_dir.exists() { return Ok(metas); }
        for entry in fs::read_dir(base_dir).map_err(|e| SnapshotError::Io(e.to_string()))? {
            let entry = entry.map_err(|e| SnapshotError::Io(e.to_string()))?;
            let meta_path = entry.path().join("meta.json");
            if meta_path.exists() {
                let bytes = fs::read(&meta_path).map_err(|e| SnapshotError::Io(e.to_string()))?;
                if let Ok(meta) = serde_json::from_slice::<SnapshotMeta>(&bytes) {
                    metas.push(meta);
                }
            }
        }
        metas.sort_by_key(|m| m.block_number);
        Ok(metas)
    }
}

/// Account snapshot entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccountSnapshot {
    pub address: [u8; 20],
    pub balance: [u8; 32],
    pub nonce: u64,
    pub code_hash: [u8; 32],
    pub storage_root: [u8; 32],
    pub storage: Vec<([u8; 32], [u8; 32])>, // (slot, value) pairs
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Snapshot already in progress")]
    SnapshotInProgress,
    #[error("No snapshot in progress")]
    NoSnapshotInProgress,
    #[error("Checksum mismatch")]
    ChecksumMismatch,
    #[error("Snapshot not found for block {0}")]
    NotFound(u64),
}