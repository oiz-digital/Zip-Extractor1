//! SEC-2026-05-09 Pass-10 (architect-review follow-up):
//! end-to-end tests for the REMOTE-validator equivocation detector
//! wired into both `HotStuff::on_vote` and `HotStuff2::on_vote`.
//!
//! Closes audit blocker (3): "current guard catches our own
//! `on_proposal` double-vote but NOT a remote validator signing two
//! block hashes for the same `(round, phase)`".

use zbx_consensus::error::ConsensusError;
use zbx_consensus::hotstuff::{HotStuffConsensus, ValidatorSet};
use zbx_consensus::safety_rules::SafetyRules;
use zbx_consensus::vote::{EquivocationEvidence, Vote, VoteData};
use zbx_crypto::bls::{BlsPrivKey, BlsPubKey};
use zbx_crypto::keccak::keccak256;
use zbx_types::{address::Address, H256};

fn priv_key(tag: u8) -> BlsPrivKey {
    let mut b = [0u8; 32];
    b[31] = tag;
    BlsPrivKey::from_bytes(&b).unwrap()
}

fn addr(tag: u8) -> Address {
    Address([tag; 20])
}

fn signed_vote(
    sk: &BlsPrivKey,
    voter: Address,
    block_hash_bytes: [u8; 32],
    block_number: u64,
    phase: u8,
    epoch: u64,
) -> (Vote, BlsPubKey) {
    let block_hash = H256(block_hash_bytes);
    let data = VoteData { block_hash, block_number, phase, epoch };
    let msg = keccak256(&data.signing_bytes());
    let sig = sk.sign(&msg);
    let pk = sk.to_pubkey();
    (Vote { data, voter, signature: sig }, pk)
}

fn make_consensus(my_addr: Address, validators: Vec<Address>) -> HotStuffConsensus {
    let sr = SafetyRules::new(priv_key(99), my_addr);
    let mut hs = HotStuffConsensus::new(my_addr, sr, ValidatorSet::new(validators.clone()));
    // Pass-10 architect-review #3 — register the canonical pubkey for
    // every validator. Tests use `priv_key(addr.0[0])` as the canonical
    // key, mirroring the convention in `signed_vote` callers below.
    for addr in &validators {
        let tag = addr.0[0];
        hs.register_validator_pubkey(*addr, priv_key(tag).to_pubkey());
    }
    hs
}

// ───────────────────────── Detector triggers ─────────────────────────

#[test]
fn remote_equivocation_two_hashes_same_round_phase_is_caught() {
    let me  = addr(1);
    let bad = addr(2);
    let good = addr(3);
    let mut hs = make_consensus(me, vec![me, bad, good]);

    let bad_sk = priv_key(2);
    let (v1, pk) = signed_vote(&bad_sk, bad, [0x11; 32], 7, 0, 0);
    let (v2, _)  = signed_vote(&bad_sk, bad, [0x22; 32], 7, 0, 0);

    // First vote accepted.
    assert!(hs.on_vote(v1.clone(), pk.clone()).is_ok());
    assert_eq!(hs.seen_votes_len(), 1);

    // Second vote on a DIFFERENT hash at same (round, phase) → caught.
    let err = hs.on_vote(v2.clone(), pk.clone()).unwrap_err();
    match err {
        ConsensusError::RemoteEquivocation { validator, round, phase, hash_a, hash_b } => {
            assert_eq!(validator, bad);
            assert_eq!(round, 7);
            assert_eq!(phase, 0);
            assert_eq!(hash_a, H256([0x11; 32]));
            assert_eq!(hash_b, H256([0x22; 32]));
        }
        other => panic!("expected RemoteEquivocation, got {other:?}"),
    }

    // Evidence assembly + verification round-trip.
    let ev = hs.build_remote_equivocation_evidence(bad, 7, 0, &v2, &pk)
        .expect("evidence must assemble");
    assert!(ev.verify(), "freshly-built evidence must self-verify");
    assert_eq!(ev.validator, bad);
    assert_eq!(ev.vote_a.data.block_hash, H256([0x11; 32]));
    assert_eq!(ev.vote_b.data.block_hash, H256([0x22; 32]));
}

