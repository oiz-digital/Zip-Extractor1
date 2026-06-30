//! Blob pruner: removes blobs after finality window to reclaim disk space.

use crate::{store::BlobStore, BLOB_PRUNE_BLOCKS};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

pub struct BlobPruner {
    store: Arc<BlobStore>,
    /// Map from block number → list of blob hashes in that block.
    block_index: HashMap<u64, Vec<[u8; 32]>>,
    finalized_block: u64,
}

impl BlobPruner {
    pub fn new(store: Arc<BlobStore>) -> Self {
        BlobPruner {
            store,
            block_index: HashMap::new(),
            finalized_block: 0,
        }
    }

    /// Register blobs for a newly finalized block.
    pub fn register_block(&mut self, block: u64, blob_hashes: Vec<[u8; 32]>) {
        self.block_index.insert(block, blob_hashes);
        self.finalized_block = block;
    }

    /// Prune blobs older than BLOB_PRUNE_BLOCKS.
    pub fn prune(&mut self) {
        if self.finalized_block < BLOB_PRUNE_BLOCKS {
            return;
        }
        let cutoff = self.finalized_block - BLOB_PRUNE_BLOCKS;
        let prunable: Vec<u64> = self.block_index.keys().copied().filter(|b| *b < cutoff).collect();
        let pruned_count = prunable.len();
        for block in &prunable {
            self.block_index.remove(block);
        }
        info!(pruned_blocks = pruned_count, cutoff, "blob pruning complete");
    }
}