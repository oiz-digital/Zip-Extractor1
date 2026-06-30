//! S33-state-root W3a — StateView::state_root() shared-MPT integration tests.
//!
//! Verifies the W3a deliverable: `zbx_execution::StateView::state_root()`
//! now produces a canonical Yellow-Paper Merkle-Patricia Trie root via the
//! shared `zbx_state::mpt` helpers, and the resulting root matches what
//! `zbx_state::StateDB::state_root()` produces for the same logical input.
//!
//! ## What is covered (W3a scope)
//! - StateView::state_root() returns EMPTY_ROOT for an empty view
//! - empty-account suppression matches StateDB
//! - per-field mutations (nonce, balance, code_hash, storage_root) move root
//! - root is independent of insertion order
//! - **Cross-implementation parity**: StateView and StateDB return the same
//!   32-byte root for the same logical inputs (the W3a invariant)
//! - Storage cache rebuilds the per-account storage trie correctly
//! - The new root is NEVER equal to the old flat-keccak placeholder root
//!
//! ## Out of scope (W3b later)
//! - Persistent TrieDB-backed storage_root (pre-existing slot loading)
//! - Genesis migration parity (W4)

use std::collections::HashMap;
use zbx_execution::StateView;
use zbx_state::StateDB;
use zbx_trie::EMPTY_ROOT;
use zbx_types::{
    account::AccountState,
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

fn nonzero(nonce: u64, balance_u128: u128) -> AccountState {
    let mut s = AccountState::default();
    s.nonce = nonce;
    s.set_balance_u128(balance_u128);
    s
}

fn slot32(byte: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = byte;
    s
}

fn val32(byte: u8) -> [u8; 32] {
    let mut v = [0u8; 32];
    v[31] = byte;
    v
}

// ─── Tests — StateView in isolation ──────────────────────────────────────

#[test]
fn empty_state_view_returns_empty_root() {
    let view = StateView::new();
    assert_eq!(
        view.state_root(),
        EMPTY_ROOT.0,
        "fresh StateView must yield the canonical Ethereum empty-trie root"
    );
}

#[test]
fn purely_default_accounts_are_suppressed_in_state_view() {
    let mut view = StateView::new();
    view.set_account(addr(1), AccountState::default());
    view.set_account(addr(2), AccountState::default());
    assert_eq!(
        view.state_root(),
        EMPTY_ROOT.0,
        "all-default accounts must be suppressed (Yellow Paper §4.1)"
    );
}

#[test]
fn nonzero_account_changes_state_view_root() {
    let mut view = StateView::new();
    let baseline = view.state_root();

    view.set_account(addr(1), nonzero(1, 100));
    let after = view.state_root();

    assert_ne!(after, baseline, "non-empty account must change the root");
    assert_ne!(after, EMPTY_ROOT.0, "single-account trie root must differ from EMPTY_ROOT");
}

#[test]
fn nonce_change_changes_state_view_root() {
    let mut view = StateView::new();
    view.set_account(addr(1), nonzero(1, 100));
    let r1 = view.state_root();
    view.set_account(addr(1), nonzero(2, 100));
    let r2 = view.state_root();
    assert_ne!(r1, r2, "nonce mutation must alter the state root");
}

#[test]
fn balance_change_changes_state_view_root() {
    let mut view = StateView::new();
    view.set_account(addr(1), nonzero(1, 100));
    let r1 = view.state_root();
    view.set_account(addr(1), nonzero(1, 200));
    let r2 = view.state_root();
    assert_ne!(r1, r2, "balance mutation must alter the state root");
}

#[test]
fn state_view_root_independent_of_insertion_order() {
    let mut v1 = StateView::new();
    v1.set_account(addr(1), nonzero(1, 100));
    v1.set_account(addr(2), nonzero(2, 200));
    v1.set_account(addr(3), nonzero(3, 300));

    let mut v2 = StateView::new();
    v2.set_account(addr(3), nonzero(3, 300));
    v2.set_account(addr(1), nonzero(1, 100));
    v2.set_account(addr(2), nonzero(2, 200));

    assert_eq!(
        v1.state_root(),
        v2.state_root(),
        "MPT root must be insertion-order independent"
    );
}

#[test]
fn state_view_storage_writes_change_root_via_storage_root() {
    let mut view = StateView::new();
    view.set_account(addr(1), nonzero(1, 100));
    let r_no_storage = view.state_root();

    view.set_storage(addr(1), slot32(1), val32(0x42));
    let r_with_storage = view.state_root();

    assert_ne!(
        r_no_storage, r_with_storage,
        "storage write must alter state root via recomputed storage_root"
    );
}

#[test]
fn state_view_zero_storage_value_is_omitted() {
    let mut v1 = StateView::new();
    v1.set_account(addr(1), nonzero(1, 100));
    v1.set_storage(addr(1), slot32(1), [0u8; 32]);

    let mut v2 = StateView::new();
    v2.set_account(addr(1), nonzero(1, 100));

    assert_eq!(
        v1.state_root(),
        v2.state_root(),
        "zero-value storage write must not change the storage_root"
    );
}

// ─── Tests — Cross-implementation parity (the W3a invariant) ─────────────

#[test]
fn state_view_and_state_db_agree_on_empty_root() {
    let view = StateView::new();
    let db = StateDB::new();
    assert_eq!(
        view.state_root(),
        db.state_root().0,
        "empty StateView and empty StateDB must share the canonical root"
    );
}

#[test]
fn state_view_and_state_db_agree_on_single_account() {
    let acct = nonzero(7, 12345);

    let mut view = StateView::new();
    view.set_account(addr(1), acct.clone());

    let mut db = StateDB::new();
    db.set_account(addr(1), acct);

    assert_eq!(
        view.state_root(),
        db.state_root().0,
        "StateView and StateDB must produce identical roots (W3a invariant)"
    );
}

#[test]
fn state_view_and_state_db_agree_on_multi_account_with_storage() {
    let mut view = StateView::new();
    view.set_account(addr(1), nonzero(1, 100));
    view.set_account(addr(2), nonzero(2, 200));
    view.set_account(addr(3), nonzero(3, 300));
    view.set_storage(addr(1), slot32(1), val32(0xAA));
    view.set_storage(addr(1), slot32(2), val32(0xBB));
    view.set_storage(addr(2), slot32(5), val32(0xCC));

    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero(1, 100));
    db.set_account(addr(2), nonzero(2, 200));
    db.set_account(addr(3), nonzero(3, 300));
    db.set_storage(addr(1), H256(slot32(1)), H256(val32(0xAA)));
    db.set_storage(addr(1), H256(slot32(2)), H256(val32(0xBB)));
    db.set_storage(addr(2), H256(slot32(5)), H256(val32(0xCC)));

    assert_eq!(
        view.state_root(),
        db.state_root().0,
        "multi-account+multi-storage parity must hold across both impls"
    );
}