#[test]
fn same_hash_redelivered_is_not_equivocation() {
    let me = addr(1);
    let v  = addr(2);
    let mut hs = make_consensus(me, vec![me, v, addr(3)]);

    let sk = priv_key(2);
    let (vote, pk) = signed_vote(&sk, v, [0xAA; 32], 5, 0, 0);

    assert!(hs.on_vote(vote.clone(), pk.clone()).is_ok());
    // Replay of the EXACT same vote → NOT equivocation. Accumulator
    // returns DuplicateVote, but that is a benign error, NOT
    // RemoteEquivocation.
    let r = hs.on_vote(vote, pk);
    assert!(
        !matches!(r, Err(ConsensusError::RemoteEquivocation { .. })),
        "identical-hash replay must not raise RemoteEquivocation"
    );
}

#[test]
fn different_phases_are_not_equivocation() {
    let me = addr(1);
    let v  = addr(2);
    let mut hs = make_consensus(me, vec![me, v, addr(3)]);
    let sk = priv_key(2);

    let (a, pk) = signed_vote(&sk, v, [0x01; 32], 9, 0, 0); // Prepare
    let (b, _)  = signed_vote(&sk, v, [0x02; 32], 9, 1, 0); // PreCommit

    assert!(hs.on_vote(a, pk.clone()).is_ok());
    // Different phase → keyed by (round, phase, voter), so no clash.
    let r = hs.on_vote(b, pk);
    assert!(
        !matches!(r, Err(ConsensusError::RemoteEquivocation { .. })),
        "different phase must not raise RemoteEquivocation"
    );
}

#[test]
fn different_validators_are_not_equivocation() {
    let me  = addr(1);
    let v_a = addr(2);
    let v_b = addr(3);
    let mut hs = make_consensus(me, vec![me, v_a, v_b]);

    let (a, pk_a) = signed_vote(&priv_key(2), v_a, [0x01; 32], 4, 0, 0);
    let (b, pk_b) = signed_vote(&priv_key(3), v_b, [0x02; 32], 4, 0, 0);

    assert!(hs.on_vote(a, pk_a).is_ok());
    let r = hs.on_vote(b, pk_b);
    assert!(
        !matches!(r, Err(ConsensusError::RemoteEquivocation { .. })),
        "different validators voting different blocks is normal disagreement, \
         not equivocation"
    );
}

// ───────────────────────── Pruning ─────────────────────────

#[test]
fn prune_seen_votes_drops_committed_rounds() {
    let me = addr(1);
    let v  = addr(2);
    let mut hs = make_consensus(me, vec![me, v, addr(3)]);
    let sk = priv_key(2);

    for r in 0u64..5 {
        let (vote, pk) = signed_vote(&sk, v, [r as u8; 32], r, 0, 0);
        let _ = hs.on_vote(vote, pk);
    }
    assert_eq!(hs.seen_votes_len(), 5);

    hs.prune_seen_votes_below(3);
    assert_eq!(
        hs.seen_votes_len(), 2,
        "rounds < 3 must be evicted from the detector"
    );
}

// ─────────────── Evidence-verification negative cases ───────────────

#[test]
fn fabricated_evidence_with_wrong_pubkey_fails_verify() {
    // An attacker submits two real-looking votes but with a pubkey that
    // does not match the signer — verify() must catch the mismatch.
    let bad = addr(2);
    let (v1, _) = signed_vote(&priv_key(2), bad, [0x11; 32], 7, 0, 0);
    let (v2, _) = signed_vote(&priv_key(2), bad, [0x22; 32], 7, 0, 0);
    let wrong_pk = priv_key(99).to_pubkey();

    let ev = EquivocationEvidence {
        validator: bad,
        round: 7,
        phase: 0,
        vote_a: v1,
        vote_b: v2,
        pubkey: wrong_pk,
    };
    assert!(!ev.verify(), "wrong pubkey must fail BLS verify()");
}

