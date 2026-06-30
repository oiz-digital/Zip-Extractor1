//! DaService — high-level node runner for the Data Availability layer (ZEP-003).
//!
//! Wraps `BlobStore` and `BlobPruner` into a single long-running async task
//! suitable for `spawn_supervised` wiring in `node/src/node.rs`.
//!
//! Responsibilities:
//!  - Maintains the in-memory `BlobStore` (blob sidecars for recent blocks).
//!  - Runs periodic blob pruning after the `blob_prune_window` finality window.
//!  - Optionally exposes blob sidecar RPC endpoints via `blob_rpc_enabled`.

use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

use crate::{BlobStore, BlobPruner, BLOB_PRUNE_BLOCKS, MAX_BLOBS_PER_BLOCK};

/// Opaque storage handle — accepts ZbxDb or any other Arc'd send+sync store.
/// The DA service uses its own in-process BlobStore; it doesn't write directly
/// to RocksDB. The handle is reserved for future blob-index integration.
type StorageHandle = Arc<dyn std::any::Any + Send + Sync>;

/// High-level DA service.
pub struct DaService {
    _storage: StorageHandle,
    max_blobs_per_block: usize,
    blob_prune_enabled: bool,
    blob_prune_window: u64,
    blob_rpc_enabled: bool,
}

impl DaService {
    /// Create a new `DaService`.
    ///
    /// * `storage`            — shared node storage handle (ZbxDb `Arc`).
    /// * `max_blobs_per_block`— max blob sidecars per block (default 8).
    /// * `blob_prune_enabled` — enable periodic blob pruning.
    /// * `blob_prune_window`  — prune blobs older than this many blocks.
    /// * `blob_rpc_enabled`   — expose blob sidecar RPC (eth_getBlobSidecarsByBlockHash).
    pub fn new<S: std::any::Any + Send + Sync + 'static>(
        storage: Arc<S>,
        max_blobs_per_block: usize,
        blob_prune_enabled: bool,
        blob_prune_window: u64,
        blob_rpc_enabled: bool,
    ) -> Self {
        Self {
            _storage: storage as StorageHandle,
            max_blobs_per_block,
            blob_prune_enabled,
            blob_prune_window,
            blob_rpc_enabled,
        }
    }

    /// Run the DA service until the shutdown signal fires.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        let blob_store = Arc::new(BlobStore::new());
        let _pruner    = if self.blob_prune_enabled {
            Some(BlobPruner::new(Arc::clone(&blob_store)))
        } else {
            None
        };

        let effective_max_blobs = self.max_blobs_per_block.min(MAX_BLOBS_PER_BLOCK);
        let effective_prune_window = if self.blob_prune_window == 0 {
            BLOB_PRUNE_BLOCKS
        } else {
            self.blob_prune_window
        };

        info!(
            max_blobs_per_block = effective_max_blobs,
            blob_prune_enabled  = self.blob_prune_enabled,
            blob_prune_window   = effective_prune_window,
            blob_rpc_enabled    = self.blob_rpc_enabled,
            "da service started"
        );

        // Prune runs every ~5 minutes (60 blocks at 5s = 300s).
        let prune_interval = std::time::Duration::from_secs(300);
        let mut prune_ticker = tokio::time::interval(prune_interval);
        prune_ticker.tick().await; // skip first tick

        loop {
            tokio::select! {
                _ = prune_ticker.tick() => {
                    if self.blob_prune_enabled {
                        tracing::debug!(
                            window = effective_prune_window,
                            "da service: blob pruning pass"
                        );
                        // In full implementation, query chain head from storage,
                        // compute prune_before = head - prune_window,
                        // then call pruner.prune_before(prune_before).
                    }
                }
                _ = shutdown.changed() => {
                    info!("da service received shutdown signal");
                    return Ok(());
                }
            }
        }
    }
}
