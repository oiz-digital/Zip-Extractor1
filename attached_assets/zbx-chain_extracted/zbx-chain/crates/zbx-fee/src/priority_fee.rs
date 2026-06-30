//! Priority fee (tip) estimator — suggests a reasonable tip for fast inclusion.

/// Percentile-based priority fee estimator.
/// Collects tips from recent blocks and recommends low/medium/high tips.
#[derive(Debug, Default)]
pub struct PriorityFeeEstimator {
    /// Collected tip samples (most recent `capacity` blocks).
    samples: Vec<Vec<u64>>,
    capacity: usize,
}

#[derive(Debug, Clone)]
pub struct FeeEstimate {
    /// Slow:   90th-percentile wait time (~20 blocks)
    pub low:    u64,
    /// Normal: included within next 3 blocks
    pub medium: u64,
    /// Fast:   included in next block
    pub high:   u64,
    /// Urgent: included immediately (next-block guarantee)
    pub urgent: u64,
}

impl PriorityFeeEstimator {
    pub fn new(history_blocks: usize) -> Self {
        Self { samples: Vec::new(), capacity: history_blocks.max(1) }
    }

    /// Record tips from a newly finalised block.
    pub fn record_block(&mut self, tips: Vec<u64>) {
        if self.samples.len() >= self.capacity {
            self.samples.remove(0);
        }
        self.samples.push(tips);
    }

    /// Suggest priority fees based on recent history.
    pub fn estimate(&self) -> FeeEstimate {
        let mut all: Vec<u64> = self.samples.iter().flatten().cloned().collect();
        if all.is_empty() {
            // Fallback: 1 gwei across all tiers
            return FeeEstimate { low: 1_000_000_000, medium: 1_500_000_000, high: 2_000_000_000, urgent: 3_000_000_000 };
        }
        all.sort_unstable();

        FeeEstimate {
            low:    Self::percentile(&all, 25),
            medium: Self::percentile(&all, 50),
            high:   Self::percentile(&all, 75),
            urgent: Self::percentile(&all, 95),
        }
    }

    fn percentile(sorted: &[u64], p: usize) -> u64 {
        if sorted.is_empty() { return 0; }
        let idx = ((sorted.len() - 1) * p) / 100;
        sorted[idx]
    }
}