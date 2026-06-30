//! State pruning — remove old state to save disk space.

use tracing::{info, warn};

/// Pruning configuration.
#[derive(Debug, Clone)]
pub struct PruningConfig {
    /// Keep state for this many blocks behind the head.
    pub keep_blocks: u64,
    /// Prune every N blocks.
    pub interval:    u64,
}

impl Default for PruningConfig {
    fn default() -> Self {
        Self { keep_blocks: 1024, interval: 256 }
    }
}

/// Prunes old state from the database.
pub struct StatePruner {
    config: PruningConfig,
}

impl StatePruner {
    pub fn new(config: PruningConfig) -> Self { Self { config } }

    /// Run pruning if the current head height triggers the interval.
    pub fn maybe_prune(&self, head_height: u64) -> bool {
        if head_height % self.config.interval != 0 { return false; }
        let prune_below = head_height.saturating_sub(self.config.keep_blocks);
        info!(prune_below, "Pruning state below height");
        true
    }
}