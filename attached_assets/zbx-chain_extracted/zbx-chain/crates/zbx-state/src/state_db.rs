//! StateDB: cached world-state over ZbxDb with dirty tracking.
//!
//! # State-root computation (S33-state-root W2 + W3a)
//!
//! `state_root()` delegates to the shared `crate::mpt` module which builds
//! a real Merkle-Patricia Trie (Yellow Paper §4.1) over the union of base
//! + dirty accounts (minus self-destructed ones). The same `mpt` module is
//! also used by `zbx-execution::StateView::state_root()`, so both paths
//! produce identical roots for identical inputs (W3a invariant).
//!
//! Account encoding is canonical RLP of `(nonce, balance, storage_root,
//! code_hash)` keyed by `keccak256(addr)`. See `crate::mpt` for the full
//! Yellow-Paper conformance details.
//!
//! ## Honest limitation (S33-state-root W3b scope)
//!
//! For accounts whose storage was partially modified during a block but
//! whose pre-existing slots are not loaded into `storage_cache`, the
//! recomputed storage_root will diverge from the canonical Yellow-Paper
//! root because the unread slots are silently treated as absent. W3b will
//! plumb a persistent `TrieDB` over `ZbxDb` (via `ZbxDbTrieAdapter`) so
//! `compute_storage_root` can reload the existing trie from
//! `AccountState.storage_root` and apply only the dirty slot deltas.

use std::collections::{HashMap, HashSet};
use zbx_types::{
    account::{AccountState, EMPTY_CODE_HASH},
    address::Address,
    H256,
};
use zbx_crypto::keccak::keccak256;
use crate::mpt;

/// In-memory overlay over persistent state.
pub struct StateDB {
    /// Original on-disk accounts (read through)
    base_accounts: HashMap<Address, AccountState>,
    /// Modified accounts (dirty)
    dirty_accounts: HashMap<Address, AccountState>,
    /// Contract code: code_hash → bytecode
    code_cache: HashMap<H256, Vec<u8>>,
    /// Storage cache: addr → (slot → value)
    storage_cache: HashMap<Address, HashMap<H256, H256>>,
    /// Dirty storage slots
    dirty_storage: HashMap<Address, HashSet<H256>>,
    /// Accounts to self-destruct
    to_delete: HashSet<Address>,
    /// Emitted logs
    pub logs: Vec<zbx_types::receipt::Log>,
    /// Refund counter (gas refunds from SSTORE clears, SELFDESTRUCT)
    pub refund: u64,
}

impl StateDB {
    pub fn new() -> Self {
        StateDB {
            base_accounts: HashMap::new(),
            dirty_accounts: HashMap::new(),
            code_cache: HashMap::new(),
            storage_cache: HashMap::new(),
            dirty_storage: HashMap::new(),
            to_delete: HashSet::new(),
            logs: Vec::new(),
            refund: 0,
        }
    }

    /// Seed an account from persistent storage.
    pub fn seed_account(&mut self, addr: Address, state: AccountState) {
        self.base_accounts.insert(addr, state);
    }

    /// Seed contract code.
    pub fn seed_code(&mut self, code_hash: H256, code: Vec<u8>) {
        self.code_cache.insert(code_hash, code);
    }

    /// Get account state (dirty overlay wins over base).
    pub fn get_account(&self, addr: &Address) -> AccountState {
        self.dirty_accounts.get(addr).cloned()
            .unwrap_or_else(|| self.base_accounts.get(addr).cloned().unwrap_or_default())
    }

