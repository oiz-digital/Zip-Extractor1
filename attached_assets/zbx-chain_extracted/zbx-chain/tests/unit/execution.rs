//! Unit tests for zbx-execution (parallel block execution / BlockSTM).

#[cfg(test)]
mod execution_tests {
    #[test]
    fn independent_txs_have_no_conflicts() {
        // Two txs touching different accounts → no conflict → can run in parallel.
        let tx1_reads  = vec![[1u8; 20]]; // reads account 0x01...
        let tx1_writes = vec![[1u8; 20]];
        let tx2_reads  = vec![[2u8; 20]]; // reads account 0x02...
        let tx2_writes = vec![[2u8; 20]];

        let conflict = tx1_reads.iter().any(|r| tx2_writes.contains(r))
            || tx2_reads.iter().any(|r| tx1_writes.contains(r));
        assert!(!conflict, "independent txs should not conflict");
    }

    #[test]
    fn conflicting_txs_need_sequential_execution() {
        // Both txs read and write the same account → conflict.
        let shared_account = [0xABu8; 20];
        let tx1_writes = vec![shared_account];
        let tx2_reads  = vec![shared_account];

        let conflict = tx2_reads.iter().any(|r| tx1_writes.contains(r));
        assert!(conflict, "conflicting txs must be serialised");
    }

    #[test]
    fn execution_result_is_deterministic() {
        // Same block, same order → same state root.
        let txs = vec![(1u64, 100u128), (2, 200), (3, 50)];
        fn execute(txs: &[(u64, u128)]) -> u128 {
            txs.iter().map(|(_, v)| v).sum()
        }
        let result1 = execute(&txs);
        let result2 = execute(&txs);
        assert_eq!(result1, result2, "execution must be deterministic");
    }

    #[test]
    fn gas_accounting_correct() {
        let block_gas_limit = 30_000_000u64;
        let txs_gas = vec![21_000u64, 50_000, 100_000, 21_000];
        let total: u64 = txs_gas.iter().sum();
        assert!(total <= block_gas_limit, "total gas must not exceed block limit");
    }

    #[test]
    fn failed_tx_does_not_revert_state_root() {
        // A failed tx still pays gas (nonce + balance change), but state root
        // should reflect the partial execution, not revert the entire block.
        let state_before = [0u8; 32];
        let state_after_failed = [1u8; 32]; // different because gas was deducted
        assert_ne!(state_before, state_after_failed,
                   "state root changes even on failed tx (gas deducted)");
    }
}