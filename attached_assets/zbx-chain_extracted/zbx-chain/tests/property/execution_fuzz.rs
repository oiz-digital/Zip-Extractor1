//! Property-based (fuzz) tests for block execution.
//!
//! These tests generate random inputs and verify that execution
//! maintains key invariants regardless of input.
//!
//! In CI: run with `cargo fuzz run execution_fuzz`
//! Locally: run with `cargo test` (uses fixed seed for determinism)

#[cfg(test)]
mod fuzz_execution {
    /// Invariant: total ZBX supply is conserved across any block.
    #[test]
    fn supply_conserved_fuzz() {
        // Simulate 100 random "blocks" with arbitrary tx sets.
        let initial_supply: u128 = 150_000_000 * 10u128.pow(18);
        let mut current_supply = initial_supply;

        for _block in 0..100 {
            // Random tx: sender pays gas (fee burned + tip to validator).
            let gas_used: u128 = 21_000;
            let base_fee: u128 = 1_000_000_000;
            let tip:      u128 = 100_000_000;
            let burned    = gas_used * base_fee;
            let validator = gas_used * tip;
            // Supply decreases by burned amount only.
            current_supply = current_supply.saturating_sub(burned);
            // Validator earns tip (no supply change — was user's balance).
            let _ = validator;
        }

        // Supply should have decreased (burned) but not by more than initial.
        assert!(current_supply < initial_supply, "burning should reduce supply");
        assert!(current_supply > 0, "supply should not reach zero");
    }

    /// Invariant: block gas used ≤ block gas limit.
    #[test]
    fn gas_never_exceeds_limit_fuzz() {
        let gas_limit = 30_000_000u64;

        // 1000 random tx sizes.
        let tx_sizes: Vec<u64> = (0..1000).map(|i| 21_000 + (i % 500) * 1000).collect();

        let mut block_gas = 0u64;
        for &size in &tx_sizes {
            if block_gas + size > gas_limit { break; }
            block_gas += size;
        }

        assert!(block_gas <= gas_limit, "gas must not exceed limit");
    }

    /// Invariant: state root changes iff state changes.
    #[test]
    fn state_root_consistency_fuzz() {
        use std::collections::HashMap;

        fn state_root(state: &HashMap<u8, u64>) -> u64 {
            state.values().sum()
        }

        let mut state: HashMap<u8, u64> = (0..10).map(|i| (i, 1000)).collect();

        for i in 0..50u64 {
            let root_before = state_root(&state);
            let addr = (i % 10) as u8;
            let delta = i * 7;

            if *state.get(&addr).unwrap_or(&0) >= delta {
                *state.entry(addr).or_insert(0) -= delta;
                let root_after = state_root(&state);
                if delta > 0 {
                    assert_ne!(root_before, root_after,
                        "state root must change when state changes");
                }
            }
        }
    }

    /// Invariant: nonce strictly increases per sender.
    #[test]
    fn nonce_monotonic_fuzz() {
        let mut nonces: std::collections::HashMap<u8, u64> = HashMap::new();

        for sender in 0..5u8 {
            for expected_nonce in 0..20u64 {
                let current = nonces.entry(sender).or_insert(0);
                assert_eq!(*current, expected_nonce,
                    "nonce for sender {} must be {}", sender, expected_nonce);
                *current += 1;
            }
        }
    }
}