    /// Modify account state.
    ///
    /// SEC-2026-05-09 (Pass-6 C4): invariant guards on every mutation.
    /// `set_account` is `pub` so any code path can write — without these
    /// checks a buggy executor branch could silently violate Yellow-Paper
    /// invariants and the state-root divergence would only surface at the
    /// next QC verification.  Guards (logged in release, panicked in
    /// debug):
    ///
    /// 1. **Nonce monotonicity** — once an account has nonce `n`, the
    ///    next stored nonce must be `≥ n`.  The only legitimate
    ///    exception is a self-destruct + re-creation in the same block,
    ///    which goes through `selfdestruct()` (which removes the dirty
    ///    entry) before re-insertion.
    /// 2. **Code immutability** — once an account has a non-empty
    ///    `code_hash`, that hash MUST NOT change to a different non-empty
    ///    hash.  The only legitimate transition is
    ///    `non-empty → EMPTY_CODE_HASH` via self-destruct (also routed
    ///    through `selfdestruct()`).  Re-deploying to a non-empty hash
    ///    on top of an existing one is a critical EVM invariant
    ///    violation — see EIP-684.
    pub fn set_account(&mut self, addr: Address, state: AccountState) {
        // SEC-2026-05-09 (Pass-6 C4, architect follow-up): if the account
        // has been self-destructed in this block, treat the prior view as
        // the default (empty) account.  Without this carve-out, recreating
        // a contract that lived in `base_accounts` and was selfdestructed
        // earlier in the same block would be falsely refused as a code
        // mutation (`selfdestruct()` only removes from `dirty_accounts`,
        // not from `base_accounts`).  `state_root()` already filters
        // `to_delete` from the visible set, so resetting here matches the
        // committed view.
        let prior = if self.to_delete.contains(&addr) {
            AccountState::default()
        } else {
            self.dirty_accounts.get(&addr).cloned()
                .unwrap_or_else(|| self.base_accounts.get(&addr).cloned().unwrap_or_default())
        };

        // The recreate path also un-marks the address — once we're writing
        // a fresh account on top of a tombstoned one, the deletion would
        // otherwise re-filter our new state out of the trie at commit time.
        if self.to_delete.contains(&addr) {
            self.to_delete.remove(&addr);
        }

        // (1) Nonce monotonicity.
        if state.nonce < prior.nonce {
            // Self-destructed accounts are removed from `dirty_accounts`
            // by `selfdestruct()`, so a later `set_account` on the same
            // address sees `prior.nonce == 0` from the default and the
            // check passes naturally.  Any other path hitting this
            // branch is a real bug.
            #[cfg(debug_assertions)]
            panic!(
                "SEC-2026-05-09 Pass-6 C4: nonce regression on {:?} \
                 (prior={}, attempted={})",
                addr, prior.nonce, state.nonce
            );
            #[cfg(not(debug_assertions))]
            tracing::error!(
                target: "state",
                ?addr, prior_nonce = prior.nonce, attempted_nonce = state.nonce,
                "SEC-2026-05-09 Pass-6 C4: refusing nonce regression — keeping prior nonce"
            );
            #[cfg(not(debug_assertions))]
            {
                let mut fixed = state;
                fixed.nonce = prior.nonce;
                self.dirty_accounts.insert(addr, fixed);
                return;
            }
        }

        // (2) Code immutability — once code is set, it cannot be replaced
        // with a *different* non-empty code hash.  Setting from non-empty
        // to EMPTY_CODE_HASH is permitted (selfdestruct path); empty →
        // empty and identical-hash overwrites are no-ops.
        if !prior.is_contract() {
            // Fresh account or EOA upgrading to contract — allowed.
        } else if state.code_hash == prior.code_hash {
            // Identical — no-op.
        } else if state.code_hash == EMPTY_CODE_HASH {
            // Contract being cleared (self-destruct path).  Permitted,
            // but typically routed through `selfdestruct()` instead;
            // log so the audit trail is visible.
            tracing::warn!(
                target: "state",
                ?addr, prior_code = ?prior.code_hash,
                "SEC-2026-05-09 Pass-6 C4: code hash cleared via set_account; \
                 prefer selfdestruct() for explicit lifecycle"
            );
        } else {
            #[cfg(debug_assertions)]
            panic!(
                "SEC-2026-05-09 Pass-6 C4: contract code_hash mutation on {:?} \
                 ({:?} → {:?}) — violates EIP-684",
                addr, prior.code_hash, state.code_hash
            );
            #[cfg(not(debug_assertions))]
            {
                tracing::error!(
                    target: "state",
                    ?addr,
                    prior_code = ?prior.code_hash,
                    attempted_code = ?state.code_hash,
                    "SEC-2026-05-09 Pass-6 C4: refusing contract code_hash mutation — keeping prior code"
                );
                let mut fixed = state;
                fixed.code_hash = prior.code_hash;
                self.dirty_accounts.insert(addr, fixed);
                return;
            }
        }

        self.dirty_accounts.insert(addr, state);
    }

