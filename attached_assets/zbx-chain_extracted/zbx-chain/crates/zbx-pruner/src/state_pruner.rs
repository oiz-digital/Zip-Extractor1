//! State pruner — removes old trie nodes to reclaim disk space.
//! Implements a mark-and-sweep pruner for the Merkle Patricia Trie.

use std::collections::{HashSet, VecDeque};
use std::time::Instant;

pub type BlockHash = [u8; 32];
pub type Address   = [u8; 20];

/// Prune mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneMode {
    /// Keep full history (no pruning)
    Full,
    /// Keep N most recent blocks
    Distance(u64),
    /// Keep specific block targets
    Selective { targets: Vec<u64> },
}

impl Default for PruneMode { fn default() -> Self { PruneMode::Distance(128) } }

/// Prune checkpoint (state at specific block)
#[derive(Debug, Clone)]
pub struct PruneCheckpoint {
    pub block_number: u64,
    pub state_root: [u8; 32],
    pub pruned: bool,
}

/// State pruner
pub struct StatePruner {
    pub mode: PruneMode,
    pub checkpoints: Vec<PruneCheckpoint>,
    pub pruned_nodes: u64,
    pub pruned_bytes: u64,
    pub config: PrunerConfig,
}

#[derive(Debug, Clone)]
pub struct PrunerConfig {
    pub batch_size: usize,
    pub max_concurrent_nodes: usize,
    pub sleep_between_batches_ms: u64,
}

impl Default for PrunerConfig {
    fn default() -> Self {
        Self { batch_size: 10_000, max_concurrent_nodes: 100_000, sleep_between_batches_ms: 10 }
    }
}

impl StatePruner {
    pub fn new(mode: PruneMode, config: PrunerConfig) -> Self {
        Self { mode, checkpoints: Vec::new(), pruned_nodes: 0, pruned_bytes: 0, config }
    }

    /// Register a new block checkpoint
    pub fn on_block(&mut self, block_number: u64, state_root: [u8; 32]) {
        self.checkpoints.push(PruneCheckpoint { block_number, state_root, pruned: false });
        self.checkpoints.sort_by_key(|c| c.block_number);
    }

    /// Determine which blocks can be pruned
    pub fn prunable_blocks(&self, current_block: u64) -> Vec<u64> {
        match &self.mode {
            PruneMode::Full => vec![],
            PruneMode::Distance(n) => {
                let threshold = current_block.saturating_sub(*n);
                self.checkpoints.iter()
                    .filter(|c| c.block_number < threshold && !c.pruned)
                    .map(|c| c.block_number)
                    .collect()
            }
            PruneMode::Selective { targets } => {
                targets.iter().copied()
                    .filter(|&b| self.checkpoints.iter().any(|c| c.block_number == b && !c.pruned))
                    .collect()
            }
        }
    }

    /// Prune nodes not referenced after prune_point
    pub fn prune<DB: TrieDatabase>(&mut self, db: &mut DB, prune_point: u64) -> PruneStats {
        let start = Instant::now();
        let mut stats = PruneStats::default();

        // Get all roots to keep (from prune_point onwards)
        let keep_roots: HashSet<[u8; 32]> = self.checkpoints.iter()
            .filter(|c| c.block_number >= prune_point)
            .map(|c| c.state_root)
            .collect();

        // Mark all nodes reachable from kept roots
        let mut keep_nodes: HashSet<Vec<u8>> = HashSet::new();
        for root in &keep_roots {
            self.mark_reachable(db, root, &mut keep_nodes);
        }

        // Sweep — delete all nodes not in keep_nodes
        let all_nodes = db.list_all_nodes();
        for node_key in all_nodes {
            if !keep_nodes.contains(&node_key) {
                let size = db.get_node_size(&node_key);
                db.delete_node(&node_key);
                stats.nodes_deleted += 1;
                stats.bytes_freed += size;
            } else {
                stats.nodes_kept += 1;
            }
        }

        // Mark checkpoints as pruned
        for cp in self.checkpoints.iter_mut() {
            if cp.block_number < prune_point { cp.pruned = true; }
        }

        self.pruned_nodes += stats.nodes_deleted;
        self.pruned_bytes += stats.bytes_freed;
        stats.elapsed_ms = start.elapsed().as_millis() as u64;

        tracing::info!(
            nodes_deleted = stats.nodes_deleted,
            bytes_freed = stats.bytes_freed,
            nodes_kept = stats.nodes_kept,
            elapsed_ms = stats.elapsed_ms,
            "State pruned"
        );
        stats
    }

    fn mark_reachable<DB: TrieDatabase>(&self, db: &DB, root: &[u8; 32], keep: &mut HashSet<Vec<u8>>) {
        let mut queue: VecDeque<Vec<u8>> = VecDeque::new();
        queue.push_back(root.to_vec());
        while let Some(key) = queue.pop_front() {
            if keep.contains(&key) { continue; }
            keep.insert(key.clone());
            if let Some(children) = db.get_node_children(&key) {
                queue.extend(children);
            }
        }
    }

    pub fn stats(&self) -> PrunerStats {
        PrunerStats {
            mode: format!("{:?}", self.mode),
            pruned_nodes: self.pruned_nodes,
            pruned_bytes: self.pruned_bytes,
            checkpoints: self.checkpoints.len(),
            unpruned: self.checkpoints.iter().filter(|c| !c.pruned).count(),
        }
    }
}

/// Trie database trait for pruner
pub trait TrieDatabase {
    fn get_node_children(&self, key: &[u8]) -> Option<Vec<Vec<u8>>>;
    fn get_node_size(&self, key: &[u8]) -> u64;
    fn delete_node(&mut self, key: &[u8]);
    fn list_all_nodes(&self) -> Vec<Vec<u8>>;
}

#[derive(Debug, Clone, Default)]
pub struct PruneStats {
    pub nodes_deleted: u64,
    pub nodes_kept: u64,
    pub bytes_freed: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PrunerStats {
    pub mode: String,
    pub pruned_nodes: u64,
    pub pruned_bytes: u64,
    pub checkpoints: usize,
    pub unpruned: usize,
}