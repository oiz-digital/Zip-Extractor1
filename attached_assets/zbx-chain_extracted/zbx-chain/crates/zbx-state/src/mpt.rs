//! Shared Merkle-Patricia Trie state-root computation (S33-state-root W3a).
//!
//! Both `StateDB` (zbx-state) and `StateView` (zbx-execution) need to compute
//! the world-state MPT root over their visible account set. To avoid two
//! divergent implementations, this module exposes the canonical computation
//! as free functions over plain types.
//!
//! # Yellow Paper §4.1 conformance
//!
//! - Account leaf = RLP-list `[nonce, balance, storage_root, code_hash]`
//! - Integer fields use minimal-byte big-endian encoding (leading zeros stripped)
//! - Hash fields are 32 bytes verbatim
//! - Account key in trie = `keccak256(addr)` (20 → 32 byte hash expansion)
//! - Storage key in trie = `keccak256(slot)` (32 → 32 byte hash)
//! - Storage value = RLP of big-endian-int with leading zeros stripped
//! - Empty accounts (n=0, b=0, c=EMPTY, s=EMPTY) suppressed from trie
//! - Zero-value storage slots suppressed
//! - Empty trie → `EMPTY_ROOT` (`56e81f17...3b421`)
//! - Empty storage trie → `EMPTY_STORAGE_ROOT` (= same constant)
//!
//! # W3a scope vs W3b deferred work
//!
//! This module performs **lazy in-memory MPT construction** per call. It does
//! NOT yet read pre-existing storage slots from a persistent TrieDB — that's
//! W3b's job. For the W3a window:
//!   - Greenfield, genesis, freshly-created contracts: 100% canonical roots
//!   - Full-overwrite blocks: 100% canonical roots
//!   - Partial-overwrite blocks against accounts with un-cached pre-existing
//!     slots: storage_root inherits the account's pre-block `storage_root`
//!     field unchanged (lossy but consistent across StateDB + StateView)
//!
//! W3b will add a `compute_state_root_with_db(..., db: Arc<dyn TrieDB>)`
//! variant that reloads via `MutableTrie::from_root(account.storage_root, db)`
//! before applying the dirty slots, closing the gap.

use std::collections::{HashMap, HashSet};

use zbx_crypto::keccak::keccak256;
use zbx_rlp::RlpStream;
use zbx_trie::trie::MemoryTrieDB;
use zbx_trie::{MutableTrie, EMPTY_ROOT};
use zbx_types::{
    account::{AccountState, EMPTY_STORAGE_ROOT},
    address::Address,
    H256,
};

// `TrieDB` is brought into scope at the bottom of this file alongside the
// `_with_db` variants — see "W3b" section below.

// ─── Public API ───────────────────────────────────────────────────────────

/// Compute the canonical world-state MPT root over the supplied visible
/// account set.
///
/// # Arguments
/// - `accounts`: every address whose account state is visible at this point.
///   Caller is responsible for assembling this from base + dirty overlays
///   and for excluding self-destructed addresses (see `compute_state_root_filtered`
///   for a convenience that does the union+filter for you).
/// - `storage`: per-account storage cache. Accounts not in the map use their
///   on-leaf `storage_root`; accounts in the map have their storage trie
///   recomputed from the cache.
///
/// # Returns
/// The MPT root, or `EMPTY_ROOT` when no non-empty accounts remain after
/// Yellow-Paper §4.1 suppression.
pub fn compute_state_root(
    accounts: &HashMap<Address, AccountState>,
    storage: &HashMap<Address, HashMap<H256, H256>>,
) -> H256 {
    let db = MemoryTrieDB::default();
    let mut acct_trie = MutableTrie::new(db);

    for (addr, state) in accounts {
        // Yellow Paper §4.1 — empty accounts (and accounts with no storage
        // cache that are otherwise empty) MUST NOT appear in the state trie.
        let acct_storage = storage.get(addr);
        let cache_is_empty = acct_storage.map_or(true, |s| s.is_empty());
        if state.is_empty() && cache_is_empty {
            continue;
        }

        let storage_root = match acct_storage {
            Some(slots) if !slots.is_empty() => compute_storage_root(slots),
            _ => state.storage_root,
        };

        let key = keccak256(&addr.0);
        let value = encode_account_rlp(state, storage_root);
        acct_trie
            .insert(&key.0, value)
            .expect("MemoryTrieDB insert never fails");
    }

    acct_trie.root()
}