    /// SEC-2026-05-09 (Pass-6 C4): bypass the invariant guards.
    ///
    /// Reserved for paths that legitimately need to "rewind" state — e.g.
    /// snapshot rollback in tests, fork resolution in unit harnesses, or
    /// reorg replay in `block_producer`.  Production execution paths MUST
    /// use `set_account`.  Every call to this function should carry an
    /// in-line `// SEC-2026-05-09 (Pass-6 C4): justified bypass — <reason>`.
    #[doc(hidden)]
    pub fn set_account_unchecked(&mut self, addr: Address, state: AccountState) {
        self.dirty_accounts.insert(addr, state);
    }

    /// Read a storage slot.
    pub fn get_storage(&self, addr: &Address, slot: &H256) -> H256 {
        self.storage_cache
            .get(addr)
            .and_then(|m| m.get(slot))
            .copied()
            .unwrap_or_else(H256::zero)
    }

    /// Write a storage slot.
    pub fn set_storage(&mut self, addr: Address, slot: H256, value: H256) {
        self.storage_cache.entry(addr).or_default().insert(slot, value);
        self.dirty_storage.entry(addr).or_default().insert(slot);
    }

    /// Get contract bytecode by code hash.
    pub fn get_code(&self, code_hash: &H256) -> Vec<u8> {
        self.code_cache.get(code_hash).cloned().unwrap_or_default()
    }

    /// Deploy contract code. Returns the code hash.
    pub fn deploy_code(&mut self, code: Vec<u8>) -> H256 {
        let code_hash = keccak256(&code);
        self.code_cache.insert(code_hash, code);
        code_hash
    }

    /// Add a refund (from SSTORE clear or SELFDESTRUCT).
    pub fn add_refund(&mut self, amount: u64) {
        self.refund = self.refund.saturating_add(amount);
    }

    /// Mark an account for self-destruction.
    pub fn selfdestruct(&mut self, addr: Address) {
        self.to_delete.insert(addr);
        self.dirty_accounts.remove(&addr);
    }

    /// Emit a log entry.
    pub fn emit_log(&mut self, log: zbx_types::receipt::Log) {
        self.logs.push(log);
    }

    /// Compute the new state root after applying all changes.
    ///
    /// Delegates to `crate::mpt::compute_state_root_filtered` so that
    /// `StateDB` and `zbx-execution::StateView` produce identical roots
    /// for identical inputs (W3a shared-helper invariant).
    pub fn state_root(&self) -> H256 {
        mpt::compute_state_root_filtered(
            &self.base_accounts,
            &self.dirty_accounts,
            &self.storage_cache,
            &self.to_delete,
        )
    }

    /// Persistent variant of [`Self::state_root`] (S33-state-root W3b
    /// production wire-up, architect-required closure of C-09).
    ///
    /// Uses the supplied persistent `TrieDB` (typically a
    /// `ZbxDbTrieAdapter`) so per-account storage tries are reopened via
    /// `MutableTrie::from_root(account.storage_root, db)` and the W2/W3a
    /// "honest limitation" — divergence on partial-overwrite blocks where
    /// pre-existing slots were not in cache — is closed.
    ///
    /// # Caller contract
    ///
    /// On success, the new trie nodes have been buffered in the adapter's
    /// pending list but not yet fsynced. The caller MUST call `db.commit()`
    /// before persisting the block header.
    ///
    /// # Errors
    ///
    /// Surfaces `TrieError::MissingNode` when `account.storage_root`
    /// references a node that is not yet on disk, or any I/O failure
    /// from the underlying `TrieDB`. Block production MUST abort on this
    /// error rather than commit a header to an undefined root.
    pub fn state_root_with_db<DB>(&self, db: DB) -> Result<H256, zbx_trie::TrieError>
    where
        DB: zbx_trie::TrieDB + Clone,
    {
        // Visible-set = (base ∪ dirty) \ to_delete.
        let mut visible: HashMap<Address, AccountState> = HashMap::new();
        for (addr, state) in &self.base_accounts {
            if !self.to_delete.contains(addr) {
                visible.insert(*addr, state.clone());
            }
        }
        for (addr, state) in &self.dirty_accounts {
            if !self.to_delete.contains(addr) {
                visible.insert(*addr, state.clone());
            }
        }
        mpt::compute_state_root_with_db(&visible, &self.storage_cache, db)
    }

