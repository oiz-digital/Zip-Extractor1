//! S35-hotstuff-safety: cross-epoch finality preservation tests
//! ==============================================================
//!
//! Closes AUDIT C-11 (cross-epoch finality reversion).
//!
//! **Pre-S35 bug**: `SafetyRules::advance_epoch` reset
//! `locked_round = 0` and `locked_qc = None`. A Byzantine adversary
//! controlling an epoch transition could revert finalised blocks
//! because the safety lock dropped — every validator in the new epoch
//! would happily vote for a block whose parent QC pre-dates the
//! previously-locked QC. The
//! `parent_qc.block_number() < locked_round` safety check in
//! `SafetyRules::vote` collapsed to `< 0`, which never trips.
//!
//! **S35 fix**: PRESERVE `locked_round` and `locked_qc` across epoch
//! transitions. Zebvix uses a single monotonic `BlockHeader.number`
//! `u64` across all epochs (verified in `zbx-types/src/block.rs`),
//! so the old locked QC's `block_number` remains a meaningful
//! strict-monotone safety guard in any new epoch. HotStuff liveness
//! is preserved because an honest leader in the new epoch will see
//! the locked QC via the sync protocol and propose extending it.
//!
//! Each test below defends a specific facet of the closure. If any of
//! these fails, C-11 has reopened.

use zbx_consensus::error::ConsensusError;
use zbx_consensus::safety_rules::SafetyRules;
use zbx_consensus::vote::{QuorumCertificate, VoteData};
use zbx_crypto::bls::{BlsPrivKey, BlsSignature};
use zbx_types::address::Address;

fn priv_key(tag: u8) -> BlsPrivKey {
    let mut b = [0u8; 32];
    b[31] = tag;
    BlsPrivKey::from_bytes(&b).expect("BlsPrivKey::from_bytes(32)")
}

fn fake_qc(block_number: u64, epoch: u64) -> QuorumCertificate {
    // SafetyRules::vote and update_locked_qc only ever read
    // `qc.block_number()` — they never call `qc.verify()` — so a
    // structurally-valid QC with a zero aggregate signature is
    // sufficient for these unit tests. Production callers always
    // construct QCs via VoteAccumulator with real BLS aggregation.
    QuorumCertificate {
        vote_data: VoteData {
            block_hash: zbx_types::H256([0u8; 32]),
            block_number,
            phase: 0,
            epoch,
        },
        agg_signature: BlsSignature([0u8; 96]),
        signers: vec![],
        signer_pubkeys: vec![],
    }
}

fn rules() -> SafetyRules {
    SafetyRules::new(priv_key(1), Address([1u8; 20]))
}

// ───────────────── 1. lock-state preservation ─────────────────

#[test]
fn advance_epoch_preserves_locked_round() {
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(10, 0));
    assert_eq!(sr.state().locked_round, 10);
    sr.advance_epoch(1);
    assert_eq!(sr.state().epoch, 1);
    assert_eq!(
        sr.state().locked_round,
        10,
        "S35 (C-11 closure): advance_epoch MUST NOT reset locked_round; \
         pre-S35 bug would regress this to 0"
    );
}

#[test]
fn advance_epoch_preserves_locked_qc() {
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(7, 0));
    assert!(sr.state().locked_qc.is_some());
    sr.advance_epoch(2);
    assert!(
        sr.state().locked_qc.is_some(),
        "S35 (C-11 closure): advance_epoch MUST NOT reset locked_qc; \
         pre-S35 bug would regress this to None"
    );
    let qc = sr.state().locked_qc.as_ref().expect("preserved");
    assert_eq!(
        qc.block_number(),
        7,
        "locked_qc identity (block_number) preserved across epoch"
    );
}

#[test]
fn multiple_epoch_advances_do_not_lose_locks() {
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(15, 0));
    sr.advance_epoch(1);
    sr.advance_epoch(2);
    sr.advance_epoch(3);
    sr.advance_epoch(7);
    assert_eq!(sr.state().epoch, 7);
    assert_eq!(
        sr.state().locked_round,
        15,
        "locked_round must persist across an arbitrary number of \
         epoch transitions"
    );
    assert!(sr.state().locked_qc.is_some());
}

// ───────────────── 2. monotonicity invariants ─────────────────

