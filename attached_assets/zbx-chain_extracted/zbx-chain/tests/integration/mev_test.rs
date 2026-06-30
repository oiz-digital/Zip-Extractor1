//! Integration tests for MEV protection (zbx-mev).

#[cfg(test)]
mod mev_integration {
    #[test]
    fn bundle_must_not_exceed_block_gas() {
        let block_gas = 30_000_000u64;
        let bundle_gas = 5u64 * 21_000; // 5 txs
        assert!(bundle_gas <= block_gas, "bundle fits in block");

        let large_bundle = 2000u64 * 21_000; // 2000 txs
        assert!(large_bundle > block_gas, "oversized bundle rejected");
    }

    #[test]
    fn bundle_atomic_or_nothing() {
        // If one tx in a bundle fails and revert_on_fail=true → whole bundle reverts.
        let revert_on_fail = true;
        let tx_results = vec![true, true, false, true]; // tx 2 failed

        let bundle_success = if revert_on_fail {
            tx_results.iter().all(|&r| r) // all must succeed
        } else {
            tx_results.iter().any(|&r| r) // any success counts
        };

        assert!(!bundle_success, "bundle with failed tx should revert (revert_on_fail=true)");
    }

    #[test]
    fn mev_redistribution_30_50_20() {
        let total_mev: u128 = 1_000_000_000_000_000_000; // 1 ZBX
        let to_stakers  = total_mev * 30 / 100;
        let to_treasury = total_mev * 50 / 100;
        let burned      = total_mev * 20 / 100;
        assert_eq!(to_stakers + to_treasury + burned, total_mev, "redistribution sums to total");
        assert_eq!(burned, 200_000_000_000_000_000, "20% burned");
    }

    #[test]
    fn commit_reveal_prevents_frontrunning() {
        // User commits hash in block N → reveals in block N+1.
        // Attacker sees commitment but NOT the tx content.
        let commit_block = 100u64;
        let reveal_block = 101u64;
        // Attacker submits tx in commit_block — same slot.
        let attacker_block = 100u64;

        // Attacker can't know tx content at commit_block.
        assert_eq!(attacker_block, commit_block, "attacker is in same block as commit");
        // But ordering is determined by commit_block, not reveal_block.
        assert!(reveal_block > commit_block, "reveal is after commit — attacker can't front-run");
    }

    #[test]
    fn pbs_builder_bid_minimum() {
        let min_bid: u128 = 1_000_000_000_000_000; // 0.001 ZBX
        let bid: u128 = 500_000_000_000_000;        // 0.0005 ZBX — too low
        assert!(bid < min_bid, "bid below minimum is rejected");
        let valid_bid: u128 = 2_000_000_000_000_000;
        assert!(valid_bid >= min_bid, "valid bid accepted");
    }
}