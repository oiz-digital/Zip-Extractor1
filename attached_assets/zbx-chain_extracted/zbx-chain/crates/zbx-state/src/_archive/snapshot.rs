//! State snapshots — fast restore without replaying all blocks.

use tracing::{info, warn};

/// A snapshot of the world state at a given block height.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub height:     u64,
    pub state_root: [u8; 32],
    pub timestamp:  u64,
}

/// Snapshot manager — creates and restores snapshots.
pub struct SnapshotManager {
    snapshots: Vec<StateSnapshot>,
    interval:  u64,  // create snapshot every N blocks
}

impl SnapshotManager {
    pub fn new(interval: u64) -> Self {
        Self { snapshots: vec![], interval }
    }

    /// Create a snapshot if the height matches the interval.
    pub fn maybe_snapshot(&mut self, height: u64, root: [u8; 32], ts: u64) -> bool {
        if height % self.interval != 0 { return false; }
        let snap = StateSnapshot { height, state_root: root, timestamp: ts };
        info!(height, "State snapshot created");
        self.snapshots.push(snap);
        true
    }

    /// Find the closest snapshot at or below the target height.
    pub fn nearest(&self, target_height: u64) -> Option<&StateSnapshot> {
        self.snapshots.iter().filter(|s| s.height <= target_height).last()
    }

    /// Number of snapshots stored.
    pub fn count(&self) -> usize { self.snapshots.len() }
}