#[test]
fn malformed_evidence_same_hash_fails_verify() {
    let bad = addr(2);
    let sk = priv_key(2);
    let (v1, pk) = signed_vote(&sk, bad, [0x11; 32], 7, 0, 0);
    let (v2, _)  = signed_vote(&sk, bad, [0x11; 32], 7, 0, 0);
    let ev = EquivocationEvidence {
        validator: bad, round: 7, phase: 0,
        vote_a: v1, vote_b: v2, pubkey: pk,
    };
    assert!(!ev.verify(), "same-hash evidence is not equivocation");
}

#[test]
fn malformed_evidence_round_mismatch_fails_verify() {
    let bad = addr(2);
    let sk = priv_key(2);
    let (v1, pk) = signed_vote(&sk, bad, [0x11; 32], 7, 0, 0);
    let (v2, _)  = signed_vote(&sk, bad, [0x22; 32], 8, 0, 0); // round differs
    let ev = EquivocationEvidence {
        validator: bad, round: 7, phase: 0,
        vote_a: v1, vote_b: v2, pubkey: pk,
    };
    assert!(!ev.verify(), "round mismatch in evidence must fail");
}

// ─────── Architect-review follow-up #2: poisoning-attack defense ───────

#[test]
fn forged_vote_with_invalid_signature_is_dropped_and_does_not_poison_detector() {
    // Attacker submits a vote claiming to be from `victim`, but the
    // signature is garbage. Our pre-detector BLS verify must drop it
    // silently. THEN the victim's REAL honest vote arrives — it MUST
    // be accepted normally and MUST NOT raise RemoteEquivocation.
    let me     = addr(1);
    let victim = addr(2);
    let mut hs = make_consensus(me, vec![me, victim, addr(3)]);

    let victim_sk = priv_key(2);
    let victim_pk = victim_sk.to_pubkey();

    // Forged vote: claim victim signed [0xDE; 32], but signature is zero.
    let forged = Vote {
        data: VoteData {
            block_hash: H256([0xDE; 32]),
            block_number: 4,
            phase: 0,
            epoch: 0,
        },
        voter: victim,
        signature: zbx_crypto::bls::BlsSignature([0u8; 96]),
    };
    // MUST be dropped (no error, no detector entry).
    assert!(hs.on_vote(forged, victim_pk.clone()).is_ok());
    assert_eq!(
        hs.seen_votes_len(), 0,
        "forged vote must NOT poison the equivocation detector"
    );

    // Honest vote from victim on a different hash — MUST be accepted.
    let (honest, _) = signed_vote(&victim_sk, victim, [0xAB; 32], 4, 0, 0);
    let r = hs.on_vote(honest, victim_pk);
    assert!(
        !matches!(r, Err(ConsensusError::RemoteEquivocation { .. })),
        "honest vote must NOT trigger false RemoteEquivocation after \
         a forged-vote poisoning attempt; got {r:?}"
    );
    assert_eq!(hs.seen_votes_len(), 1, "honest vote must populate detector");
}

