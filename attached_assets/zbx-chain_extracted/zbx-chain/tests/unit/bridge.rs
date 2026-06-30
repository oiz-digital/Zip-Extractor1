//! Unit tests for zbx-bridge — exercises BridgeLockContract directly.
//!
//! All tests here import and test the real `BridgeLockContract` implementation.
//! The previous version contained logic-only stubs with no integration with the
//! actual contract code (P0-T05 orphan issue); those are replaced below.

#[cfg(test)]
mod bridge_unit {
    use zbx_contracts::bridge_lock::{
        BridgeLockContract, BridgeError, MIN_DEPOSIT_WEI, MAX_RELAYERS,
    };
    use zbx_types::address::Address;

    const CHAIN_ZBX: u64   = 8989;
    const CHAIN_BSC: u64   = 56;
    const OWNER:    Address = Address([0x01u8; 20]);
    const RELAYER1: Address = Address([0xA1u8; 20]);
    const RELAYER2: Address = Address([0xA2u8; 20]);
    const ALICE:    Address = Address([0xCCu8; 20]);
    const BOB_BSC: [u8; 20] = [0xBBu8; 20];
    const ONE_ZBX: u128 = 1_000_000_000_000_000_000;

    fn make_2of3_contract() -> BridgeLockContract {
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 30, 2);
        c.add_relayer(&OWNER, RELAYER1).unwrap();
        c.add_relayer(&OWNER, RELAYER2).unwrap();
        c.add_relayer(&OWNER, Address([0xA3u8; 20])).unwrap();
        c
    }

    /// OUT1 — nonce uniqueness: every deposit must get a distinct ID.
    #[test]
    fn bridge_message_has_unique_nonce() {
        let mut c = make_2of3_contract();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();
            assert!(seen.insert(id), "deposit IDs must be unique — OUT1 regression");
        }
    }

    /// S11-1 — minimum deposit: sub-minimum deposits must be rejected.
    #[test]
    fn deposit_requires_min_amount() {
        let mut c = make_2of3_contract();

        let too_small = MIN_DEPOSIT_WEI - 1;
        let err = c.lock(ALICE, too_small, CHAIN_BSC, BOB_BSC, 1).unwrap_err();
        assert_eq!(err, BridgeError::DepositTooSmall,
            "S11-1: sub-minimum deposit must be rejected");

        // Exact minimum must succeed.
        c.lock(ALICE, MIN_DEPOSIT_WEI, CHAIN_BSC, BOB_BSC, 1).unwrap();
    }

    /// MS1 — multisig threshold: release requires the configured quorum.
    #[test]
    fn bridge_proof_requires_confirmations() {
        let mut c = make_2of3_contract(); // threshold = 2
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        // 1-of-2 must NOT release.
        let result = c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        assert!(result.is_none(),
            "MS1: below-threshold vote must not release funds");

        // 2-of-2 MUST release.
        let result = c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        assert!(result.is_some(),
            "MS1: threshold met — funds must be released");
    }

    /// MS1 — multisig threshold: numeric sanity check mirrors on-chain rule.
    #[test]
    fn multisig_threshold_enforced() {
        let threshold = 3usize;
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 0, threshold);
        for (i, &r) in [RELAYER1, RELAYER2, Address([0xA3u8; 20])].iter().enumerate() {
            c.add_relayer(&OWNER, r).unwrap();
            // Below threshold votes return None.
            let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();
            let result = c.vote_release(&r, id, CHAIN_ZBX).unwrap();
            if i + 1 < threshold {
                assert!(result.is_none(), "vote {} of {} must not release", i + 1, threshold);
            } else {
                assert!(result.is_some(), "vote {} of {} must release",  i + 1, threshold);
            }
        }
    }

    /// Replay protection — the same deposit cannot be released twice.
    #[test]
    fn bridge_replay_attack_prevented() {
        let mut c = make_2of3_contract();
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();

        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap(); // finalises

        // Third vote on an already-released deposit must error.
        let err = c.vote_release(&Address([0xA3u8; 20]), id, CHAIN_ZBX).unwrap_err();
        assert_eq!(err, BridgeError::AlreadyReleased,
            "replay must be rejected after finalisation");
    }

    /// S11-2 — fee deducted correctly.
    #[test]
    fn fee_deducted_correctly() {
        let fee_bps: u128 = 30; // 0.3%
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, fee_bps, 1);
        c.add_relayer(&OWNER, RELAYER1).unwrap();

        let gross = ONE_ZBX;
        let (id, fee, net) = c.lock(ALICE, gross, CHAIN_BSC, BOB_BSC, 1).unwrap();

        let expected_fee = gross * fee_bps / 10_000;
        assert_eq!(fee, expected_fee,             "S11-2: 0.3% fee");
        assert_eq!(net, gross - fee,              "S11-2: net = gross - fee");
        assert_eq!(net + fee, gross,              "S11-2: fee + net == gross");
        assert_eq!(c.deposit(id).unwrap().amount, net, "S11-2: stored amount is net");
    }

    /// OUT2 — source-binding: wrong source chain must be rejected.
    #[test]
    fn source_binding_enforced() {
        let mut c = make_2of3_contract();
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();

        let err = c.vote_release(&RELAYER1, id, 1 /* ETH, wrong */).unwrap_err();
        assert_eq!(err, BridgeError::SourceChainMismatch,
            "OUT2: wrong source chain must be rejected");
    }

    /// S11-6 — relayer set is bounded.
    #[test]
    fn relayer_set_bounded() {
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 0, 1);
        for i in 0..(MAX_RELAYERS as u8) {
            c.add_relayer(&OWNER, Address([i; 20])).unwrap();
        }
        let err = c.add_relayer(&OWNER, Address([0xFF; 20])).unwrap_err();
        assert_eq!(err, BridgeError::RelayerSetFull,
            "S11-6: relayer set must not exceed MAX_RELAYERS");
    }
}
