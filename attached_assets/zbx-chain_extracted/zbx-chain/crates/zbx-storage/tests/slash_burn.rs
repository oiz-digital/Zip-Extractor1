//! Round-4 follow-up: regression test for `ZbxDb::apply_slash_burns`.
//!
//! Proves that a slashing burn debits actual on-disk account balances
//! (so subsequent `get_account` / `eth_getBalance` reads see the
//! reduced balance) — bridging the staking pipeline's metadata-only
//! `self_stake` debit into the EVM account state.

use std::sync::Arc;
use tempfile::TempDir;

use zbx_storage::ZbxDb;
use zbx_types::{account::AccountState, address::Address};

#[test]
fn apply_slash_burns_debits_on_disk_account_balance() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());

    let offender = Address([0xa1; 20]);
    let untouched = Address([0xb2; 20]);

    let starter: u128 = 100 * 10u128.pow(18);
    let mut a = AccountState::default();
    a.set_balance_u128(starter);
    db.put_account(&offender, &a).unwrap();

    let mut b = AccountState::default();
    b.set_balance_u128(starter);
    db.put_account(&untouched, &b).unwrap();

    // ── Burn 30 ZBX from the offender ──
    let burn_wei: u128 = 30 * 10u128.pow(18);
    let actual = db
        .apply_slash_burns(&[(offender, burn_wei)])
        .expect("apply_slash_burns must succeed");
    assert_eq!(actual, vec![burn_wei]);

    // ── Offender's on-disk balance MUST be reduced ──
    let after = db.get_account(&offender).unwrap();
    assert_eq!(
        after.balance_u128(),
        starter - burn_wei,
        "ON-STATE BURN FAILED: offender balance not debited",
    );

    // ── Other accounts MUST be untouched ──
    let other = db.get_account(&untouched).unwrap();
    assert_eq!(other.balance_u128(), starter);
}

#[test]
fn apply_slash_burns_saturates_at_current_balance() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());

    let offender = Address([0xc3; 20]);
    let starter: u128 = 5 * 10u128.pow(18);
    let mut a = AccountState::default();
    a.set_balance_u128(starter);
    db.put_account(&offender, &a).unwrap();

    // Try to burn more than the offender holds.
    let huge: u128 = 1_000 * 10u128.pow(18);
    let actual = db.apply_slash_burns(&[(offender, huge)]).unwrap();
    assert_eq!(
        actual,
        vec![starter],
        "saturating burn MUST report only what was actually debited",
    );
    assert_eq!(db.get_account(&offender).unwrap().balance_u128(), 0);
}

#[test]
fn apply_slash_burns_atomic_batch_multiple_offenders() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());

    let o1 = Address([0xd4; 20]);
    let o2 = Address([0xe5; 20]);
    let starter: u128 = 50 * 10u128.pow(18);
    for o in [&o1, &o2] {
        let mut a = AccountState::default();
        a.set_balance_u128(starter);
        db.put_account(o, &a).unwrap();
    }

    let b1: u128 = 10 * 10u128.pow(18);
    let b2: u128 = 25 * 10u128.pow(18);
    let actual = db.apply_slash_burns(&[(o1, b1), (o2, b2)]).unwrap();
    assert_eq!(actual, vec![b1, b2]);

    assert_eq!(db.get_account(&o1).unwrap().balance_u128(), starter - b1);
    assert_eq!(db.get_account(&o2).unwrap().balance_u128(), starter - b2);
}