/// Convenience: assemble visible-set from base + dirty overlays, exclude
/// self-destructed addresses, then compute the root.
///
/// This matches the StateDB call shape exactly. StateView does not yet
/// support self-destruct, so it can pass an empty `to_delete`.
pub fn compute_state_root_filtered(
    base: &HashMap<Address, AccountState>,
    dirty: &HashMap<Address, AccountState>,
    storage: &HashMap<Address, HashMap<H256, H256>>,
    to_delete: &HashSet<Address>,
) -> H256 {
    // Materialise the visible set: union(base, dirty) \ to_delete.
    // dirty wins on collision.
    let mut visible: HashMap<Address, AccountState> = base.clone();
    for (addr, st) in dirty {
        visible.insert(*addr, st.clone());
    }
    for addr in to_delete {
        visible.remove(addr);
    }
    compute_state_root(&visible, storage)
}

/// Build a per-account storage Merkle-Patricia Trie from a slot cache.
///
/// - Key  = `keccak256(slot.as_bytes())`
/// - Value = RLP-encoded big-endian value byte string with leading zeros stripped
///
/// Zero-value slots are omitted per Yellow Paper §4.1 ("a value of zero
/// signifies the absence of a binding"). Returns `EMPTY_STORAGE_ROOT` when
/// the resulting trie is empty.
pub fn compute_storage_root(slots: &HashMap<H256, H256>) -> H256 {
    let db = MemoryTrieDB::default();
    let mut trie = MutableTrie::new(db);

    for (slot, value) in slots {
        if value.is_zero() {
            continue;
        }
        let key = keccak256(&slot.0);
        let stripped = strip_leading_zeros(&value.0);
        let mut s = RlpStream::new();
        s.append(&stripped);
        let encoded = s.out();
        trie.insert(&key.0, encoded)
            .expect("MemoryTrieDB insert never fails");
    }

    let r = trie.root();
    if r == EMPTY_ROOT {
        EMPTY_STORAGE_ROOT
    } else {
        r
    }
}

/// RLP-encode an account leaf per Yellow Paper §4.1, with a ZBX
/// extension carrying the VM-discriminator byte for `VmKind::Zvm`
/// accounts:
///   `[nonce, balance, storage_root, code_hash]`           (Evm — default)
///   `[nonce, balance, storage_root, code_hash, vm_byte]`  (Zvm)
///
/// The 5th element is appended **only** when `state.vm == Zvm`. This
/// keeps the canonical state root unchanged for every existing
/// (Evm-deployed) account, and adds a deterministic, consensus-bound
/// commitment to the VM discriminator for ZVM-deployed accounts so it
/// survives serialise/deserialise cycles and is replicated by every
/// node.
pub fn encode_account_rlp(state: &AccountState, storage_root: H256) -> Vec<u8> {
    use zbx_types::account::VmKind;
    let zvm = state.vm == VmKind::Zvm;
    let mut s = RlpStream::new();
    s.begin_list(if zvm { 5 } else { 4 });

    s.append(&u64_min_be(state.nonce));

    let mut bal_buf = [0u8; 32];
    state.balance.to_big_endian(&mut bal_buf);
    s.append(&strip_leading_zeros(&bal_buf));

    s.append(&storage_root.0[..]);
    s.append(&state.code_hash.0[..]);

    if zvm {
        s.append(&[VmKind::Zvm as u8][..]);
    }

    s.out()
}

// ─── Private helpers ──────────────────────────────────────────────────────

/// Minimal big-endian byte representation of `n`. Returns an empty
/// `Vec` when `n == 0` (RLP empty-string encoding).
fn u64_min_be(n: u64) -> Vec<u8> {
    if n == 0 {
        Vec::new()
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        bytes[skip..].to_vec()
    }
}

/// Strip leading zero bytes. An all-zero input collapses to an empty
/// `Vec` (the RLP encoding of integer zero).
fn strip_leading_zeros(b: &[u8]) -> Vec<u8> {
    let skip = b.iter().take_while(|&&x| x == 0).count();
    b[skip..].to_vec()
}