    /// True if any state was modified.
    pub fn is_dirty(&self) -> bool {
        !self.dirty_accounts.is_empty() || !self.dirty_storage.is_empty()
    }

    /// List all dirtied accounts.
    pub fn dirty_addresses(&self) -> Vec<Address> {
        self.dirty_accounts.keys().cloned().collect()
    }

    /// Create a snapshot for revert.
    ///
    /// SEC-2026-05-09 (Pass-6 C4 architect follow-up): `to_delete` MUST
    /// be captured here — earlier versions only snapshotted dirty
    /// accounts / storage, so a `selfdestruct()` performed inside a sub-call
    /// that later REVERTed would leave the address tombstoned in
    /// `to_delete` after revert, silently filtering it out of
    /// `state_root()` and corrupting consensus on the parent frame.
    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            dirty_accounts: self.dirty_accounts.clone(),
            dirty_storage: self.dirty_storage.clone(),
            storage_cache: self.storage_cache.clone(),
            to_delete: self.to_delete.clone(),
            logs_len: self.logs.len(),
            refund: self.refund,
        }
    }

    /// Revert to a snapshot (for REVERT opcode).
    pub fn revert_to(&mut self, snap: StateSnapshot) {
        self.dirty_accounts = snap.dirty_accounts;
        self.dirty_storage = snap.dirty_storage;
        self.storage_cache = snap.storage_cache;
        self.to_delete = snap.to_delete;
        self.logs.truncate(snap.logs_len);
        self.refund = snap.refund;
    }
}

impl Default for StateDB {
    fn default() -> Self { Self::new() }
}

/// Snapshot for revert support.
pub struct StateSnapshot {
    dirty_accounts: HashMap<Address, AccountState>,
    dirty_storage: HashMap<Address, HashSet<H256>>,
    storage_cache: HashMap<Address, HashMap<H256, H256>>,
    /// SEC-2026-05-09 (Pass-6 C4 architect follow-up): pending
    /// self-destruct tombstones MUST be part of the snapshot so a
    /// REVERT inside a sub-call that selfdestructed an account
    /// correctly un-tombstones it on the parent frame.
    to_delete: HashSet<Address>,
    logs_len: usize,
    refund: u64,
}

// RLP encoding helpers were extracted to `crate::mpt` in S33-state-root W3a
// so they can be shared with `zbx-execution::StateView::state_root()`.

