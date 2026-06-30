//! S33-state-root W2 — StateDB::state_root() Merkle-Patricia Trie tests.
//!
//! These tests verify the W2 deliverable: that `StateDB::state_root()`
//! produces a real MPT root per Yellow Paper §4.1 instead of the previous
//! flat-keccak placeholder.
//!
//! ## What is covered (W2 scope)
//! - empty-DB returns the canonical EMPTY_ROOT
//! - empty-account suppression (Yellow Paper "no zero-state in trie")
//! - root changes on every visible field mutation (nonce, balance, code, storage_root)
//! - root is independent of insertion order (HashMap iteration safety)
//! - per-account storage trie computed from `storage_cache` when populated
//! - self-destructed addresses excluded
//! - the new root NEVER equals the deprecated keccak-of-blob root for the
//!   same account set (proves we are no longer running the placeholder)
//!
//! ## What is deliberately NOT covered here (W3 scope)
//! - partial-overwrite storage_root parity (needs persistent TrieDB plumb-through)
//! - executor migration parity tests (W3-W4)
//! - genesis re-computation parity (W4)

use zbx_state::StateDB;
use zbx_trie::EMPTY_ROOT;
use zbx_types::{
    account::{AccountState, EMPTY_CODE_HASH, EMPTY_STORAGE_ROOT},
    address::Address,
    H256, U256,
};
use zbx_crypto::keccak::keccak256;

// ─── Helpers ──────────────────────────────────────────────────────────────

fn addr(byte: u8) -> Address {
    let mut a = [0u8; 20];
    a[19] = byte;
    Address(a)
}

fn slot(byte: u8) -> H256 {
    let mut s = [0u8; 32];
    s[31] = byte;
    H256(s)
}