// ─── W3b — Persistent-DB-backed variants ──────────────────────────────────
//
// The `_with_db` variants replace the in-memory `MemoryTrieDB` used above
// with a caller-supplied persistent `TrieDB` (typically a `ZbxDbTrieAdapter`
// over a RocksDB column). This unlocks two things the W2/W3a in-memory
// path could never do:
//
// 1. **Partial-overwrite correctness**: per-account storage tries can be
//    re-opened from `account.storage_root` via `MutableTrie::from_root`
//    and have only the dirty slot deltas applied. Pre-existing un-cached
//    slots are read transparently from the persistent store.
//
// 2. **Persistence across blocks**: trie nodes generated this block are
//    flushed to disk so the next block's `from_root(...)` call can find
//    them. The caller is responsible for invoking `db.commit()` after
//    `state_root` returns and before the block header is published.

use zbx_trie::TrieDB;

/// Persistent variant of `compute_state_root`.
///
/// Uses the supplied `TrieDB` for **both** the account-trie and the per-
/// account storage tries. Each per-account storage trie is opened via
/// `MutableTrie::from_root(account.storage_root, db)` so existing slots
/// are preserved when only some are overwritten this block.
///
/// # Important — caller commits
///
/// This function does NOT call `db.commit()`. The caller MUST flush the
/// adapter's pending buffer after reading the returned root, otherwise
/// the new trie nodes will be lost on shutdown.
///
/// # Errors
///
/// Returns `Err` when the persistent store yields a `MissingNode` (e.g.
/// the supplied `storage_root` references a node that isn't on disk
/// yet) or when an underlying I/O failure surfaces from `TrieDB`.
pub fn compute_state_root_with_db<DB>(
    accounts: &HashMap<Address, AccountState>,
    storage: &HashMap<Address, HashMap<H256, H256>>,
    db: DB,
) -> Result<H256, zbx_trie::TrieError>
where
    DB: TrieDB + Clone,
{
    // Account trie is fresh per call (we don't yet thread a stable account-
    // trie root through the executor; that's a future optimisation). Each
    // storage trie reuses the per-account `storage_root` via from_root.
    let mut acct_trie = MutableTrie::new(db.clone());

    for (addr, state) in accounts {
        let acct_storage = storage.get(addr);
        let cache_is_empty = acct_storage.map_or(true, |s| s.is_empty());
        if state.is_empty() && cache_is_empty {
            continue;
        }

        // Per-account storage: open the existing trie at account.storage_root
        // and apply dirty-slot deltas. Empty cache → preserve existing root.
        let storage_root = match acct_storage {
            Some(slots) if !slots.is_empty() => {
                compute_storage_root_with_db(slots, state.storage_root, db.clone())?
            }
            _ => state.storage_root,
        };

        let key = keccak256(&addr.0);
        let value = encode_account_rlp(state, storage_root);
        acct_trie.insert(&key.0, value)?;
    }

    Ok(acct_trie.root())
}

/// Persistent variant of `compute_storage_root`.
///
/// Re-opens the existing per-account storage trie at `prev_root` via
/// `MutableTrie::from_root` and applies only the dirty slot deltas.
/// Zero-value slots delete the binding (Yellow Paper §4.1 "absence").
///
/// Returns `EMPTY_STORAGE_ROOT` when the trie ends up empty after all
/// dirty deltas are applied.
pub fn compute_storage_root_with_db<DB>(
    slots: &HashMap<H256, H256>,
    prev_root: H256,
    db: DB,
) -> Result<H256, zbx_trie::TrieError>
where
    DB: TrieDB,
{
    let mut trie = MutableTrie::from_root(prev_root, db);

    for (slot, value) in slots {
        let key = keccak256(&slot.0);
        if value.is_zero() {
            // Yellow Paper: zero is "absence" — delete any existing binding.
            // Tolerate KeyNotFound (the slot may simply not exist yet).
            match trie.delete(&key.0) {
                Ok(_) => {}
                Err(zbx_trie::TrieError::KeyNotFound) => {}
                Err(e) => return Err(e),
            }
        } else {
            let stripped = strip_leading_zeros(&value.0);
            let mut s = RlpStream::new();
            s.append(&stripped);
            trie.insert(&key.0, s.out())?;
        }
    }

    let r = trie.root();
    Ok(if r == EMPTY_ROOT {
        EMPTY_STORAGE_ROOT
    } else {
        r
    })
}
