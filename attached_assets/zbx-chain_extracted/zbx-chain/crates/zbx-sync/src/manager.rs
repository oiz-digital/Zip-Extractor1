//! SyncManager: top-level sync orchestrator.

use crate::fast_sync::BlockNumber;
use crate::error::SyncError;
use zbx_types::H256;
use tracing::{info, warn};

/// The current synchronisation strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMode {
    Live,
    FastSync,
    SnapSync,
    Idle,
}

/// Snapshot of sync progress.
#[derive(Debug, Clone)]
pub struct SyncStatus {
    pub mode:         SyncMode,
    pub local_head:   BlockNumber,
    pub network_head: BlockNumber,
    pub percent:      f64,
}

/// Orchestrates fast-sync and snap-sync, transitioning to live sync once caught up.
pub struct SyncManager {
    mode:         SyncMode,
    local_head:   BlockNumber,
    network_head: BlockNumber,
    network_hash: H256,
}

impl SyncManager {
    pub fn new(local_head: BlockNumber, network_head: BlockNumber, network_hash: H256) -> Self {
        let mode = if network_head.saturating_sub(local_head) > 256 {
            SyncMode::SnapSync
        } else if network_head > local_head {
            SyncMode::FastSync
        } else {
            SyncMode::Live
        };
        info!("sync: mode={:?}, local={}, network={}", mode, local_head, network_head);
        Self { mode, local_head, network_head, network_hash }
    }

    pub fn status(&self) -> SyncStatus {
        let percent = if self.network_head == 0 {
            100.0
        } else {
            (self.local_head as f64 / self.network_head as f64) * 100.0
        };
        SyncStatus {
            mode: self.mode.clone(),
            local_head: self.local_head,
            network_head: self.network_head,
            percent,
        }
    }

    pub fn is_synced(&self) -> bool { self.mode == SyncMode::Live }

    pub fn on_network_update(&mut self, new_head: BlockNumber, new_hash: H256) {
        if new_head > self.network_head {
            self.network_head = new_head;
            self.network_hash = new_hash;
            if self.mode == SyncMode::Live
                && new_head.saturating_sub(self.local_head) > 8 {
                warn!("sync: fell behind ({} blocks), switching to fast-sync",
                    new_head - self.local_head);
                self.mode = SyncMode::FastSync;
            }
        }
    }

    pub fn on_block_applied(&mut self, height: BlockNumber) {
        self.local_head = height;
        if self.local_head >= self.network_head {
            info!("sync: caught up at block {}, entering live sync", height);
            self.mode = SyncMode::Live;
        }
    }
}
