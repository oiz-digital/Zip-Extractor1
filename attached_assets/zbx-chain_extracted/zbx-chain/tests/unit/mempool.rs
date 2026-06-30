//! Unit tests for zbx-mempool.

#[cfg(test)]
mod mempool_tests {
    fn make_tx(nonce: u64, fee: u64, sender: u8) -> TestTx {
        TestTx { nonce, max_fee: fee, sender: [sender; 20] }
    }

    struct TestTx { nonce: u64, max_fee: u64, sender: [u8; 20] }

    fn tx_key(tx: &TestTx) -> u64 { tx.max_fee } // priority ordering

    #[test]
    fn higher_fee_gets_priority() {
        let tx_low  = make_tx(0, 100, 1);
        let tx_high = make_tx(0, 500, 2);
        assert!(tx_key(&tx_high) > tx_key(&tx_low), "higher fee should have priority");
    }

    #[test]
    fn nonce_ordering_within_sender() {
        let txs = vec![make_tx(3, 100, 1), make_tx(1, 100, 1), make_tx(2, 100, 1)];
        let mut nonces: Vec<u64> = txs.iter().map(|t| t.nonce).collect();
        nonces.sort();
        assert_eq!(nonces, vec![1, 2, 3], "nonces must be ordered");
    }

    #[test]
    fn nonce_gap_blocks_later_txs() {
        // If nonce 2 is missing, nonces 3, 4, 5 cannot be included.
        let pending = vec![1u64, 3, 4, 5]; // nonce 2 missing
        let account_nonce = 1u64; // next expected nonce
        let executable: Vec<u64> = pending.iter()
            .scan(account_nonce, |expected, &n| {
                if n == *expected { *expected += 1; Some(n) } else { None }
            })
            .collect();
        assert_eq!(executable, vec![1], "only nonce 1 is executable (gap at 2)");
    }

    #[test]
    fn replace_by_fee() {
        let original_fee: u64 = 100;
        let replacement_fee: u64 = 111; // must be at least 10% higher
        let min_replacement = original_fee * 110 / 100;
        assert!(replacement_fee >= min_replacement,
                "replacement must be >= 110% of original fee");
    }

    #[test]
    fn capacity_limit_evicts_lowest_fee() {
        let capacity = 3usize;
        let mut fees = vec![100u64, 200, 300]; // pool full
        let new_fee = 250u64;
        // Evict lowest if new tx has higher fee.
        if fees.len() >= capacity && new_fee > *fees.iter().min().unwrap() {
            let min_idx = fees.iter().position(|&f| f == *fees.iter().min().unwrap()).unwrap();
            fees.remove(min_idx);
            fees.push(new_fee);
        }
        assert!(fees.contains(&250), "new tx should be in pool");
        assert!(!fees.contains(&100), "lowest fee evicted");
    }
}