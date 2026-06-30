//! Batch executor — processes a full block's transactions in order.

use std::time::Instant;

use zbx_primitives::{Address, U256};
use zbx_tx::Transaction;

/// Type aliases for hash types
pub type BlockHash = [u8; 32];
pub type TxHash    = [u8; 32];

/// Block execution context
#[derive(Debug, Clone)]
pub struct BlockContext {
    pub number:          u64,
    pub timestamp:       u64,
    pub gas_limit:       u64,
    pub base_fee:        U256,
    pub miner:           Address,
    pub chain_id:        u64,
    pub prev_randao:     [u8; 32],
    pub excess_blob_gas: Option<u64>,
}

/// A log emitted during EVM execution.
#[derive(Debug, Clone, Default)]
pub struct Log {
    pub address: Address,
    pub topics:  Vec<[u8; 32]>,
    pub data:    Vec<u8>,
}

/// Simplified execution result for a single transaction.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success:     bool,
    pub gas_used:    u64,
    pub return_data: Vec<u8>,
    pub logs:        Vec<Log>,
}

impl ExecutionResult {
    pub fn success(return_data: Vec<u8>, gas_used: u64, logs: Vec<Log>) -> Self {
        Self { success: true, gas_used, return_data, logs }
    }
    pub fn failure(gas_used: u64) -> Self {
        Self { success: false, gas_used, return_data: vec![], logs: vec![] }
    }
}

/// Transaction execution receipt
#[derive(Debug, Clone)]
pub struct TxReceipt {
    pub tx_hash:              TxHash,
    pub block_number:         u64,
    pub tx_index:             u32,
    pub gas_used:             u64,
    pub cumulative_gas_used:  u64,
    pub success:              bool,
    pub contract_address:     Option<Address>,
    pub logs_bloom:           [u8; 256],
    pub return_data:          Vec<u8>,
}

/// Block execution result
#[derive(Debug, Default)]
pub struct BlockExecutionResult {
    pub receipts:        Vec<TxReceipt>,
    pub total_gas_used:  u64,
    pub failed_txs:      usize,
    pub state_root:      [u8; 32],
    pub receipts_root:   [u8; 32],
    pub elapsed_ms:      u64,
    pub gas_refunds:     u64,
}

/// Batch executor config
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub max_gas_per_block:   u64,
    pub enable_parallel:     bool,
    pub verify_signatures:   bool,
    pub skip_balance_check:  bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_gas_per_block:  30_000_000,
            enable_parallel:    true,
            verify_signatures:  true,
            skip_balance_check: false,
        }
    }
}

/// Batch executor — executes transactions one by one in block order.
pub struct BatchExecutor {
    pub chain_id: u64,
    pub config:   BatchConfig,
}

impl BatchExecutor {
    pub fn new(chain_id: u64, config: BatchConfig) -> Self {
        Self { chain_id, config }
    }

    /// Execute all transactions in a block (simplified stub).
    pub fn execute_block(
        &mut self,
        block_ctx: &BlockContext,
        txs: &[Transaction],
    ) -> BlockExecutionResult {
        let start = Instant::now();
        let mut result = BlockExecutionResult::default();
        let mut cumulative_gas = 0u64;

        for (idx, tx) in txs.iter().enumerate() {
            let gas_used = tx.gas_limit.min(21_000);
            cumulative_gas += gas_used;

            let tx_hash = compute_tx_hash(tx);
            let receipt = TxReceipt {
                tx_hash,
                block_number:        block_ctx.number,
                tx_index:            idx as u32,
                gas_used,
                cumulative_gas_used: cumulative_gas,
                success:             true,
                contract_address:    None,
                logs_bloom:          [0u8; 256],
                return_data:         vec![],
            };
            result.receipts.push(receipt);
        }

        result.total_gas_used = cumulative_gas;
        result.elapsed_ms = start.elapsed().as_millis() as u64;

        tracing::info!(
            block = block_ctx.number,
            txs   = txs.len(),
            gas_used = result.total_gas_used,
            elapsed_ms = result.elapsed_ms,
            "Block executed"
        );
        result
    }
}

pub fn compute_tx_hash_pub(tx: &Transaction) -> TxHash {
    compute_tx_hash(tx)
}

fn compute_tx_hash(tx: &Transaction) -> TxHash {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(tx.chain_id.to_be_bytes());
    h.update(tx.nonce.to_be_bytes());
    h.update(tx.gas_limit.to_be_bytes());
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_config_defaults_are_sane() {
        let cfg = BatchConfig::default();
        assert_eq!(cfg.max_gas_per_block, 30_000_000);
        assert!(cfg.enable_parallel);
        assert!(cfg.verify_signatures);
        assert!(!cfg.skip_balance_check);
    }

    #[test]
    fn execution_result_success_has_correct_fields() {
        let r = ExecutionResult::success(vec![0xAB], 21_000, vec![]);
        assert!(r.success);
        assert_eq!(r.gas_used, 21_000);
        assert_eq!(r.return_data, vec![0xAB]);
        assert!(r.logs.is_empty());
    }

    #[test]
    fn execution_result_failure_is_marked_failed() {
        let r = ExecutionResult::failure(21_000);
        assert!(!r.success);
        assert_eq!(r.gas_used, 21_000);
        assert!(r.return_data.is_empty());
    }

    #[test]
    fn block_execution_result_defaults_zero() {
        let r = BlockExecutionResult::default();
        assert_eq!(r.total_gas_used, 0);
        assert_eq!(r.failed_txs, 0);
        assert!(r.receipts.is_empty());
    }

    #[test]
    fn batch_executor_constructed_with_chain_id() {
        let exec = BatchExecutor::new(8990, BatchConfig::default());
        assert_eq!(exec.chain_id, 8990);
    }

    #[test]
    fn tx_receipt_fields_accessible() {
        let r = TxReceipt {
            tx_hash:             [0xABu8; 32],
            block_number:        100,
            tx_index:            0,
            gas_used:            21_000,
            cumulative_gas_used: 21_000,
            success:             true,
            contract_address:    None,
            logs_bloom:          [0u8; 256],
            return_data:         vec![],
        };
        assert_eq!(r.block_number, 100);
        assert!(r.contract_address.is_none());
    }
}
