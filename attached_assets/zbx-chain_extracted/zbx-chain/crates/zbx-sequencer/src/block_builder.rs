//! Block assembler — builds a complete block body from ordered transactions.

use crate::{error::SequencerError, ordering::{OrderedBlock, PendingTx}};

/// A fully assembled (but not yet sealed) block.
#[derive(Debug, Clone)]
pub struct AssembledBlock {
    pub number:       u64,
    pub parent_hash:  [u8; 32],
    pub proposer:     [u8; 20],
    pub txs:          Vec<Vec<u8>>,   // RLP-encoded signed txs
    pub gas_used:     u64,
    pub gas_limit:    u64,
    pub base_fee:     u128,
    pub timestamp:    u64,
    /// Partial: receipts/state root computed by sealer after execution.
    pub state_root:   Option<[u8; 32]>,
    pub receipts_root: Option<[u8; 32]>,
    pub tx_root:      [u8; 32],
}

/// Assembles a block body from ordered transactions.
pub struct BlockAssembler {
    gas_limit:  u64,
    chain_id:   u64,
}

impl BlockAssembler {
    pub fn new(gas_limit: u64, chain_id: u64) -> Self {
        Self { gas_limit, chain_id }
    }

    pub fn assemble(
        &self,
        parent_hash: [u8; 32],
        number:      u64,
        proposer:    [u8; 20],
        ordered:     OrderedBlock,
        base_fee:    u128,
        timestamp:   u64,
    ) -> Result<AssembledBlock, SequencerError> {
        if ordered.txs.is_empty() {
            return Err(SequencerError::EmptyBlock);
        }
        if ordered.total_gas > self.gas_limit {
            return Err(SequencerError::GasLimitExceeded {
                used:  ordered.total_gas,
                limit: self.gas_limit,
            });
        }

        let txs: Vec<Vec<u8>> = ordered.txs.iter().map(|t| t.rlp.clone()).collect();
        let tx_root = Self::compute_tx_root(&txs);

        Ok(AssembledBlock {
            number, parent_hash, proposer, txs,
            gas_used:      ordered.total_gas,
            gas_limit:     self.gas_limit,
            base_fee, timestamp,
            state_root:     None,    // filled by BlockSealer
            receipts_root:  None,    // filled by BlockSealer
            tx_root,
        })
    }

    fn compute_tx_root(txs: &[Vec<u8>]) -> [u8; 32] {
        // Real impl: Patricia trie of RLP-encoded txs, return root.
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        for tx in txs { h.update(tx); }
        h.finalize().into()
    }
}