//! eth_feeHistory — historical base fees and reward percentiles.

use serde::{Deserialize, Serialize};

/// One entry in the fee history — represents a single block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeHistoryEntry {
    pub base_fee_per_gas: u64,
    pub gas_used_ratio:   f64,    // gas_used / gas_limit
    pub rewards:          Vec<u64>, // tip at each requested percentile
}

/// Sliding-window fee history store.
pub struct FeeHistory {
    entries:  Vec<FeeHistoryEntry>,
    capacity: usize,
}

impl FeeHistory {
    pub fn new(window: usize) -> Self {
        Self { entries: Vec::new(), capacity: window.max(1) }
    }

    pub fn push(&mut self, entry: FeeHistoryEntry) {
        if self.entries.len() >= self.capacity {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// Return the last `count` entries (for eth_feeHistory RPC).
    pub fn last_n(&self, count: usize) -> &[FeeHistoryEntry] {
        let start = self.entries.len().saturating_sub(count);
        &self.entries[start..]
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Next block's predicted base fee (from last entry).
    pub fn next_base_fee(&self) -> Option<u64> {
        self.entries.last().map(|e| {
            crate::base_fee::BaseFeeCalculator::next_base_fee(
                e.base_fee_per_gas,
                (e.gas_used_ratio * 30_000_000.0) as u64,
                30_000_000,
            ).unwrap_or(e.base_fee_per_gas)
        })
    }
}