#[test]
fn forged_vote_does_not_block_subsequent_honest_accumulation() {
    // Same as above but exercise the full path with multiple honest
    // votes for the same block — no false equivocation must fire.
    let me = addr(1);
    let v1 = addr(2);
    let v2 = addr(3);
    let mut hs = make_consensus(me, vec![me, v1, v2]);

    let sk1 = priv_key(2);
    let pk1 = sk1.to_pubkey();
    let sk2 = priv_key(3);
    let pk2 = sk2.to_pubkey();

    // Attacker forges a vote in v1's name on hash X.
    let forged = Vote {
        data: VoteData {
            block_hash: H256([0xFF; 32]),
            block_number: 6, phase: 0, epoch: 0,
        },
        voter: v1,
        signature: zbx_crypto::bls::BlsSignature([0u8; 96]),
    };
    assert!(hs.on_vote(forged, pk1.clone()).is_ok());

    // Honest votes from v1 and v2 on hash Y.
    let (a, _) = signed_vote(&sk1, v1, [0x77; 32], 6, 0, 0);
    let (b, _) = signed_vote(&sk2, v2, [0x77; 32], 6, 0, 0);

    let r1 = hs.on_vote(a, pk1);
    let r2 = hs.on_vote(b, pk2);
    assert!(!matches!(r1, Err(ConsensusError::RemoteEquivocation { .. })),
            "honest v1 vote must not trigger false equivocation: {r1:?}");
    assert!(!matches!(r2, Err(ConsensusError::RemoteEquivocation { .. })),
            "honest v2 vote must not trigger false equivocation: {r2:?}");
}

// ─────── Architect-review follow-up #3: voter↔pubkey binding ───────

#[test]
fn vote_with_mismatched_pubkey_is_dropped_no_poisoning() {
    // Attacker signs with their OWN key (priv_key(99)) but sets
    // vote.voter = victim. Supplied pubkey = attacker's. Sig
    // verifies, but the registry binding check must drop the vote
    // because the attacker pubkey != the registered pubkey for victim.
    let me     = addr(1);
    let victim = addr(2);
    let mut hs = make_consensus(me, vec![me, victim, addr(3)]);

    let attacker_sk = priv_key(99);
    let attacker_pk = attacker_sk.to_pubkey();

    // Attacker forges a self-signed vote in victim's name.
    let (forged, _) = signed_vote(&attacker_sk, victim, [0xDE; 32], 4, 0, 0);
    let r = hs.on_vote(forged, attacker_pk);
    assert!(r.is_ok(), "mismatched-pubkey vote must be dropped silently");
    assert_eq!(
        hs.seen_votes_len(), 0,
        "mismatched-pubkey vote must NOT poison the equivocation detector"
    );

    // Honest victim vote on hash B → must NOT raise RemoteEquivocation.
    let victim_sk = priv_key(2);
    let victim_pk = victim_sk.to_pubkey();
    let (honest, _) = signed_vote(&victim_sk, victim, [0xAB; 32], 4, 0, 0);
    let r2 = hs.on_vote(honest, victim_pk);
    assert!(
        !matches!(r2, Err(ConsensusError::RemoteEquivocation { .. })),
        "honest victim vote must be accepted after attacker mismatched-pubkey \
         poisoning attempt; got {r2:?}"
    );
    assert_eq!(hs.seen_votes_len(), 1);
}

#[test]
fn vote_from_unregistered_validator_is_dropped() {
    // Even a member of `validator_set` is dropped if their pubkey is
    // not registered (no auth basis). Confirms the registry is a hard
    // pre-condition for vote acceptance.
    let me = addr(1);
    let v  = addr(2);
    let mut hs = HotStuffConsensus::new(
        me,
        SafetyRules::new(priv_key(99), me),
        ValidatorSet::new(vec![me, v, addr(3)]),
    );
    // Note: no register_validator_pubkey calls.
    let (vote, pk) = signed_vote(&priv_key(2), v, [0xAA; 32], 5, 0, 0);
    assert!(hs.on_vote(vote, pk).is_ok());
    assert_eq!(hs.seen_votes_len(), 0,
        "unregistered validator votes must not enter the detector");
}

// ─────────────── HotStuff2 detector mirror ───────────────

