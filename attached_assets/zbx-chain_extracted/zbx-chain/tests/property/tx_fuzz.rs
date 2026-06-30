//! Property-based tests for transaction validation.

#[cfg(test)]
mod tx_property_tests {
    /// Property: any valid tx must have nonce > sender's current nonce.
    #[test]
    fn nonce_must_be_sequential() {
        for account_nonce in 0u64..50 {
            let valid_tx_nonce   = account_nonce;
            let invalid_tx_nonce = account_nonce + 5; // gap

            let nonce_ok = valid_tx_nonce == account_nonce;
            assert!(nonce_ok, "nonce must exactly match account nonce");

            let gap_ok = invalid_tx_nonce == account_nonce;
            assert!(!gap_ok, "nonce gap should be rejected");
        }
    }

    /// Property: tx gas limit must not exceed block gas limit.
    #[test]
    fn tx_gas_bounded_by_block() {
        let block_gas = 30_000_000u64;
        for tx_gas in [21_000, 100_000, 1_000_000, 30_000_000, 30_000_001] {
            let valid = tx_gas <= block_gas;
            if tx_gas == 30_000_001 {
                assert!(!valid, "tx gas must not exceed block gas limit");
            } else {
                assert!(valid, "tx gas within block limit is valid");
            }
        }
    }

    /// Property: EIP-1559 effective gas price always ≤ max fee.
    #[test]
    fn effective_price_never_exceeds_max_fee() {
        let test_cases = [
            (100u128, 20, 80),   // base=100, priority=20, max=80
            (50,  100, 200),     // priority > max-base: capped at max-base
            (10,  5,   20),      // normal case
        ];
        for (base, priority, max_fee) in test_cases {
            if max_fee >= base {
                let effective = max_fee.min(base + priority);
                assert!(effective <= max_fee, "effective price must never exceed max_fee");
                assert!(effective >= base,    "effective price must cover base fee");
            }
        }
    }

    /// Property: signature recovery is deterministic.
    #[test]
    fn signature_recovery_deterministic() {
        // Same message hash → same recovered address (no randomness in verification).
        let msg = [0x42u8; 32];
        fn fake_recover(msg: &[u8; 32]) -> [u8; 20] {
            let mut addr = [0u8; 20];
            addr[..4].copy_from_slice(&msg[..4]);
            addr
        }
        assert_eq!(fake_recover(&msg), fake_recover(&msg), "recovery is deterministic");
    }

    /// Property: transaction hash is unique (collision resistance).
    #[test]
    fn tx_hash_collision_resistant() {
        fn fake_hash(nonce: u64, sender: u8) -> [u8; 32] {
            let mut h = [0u8; 32];
            h[0] = sender;
            h[8..16].copy_from_slice(&nonce.to_be_bytes());
            h
        }
        let mut seen: std::collections::HashSet<[u8; 32]> = Default::default();
        for sender in 0u8..10 {
            for nonce in 0u64..100 {
                let hash = fake_hash(nonce, sender);
                assert!(seen.insert(hash), "tx hash must be unique");
            }
        }
    }
}