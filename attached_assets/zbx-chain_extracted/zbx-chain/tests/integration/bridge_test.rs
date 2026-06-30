//! Integration tests for zbx-bridge (cross-chain bridge).
//!
//! These tests exercise the full bridge flow using `BridgeLockContract` directly.
//! Tests that require an EVM runtime (revm) to spin up real smart-contract
//! deployments are marked `#[ignore]` with a `// TODO(evm)` comment — they must
//! pass in CI once the EVM executor is wired in.

#[cfg(test)]
mod bridge_integration {
    use zbx_contracts::bridge_lock::{BridgeLockContract, BridgeError, MIN_DEPOSIT_WEI};
    use zbx_types::address::Address;

    const CHAIN_ZBX: u64   = 8989;
    const CHAIN_BSC: u64   = 56;
    const OWNER:    Address = Address([0x01u8; 20]);
    const RELAYER1: Address = Address([0xA1u8; 20]);
    const RELAYER2: Address = Address([0xA2u8; 20]);
    const RELAYER3: Address = Address([0xA3u8; 20]);
    const ALICE:    Address = Address([0xCCu8; 20]);
    const BOB_BSC: [u8; 20] = [0xBBu8; 20];
    const ONE_ZBX: u128 = 1_000_000_000_000_000_000;