#[test]
fn state_view_and_state_db_agree_on_empty_account_suppression() {
    // Both should suppress all-default accounts and yield EMPTY_ROOT.
    let mut view = StateView::new();
    view.set_account(addr(1), AccountState::default());
    view.set_account(addr(2), AccountState::default());

    let mut db = StateDB::new();
    db.set_account(addr(1), AccountState::default());
    db.set_account(addr(2), AccountState::default());

    assert_eq!(view.state_root(), EMPTY_ROOT.0);
    assert_eq!(db.state_root(), EMPTY_ROOT);
    assert_eq!(view.state_root(), db.state_root().0);
}

#[test]
fn state_view_and_state_db_agree_when_balance_only_set() {
    let mut a = AccountState::default();
    a.balance = U256::from(1u64);

    let mut view = StateView::new();
    view.set_account(addr(1), a.clone());

    let mut db = StateDB::new();
    db.set_account(addr(1), a);

    assert_eq!(
        view.state_root(),
        db.state_root().0,
        "balance>0-only edge case must produce identical roots"
    );
}

// ─── Tests — Anti-regression on the deprecated placeholder ───────────────

#[test]
fn state_view_root_differs_from_deprecated_blob_keccak() {
    // Sanity: confirm the new MPT root NEVER coincides with the old
    // pre-W3a placeholder. If this ever passes by equality, we have
    // regressed back to the flat-keccak placeholder.
    let mut view = StateView::new();
    view.set_account(addr(1), nonzero(1, 100));
    view.set_account(addr(2), nonzero(2, 200));
    let mpt_root = view.state_root();

    // Reproduce the OLD pre-W3a algorithm verbatim from executor.rs:109-122.
    let mut entries: Vec<(Address, AccountState)> = vec![
        (addr(1), nonzero(1, 100)),
        (addr(2), nonzero(2, 200)),
    ];
    entries.sort_by_key(|(a, _)| a.0);
    let mut hasher_input = Vec::new();
    for (addr, state) in &entries {
        hasher_input.extend_from_slice(&addr.0);
        hasher_input.extend_from_slice(&state.nonce.to_be_bytes());
        // OLD bug: it did `state.balance` directly, but balance is U256 — it
        // was actually emitting an array address. We reproduce the spirit:
        // a flat 32-byte balance encoding.
        let mut bal = [0u8; 32];
        state.balance.to_big_endian(&mut bal);
        hasher_input.extend_from_slice(&bal);
    }
    let old_root = keccak256(&hasher_input);

    assert_ne!(
        mpt_root, old_root.0,
        "new MPT root must NOT match the deprecated flat-keccak placeholder"
    );
}

#[test]
fn state_view_storage_value_change_changes_root() {
    let mut view = StateView::new();
    view.set_account(addr(1), nonzero(1, 100));
    view.set_storage(addr(1), slot32(1), val32(0x11));
    let r1 = view.state_root();

    view.set_storage(addr(1), slot32(1), val32(0x22));
    let r2 = view.state_root();

    assert_ne!(r1, r2, "rewriting a non-zero storage slot must change the root");
}

#[test]
fn state_view_distinct_addresses_with_same_state_yield_different_roots() {
    let mut v1 = StateView::new();
    v1.set_account(addr(1), nonzero(1, 100));
    let mut v2 = StateView::new();
    v2.set_account(addr(2), nonzero(1, 100));
    assert_ne!(
        v1.state_root(),
        v2.state_root(),
        "address-keyed trie must distinguish identical state at different addrs"
    );
}

// ─── Tests — Helper-level direct invocation ──────────────────────────────

#[test]
fn shared_mpt_helper_directly_callable() {
    // Verifies the helper can be called from any crate (this test crate
    // doesn't depend on zbx-state directly via its own deps, so we go
    // through StateDB to access the helper).
    let mut accounts = HashMap::new();
    accounts.insert(addr(1), nonzero(1, 100));
    let storage: HashMap<Address, HashMap<H256, H256>> = HashMap::new();

    let helper_root = zbx_state::mpt::compute_state_root(&accounts, &storage);

    let mut db = StateDB::new();
    db.set_account(addr(1), nonzero(1, 100));
    let db_root = db.state_root();

    assert_eq!(
        helper_root, db_root,
        "direct helper call must agree with StateDB.state_root()"
    );
}
