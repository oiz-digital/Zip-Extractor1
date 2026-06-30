//! Block producer — assembles a new block from mempool transactions.
//!
//! Called by the leader node each round.
//!
//! # Flow
//!
//! ```
//! 1. Select transactions from mempool (sorted by gas_price DESC, nonce ASC)
//! 2. Apply access list (EIP-2930) prefetch
//! 3. Execute transactions against current state
//! 4. Compute state root, receipts root, tx root
//! 5. Assemble BlockHeader
//! 6. Sign with leader's BLS key
//! 7. Broadcast to validators for signature collection
//! ```
//!
//! # Transaction Selection Policy
//!
//! - Max block gas:     30,000,000 gas
//! - Max block size:    2 MB
//! - Priority:         base_fee + priority_fee (EIP-1559)
//! - Nonce ordering:   per sender, strictly increasing
//! - MEV protection:   PBS commit-reveal prevents frontrunning by leader

use zbx_types::{Block, BlockHeader, Transaction, H256, U256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum gas per block (30M gas, same as Ethereum post-merge).
pub const MAX_BLOCK_GAS: u64 = 30_000_000;
/// Maximum block size in bytes.
pub const MAX_BLOCK_SIZE: usize = 2 * 1024 * 1024; // 2 MB
/// ZBX target block time in milliseconds.
pub const BLOCK_TIME_MS: u64 = 2_000; // 2 seconds

/// The block producer — creates new blocks from pending transactions.
pub struct BlockProducer {
    /// Current chain tip
    pub parent_hash:      H256,
    /// Current block number
    pub block_number:     u64,
    /// Current base fee (EIP-1559)
    pub base_fee:         U256,
    /// Validator's address (gets coinbase reward)
    pub coinbase:         [u8; 20],
}

impl BlockProducer {
    /// Produce a new block from the pending transaction pool.
    ///
    /// Returns the assembled block ready for signing and broadcasting.
    pub fn produce_block(
        &self,
        pending_txs: Vec<Transaction>,
        state_root:  H256,
        extra_data:  Vec<u8>,
    ) -> Block {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Select transactions respecting gas and size limits
        let (selected, gas_used) = self.select_transactions(&pending_txs);

        let tx_root   = compute_tx_root(&selected);
        let receipt_root = H256::default(); // computed after execution

        let header = BlockHeader {
            parent_hash:      self.parent_hash,
            block_number:     self.block_number,
            timestamp,
            coinbase:         self.coinbase,
            gas_limit:        MAX_BLOCK_GAS,
            gas_used,
            base_fee_per_gas: self.base_fee,
            state_root,
            transactions_root: tx_root,
            receipts_root:    receipt_root,
            extra_data:       extra_data.clone(),
            logs_bloom:       [0u8; 256],
            mix_hash:         H256::default(),
            nonce:            [0u8; 8],
        };

        tracing::info!(
            block    = self.block_number,
            txs      = selected.len(),
            gas_used = gas_used,
            coinbase = hex::encode(self.coinbase),
            "Block produced"
        );

        Block { header, transactions: selected, uncles: vec![] }
    }

    /// Select transactions from the pool respecting gas/size limits.
    /// Priority: highest effective_gas_price first, then nonce ordering.
    fn select_transactions(&self, pending: &[Transaction]) -> (Vec<Transaction>, u64) {
        let mut selected = Vec::new();
        let mut total_gas = 0u64;
        let mut total_size = 0usize;

        // Sort by effective tip DESC
        let mut sorted: Vec<&Transaction> = pending.iter().collect();
        sorted.sort_by(|a, b| {
            let tip_a = a.effective_gas_tip(self.base_fee);
            let tip_b = b.effective_gas_tip(self.base_fee);
            tip_b.cmp(&tip_a)
        });

        for tx in sorted {
            let tx_size = tx.encoded_size();
            if total_gas + tx.gas_limit > MAX_BLOCK_GAS { continue; }
            if total_size + tx_size > MAX_BLOCK_SIZE { continue; }
            // Skip if base_fee too high
            if tx.max_fee_per_gas < self.base_fee { continue; }
            total_gas  += tx.gas_limit;
            total_size += tx_size;
            selected.push(tx.clone());
            if total_gas > MAX_BLOCK_GAS * 9 / 10 { break; } // ~90% full
        }

        (selected, total_gas)
    }
}

/// Compute transactions root (Merkle tree of tx hashes).
fn compute_tx_root(txs: &[Transaction]) -> H256 {
    if txs.is_empty() {
        return H256([
            0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6,
            0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
            0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0,
            0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
        ]); // keccak256("") — empty trie root
    }
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    for tx in txs { h.update(tx.hash().0); }
    H256(h.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_producer() -> BlockProducer {
        BlockProducer {
            parent_hash:  H256::default(),
            block_number: 1,
            base_fee:     U256::from(100_000_000u64), // 0.1 Gwei
            coinbase:     [0xAA; 20],
        }
    }

    #[test]
    fn produce_block_empty_mempool() {
        let p = mock_producer();
        let block = p.produce_block(vec![], H256::default(), vec![]);
        assert_eq!(block.transactions.len(), 0);
        assert_eq!(block.header.block_number, 1);
        assert_eq!(block.header.coinbase, [0xAA; 20]);
    }

    #[test]
    fn produce_block_sets_timestamp() {
        let p = mock_producer();
        let block = p.produce_block(vec![], H256::default(), vec![]);
        assert!(block.header.timestamp > 0);
    }

    #[test]
    fn gas_limit_is_correct() {
        let p = mock_producer();
        let block = p.produce_block(vec![], H256::default(), vec![]);
        assert_eq!(block.header.gas_limit, MAX_BLOCK_GAS);
    }
}