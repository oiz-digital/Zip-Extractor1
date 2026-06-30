//! FeeOracle — shared, thread-safe fee state for ZBX node.
//!
//! The sequencer calls `update_on_new_block` after each block is sealed.
//! RPC handlers call `current_base_fee` / `fee_history` to serve responses.
//!
//! Usage:
//! ```
//! use zbx_rpc::fee_oracle::FeeOracle;
//! use std::sync::Arc;
//!
//! // At node startup
//! let oracle = Arc::new(FeeOracle::new());
//!
//! // After each new block (called by sequencer/sync)
//! oracle.update_on_new_block(block_height, gas_used, gas_limit);
//!
//! // In RPC handler
//! let base_fee = oracle.current_base_fee();
//! let hex = oracle.current_base_fee_hex();
//! ```

use std::sync::RwLock;
use zbx_sequencer::base_fee::{
    compute_next_base_fee,
    recommended_max_fee,
    wei_to_hex, wei_to_gwei,
    INITIAL_BASE_FEE_WEI,
    RECOMMENDED_PRIORITY_FEE_WEI,
    BLOCK_GAS_LIMIT,
    BLOCK_GAS_TARGET,
};

/// Number of historical base fee entries to keep (for eth_feeHistory).
const FEE_HISTORY_SIZE: usize = 100;

/// One block's worth of fee data.
#[derive(Clone, Debug)]
pub struct BlockFeeRecord {
    pub block_height:  u64,
    pub base_fee_wei:  u128,
    pub gas_used:      u64,
    pub gas_limit:     u64,
    /// gas_used / gas_limit as a ratio 0.0–1.0
    pub utilization:   f64,
}

impl BlockFeeRecord {
    fn new(block_height: u64, base_fee_wei: u128, gas_used: u64, gas_limit: u64) -> Self {
        let utilization = if gas_limit == 0 { 0.0 }
                          else { gas_used as f64 / gas_limit as f64 };
        Self { block_height, base_fee_wei, gas_used, gas_limit, utilization }
    }
}

/// Inner state — held inside RwLock.
struct OracleState {
    /// Current base fee (wei). Returned by eth_gasPrice and eth_baseFee.
    current_base_fee:   u128,
    /// Base fee of the NEXT block (computed but not yet on-chain).
    pending_base_fee:   u128,
    /// Height of the most recently processed block.
    latest_height:      u64,
    /// Ring buffer of recent block fee records.
    history:            Vec<BlockFeeRecord>,
}

impl OracleState {
    fn new() -> Self {
        // Seed with a genesis record at height 0
        let genesis = BlockFeeRecord::new(0, INITIAL_BASE_FEE_WEI, BLOCK_GAS_TARGET, BLOCK_GAS_LIMIT);
        Self {
            current_base_fee: INITIAL_BASE_FEE_WEI,
            pending_base_fee: INITIAL_BASE_FEE_WEI,
            latest_height:    0,
            history:          vec![genesis],
        }
    }
}

/// Thread-safe fee oracle.
pub struct FeeOracle {
    state: RwLock<OracleState>,
}

impl FeeOracle {
    /// Create a new oracle starting at genesis fee.
    pub fn new() -> Self {
        Self { state: RwLock::new(OracleState::new()) }
    }

    /// Create an oracle with a known starting fee (e.g. loaded from DB on restart).
    pub fn with_starting_fee(base_fee_wei: u128, latest_height: u64) -> Self {
        let mut state = OracleState::new();
        state.current_base_fee = base_fee_wei;
        state.pending_base_fee = base_fee_wei;
        state.latest_height    = latest_height;
        Self { state: RwLock::new(state) }
    }

    /// Called by the sequencer/sync layer each time a new block is finalized.
    ///
    /// Updates current_base_fee using the EIP-1559 algorithm, and computes
    /// the pending base fee for the next block.
    ///
    /// # Arguments
    /// * `block_height` — height of the block just finalized
    /// * `gas_used`     — total gas consumed in that block
    /// * `gas_limit`    — gas limit of that block
    pub fn update_on_new_block(&self, block_height: u64, gas_used: u64, gas_limit: u64) {
        let mut state = self.state.write().expect("fee oracle write lock poisoned");

        let prev_base_fee = state.current_base_fee;
        let next_base_fee = compute_next_base_fee(prev_base_fee, gas_used, gas_limit);

        // Pending = next block's predicted base fee
        let pending = compute_next_base_fee(next_base_fee, BLOCK_GAS_TARGET, gas_limit);

        state.current_base_fee = next_base_fee;
        state.pending_base_fee = pending;
        state.latest_height    = block_height;

        // Append to history ring buffer
        let record = BlockFeeRecord::new(block_height, next_base_fee, gas_used, gas_limit);
        if state.history.len() >= FEE_HISTORY_SIZE {
            state.history.remove(0); // drop oldest
        }
        state.history.push(record);

        tracing::debug!(
            height    = block_height,
            prev_gwei = wei_to_gwei(prev_base_fee),
            next_gwei = wei_to_gwei(next_base_fee),
            gas_used,
            utilization = format!("{:.1}%%", gas_used as f64 / gas_limit as f64 * 100.0),
            "Base fee updated"
        );
    }