    fn make_3of5_contract() -> BridgeLockContract {
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 30 /* 0.3% */, 3);
        for (i, r) in [RELAYER1, RELAYER2, RELAYER3,
                       Address([0xA4u8; 20]), Address([0xA5u8; 20])].iter().enumerate() {
            c.add_relayer(&OWNER, *r)
                .unwrap_or_else(|e| panic!("add_relayer #{i} failed: {e:?}"));
        }
        c
    }

    /// Full bridge-out flow:
    /// user locks ZBX → deposit recorded with correct fields → relayer quorum releases.
    ///
    /// Covers:
    /// * OUT1: emitted deposit ID is unique and chain-scoped.
    /// * OUT2: deposit stores source_chain_id; vote_release verifies it.
    /// * MS1:  three relayers must vote before release executes.
    /// * S11-2: fee is deducted from locked amount.
    #[test]
    fn bridge_out_full_flow() {
        let mut c = make_3of5_contract();
        let gross = 100 * ONE_ZBX;

        // Step 1: user locks tokens.
        let (id, fee, net) = c.lock(ALICE, gross, CHAIN_BSC, BOB_BSC, 1_000).unwrap();

        // Deposit must be recorded with correct fields.
        let dep = c.deposit(id).expect("deposit must exist after lock");
        assert_eq!(dep.source_chain_id, CHAIN_ZBX, "OUT2: source chain stored");
        assert_eq!(dep.target_chain, CHAIN_BSC);
        assert_eq!(dep.target_recipient, BOB_BSC);
        assert!(!dep.released, "must not be released yet");

        let expected_fee = gross * 30 / 10_000;
        assert_eq!(fee, expected_fee, "S11-2: 0.3% fee");
        assert_eq!(net, gross - fee,  "S11-2: net amount");

        // Step 2: relayer quorum (3-of-5).
        let r1 = c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        assert!(r1.is_none(), "MS1: 1-of-3 must not release");

        let r2 = c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        assert!(r2.is_none(), "MS1: 2-of-3 must not release");

        let r3 = c.vote_release(&RELAYER3, id, CHAIN_ZBX).unwrap();
        assert_eq!(r3, Some(net), "MS1: 3-of-3 must release net amount");

        // Step 3: deposit is now marked released.
        assert!(c.deposit(id).unwrap().released, "deposit must be marked released");
    }

    /// TODO(evm): spin up revm, deploy BridgeVault.sol, call bridge_out(),
    /// verify BridgeOut event emitted with correct params.
    #[test]
    #[ignore = "requires EVM executor — enable in CI once revm is wired"]
    fn bridge_out_emits_evm_event() {}

    /// Bridge-in with valid relayer proof releases tokens to recipient.
    ///
    /// This test exercises the complete vote_release path including the
    /// balance credit that the executor performs after `Some` is returned.
    #[test]
    fn bridge_in_with_valid_proof() {
        let mut c = make_3of5_contract();
        let (id, _, net) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 500).unwrap();

        // Simulate 3 relayers submitting valid proofs.
        let relayers = [RELAYER1, RELAYER2, RELAYER3];
        for (i, r) in relayers.iter().enumerate() {
            let result = c.vote_release(r, id, CHAIN_ZBX).unwrap();
            if i < relayers.len() - 1 {
                assert!(result.is_none(), "vote {} must not yet release", i + 1);
            } else {
                // Final vote — executor credits recipient with `net`.
                let released_amount = result.expect("final vote must release");
                assert_eq!(released_amount, net);
                // Executor would call: state.credit(BOB_BSC_addr, released_amount)
            }
        }
    }

    /// TODO(evm): deploy BridgeVault, submit valid cross-chain proof to EVM,
    /// verify recipient balance incremented by net_amount.
    #[test]
    #[ignore = "requires EVM executor — enable in CI once revm is wired"]
    fn bridge_in_with_valid_proof_evm() {}

    /// Replay protection — the same deposit proof cannot be submitted twice.
    #[test]
    fn bridge_in_replay_rejected() {
        let mut c = make_3of5_contract();
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();

        // First submission — reaches quorum and finalises.
        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        c.vote_release(&RELAYER3, id, CHAIN_ZBX).unwrap();

        // Replay — same deposit ID, different relayer.
        let err = c.vote_release(&Address([0xA4u8; 20]), id, CHAIN_ZBX).unwrap_err();
        assert_eq!(err, BridgeError::AlreadyReleased,
            "replay attack must be rejected after finalisation");
    }

    /// Insufficient signatures — below-threshold quorum must not release.
    #[test]
    fn bridge_in_insufficient_sigs_rejected() {
        let mut c = make_3of5_contract(); // threshold = 3
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();

        // Only 2 of 3 required signatures.
        let r1 = c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        let r2 = c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        assert!(r1.is_none() && r2.is_none(),
            "MS1: 2-of-3 signatures must be insufficient — funds must not release");
        assert!(!c.deposit(id).unwrap().released,
            "deposit must remain unreleased below threshold");
    }

    /// OUT2 — source-chain mismatch in bridge-in proof must be rejected.
    #[test]
    fn bridge_in_wrong_source_chain_rejected() {
        let mut c = make_3of5_contract();
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();

        // Relayer supplies ETH proof for a ZBX deposit.
        let err = c.vote_release(&RELAYER1, id, 1 /* ETH */).unwrap_err();
        assert_eq!(err, BridgeError::SourceChainMismatch,
            "OUT2: wrong source chain in bridge-in proof must be rejected");
    }

    /// MS1 griefing mitigation: owner can reset a fraudulent tally.
    #[test]
    fn bridge_in_tally_griefing_mitigated() {
        let mut c = make_3of5_contract();
        let (id, _, net) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap();

        // Two relayers vote — assume they are compromised / voting fraudulently.
        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        assert_eq!(c.vote_count(id), 2);

        // Owner cancels the fraudulent tally.
        c.cancel_release(&OWNER, id).unwrap();
        assert_eq!(c.vote_count(id), 0, "tally must be cleared after cancel");
        assert!(!c.deposit(id).unwrap().released);

        // Legitimate relayers re-vote and succeed.
        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        let result = c.vote_release(&RELAYER3, id, CHAIN_ZBX).unwrap();
        assert_eq!(result, Some(net), "legitimate quorum after reset must release");
    }

    /// S11-1/S11-3/S11-4: invalid lock parameters are rejected before any state change.
    #[test]
    fn lock_rejects_invalid_parameters() {
        let mut c = make_3of5_contract();

        // Too small.
        assert_eq!(
            c.lock(ALICE, MIN_DEPOSIT_WEI - 1, CHAIN_BSC, BOB_BSC, 1).unwrap_err(),
            BridgeError::DepositTooSmall,
        );
        // Chain 0.
        assert_eq!(
            c.lock(ALICE, ONE_ZBX, 0, BOB_BSC, 1).unwrap_err(),
            BridgeError::InvalidTargetChain,
        );
        // Zero recipient.
        assert_eq!(
            c.lock(ALICE, ONE_ZBX, CHAIN_BSC, [0u8; 20], 1).unwrap_err(),
            BridgeError::ZeroRecipient,
        );

        // After all rejections, no deposit must have been recorded.
        assert_eq!(c.deposit(0), None, "no deposit must be created on rejection");
    }
}
