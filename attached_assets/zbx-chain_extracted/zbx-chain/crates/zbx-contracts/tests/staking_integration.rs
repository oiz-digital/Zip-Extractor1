//! P4-PROD: Integration tests for staking escrow — unbonding chunk tracking,
//! multi-delegator lifecycle, proportional slash, and withdrawal sequencing.
//!
//! These tests exercise the full delegation → undelegate → withdraw round-trip
//! including the P3-PROD partial-undelegate fix (NEW-HIGH-02). Run with:
//!
//!   cargo test -p zbx-contracts --test staking_integration

use zbx_contracts::staking_escrow::{StakingEscrow, MIN_DELEGATION, MIN_STAKE};
use zbx_types::address::Address;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn addr(b: u8) -> Address {
    let mut a = [0u8; 20];
    a[19] = b;
    Address(a)
}

const STAKE: u128 = MIN_STAKE;                     // 100 ZBX
const DELG:  u128 = MIN_DELEGATION;               //  10 ZBX
const T0:    u64  = 1_000_000;                    // arbitrary start time

// ─── Full delegation lifecycle ────────────────────────────────────────────────

#[test]
fn full_lifecycle_delegate_undelegate_withdraw() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);

    e.stake(v, STAKE).unwrap();
    e.delegate(v, d, DELG * 5).unwrap();
    assert_eq!(e.delegation_of(&v, &d), DELG * 5);
    assert_eq!(e.stake_of(&v), STAKE + DELG * 5);

    // Undelegate full amount.
    e.undelegate(&v, &d, DELG * 5, T0).unwrap();
    // Active amount drops to 0.
    assert_eq!(e.delegation_of(&v, &d), 0);

    // Withdraw before period → error.
    assert!(e.withdraw_delegation(&v, &d, T0 + 1).is_err());

    // Withdraw after period → Ok.
    let period = e.unbonding_period;
    let got = e.withdraw_delegation(&v, &d, T0 + period).unwrap();
    assert_eq!(got, DELG * 5);

    // Record removed → not found.
    assert!(e.withdraw_delegation(&v, &d, T0 + period + 1).is_err());
}

// ─── Partial undelegate chunk tracking (P3-PROD / NEW-HIGH-02) ───────────────

#[test]
fn partial_undelegate_creates_unbonding_chunk() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);

    e.stake(v, STAKE).unwrap();
    e.delegate(v, d, DELG * 10).unwrap();

    // Partial undelegate: 3 out of 10 ZBX.
    e.undelegate(&v, &d, DELG * 3, T0).unwrap();

    // 7 ZBX should still be active.
    assert_eq!(e.delegation_of(&v, &d), DELG * 7);
    // Active voting power reflects only the active portion.
    assert_eq!(e.stake_of(&v), STAKE + DELG * 7);

    // Cannot withdraw before period.
    assert!(e.withdraw_delegation(&v, &d, T0 + 1).is_err());

    // After unbonding period, withdraw the 3 ZBX chunk.
    let period = e.unbonding_period;
    let got = e.withdraw_delegation(&v, &d, T0 + period).unwrap();
    assert_eq!(got, DELG * 3);

    // 7 ZBX delegation still active (record not removed).
    assert_eq!(e.delegation_of(&v, &d), DELG * 7);
}

#[test]
fn multiple_partial_undelegates_at_different_times() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);
    let period = e.unbonding_period;

    e.stake(v, STAKE).unwrap();
    e.delegate(v, d, DELG * 30).unwrap();

    // Three partial undelegates at t=1000, t=2000, t=3000.
    e.undelegate(&v, &d, DELG * 5, 1000).unwrap();
    e.undelegate(&v, &d, DELG * 5, 2000).unwrap();
    e.undelegate(&v, &d, DELG * 5, 3000).unwrap();

    // 15 ZBX still active.
    assert_eq!(e.delegation_of(&v, &d), DELG * 15);

    // Only the first chunk matures at 1000 + period.
    let t_first = 1000 + period;
    let w1 = e.withdraw_delegation(&v, &d, t_first).unwrap();
    assert_eq!(w1, DELG * 5, "only first chunk should mature");
    assert_eq!(e.delegation_of(&v, &d), DELG * 15, "active amount unchanged");

    // First two chunks mature at 2000 + period (but first is already withdrawn).
    let t_second = 2000 + period;
    let w2 = e.withdraw_delegation(&v, &d, t_second).unwrap();
    assert_eq!(w2, DELG * 5, "second chunk should now be withdrawable");

    // All three chunks withdrawn by 3000 + period.
    let t_third = 3000 + period;
    let w3 = e.withdraw_delegation(&v, &d, t_third).unwrap();
    assert_eq!(w3, DELG * 5, "third chunk should now be withdrawable");

    // Total withdrawn = 15 ZBX; 15 ZBX still active.
    assert_eq!(e.delegation_of(&v, &d), DELG * 15);
}

