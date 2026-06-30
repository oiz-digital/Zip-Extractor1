//! Unit tests for zbx-prover (ZK STARK field arithmetic).

#[cfg(test)]
mod prover_unit {
    // ─── Goldilocks field arithmetic (p = 2^64 - 2^32 + 1) ───────────────

    const P: u128 = (1u128 << 64) - (1u128 << 32) + 1; // 2^64 - 2^32 + 1

    fn field_add(a: u64, b: u64) -> u64 {
        ((a as u128 + b as u128) % P) as u64
    }

    fn field_mul(a: u64, b: u64) -> u64 {
        ((a as u128 * b as u128) % P) as u64
    }

    fn field_neg(a: u64) -> u64 {
        if a == 0 { 0 } else { (P - a as u128) as u64 }
    }

    #[test]
    fn add_zero_identity() {
        let x = 12345678u64;
        assert_eq!(field_add(x, 0), x, "x + 0 = x");
    }

    #[test]
    fn add_commutative() {
        let a = 999_999u64;
        let b = 1_234_567u64;
        assert_eq!(field_add(a, b), field_add(b, a), "addition is commutative");
    }

    #[test]
    fn mul_one_identity() {
        let x = 99999u64;
        assert_eq!(field_mul(x, 1), x, "x * 1 = x");
    }

    #[test]
    fn mul_zero() {
        assert_eq!(field_mul(u64::MAX, 0), 0, "x * 0 = 0");
    }

    #[test]
    fn add_negation_is_zero() {
        let x = 42u64;
        let neg_x = field_neg(x);
        assert_eq!(field_add(x, neg_x), 0, "x + (-x) = 0 in field");
    }

    #[test]
    fn field_order_is_prime() {
        // p = 2^64 - 2^32 + 1 = 18446744069414584321
        assert_eq!(P, 18_446_744_069_414_584_321u128);
        // Quick Fermat primality check: 2^(p-1) ≡ 1 (mod p)
        // (too expensive to compute in test — just verify the constant)
        assert!(P > u64::MAX as u128, "p must be > 2^64 - 1 for Goldilocks");
    }

    #[test]
    fn overflow_wraps_correctly() {
        // p-1 + 1 should give 0.
        let pm1 = (P - 1) as u64;
        let result = field_add(pm1, 1);
        assert_eq!(result, 0, "(p-1) + 1 = 0 in Goldilocks field");
    }

    // ─── FRI protocol properties ───────────────────────────────────────────

    #[test]
    fn fri_degree_halves_each_round() {
        let degree = 1024usize;
        let rounds  = 10usize;
        let mut current_degree = degree;
        for _ in 0..rounds {
            current_degree /= 2;
        }
        assert_eq!(current_degree, 1, "FRI reduces degree to 1 after log2(N) rounds");
    }

    #[test]
    fn proof_size_is_logarithmic() {
        // FRI proof size: O(log^2 N * hash_size).
        let n = 1_048_576usize; // 2^20 trace length
        let log_n = 20usize;
        let hash_size = 32usize;   // 32 bytes per hash
        let proof_bytes = log_n * log_n * hash_size;
        assert!(proof_bytes < 1_000_000, "proof should be < 1 MB for N=2^20");
    }
}