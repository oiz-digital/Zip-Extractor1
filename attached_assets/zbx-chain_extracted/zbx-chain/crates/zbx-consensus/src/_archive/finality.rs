//! Block finality tracking.

use std::collections::HashMap;
use tracing::info;

/// Tracks which blocks have been finalised.
pub struct FinalityTracker {
    /// height → block_hash of finalised blocks
    finalised: HashMap<u64, [u8; 32]>,
    latest:    u64,
}

impl FinalityTracker {
    pub fn new() -> Self {
        Self { finalised: HashMap::new(), latest: 0 }
    }

    /// Mark a block as finalised.
    pub fn finalise(&mut self, height: u64, hash: [u8; 32]) {
        self.finalised.insert(height, hash);
        if height > self.latest { self.latest = height; }
        info!(height, hash = hex::encode(hash), "Block finalised");
    }

    pub fn is_finalised(&self, height: u64) -> bool {
        self.finalised.contains_key(&height)
    }

    pub fn latest_finalised(&self) -> u64 { self.latest }
}

impl Default for FinalityTracker {
    fn default() -> Self { Self::new() }
}