#[test]
fn hotstuff2_detector_also_catches_remote_equivocation() {
    use zbx_consensus::{HotStuff2, vote::QuorumCertificate};
    use zbx_crypto::bls::BlsSignature;

    let me  = addr(1);
    let bad = addr(2);
    let validators = vec![me, bad, addr(3)];
    let genesis = QuorumCertificate {
        vote_data: VoteData {
            block_hash: H256([0u8; 32]), block_number: 0, phase: 0, epoch: 0,
        },
        agg_signature: BlsSignature([0u8; 96]),
        signers: vec![],
        signer_pubkeys: vec![],
    };
    let mut hs2 = HotStuff2::new(genesis, me, validators.clone());
    // Pass-10 architect-review #3 — register pubkeys for every member.
    for a in &validators {
        hs2.register_validator_pubkey(*a, priv_key(a.0[0]).to_pubkey());
    }

    let sk = priv_key(2);
    let (v1, pk) = signed_vote(&sk, bad, [0xAA; 32], 1, 0, 0);
    let (v2, _)  = signed_vote(&sk, bad, [0xBB; 32], 1, 0, 0);

    let _ = hs2.on_vote(v1, pk.clone());
    assert_eq!(hs2.seen_votes_len(), 1);

    let err = hs2.on_vote(v2, pk).unwrap_err();
    assert!(matches!(err, ConsensusError::RemoteEquivocation { .. }),
            "HotStuff2 must mirror the same detector, got {err:?}");
}

#[test]
fn hotstuff2_drops_unregistered_or_mismatched_pubkey_votes() {
    use zbx_consensus::{HotStuff2, vote::QuorumCertificate};
    use zbx_crypto::bls::BlsSignature;

    let me  = addr(1);
    let v   = addr(2);
    let validators = vec![me, v, addr(3)];
    let genesis = QuorumCertificate {
        vote_data: VoteData {
            block_hash: H256([0u8; 32]), block_number: 0, phase: 0, epoch: 0,
        },
        agg_signature: BlsSignature([0u8; 96]),
        signers: vec![],
        signer_pubkeys: vec![],
    };
    let mut hs2 = HotStuff2::new(genesis, me, validators);
    // Register only `me` and `addr(3)`, NOT `v`.
    hs2.register_validator_pubkey(me, priv_key(me.0[0]).to_pubkey());
    hs2.register_validator_pubkey(addr(3), priv_key(3).to_pubkey());

    // Unregistered validator → drop.
    let (vote, pk) = signed_vote(&priv_key(2), v, [0xAA; 32], 1, 0, 0);
    assert!(hs2.on_vote(vote, pk).is_ok());
    assert_eq!(hs2.seen_votes_len(), 0);

    // Now register v with priv_key(2).pubkey, then send a vote signed
    // by attacker key (priv_key(99)) but claiming v. Mismatched pubkey
    // → drop.
    hs2.register_validator_pubkey(v, priv_key(2).to_pubkey());
    let attacker_sk = priv_key(99);
    let (forged, _) = signed_vote(&attacker_sk, v, [0xDE; 32], 1, 0, 0);
    assert!(hs2.on_vote(forged, attacker_sk.to_pubkey()).is_ok());
    assert_eq!(hs2.seen_votes_len(), 0,
        "HS2 must not poison detector on mismatched-pubkey attempt");
}

// ───────────────────────── Pass-11 round-3: dynamic active-set ──────

