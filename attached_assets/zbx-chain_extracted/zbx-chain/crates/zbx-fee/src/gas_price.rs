//! Gas-price oracle — combines base fee + tip suggestion for legacy txs.

use crate::PriorityFeeEstimator;

/// Legacy gas-price oracle for `eth_gasPrice`.
/// Returns base_fee + medium_tip as the recommended gas price.
pub struct GasPriceOracle {
    tip_estimator: PriorityFeeEstimator,
    last_base_fee: u64,
}

impl GasPriceOracle {
    pub fn new(history_blocks: usize) -> Self {
        Self {
            tip_estimator: PriorityFeeEstimator::new(history_blocks),
            last_base_fee: 1_000_000_000, // 1 gwei initial
        }
    }

    pub fn update(&mut self, base_fee: u64, tips: Vec<u64>) {
        self.last_base_fee = base_fee;
        self.tip_estimator.record_block(tips);
    }

    /// Recommended gas price for `eth_gasPrice` (base + medium tip).
    pub fn gas_price(&self) -> u64 {
        let tip = self.tip_estimator.estimate().medium;
        self.last_base_fee.saturating_add(tip)
    }

    /// Recommended gas price for fast inclusion (base + high tip).
    pub fn fast_gas_price(&self) -> u64 {
        let tip = self.tip_estimator.estimate().high;
        self.last_base_fee.saturating_add(tip)
    }

    pub fn base_fee(&self) -> u64 { self.last_base_fee }
}