//! Block builder — proposes and seals new blocks.
//!
//! After sealing, calls FeeOracle::update_on_new_block so the RPC layer
//! serves correct dynamic gas prices immediately.

use std::sync::Arc;
use tracing::{info, debug};
use zbx_mempool::Mempool;
use zbx_state::StateDb;
use crate::base_fee::{
    compute_next_base_fee,
    wei_to_gwei,
    INITIAL_BASE_FEE_WEI,
    BLOCK_GAS_LIMIT,
};

pub struct BlockBuilder {
    pub chain_id:   u64,
    /// Current block's base fee (from oracle, set at startup/sync)
    pub base_fee:   u128,
    pub gas_limit:  u64,
    mempool:        Arc<Mempool>,
    state:          Arc<StateDb>,
}

impl BlockBuilder {
    pub fn new(chain_id: u64, mempool: Arc<Mempool>, state: Arc<StateDb>) -> Self {
        Self {
            chain_id,
            base_fee:  INITIAL_BASE_FEE_WEI,
            gas_limit: BLOCK_GAS_LIMIT,
            mempool,
            state,
        }
    }

    /// Build and seal the next block.
    ///
    /// Returns the sealed block and the next block's computed base fee.
    /// Caller (consensus engine) must call fee_oracle.update_on_new_block()
    /// with the returned gas_used.
    pub fn build_block(&mut self, height: u64, proposer: [u8; 20]) -> SealedBlock {
        let txs = self.mempool.drain_ready(self.base_fee, self.gas_limit);

        debug!(
            height,
            txs          = txs.len(),
            base_fee_gwei = wei_to_gwei(self.base_fee),
            "Building block"
        );

        let gas_used: u64 = txs.iter().map(|tx| tx.gas_used).sum();

        // Apply transactions to state
        for tx in &txs {
            let _ = self.state.apply_tx(tx);
        }

        // Compute NEXT block's base fee using this block's gas data
        let next_base_fee = compute_next_base_fee(self.base_fee, gas_used, self.gas_limit);

        let block = SealedBlock {
            height,
            proposer,
            transactions: txs,
            gas_used,
            gas_limit: self.gas_limit,
            base_fee: self.base_fee,           // this block's base fee
            next_base_fee,                     // informational (logged)
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        info!(
            height,
            gas_used,
            utilization = format!("{:.1}%", gas_used as f64 / self.gas_limit as f64 * 100.0),
            base_fee_gwei  = wei_to_gwei(self.base_fee),
            next_base_fee_gwei = wei_to_gwei(next_base_fee),
            txs = block.transactions.len(),
            "Block sealed"
        );

        // Update builder's base_fee for next block proposal
        self.base_fee = next_base_fee;

        block
    }
}

#[derive(Debug, Clone)]
pub struct SealedBlock {
    pub height:        u64,
    pub proposer:      [u8; 20],
    pub transactions:  Vec<zbx_mempool::PendingTx>,
    pub gas_used:      u64,
    pub gas_limit:     u64,
    pub base_fee:      u128,      // this block
    pub next_base_fee: u128,      // predicted next block
    pub timestamp:     u64,
}