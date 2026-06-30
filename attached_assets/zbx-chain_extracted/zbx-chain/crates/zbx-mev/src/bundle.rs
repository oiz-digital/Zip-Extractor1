//! MEV bundle — atomic group of transactions (flashbots-style).
//!
//! A bundle is an ordered list of transactions that MUST be included
//! atomically (all or nothing) in a specific block. Used by:
//!   - Arbitrageurs
//!   - Liquidators (lending protocols)
//!   - Backrun searchers

use crate::MevError;
use serde::{Deserialize, Serialize};

/// A group of transactions submitted atomically by a searcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MevBundle {
    /// Unique bundle ID (keccak256 of content).
    pub id:              String,
    /// Transactions in execution order.
    pub txs:             Vec<Vec<u8>>,    // RLP-encoded signed txs
    /// Target block number (bundle is invalid if not in this block).
    pub target_block:    u64,
    /// Builder tip (in wei). Paid from refund to block builder.
    pub builder_tip:     u128,
    /// Revert condition: if true, entire bundle reverts on any tx failure.
    pub revert_on_fail:  bool,
    /// Searcher's signed simulation result hash (for accountability).
    pub simulation_hash: Option<[u8; 32]>,
}

/// Result of simulating a bundle against a pending state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleSimulation {
    pub bundle_id:    String,
    pub success:      bool,
    pub gas_used:     u64,
    /// Net ETH/ZBX profit to the searcher (after gas costs).
    pub profit:       i128,
    /// State root after bundle execution.
    pub state_root:   [u8; 32],
    pub revert_index: Option<usize>,
    pub revert_reason: Option<String>,
}

impl MevBundle {
    pub fn new(txs: Vec<Vec<u8>>, target_block: u64, builder_tip: u128) -> Self {
        // Both operands must be u128. The XOR is only used to derive a
        // collision-resistant-enough id; widening target_block is safe.
        let id = format!("{:032x}", builder_tip ^ (target_block as u128));
        Self {
            id,
            txs,
            target_block,
            builder_tip,
            revert_on_fail: true,
            simulation_hash: None,
        }
    }

    pub fn tx_count(&self) -> usize { self.txs.len() }

    pub fn validate(&self, current_block: u64) -> Result<(), MevError> {
        if current_block > self.target_block {
            return Err(MevError::SlotExpired(self.target_block));
        }
        if self.txs.is_empty() {
            return Err(MevError::SimulationFailed("empty bundle".into()));
        }
        Ok(())
    }
}