    /// Current base fee in wei.
    pub fn current_base_fee(&self) -> u128 {
        self.state.read().expect("fee oracle read lock poisoned").current_base_fee
    }

    /// Current base fee as hex string (for eth_baseFee, eth_gasPrice).
    pub fn current_base_fee_hex(&self) -> String {
        wei_to_hex(self.current_base_fee())
    }

    /// Pending base fee (next block prediction) as hex string.
    pub fn pending_base_fee_hex(&self) -> String {
        wei_to_hex(self.state.read().expect("lock").pending_base_fee)
    }

    /// Recommended max_fee_per_gas for users (base_fee × 2 + priority_tip).
    /// This gives a 1-block buffer against fee spikes.
    pub fn recommended_max_fee_hex(&self) -> String {
        let base = self.current_base_fee();
        wei_to_hex(recommended_max_fee(base))
    }

    /// Recommended priority fee (tip) as hex string.
    pub fn recommended_priority_fee_hex(&self) -> String {
        wei_to_hex(RECOMMENDED_PRIORITY_FEE_WEI)
    }

    /// Latest processed block height.
    pub fn latest_height(&self) -> u64 {
        self.state.read().expect("lock").latest_height
    }

    /// Fee history for eth_feeHistory response.
    /// Returns up to `block_count` recent records, newest-first.
    pub fn fee_history(&self, block_count: usize) -> Vec<BlockFeeRecord> {
        let state = self.state.read().expect("lock");
        let n = block_count.min(state.history.len());
        state.history[state.history.len() - n..].iter().rev().cloned().collect()
    }

    /// Summary info for the `zbx gas` CLI command.
    pub fn gas_summary(&self) -> GasSummary {
        let state  = self.state.read().expect("lock");
        let base   = state.current_base_fee;
        let pend   = state.pending_base_fee;
        let hist   = &state.history;
        let height = state.latest_height;

        let avg_24h = if hist.is_empty() { base }
        else {
            hist.iter().map(|r| r.base_fee_wei).sum::<u128>() / hist.len() as u128
        };

        let last_util = hist.last().map(|r| r.utilization).unwrap_or(0.5);

        GasSummary {
            current_base_fee_wei:  base,
            pending_base_fee_wei:  pend,
            priority_fee_wei:      RECOMMENDED_PRIORITY_FEE_WEI,
            max_fee_wei:           recommended_max_fee(base),
            avg_24h_base_fee_wei:  avg_24h,
            latest_height:         height,
            last_block_utilization: last_util,
        }
    }
}

impl Default for FeeOracle {
    fn default() -> Self { Self::new() }
}

/// Gas summary for display.
#[derive(Debug, Clone)]
pub struct GasSummary {
    pub current_base_fee_wei:   u128,
    pub pending_base_fee_wei:   u128,
    pub priority_fee_wei:       u128,
    pub max_fee_wei:            u128,
    pub avg_24h_base_fee_wei:   u128,
    pub latest_height:          u64,
    pub last_block_utilization: f64,
}

impl GasSummary {
    pub fn base_fee_gwei(&self)  -> f64 { wei_to_gwei(self.current_base_fee_wei) }
    pub fn pending_gwei(&self)   -> f64 { wei_to_gwei(self.pending_base_fee_wei) }
    pub fn priority_gwei(&self)  -> f64 { wei_to_gwei(self.priority_fee_wei) }
    pub fn max_fee_gwei(&self)   -> f64 { wei_to_gwei(self.max_fee_wei) }
    pub fn avg_gwei(&self)       -> f64 { wei_to_gwei(self.avg_24h_base_fee_wei) }
    pub fn utilization_pct(&self)-> f64 { self.last_block_utilization * 100.0 }
}