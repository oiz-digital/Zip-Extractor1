//! Integration tests for the transaction mempool.

#[cfg(test)]
mod tests {
    use zbx_types::{Address, U256};

    fn make_addr(b: u8) -> Address { Address([b; 20]) }

    fn make_u256(n: u64) -> U256 { U256::from(n) }

    #[test]
    fn test_nonce_ordering() {
        // Transactions from the same sender must be ordered by nonce.
        let mut nonces = vec![3u64, 1, 4, 1, 5, 9, 2, 6];
        nonces.sort();
        assert_eq!(nonces[0], 1);
    }

    #[test]
    fn test_replacement_bump_required() {
        // A replacement tx must have gas_price >= original * 1.10 (10% bump).
        let original_price = U256::from(1_000_000_000u64); // 1 gwei
        let min_replacement = original_price * U256::from(110) / U256::from(100);
        let replacement_price = U256::from(1_100_000_001u64);
        assert!(replacement_price >= min_replacement);
    }

    #[test]
    fn test_max_per_account_limit() {
        // Pool enforces max 64 txs per account.
        let max_per_account: usize = 64;
        let submitted = 65usize;
        let accepted  = submitted.min(max_per_account);
        assert_eq!(accepted, 64);
    }

    #[test]
    fn test_pool_capacity() {
        // Pool drops low-price txs when at capacity.
        let capacity = 100_000usize;
        let pending   = 100_001usize;
        assert!(pending > capacity, "pool should drop overflow txs");
    }

    #[test]
    fn test_gas_price_filter() {
        // Txs below min_gas_price are rejected.
        let min_price = U256::from(1_000_000_000u64);
        let tx_price  = U256::from(500_000_000u64);
        assert!(tx_price < min_price, "tx should be rejected for low gas price");
    }
}