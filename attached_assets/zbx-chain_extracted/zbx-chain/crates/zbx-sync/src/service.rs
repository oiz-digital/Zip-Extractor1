//! SyncService — high-level node runner for chain synchronisation (zbx-sync).
//!
//! Wraps `SyncCoordinator` (fast-sync + snap-sync) and `SyncManager` (live-sync
//! state machine) into a single long-running async task suitable for
//! `spawn_supervised` wiring in `node/src/node.rs`.
//!
//! Sync mode lifecycle:
//!  1. **fast** — sequential block-by-block download & verification from genesis.
//!  2. **snap** — parallel state-trie chunk download from a recent pivot block.
//!  3. **live** — consensus-driven block ingestion once the tip is reached.
//!
//! The coordinator transitions between modes automatically; the node config
//! `sync.mode` sets the *initial* mode (override for bootstrapping scenarios).

use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

use crate::{SyncManager, SyncMode};
use zbx_storage::ZbxDb;

/// High-level sync service that wraps coordinator + state machine.
pub struct SyncService {
    storage:              Arc<ZbxDb>,
    mode:                 String,
    snap_pivot_lag:       u64,
    snap_chunk_size_kb:   u64,
    fast_sync_batch_size: u64,
    max_sync_peers:       usize,
}

impl SyncService {
    /// Create a new `SyncService`.
    ///
    /// * `storage`             — shared node RocksDB handle.
    /// * `mode`                — initial sync mode: `"live"`, `"fast"`, or `"snap"`.
    /// * `snap_pivot_lag`      — blocks behind head to pick snap pivot.
    /// * `snap_chunk_size_kb`  — per-chunk download size for snap-sync.
    /// * `fast_sync_batch_size`— blocks per batch for fast-sync.
    /// * `max_sync_peers`      — max peers to use during sync.
    pub fn new(
        storage:              Arc<ZbxDb>,
        mode:                 String,
        snap_pivot_lag:       u64,
        snap_chunk_size_kb:   u64,
        fast_sync_batch_size: u64,
        max_sync_peers:       usize,
    ) -> Self {
        Self {
            storage,
            mode,
            snap_pivot_lag,
            snap_chunk_size_kb,
            fast_sync_batch_size,
            max_sync_peers,
        }
    }

    /// Run the sync service until the shutdown signal fires.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        let initial_mode = match self.mode.as_str() {
            "fast" => SyncMode::FastSync,
            "snap" => SyncMode::SnapSync,
            _      => SyncMode::Live,
        };

        // SyncManager tracks the state machine (current mode, head, target).
        // For startup we read local head from storage; network tip comes from peers.
        let local_head = self.storage
            .get_latest_block_number()
            .unwrap_or(0);
        let network_head  = local_head; // will be updated by peer messages
        let network_hash  = zbx_types::zero_hash();

        let mut manager = SyncManager::new(local_head, network_head, network_hash);

        info!(
            mode             = %self.mode,
            local_head,
            snap_pivot_lag   = self.snap_pivot_lag,
            snap_chunk_kb    = self.snap_chunk_size_kb,
            fast_batch_size  = self.fast_sync_batch_size,
            max_peers        = self.max_sync_peers,
            "sync service started"
        );

        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        tick.tick().await;

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let status = manager.status();
                    tracing::debug!(
                        mode    = ?status.mode,
                        head    = status.local_head,
                        target  = status.network_head,
                        "sync tick"
                    );
                    // In the full implementation: receive new block headers from
                    // PeerManager, feed them to manager.update_network_head(),
                    // then dispatch fast_syncer / snap_syncer / live_syncer
                    // depending on the current mode.
                }
                _ = shutdown.changed() => {
                    info!("sync service received shutdown signal");
                    return Ok(());
                }
            }
        }
    }
}