#[cfg(test)]
mod sec_pass6_c4_tests {
    //! SEC-2026-05-09 (Pass-6 C4): invariant guards on `StateDB::set_account`.
    //!
    //! Each test exercises one of the two enforced invariants by running the
    //! check in non-debug mode (debug builds panic instead of clamping).  The
    //! purpose is to confirm the *recovery* path keeps the chain producing
    //! valid state roots even under a buggy executor branch.
    use super::*;
    use zbx_types::address::Address;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn nonce_regression_is_clamped_to_prior() {
        let mut db = StateDB::new();
        let a = addr(1);
        let mut acct = AccountState::default();
        acct.nonce = 5;
        db.set_account(a, acct);

        let mut bad = AccountState::default();
        bad.nonce = 3; // attempt to roll back
        db.set_account(a, bad);

        assert_eq!(db.get_account(&a).nonce, 5,
            "nonce regression must be refused — prior nonce kept");
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn contract_code_hash_mutation_is_refused() {
        let mut db = StateDB::new();
        let a = addr(2);
        let mut deployed = AccountState::default();
        deployed.code_hash = H256([0xAA; 32]);
        db.set_account(a, deployed);

        let mut hijack = AccountState::default();
        hijack.code_hash = H256([0xBB; 32]); // different non-empty hash
        db.set_account(a, hijack);

        assert_eq!(db.get_account(&a).code_hash, H256([0xAA; 32]),
            "EIP-684: deployed code hash must be immutable");
    }

    #[test]
    fn nonce_monotonic_increase_is_allowed() {
        let mut db = StateDB::new();
        let a = addr(3);
        for n in 0..10u64 {
            let mut acct = AccountState::default();
            acct.nonce = n;
            db.set_account(a, acct);
        }
        assert_eq!(db.get_account(&a).nonce, 9);
    }

    #[test]
    fn fresh_eoa_to_contract_upgrade_is_allowed() {
        let mut db = StateDB::new();
        let a = addr(4);

        // EOA with balance.
        let mut eoa = AccountState::default();
        eoa.set_balance_u128(1000);
        db.set_account(a, eoa);

        // Deploy contract on top — fresh code_hash from empty.
        let mut contract = AccountState::default();
        contract.set_balance_u128(1000);
        contract.code_hash = H256([0x11; 32]);
        db.set_account(a, contract);

        assert_eq!(db.get_account(&a).code_hash, H256([0x11; 32]));
    }

    #[test]
    fn selfdestruct_then_recreate_is_allowed() {
        let mut db = StateDB::new();
        let a = addr(5);

        // Deploy contract.
        let mut contract = AccountState::default();
        contract.code_hash = H256([0x22; 32]);
        db.set_account(a, contract);

        // Self-destruct removes the dirty entry and tombstones the addr.
        db.selfdestruct(a);

        // Re-create with different code in the same block — allowed
        // because the to_delete tombstone resets the prior view.  The
        // tombstone is also lifted so state_root() keeps the new entry.
        let mut redeployed = AccountState::default();
        redeployed.code_hash = H256([0x33; 32]);
        db.set_account(a, redeployed);

        assert_eq!(db.get_account(&a).code_hash, H256([0x33; 32]));
        assert!(!db.to_delete.contains(&a),
            "tombstone must be cleared on recreate so state_root keeps the entry");
    }

    #[test]
    fn base_account_selfdestruct_then_recreate_is_allowed() {
        // Architect-flagged coverage gap: prior version of `set_account`
        // resolved `prior` from dirty→base only, so a contract that
        // existed in BASE (not just dirty) and was selfdestructed in
        // this block would have its base code_hash leak through and
        // the recreate would be falsely blocked.
        let mut db = StateDB::new();
        let a = addr(6);

        // Seed an existing contract into BASE (simulates a previous block).
        let mut existing = AccountState::default();
        existing.code_hash = H256([0xCC; 32]);
        db.seed_account(a, existing);

        // Self-destruct it in this block.
        db.selfdestruct(a);

        // Re-create with completely different code.  This must succeed
        // even though the base_accounts entry still holds the old code_hash.
        let mut redeployed = AccountState::default();
        redeployed.code_hash = H256([0xDD; 32]);
        db.set_account(a, redeployed);

        assert_eq!(db.get_account(&a).code_hash, H256([0xDD; 32]));
        assert!(!db.to_delete.contains(&a));
    }

    #[test]
    fn snapshot_revert_undoes_selfdestruct() {
        // Architect-flagged: snapshot was missing to_delete, so a REVERT
        // after selfdestruct would leave the address tombstoned.
        let mut db = StateDB::new();
        let a = addr(7);
        let mut acct = AccountState::default();
        acct.code_hash = H256([0xEE; 32]);
        acct.set_balance_u128(500);
        db.set_account(a, acct.clone());

        let snap = db.snapshot();

        db.selfdestruct(a);
        assert!(db.to_delete.contains(&a));

        db.revert_to(snap);

        assert!(!db.to_delete.contains(&a),
            "REVERT must un-tombstone — to_delete is part of the snapshot");
        assert_eq!(db.get_account(&a).code_hash, H256([0xEE; 32]));
        assert_eq!(db.get_account(&a).balance_u128(), 500);
    }
}