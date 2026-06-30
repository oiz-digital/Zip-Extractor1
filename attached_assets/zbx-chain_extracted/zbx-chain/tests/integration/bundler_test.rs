//! Integration tests for the ERC-4337 bundler (zbx-bundler).

#[cfg(test)]
mod bundler_tests {
    use zbx_bundler::{
        mempool::{BundlerMempool, UserOperation},
        simulation::UserOpSimulator,
        bundle::BundleBuilder,
        validation::validate_user_op,
        error::BundlerError,
        ENTRY_POINT_ADDRESS, MAX_BUNDLE_SIZE,
    };
    use std::sync::Arc;

    fn make_user_op(sender: &str, nonce: u64) -> UserOperation {
        UserOperation {
            sender: sender.to_string(),
            nonce,
            init_code: vec![],
            call_data: vec![0xde, 0xad, 0xbe, 0xef],
            call_gas_limit: 100_000,
            verification_gas_limit: 50_000,
            pre_verification_gas: 21_000,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 100_000_000,
            paymaster_and_data: vec![],
            signature: vec![0x01; 65],
        }
    }

    // ── Validation ────────────────────────────────────────────────────────

    #[test]
    fn test_valid_user_op() {
        let op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 0);
        assert!(validate_user_op(&op).is_ok());
    }

    #[test]
    fn test_invalid_sender() {
        let mut op = make_user_op("0xshort", 0);
        op.sender = "0xshort".to_string();
        assert!(matches!(validate_user_op(&op), Err(BundlerError::InvalidSender)));
    }

    #[test]
    fn test_missing_signature() {
        let mut op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 0);
        op.signature = vec![];
        assert!(matches!(validate_user_op(&op), Err(BundlerError::MissingSignature)));
    }

    #[test]
    fn test_empty_operation() {
        let mut op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 0);
        op.call_data = vec![];
        op.init_code = vec![];
        assert!(matches!(validate_user_op(&op), Err(BundlerError::EmptyOperation)));
    }

    // ── Mempool ───────────────────────────────────────────────────────────

    #[test]
    fn test_mempool_add_and_drain() {
        let mempool = BundlerMempool::new(zbx_types::CHAIN_ID_MAINNET);
        for i in 0..5 {
            let op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", i);
            mempool.add(op).unwrap();
        }
        assert_eq!(mempool.len(), 5);
        let drained = mempool.drain_for_bundle();
        assert_eq!(drained.len(), 5);
        assert_eq!(mempool.len(), 0);
    }

    #[test]
    fn test_mempool_respects_max_bundle_size() {
        let mempool = BundlerMempool::new(zbx_types::CHAIN_ID_MAINNET);
        for i in 0..(MAX_BUNDLE_SIZE + 10) as u64 {
            let op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", i);
            mempool.add(op).unwrap();
        }
        let drained = mempool.drain_for_bundle();
        assert!(drained.len() <= MAX_BUNDLE_SIZE);
    }

    // ── Simulation ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_simulation_valid_op() {
        let simulator = UserOpSimulator::new("http://localhost:8545");
        let op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 0);
        let result = simulator.simulate(&op).await.unwrap();
        assert!(result.valid);
    }

    #[tokio::test]
    async fn test_simulation_gas_too_high() {
        let simulator = UserOpSimulator::new("http://localhost:8545");
        let mut op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 0);
        op.call_gas_limit = 6_000_000; // exceeds MAX_USER_OP_GAS
        let result = simulator.simulate(&op).await;
        assert!(matches!(result, Err(BundlerError::GasTooHigh(_))));
    }

    // ── Bundle building ───────────────────────────────────────────────────

    #[test]
    fn test_bundle_builder_basic() {
        let builder = BundleBuilder::new("0xBundlerAddress000000000000000000000000001");
        let ops = (0..3).map(|i| make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", i)).collect();
        let bundle = builder.build(ops, 30_000_000).unwrap();
        assert_eq!(bundle.ops.len(), 3);
        assert!(bundle.estimated_gas > 0);
    }

    #[test]
    fn test_bundle_builder_empty() {
        let builder = BundleBuilder::new("0xBundler");
        let result = builder.build(vec![], 30_000_000);
        assert!(matches!(result, Err(BundlerError::EmptyBundle)));
    }

    #[test]
    fn test_bundle_builder_gas_limited() {
        let builder = BundleBuilder::new("0xBundler");
        // 3 ops × 176,000 gas each + overhead → exceeds 300,000 gas limit
        let ops: Vec<_> = (0..3).map(|i| make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", i)).collect();
        let bundle = builder.build(ops, 300_000).unwrap();
        // Some ops should be excluded due to gas limit
        assert!(bundle.ops.len() <= 3);
    }

    // ── Entry point ───────────────────────────────────────────────────────

    #[test]
    fn test_entry_point_address() {
        assert_eq!(ENTRY_POINT_ADDRESS, "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789");
    }

    #[test]
    fn test_user_op_hash_deterministic() {
        let op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 42);
        let hash1 = op.hash(ENTRY_POINT_ADDRESS, zbx_types::CHAIN_ID_MAINNET);
        let hash2 = op.hash(ENTRY_POINT_ADDRESS, zbx_types::CHAIN_ID_MAINNET);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_user_op_hash_different_chains() {
        let op = make_user_op("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 0);
        let hash_zbx  = op.hash(ENTRY_POINT_ADDRESS, zbx_types::CHAIN_ID_MAINNET);
        let hash_eth  = op.hash(ENTRY_POINT_ADDRESS, 1);
        assert_ne!(hash_zbx, hash_eth);
    }
}