#[test]
fn partial_then_full_undelegate() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);
    let period = e.unbonding_period;

    e.stake(v, STAKE).unwrap();
    e.delegate(v, d, DELG * 20).unwrap();

    // Partial: undelegate 10 at T0.
    e.undelegate(&v, &d, DELG * 10, T0).unwrap();
    assert_eq!(e.delegation_of(&v, &d), DELG * 10);

    // Full: undelegate remaining 10 at T0 + 100.
    e.undelegate(&v, &d, DELG * 10, T0 + 100).unwrap();
    assert_eq!(e.delegation_of(&v, &d), 0);

    // After period, partial chunk matures first.
    let got1 = e.withdraw_delegation(&v, &d, T0 + period).unwrap();
    assert_eq!(got1, DELG * 10, "partial chunk should mature");

    // Full unbond chunk matures 100s later.
    let got2 = e.withdraw_delegation(&v, &d, T0 + 100 + period).unwrap();
    assert_eq!(got2, DELG * 10, "full-unbond amount should mature");

    // Record fully drained.
    assert!(e.withdraw_delegation(&v, &d, T0 + 200 + period).is_err());
}

// ─── Multi-delegator independence ─────────────────────────────────────────────

#[test]
fn two_delegators_independent_unbonding() {
    let mut e = StakingEscrow::new();
    let v  = addr(1);
    let d1 = addr(2);
    let d2 = addr(3);
    let period = e.unbonding_period;

    e.stake(v, STAKE).unwrap();
    e.delegate(v, d1, DELG * 10).unwrap();
    e.delegate(v, d2, DELG * 20).unwrap();

    // d1 undelegates at T0, d2 undelegates at T0 + 1000.
    e.undelegate(&v, &d1, DELG * 10, T0).unwrap();
    e.undelegate(&v, &d2, DELG * 20, T0 + 1000).unwrap();

    // d1 withdraws at T0 + period.
    let w1 = e.withdraw_delegation(&v, &d1, T0 + period).unwrap();
    assert_eq!(w1, DELG * 10);

    // d2 not yet matured.
    assert!(e.withdraw_delegation(&v, &d2, T0 + period).is_err());

    // d2 withdraws at T0 + 1000 + period.
    let w2 = e.withdraw_delegation(&v, &d2, T0 + 1000 + period).unwrap();
    assert_eq!(w2, DELG * 20);
}

// ─── Proportional slash propagates to delegators ──────────────────────────────

#[test]
fn slash_reduces_active_delegation_proportionally() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);

    e.stake(v, STAKE).unwrap();         // 100 ZBX self
    e.delegate(v, d, STAKE).unwrap();   // 100 ZBX delegation; total = 200 ZBX

    // Slash 100 ZBX → 50% of total stake.
    let slashed = e.slash(&v, STAKE);
    assert_eq!(slashed, STAKE, "exactly 100 ZBX should be slashed");

    // Both self-stake and delegation lose 50 ZBX each.
    assert_eq!(e.self_stake_of(&v), STAKE / 2);
    assert_eq!(e.delegation_of(&v, &d), STAKE / 2);
}

// ─── Minimum rump guard ───────────────────────────────────────────────────────

#[test]
fn undelegate_leaving_rump_below_minimum_is_rejected() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);

    e.stake(v, STAKE).unwrap();
    e.delegate(v, d, DELG * 5).unwrap();

    // Would leave 1 ZBX remaining which is below MIN_DELEGATION (10 ZBX).
    let rump_amount = DELG * 5 - DELG / 10;
    assert!(
        e.undelegate(&v, &d, rump_amount, T0).is_err(),
        "rump below minimum should be rejected"
    );
}

// ─── Validator jailed/slashed guards ─────────────────────────────────────────

#[test]
fn cannot_delegate_to_jailed_validator() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);

    e.stake(v, STAKE).unwrap();
    e.slash(&v, 1);  // partial slash → Jailed
    assert!(e.delegate(v, d, DELG).is_err());
}

#[test]
fn unjail_restores_delegation_ability() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let d = addr(2);

    e.stake(v, STAKE).unwrap();
    e.slash(&v, 1);  // Jailed
    assert!(e.delegate(v, d, DELG).is_err());

    e.unjail(&v).unwrap();
    assert!(e.delegate(v, d, DELG).is_ok());
}

// ─── Staking self-stake lifecycle ────────────────────────────────────────────

#[test]
fn validator_stake_full_lifecycle() {
    let mut e = StakingEscrow::new();
    let v = addr(1);
    let period = e.unbonding_period;

    e.stake(v, STAKE).unwrap();
    assert_eq!(e.self_stake_of(&v), STAKE);

    e.begin_unbonding(&v, T0).unwrap();
    // Second call must fail (not Active anymore).
    assert!(e.begin_unbonding(&v, T0 + 100).is_err());

    // Cannot withdraw before period.
    assert!(e.withdraw(&v, T0 + 1).is_err());

    // Withdraw after period — record removed.
    let got = e.withdraw(&v, T0 + period).unwrap();
    assert_eq!(got, STAKE);
    assert_eq!(e.validator_status(&v), None);
}