/// SEC-2026-05-09 Pass-11 architect-review round 3: jailed-validator
/// hot-swap. Confirms `update_validator_set` (a) shrinks quorum
/// (2f+1 recomputed), (b) causes votes from the removed (jailed)
/// validator to be silently dropped via the existing `contains`
/// membership gate in `on_vote`, and (c) does NOT clobber the
/// pubkey registry (so unjail-on-next-epoch works without a
/// false-positive `dropped_unregistered` spike).
#[test]
fn pass11_update_validator_set_evicts_jailed_voter() {
    let me = addr(1);
    let v2 = addr(2);
    let v3 = addr(3);
    let v4 = addr(4);
    let initial = vec![me, v2, v3, v4];
    let mut hs = make_consensus(me, initial.clone());
    // SEC-2026-05-09 Pass-11 round-4: safe BFT quorum is
    // floor(2n/3)+1, not the old 2f+1 (which was unsafe for n != 3f+1).
    assert_eq!(hs.validator_set.quorum, 3, "n=4 → floor(8/3)+1 = 3");

    // Jail v2 — new active set is [me, v3, v4]; n=3 → quorum=3 (must
    // be unanimous; safe but tolerates no faults — the operator's
    // signal to un-jail or add validators).
    let after_jail = vec![me, v3, v4];
    hs.update_validator_set(after_jail.clone());
    assert_eq!(hs.validator_set.validators.len(), 3);
    assert_eq!(hs.validator_set.quorum, 3,
        "n=3 must require unanimous quorum — old 2f+1 formula gave \
         unsafe quorum=1 (single-validator commit). Pass-11 round-4 \
         adopted floor(2n/3)+1 to close this.");

    // v2 is no longer a member; their vote drops via membership gate.
    let (vote, pk) = signed_vote(&priv_key(2), v2, [0xAA; 32], 5, 0, 0);
    let before = hs.dropped_vote_counters().3; // dropped_non_validator
    let res = hs.on_vote(vote, pk);
    assert!(res.is_ok(), "membership drop should be silent (no Err)");
    let after = hs.dropped_vote_counters().3;
    assert_eq!(after, before + 1,
        "jailed validator's vote must increment dropped_non_validator");

    // Pubkey registry MUST still hold v2's key — keeps unjail-recovery
    // path quiet (no false dropped_unregistered spike). Verified
    // indirectly: dropped_unregistered counter does NOT advance for
    // v2's drop above (which incremented dropped_non_validator
    // instead, proving the membership gate fired BEFORE the registry
    // gate — i.e. the registry never got consulted, so we can't
    // observe it directly. But re-adding v2 to the active set and
    // having their vote pass IS the canonical proof:)
    hs.update_validator_set(initial.clone()); // un-jail v2
    let (vote2, pk2) = signed_vote(&priv_key(2), v2, [0xAB; 32], 6, 0, 0);
    let before_unreg = hs.dropped_vote_counters().0;
    let _ = hs.on_vote(vote2, pk2);
    let after_unreg = hs.dropped_vote_counters().0;
    assert_eq!(after_unreg, before_unreg,
        "after un-jail, v2's vote must NOT trip dropped_unregistered \
         (proves pubkey registry was preserved across hot-swap)");
}

/// SEC-2026-05-09 Pass-11 round-4: empty active validator set must
/// fail-fast at construction. Mass-jailing all validators is itself
/// a chain liveness failure and must surface explicitly rather than
/// silently producing a panic mid-round inside `proposer_for_round`.
#[test]
#[should_panic(expected = "empty active validator set")]
fn pass11_empty_validator_set_is_rejected() {
    let _ = zbx_consensus::hotstuff::ValidatorSet::new(vec![]);
}

/// SEC-2026-05-09 Pass-11 round-4: spot-check the safe BFT quorum
/// table for the cardinalities reachable via slashing-driven shrink.
/// Catches regressions that revert to the old `2f+1` formula.
#[test]
fn pass11_safe_bft_quorum_table() {
    use zbx_consensus::hotstuff::ValidatorSet;
    let mk = |n: usize| {
        ValidatorSet::new((0..n as u8).map(addr).collect()).quorum
    };
    assert_eq!(mk(1), 1, "single-validator devnet");
    assert_eq!(mk(2), 2, "unanimous");
    assert_eq!(mk(3), 3, "unanimous (no fault tolerance — safe)");
    assert_eq!(mk(4), 3, "BFT minimum: tolerates f=1");
    assert_eq!(mk(5), 4);
    assert_eq!(mk(6), 5);
    assert_eq!(mk(7), 5, "tolerates f=2");
    assert_eq!(mk(10), 7);
}
