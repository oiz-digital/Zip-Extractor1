//! Witness generation — converts raw block data into a ZK-friendly trace.
//!
//! The witness is the private input to the ZK circuit.
//! It contains all intermediate values needed to satisfy the constraints,
//! but is NEVER revealed to the verifier.
//!
//! Witness types:
//!   - BlockWitness:  all execution steps in a block
//!   - TxWitness:     execution steps for a single transaction
//!   - StateWitness:  Merkle path for a single account lookup

use crate::{error::{ProverResult, ProverError}, field::GoldilocksField, MAX_BLOCK_TXS};
use serde::{Deserialize, Serialize};

/// One row of the execution trace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceRow {
    /// Column values for this execution step.
    pub values: Vec<GoldilocksField>,
    /// Step label (for debugging — stripped in production builds).
    #[cfg(debug_assertions)]
    pub label: String,
}

/// Full witness for a block execution proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockWitness {
    pub block_number:    u64,
    pub block_hash:      [u8; 32],
    pub parent_hash:     [u8; 32],
    pub state_root_pre:  [u8; 32],   // state root before execution
    pub state_root_post: [u8; 32],   // state root after execution
    pub tx_count:        u32,
    pub gas_used:        u64,
    /// Full execution trace (one row per EVM step across all txs in block).
    pub trace:           Vec<TraceRow>,
    /// Merkle proofs for all state accesses during execution.
    pub state_proofs:    Vec<Vec<u8>>,
}

/// Witness for a single transaction proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxWitness {
    pub tx_hash:      [u8; 32],
    pub sender:       [u8; 20],
    pub to:           Option<[u8; 20]>,
    pub value:        u128,
    pub nonce:        u64,
    pub gas_limit:    u64,
    pub gas_used:     u64,
    pub sig_r:        [u8; 32],
    pub sig_s:        [u8; 32],
    pub sig_v:        u8,
    pub trace:        Vec<TraceRow>,
}

/// Witness for a state (account/storage) proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateWitness {
    pub address:      [u8; 20],
    pub balance:      u128,
    pub nonce:        u64,
    pub code_hash:    [u8; 32],
    pub storage_root: [u8; 32],
    pub block_number: u64,
    pub state_root:   [u8; 32],
    /// Merkle Patricia Trie path from root → account node.
    pub merkle_path:  Vec<Vec<u8>>,
}

/// Top-level witness enum.
pub enum Witness {
    Block(BlockWitness),
    Transaction(TxWitness),
    State(StateWitness),
}

/// Generates a block witness from raw block execution data.
pub struct WitnessGenerator {
    num_columns: usize,
}

impl WitnessGenerator {
    pub fn new(num_columns: usize) -> Self {
        Self { num_columns }
    }

    /// Generate a BlockWitness by replaying block execution and recording every EVM step.
    ///
    /// In production this hooks into `zbx-evm`'s interpreter to capture
    /// every opcode execution as a trace row.
    pub fn generate_block_witness(
        &self,
        block_number:    u64,
        block_hash:      [u8; 32],
        parent_hash:     [u8; 32],
        state_root_pre:  [u8; 32],
        state_root_post: [u8; 32],
        tx_hashes:       Vec<[u8; 32]>,
        raw_trace:       Vec<Vec<u64>>,
    ) -> ProverResult<BlockWitness> {
        if tx_hashes.len() > MAX_BLOCK_TXS {
            return Err(ProverError::TraceTooLarge {
                size: tx_hashes.len(),
                max:  MAX_BLOCK_TXS,
            });
        }

        // Pad trace height to next power of two.
        let height = raw_trace.len().next_power_of_two();
        let mut trace = Vec::with_capacity(height);

        for raw_row in &raw_trace {
            let values: Vec<GoldilocksField> = raw_row.iter()
                .map(|&v| GoldilocksField::new(v))
                .chain(std::iter::repeat(GoldilocksField::ZERO))
                .take(self.num_columns)
                .collect();
            trace.push(TraceRow {
                values,
                #[cfg(debug_assertions)]
                label: format!("step_{}", trace.len()),
            });
        }

        // Pad with zero rows to reach power-of-two height.
        while trace.len() < height {
            trace.push(TraceRow {
                values: vec![GoldilocksField::ZERO; self.num_columns],
                #[cfg(debug_assertions)]
                label: "padding".into(),
            });
        }

        Ok(BlockWitness {
            block_number,
            block_hash,
            parent_hash,
            state_root_pre,
            state_root_post,
            tx_count:    tx_hashes.len() as u32,
            gas_used:    0, // filled by caller
            trace,
            state_proofs: vec![],
        })
    }

    /// Pad `n` to the next power of two (minimum 64).
    pub fn next_power_of_two(n: usize) -> usize {
        n.next_power_of_two().max(64)
    }
}