fn nonzero_account(nonce: u64, balance_u128: u128) -> AccountState {
    let mut s = AccountState::default();
    s.nonce = nonce;
    s.set_balance_u128(balance_u128);
    s
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[test]
fn empty_state_db_returns_empty_root() {
    let db = StateDB::new();
    assert_eq!(
        db.state_root(),
        EMPTY_ROOT,
        "fresh StateDB must yield the canonical Ethereum empty-trie root"
    );
}

#[test]
fn purely_default_accounts_are_suppressed() {
    // Per Yellow Paper §4.1, an account with nonce=0, balance=0,
    // code=EMPTY, storage=EMPTY must NOT appear in the state trie.
    let mut db = StateDB::new();
    db.set_account(addr(1), AccountState::default());
    db.set_account(addr(2), AccountState::default());
    db.set_account(addr(3), AccountState::default());

    assert_eq!(
        db.state_root(),
        EMPTY_ROOT,
        "all-default accounts must be suppressed; root must equal empty-trie root"
    );
}

#[test]
fn single_nonzero_account_changes_root() {
    let mut db = StateDB::new();
    let baseline = db.state_root();

    db.set_account(addr(1), nonzero_account(1, 100));
    let after = db.state_root();

    assert_ne!(after, baseline, "non-empty account must change the root");
    assert_ne!(after, EMPTY_ROOT, "single-account trie root must differ from EMPTY_ROOT");
}

#[test]
fn nonce_change_changes_root() {
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    let r1 = db.state_root();

    db.set_account(addr(1), nonzero_account(2, 100));
    let r2 = db.state_root();

    assert_ne!(r1, r2, "nonce mutation must alter the state root");
}

#[test]
fn balance_change_changes_root() {
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    let r1 = db.state_root();

    db.set_account(addr(1), nonzero_account(1, 200));
    let r2 = db.state_root();

    assert_ne!(r1, r2, "balance mutation must alter the state root");
}

#[test]
fn code_hash_change_changes_root() {
    let mut db = StateDB::new();
    let mut a = nonzero_account(1, 100);
    db.set_account(addr(1), a.clone());
    let r1 = db.state_root();

    a.code_hash = H256([0xAB; 32]);
    db.set_account(addr(1), a);
    let r2 = db.state_root();

    assert_ne!(r1, r2, "code_hash mutation must alter the state root");
}

#[test]
fn storage_root_change_changes_root() {
    let mut db = StateDB::new();
    let mut a = nonzero_account(1, 100);
    db.set_account(addr(1), a.clone());
    let r1 = db.state_root();

    a.storage_root = H256([0xCD; 32]);
    db.set_account(addr(1), a);
    let r2 = db.state_root();

    assert_ne!(
        r1, r2,
        "AccountState.storage_root mutation must alter the state root \
         (proves storage_root is part of the RLP'd account leaf)"
    );
}

#[test]
fn root_independent_of_insertion_order() {
    let mut db1 = StateDB::new();
    db1.set_account(addr(1), nonzero_account(1, 100));
    db1.set_account(addr(2), nonzero_account(2, 200));
    db1.set_account(addr(3), nonzero_account(3, 300));
    let r1 = db1.state_root();

    let mut db2 = StateDB::new();
    db2.set_account(addr(3), nonzero_account(3, 300));
    db2.set_account(addr(1), nonzero_account(1, 100));
    db2.set_account(addr(2), nonzero_account(2, 200));
    let r2 = db2.state_root();

    assert_eq!(
        r1, r2,
        "MPT root must be insertion-order independent (HashMap iteration safety)"
    );
}

#[test]
fn dirty_overrides_base_for_same_address() {
    let mut db = StateDB::new();
    db.seed_account(addr(1), nonzero_account(5, 500));
    let r_base = db.state_root();

    db.set_account(addr(1), nonzero_account(7, 700));
    let r_dirty = db.state_root();

    assert_ne!(r_base, r_dirty, "dirty write must take precedence over seeded base");
}

#[test]
fn selfdestruct_excludes_address_from_root() {
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    let r_with = db.state_root();

    db.selfdestruct(addr(1));
    let r_without = db.state_root();

    assert_eq!(
        r_without, EMPTY_ROOT,
        "self-destructed address must be excluded; only-account-removed → empty trie"
    );
    assert_ne!(r_with, r_without, "selfdestruct must alter the root");
}

#[test]
fn storage_cache_modifies_root_via_storage_root() {
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    let r_no_storage = db.state_root();

    db.set_storage(addr(1), slot(1), H256([0x42; 32]));
    let r_with_storage = db.state_root();

    assert_ne!(
        r_no_storage, r_with_storage,
        "storage write must alter state root via recomputed storage_root field"
    );
}

#[test]
fn zero_value_storage_slot_is_omitted() {
    let mut db1 = StateDB::new();
    db1.set_account(addr(1), nonzero_account(1, 100));
    db1.set_storage(addr(1), slot(1), H256::zero());
    let r1 = db1.state_root();

    let mut db2 = StateDB::new();
    db2.set_account(addr(1), nonzero_account(1, 100));
    let r2 = db2.state_root();

    assert_eq!(
        r1, r2,
        "zero-value storage write must not change the storage_root \
         (Yellow Paper: zero signifies absence of binding)"
    );
}

#[test]
fn storage_slot_value_change_changes_root() {
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    db.set_storage(addr(1), slot(1), H256([0x11; 32]));
    let r1 = db.state_root();

    db.set_storage(addr(1), slot(1), H256([0x22; 32]));
    let r2 = db.state_root();

    assert_ne!(r1, r2, "rewriting a non-zero storage slot must change the root");
}

#[test]
fn distinct_addresses_with_same_state_yield_different_roots() {
    let mut db1 = StateDB::new();
    db1.set_account(addr(1), nonzero_account(1, 100));

    let mut db2 = StateDB::new();
    db2.set_account(addr(2), nonzero_account(1, 100));

    assert_ne!(
        db1.state_root(),
        db2.state_root(),
        "address-keyed trie must distinguish identical state at different addrs"
    );
}

#[test]
fn new_root_differs_from_deprecated_blob_keccak() {
    // Sanity check: the new MPT root MUST NOT collide with the old
    // placeholder (sorted-blob keccak). If this test ever passes by
    // equality, we have either reverted the W2 change or the MPT
    // happens to produce the same digest by astronomical coincidence.
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    db.set_account(addr(2), nonzero_account(2, 200));
    let mpt_root = db.state_root();

    // Reproduce the OLD pre-W2 algorithm verbatim.
    let mut accounts: Vec<(Address, AccountState)> = vec![
        (addr(1), nonzero_account(1, 100)),
        (addr(2), nonzero_account(2, 200)),
    ];
    accounts.sort_by_key(|(a, _)| a.0);
    let mut input = Vec::new();
    for (addr, state) in &accounts {
        input.extend_from_slice(&addr.0);
        input.extend_from_slice(&state.nonce.to_be_bytes());
        let mut bal = [0u8; 32];
        state.balance.to_big_endian(&mut bal);
        input.extend_from_slice(&bal);
        input.extend_from_slice(&state.code_hash.0);
        input.extend_from_slice(&state.storage_root.0);
    }
    let old_root = keccak256(&input);

    assert_ne!(
        mpt_root, old_root,
        "new MPT root must NOT match the deprecated flat-keccak placeholder"
    );
}

#[test]
fn account_default_constants_match_yellow_paper() {
    // Spec sanity: confirm the constants we encode against haven't drifted.
    let default = AccountState::default();
    assert_eq!(default.nonce, 0);
    assert!(default.balance.is_zero());
    assert_eq!(default.code_hash, EMPTY_CODE_HASH);
    assert_eq!(default.storage_root, EMPTY_STORAGE_ROOT);
    assert!(default.is_empty(), "all-default account must satisfy is_empty()");
}

#[test]
fn account_with_nonzero_nonce_only_is_in_trie() {
    let mut db = StateDB::new();
    let mut a = AccountState::default();
    a.nonce = 1;
    db.set_account(addr(1), a);

    assert_ne!(db.state_root(), EMPTY_ROOT, "nonce>0 alone must inhibit suppression");
}

#[test]
fn account_with_nonzero_balance_only_is_in_trie() {
    let mut db = StateDB::new();
    let mut a = AccountState::default();
    a.balance = U256::from(1u64);
    db.set_account(addr(1), a);

    assert_ne!(db.state_root(), EMPTY_ROOT, "balance>0 alone must inhibit suppression");
}

#[test]
fn snapshot_revert_preserves_state_root_invariance() {
    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero_account(1, 100));
    let r_before = db.state_root();

    let snap = db.snapshot();
    db.set_account(addr(2), nonzero_account(2, 200));
    db.set_storage(addr(1), slot(1), H256([0xFF; 32]));
    let r_during = db.state_root();
    assert_ne!(r_before, r_during, "mutation between snapshot+revert must change root");

    db.revert_to(snap);
    let r_after = db.state_root();
    assert_eq!(
        r_before, r_after,
        "revert_to(snap) must restore the exact pre-snapshot state root"
    );
}