#[test]
fn epoch_advances_only_forward() {
    let mut sr = rules();
    sr.advance_epoch(5);
    assert_eq!(sr.state().epoch, 5);
    sr.advance_epoch(3); // strictly less → no-op
    assert_eq!(
        sr.state().epoch,
        5,
        "advance_epoch must not allow epoch regression"
    );
    sr.advance_epoch(5); // equal → no-op
    assert_eq!(sr.state().epoch, 5);
}

#[test]
fn locked_qc_continues_to_advance_within_new_epoch() {
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(10, 0));
    sr.advance_epoch(2);
    assert_eq!(sr.state().locked_round, 10);
    // Within the new epoch, observing a higher 2-chain QC should still
    // advance the locked round monotonically — preservation across
    // epochs must not freeze the in-epoch lock-update path.
    sr.update_locked_qc(fake_qc(25, 2));
    assert_eq!(sr.state().locked_round, 25);
    let qc = sr.state().locked_qc.as_ref().unwrap();
    assert_eq!(qc.block_number(), 25);
}

// ───── 3. end-to-end safety: stale parent QC must be rejected ─────

#[test]
fn vote_in_new_epoch_with_stale_parent_qc_violates_safety() {
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(20, 0));
    sr.advance_epoch(5);

    // Adversary in the new epoch tries to vote for a block whose parent
    // QC lives at round 10 — strictly below the cross-epoch-preserved
    // locked_round of 20. Pre-S35, advance_epoch had cleared
    // locked_round to 0 and this would have been accepted, finalising a
    // fork that erases blocks 11..20.
    let vote_data = VoteData {
        block_hash: zbx_types::H256([0xAA; 32]),
        block_number: 21,
        phase: 0,
        epoch: 5,
    };
    let stale_parent_qc = fake_qc(10, 5);
    let result = sr.vote(vote_data, &stale_parent_qc);
    match result {
        Err(ConsensusError::SafetyViolation(msg)) => {
            assert!(
                msg.contains("locked round"),
                "expected locked-round safety violation, got: {msg}"
            );
        }
        Err(other) => panic!("expected SafetyViolation, got: {other:?}"),
        Ok(_) => panic!(
            "S35 REGRESSION: vote with parent_qc.block_number=10 < \
             cross-epoch-preserved locked_round=20 was ACCEPTED — \
             C-11 has reopened"
        ),
    }
}

#[test]
fn vote_in_new_epoch_extending_locked_qc_succeeds() {
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(20, 0));
    sr.advance_epoch(5);

    // Honest leader in the new epoch extends the cross-epoch-preserved
    // locked QC at exactly round 20 (parent_qc.block_number() ==
    // locked_round, satisfying the >= safety check). Liveness must not
    // be blocked by the cross-epoch lock preservation.
    let vote_data = VoteData {
        block_hash: zbx_types::H256([0xBB; 32]),
        block_number: 21,
        phase: 0,
        epoch: 5,
    };
    let extending_parent_qc = fake_qc(20, 5);
    let vote = sr
        .vote(vote_data, &extending_parent_qc)
        .expect("honest extending vote must succeed across epoch");
    assert_eq!(vote.data.block_number, 21);
}

#[test]
fn cross_epoch_attacker_revert_scenario_blocked() {
    // End-to-end attacker scenario walkthrough:
    //   * Validator has locked QC at round 100 (epoch 1).
    //   * Validator-set rotation triggers epoch 2.
    //   * Byzantine new-epoch leader proposes a block at round 51,
    //     extending an old block 50 (i.e. trying to revert finalised
    //     prefix [51..100]).
    //
    // Pre-S35: advance_epoch wiped locked_round to 0; the proposed
    // vote would slip past safety check 2 because
    // `parent.block_number()=50 < locked_round=0` is false. Adversary
    // wins the revert.
    //
    // Post-S35: locked_round survives epoch transition at 100; the
    // proposed vote fails safety check 2 because `50 < 100` is true.
    // Adversary's revert is rejected.
    let mut sr = rules();
    sr.update_locked_qc(fake_qc(100, 1));
    sr.advance_epoch(2);
    let stale_parent = fake_qc(50, 2);
    let attempted = VoteData {
        block_hash: zbx_types::H256([0xFF; 32]),
        block_number: 51,
        phase: 0,
        epoch: 2,
    };
    let r = sr.vote(attempted, &stale_parent);
    assert!(
        matches!(r, Err(ConsensusError::SafetyViolation(_))),
        "S35 REGRESSION: cross-epoch attacker revert at round 51 \
         (parent=50 < preserved locked_round=100) was not blocked: \
         got {r:?}"
    );
}
