//! State sync: download the full world state trie at a pivot block.
//!
//! Implemented as a BFS over the Merkle Patricia Trie, requesting nodes
//! by hash from peers and persisting them to the database.

use crate::error::SyncError;
use zbx_types::H256;
use std::collections::{HashSet, VecDeque};
use tracing::{info, debug, warn};

pub const PARALLEL_NODE_REQUESTS: usize = 128;
pub const MAX_NODES_PER_REQUEST:  usize = 384;
pub const NODE_REQUEST_TIMEOUT_MS: u64  = 20_000;

/// Progress tracker for state sync.
#[derive(Debug, Default, Clone)]
pub struct StateSyncProgress {
    pub pivot_block:       u64,
    pub nodes_downloaded:  u64,
    pub nodes_total_est:   u64,   // estimate (trie size unknown upfront)
    pub code_downloaded:   u64,
    pub bytes_downloaded:  u64,
    pub accounts_healed:   u64,   // snap-heal path
    pub is_complete:       bool,
}

/// State sync engine (trie node downloader).
pub struct StateSyncer {
    pivot:     u64,
    root:      H256,
    queue:     VecDeque<H256>,  // hashes to download
    fetched:   HashSet<H256>,   // hashes already stored
    progress:  StateSyncProgress,
}

impl StateSyncer {
    pub fn new(pivot_block: u64, state_root: H256) -> Self {
        let mut queue = VecDeque::new();
        queue.push_back(state_root);
        Self {
            pivot:    pivot_block,
            root:     state_root,
            queue,
            fetched:  HashSet::new(),
            progress: StateSyncProgress {
                pivot_block,
                ..Default::default()
            },
        }
    }

    /// Run one batch of state node downloads.
    /// Returns `true` when the full state has been synced.
    pub async fn tick(&mut self) -> Result<bool, SyncError> {
        if self.queue.is_empty() {
            self.progress.is_complete = true;
            info!("state-sync: complete at pivot #{} ({} nodes)", 
                  self.pivot, self.progress.nodes_downloaded);
            return Ok(true);
        }

        let batch: Vec<H256> = self.queue
            .drain(..self.queue.len().min(MAX_NODES_PER_REQUEST))
            .filter(|h| !self.fetched.contains(h))
            .collect();

        if batch.is_empty() { return Ok(false); }

        debug!("state-sync: requesting {} trie nodes (queue={})", 
               batch.len(), self.queue.len());

        let nodes = self.fetch_nodes(&batch).await?;

        for (hash, data) in &nodes {
            self.fetched.insert(*hash);
            self.progress.nodes_downloaded += 1;
            self.progress.bytes_downloaded += data.len() as u64;
            // Decode node and enqueue child hashes.
            let children = decode_node_children(data);
            for child in children {
                if !self.fetched.contains(&child) {
                    self.queue.push_back(child);
                }
            }
        }

        self.persist_nodes(nodes).await?;
        Ok(false)
    }

    async fn fetch_nodes(&self, hashes: &[H256]) -> Result<Vec<(H256, Vec<u8>)>, SyncError> {
        // In production: parallel P2P GetNodeData requests.
        Ok(Vec::new())
    }

    async fn persist_nodes(&self, nodes: Vec<(H256, Vec<u8>)>) -> Result<(), SyncError> {
        // In production: batch write to AccountTrie / StorageTrie columns.
        Ok(())
    }

    pub fn progress(&self) -> &StateSyncProgress { &self.progress }
}

/// Decode child hashes from a RLP-encoded trie node.
fn decode_node_children(data: &[u8]) -> Vec<H256> {
    // In production: RLP-decode the node, extract 32-byte hash references.
    Vec::new()
}