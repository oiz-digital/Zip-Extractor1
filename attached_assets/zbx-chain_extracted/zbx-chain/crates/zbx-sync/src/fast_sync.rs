//! Sequential fast-sync: download and verify blocks in order.

use crate::error::SyncError;
use zbx_types::{Block, H256};
use zbx_crypto::keccak::keccak256;
use tracing::debug;
use tokio::time::Duration;

pub type BlockNumber = u64;

/// Configuration for the fast syncer.
#[derive(Debug, Clone)]
pub struct FastSyncConfig {
    pub batch_size:      u32,
    pub request_timeout: Duration,
    pub max_parallel:    usize,
}

impl Default for FastSyncConfig {
    fn default() -> Self {
        Self {
            batch_size:      256,
            request_timeout: Duration::from_secs(30),
            max_parallel:    8,
        }
    }
}

pub struct FastSyncer {
    config:      FastSyncConfig,
    local_head:  BlockNumber,
    target_head: BlockNumber,
    target_hash: H256,
}

impl FastSyncer {
    pub fn new(
        config:      FastSyncConfig,
        local_head:  BlockNumber,
        target_head: BlockNumber,
        target_hash: H256,
    ) -> Self {
        Self { config, local_head, target_head, target_hash }
    }

    pub fn remaining(&self) -> u64 {
        self.target_head.saturating_sub(self.local_head)
    }

    pub fn verify_block(block: &Block, expected_parent: &H256) -> Result<(), SyncError> {
        let parent = block.header.parent_hash;
        if &parent != expected_parent {
            return Err(SyncError::InvalidBlock(
                block.header.number,
                format!("parent hash mismatch: expected {:?}", expected_parent),
            ));
        }
        let computed = keccak256(block.header.hash().as_bytes());
        if computed != block.header.hash() {
            return Err(SyncError::InvalidBlock(
                block.header.number,
                "block hash does not match header".to_string(),
            ));
        }
        Ok(())
    }

    pub fn next_batch_range(&self) -> (BlockNumber, BlockNumber) {
        let from = self.local_head + 1;
        let to   = (self.local_head + self.config.batch_size as u64).min(self.target_head);
        (from, to)
    }

    pub fn advance(&mut self, height: BlockNumber) {
        if height > self.local_head {
            debug!("fast-sync: advanced local head {} → {}", self.local_head, height);
            self.local_head = height;
        }
    }

    pub fn is_complete(&self) -> bool {
        self.local_head >= self.target_head
    }
}
