//! SEC-2026-05-09 Pass-19 (Task #9) — Epoch-boundary seed-rotation tests.
//!
//! Validates the three behavioural guarantees of the per-epoch shuffle
//! seed introduced in Pass-15 (HIGH-R03) and wired into the live commit
//! path in Pass-19:
//!
//! 1. **Distinct schedules across epochs.** Two distinct seeds produce
//!    different proposer addresses for the same round number, with
//!    overwhelming probability across a small sample of rounds. This
//!    is the property that defeats the multi-epoch leader-prediction
//!    DoS attack.
//!
//! 2. **Hot-swap preserves seed.** `HotStuffConsensus::update_validator_set`
//!    (called when slashing shrinks the active set) must NOT silently
//!    demote `epoch_seed` to `H256::zero()` — that would re-enable the
//!    predictable round-robin fallback path that HIGH-R03 closed.
//!
//! 3. **Legacy fallback only on zero seed.** When `epoch_seed ==
//!    H256::zero()`, `proposer_for_round` must return the plain
//!    round-robin selection. This preserves bug-for-bug compatibility
//!    with pre-Pass-15 callers / fixtures.

use zbx_consensus::hotstuff::{HotStuffConsensus, ValidatorSet};
use zbx_consensus::safety_rules::SafetyRules;
use zbx_crypto::bls::BlsPrivKey;
use zbx_types::address::Address;
use zbx_types::H256;

fn addrs(n: u8) -> Vec<Address> {
    (1..=n).map(|i| Address([i; 20])).collect()
}

fn priv_key(tag: u8) -> BlsPrivKey {
    let mut b = [0u8; 32];
    b[31] = tag;
    BlsPrivKey::from_bytes(&b).expect("BlsPrivKey::from_bytes(32)")
}

fn seed(byte: u8) -> H256 {
    let mut s = [0u8; 32];
    s[0] = byte;
    H256(s)
}

#[test]
fn rotation_produces_distinct_schedules_across_epochs() {
    // Same validator set, two distinct epoch seeds → schedules must
    // differ on at least one round in a small sample.  Probability
    // of accidental match across 16 rounds with n=4 validators is
    // (1/4)^16 ≈ 2^-32, well below any test-flake threshold.
    let n = 4;
    let validators = addrs(n);
    let mut vs_a = ValidatorSet::new(validators.clone());
    let mut vs_b = ValidatorSet::new(validators.clone());
    vs_a.set_epoch_seed(seed(0xA1));
    vs_b.set_epoch_seed(seed(0xB2));

    let mut diffs = 0;
    for round in 0..16 {
        if vs_a.proposer_for_round(round) != vs_b.proposer_for_round(round) {
            diffs += 1;
        }
    }
    assert!(
        diffs > 0,
        "two distinct seeds produced identical proposer schedules across 16 rounds — \
         keccak-keyed rotation degenerated to a constant"
    );
}

#[test]
fn rotation_seed_changes_observable_at_round_zero() {
    // Targeted: at round 0 specifically, an attacker pre-Pass-15
    // already knew the proposer was `validators[0]`. After rotation
    // round-0 must shuffle. We exercise several seeds to guarantee
    // at least one moves the round-0 leader off `validators[0]`.
    let validators = addrs(4);
    let mut shuffled_off_zero = false;
    for tag in 0u8..32u8 {
        let mut vs = ValidatorSet::new(validators.clone());
        vs.set_epoch_seed(seed(tag.wrapping_add(1))); // skip 0
        if vs.proposer_for_round(0) != validators[0] {
            shuffled_off_zero = true;
            break;
        }
    }
    assert!(
        shuffled_off_zero,
        "no seed in [1..32] moved round-0 proposer off validators[0] — \
         keccak shuffle ignored seed bytes"
    );
}

#[test]
fn hot_swap_preserves_epoch_seed() {
    // Pass-15 architect-review wiring: update_validator_set MUST
    // copy the active epoch_seed into the freshly-constructed set.
    // Without this, every slashing-driven shrink silently demotes
    // proposer rotation to the predictable round-robin fallback.
    let initial = addrs(4);
    let mut hs = HotStuffConsensus::new(
        Address([0x01; 20]),
        SafetyRules::new(priv_key(1), Address([0x01; 20])),
        ValidatorSet::new(initial.clone()),
    );
    let live_seed = seed(0xCD);
    hs.rotate_epoch_seed(live_seed);
    assert_eq!(hs.validator_set.epoch_seed, live_seed);

    // Slashing shrinks the active set 4 → 3.
    let after_slash: Vec<Address> = initial.iter().take(3).copied().collect();
    hs.update_validator_set(after_slash.clone());

    assert_eq!(hs.validator_set.validators, after_slash);
    assert_eq!(
        hs.validator_set.epoch_seed, live_seed,
        "hot-swap dropped epoch_seed → proposer rotation silently demoted to round-robin"
    );
}

#[test]
fn legacy_fallback_only_on_zero_seed() {
    // `H256::zero()` MUST cleanly degenerate to plain `round % n`
    // round-robin (preserves compatibility with pre-Pass-15 fixtures
    // and with the freshly-constructed default state).  Any non-zero
    // seed MUST take the keccak path.
    let validators = addrs(4);
    let mut vs = ValidatorSet::new(validators.clone());
    // Default is H256::zero() — should be exactly round-robin.
    for round in 0u64..12 {
        assert_eq!(
            vs.proposer_for_round(round),
            validators[(round as usize) % validators.len()],
            "zero seed: round {round} must select via plain round-robin"
        );
    }
    // Non-zero seed must NOT be byte-identical to round-robin across
    // the whole window — same probabilistic argument as test #1.
    vs.set_epoch_seed(seed(0x77));
    let mut any_diff = false;
    for round in 0u64..16 {
        if vs.proposer_for_round(round)
            != validators[(round as usize) % validators.len()]
        {
            any_diff = true;
            break;
        }
    }
    assert!(
        any_diff,
        "non-zero seed produced identical schedule to round-robin — keccak path not taken"
    );
}

#[test]
fn rotation_seed_is_chain_dependent() {
    // Defence-in-depth: the seed-derivation in `do_commit`
    // (keccak256(block_hash || next_epoch_be8 || prev_seed)) means
    // distinct prev_seeds MUST yield distinct rotated seeds even when
    // (block_hash, next_epoch) match.  We replay the derivation here
    // and assert the keccak primitive is doing real work.
    let block_hash = H256([0x42; 32]);
    let next_epoch: u64 = 7;
    let derive = |prev: H256| -> H256 {
        let mut buf = Vec::with_capacity(72);
        buf.extend_from_slice(block_hash.as_bytes());
        buf.extend_from_slice(&next_epoch.to_be_bytes());
        buf.extend_from_slice(prev.as_bytes());
        zbx_crypto::keccak::keccak256(&buf)
    };
    let s1 = derive(H256::zero());
    let s2 = derive(seed(0x01));
    let s3 = derive(seed(0x02));
    assert_ne!(s1, s2);
    assert_ne!(s2, s3);
    assert_ne!(s1, s3);
}
