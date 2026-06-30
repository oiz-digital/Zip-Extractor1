//! S33-state-root W3b — `ZbxDbTrieAdapter` persistence & MPT integration tests.
//!
//! Verifies that trie nodes written via the adapter survive across:
//! 1. Adapter clones (shared pending buffer)
//! 2. Explicit `commit()` flushes (durable on RocksDB)
//! 3. ZbxDb close + re-open (cold-restart correctness)
//!
//! Plus end-to-end Merkle-Patricia Trie ops:
//! - Insert + read-back through the adapter
//! - `MutableTrie::from_root(prev_root, adapter)` reopens the persisted trie
//! - Partial-overwrite parity: incremental update via `from_root` produces
//!   the same root as a full rebuild from scratch (the W2 honest-limitation
//!   closure that W3b was scoped to deliver)
//! - State-root with adapter: `mpt::compute_state_root_with_db` matches
//!   `mpt::compute_state_root` for full-cache inputs (regression boundary)

use std::collections::HashMap;
use std::sync::Arc;

use tempfile::TempDir;

use zbx_state::mpt;
use zbx_state::ZbxDbTrieAdapter;
use zbx_storage::ZbxDb;
use zbx_trie::{MutableTrie, TrieDB, EMPTY_ROOT};
use zbx_types::{
    account::AccountState,
    address::Address,
    H256,
};

// ─── Test helpers ─────────────────────────────────────────────────────────

fn open_db() -> (TempDir, Arc<ZbxDb>) {
    let dir = TempDir::new().expect("tempdir");
    let db = ZbxDb::open(dir.path()).expect("open ZbxDb");
    (dir, Arc::new(db))
}

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

fn val(byte: u8) -> H256 {
    let mut v = [0u8; 32];
    v[31] = byte;
    H256(v)
}

fn nonzero(nonce: u64, balance_u128: u128) -> AccountState {
    let mut s = AccountState::default();
    s.nonce = nonce;
    s.set_balance_u128(balance_u128);
    s
}

// ─── Adapter — basic correctness ──────────────────────────────────────────

#[test]
fn adapter_get_returns_none_for_unknown_hash() {
    let (_d, db) = open_db();
    let adapter = ZbxDbTrieAdapter::new(db);
    let h = H256([0xAB; 32]);
    let got = adapter.get(&h).expect("get");
    assert!(got.is_none(), "unknown hash must return None");
}

#[test]
fn adapter_insert_then_get_within_pending_buffer() {
    let (_d, db) = open_db();
    let mut adapter = ZbxDbTrieAdapter::new(db);
    let h = H256([0x11; 32]);
    adapter.insert(h, b"hello".to_vec()).expect("insert");
    assert_eq!(adapter.pending_len(), 1, "insert must enqueue");
    let got = adapter.get(&h).expect("get");
    assert_eq!(
        got.as_deref(),
        Some(b"hello".as_ref()),
        "in-buffer reads must see pending writes"
    );
}

#[test]
fn adapter_commit_flushes_to_disk() {
    let (_d, db) = open_db();
    let mut adapter = ZbxDbTrieAdapter::new(Arc::clone(&db));
    let h = H256([0x22; 32]);
    adapter.insert(h, b"world".to_vec()).expect("insert");
    adapter.commit().expect("commit");
    assert_eq!(adapter.pending_len(), 0, "commit must drain pending");

    // Verify by direct ZbxDb read (bypassing adapter buffer).
    let raw = db.get_trie_node(&h).expect("get_trie_node");
    assert_eq!(raw.as_deref(), Some(b"world".as_ref()));
}

#[test]
fn adapter_commit_is_idempotent_when_empty() {
    let (_d, db) = open_db();
    let adapter = ZbxDbTrieAdapter::new(db);
    adapter.commit().expect("first commit");
    adapter.commit().expect("second commit on empty buffer");
}

#[test]
fn adapter_clones_share_pending_buffer() {
    let (_d, db) = open_db();
    let mut a1 = ZbxDbTrieAdapter::new(db);
    let a2 = a1.clone();
    let h = H256([0x33; 32]);
    a1.insert(h, b"shared".to_vec()).expect("insert via a1");
    // a2 should see the buffered write because they share pending.
    let got = a2.get(&h).expect("get via a2");
    assert_eq!(
        got.as_deref(),
        Some(b"shared".as_ref()),
        "cloned adapter must share pending buffer"
    );
}

#[test]
fn adapter_survives_db_close_and_reopen() {
    let dir = TempDir::new().expect("tempdir");
    let h = H256([0x44; 32]);

    {
        let db = Arc::new(ZbxDb::open(dir.path()).expect("open"));
        let mut adapter = ZbxDbTrieAdapter::new(db);
        adapter.insert(h, b"durable".to_vec()).expect("insert");
        adapter.commit().expect("commit");
    } // db dropped here

    // Re-open and verify the value is still there.
    let db2 = ZbxDb::open(dir.path()).expect("reopen");
    let raw = db2.get_trie_node(&h).expect("get after reopen");
    assert_eq!(
        raw.as_deref(),
        Some(b"durable".as_ref()),
        "trie node must survive db close+reopen"
    );
}

#[test]
fn adapter_contains_reflects_buffer_and_disk() {
    let (_d, db) = open_db();
    let mut adapter = ZbxDbTrieAdapter::new(Arc::clone(&db));
    let h_buf = H256([0x55; 32]);
    let h_disk = H256([0x66; 32]);
    let h_absent = H256([0x77; 32]);

    adapter.insert(h_buf, b"a".to_vec()).expect("insert buffered");
    db.put_trie_node(h_disk, b"b".to_vec()).expect("direct disk write");

    assert!(adapter.contains(&h_buf).expect("contains buf"));
    assert!(adapter.contains(&h_disk).expect("contains disk"));
    assert!(!adapter.contains(&h_absent).expect("contains absent"));
}

// ─── MutableTrie ↔ adapter end-to-end ────────────────────────────────────

#[test]
fn trie_built_via_adapter_persists_across_reopen() {
    let dir = TempDir::new().expect("tempdir");
    let key = b"hello";
    let value = b"world".to_vec();

    let root = {
        let db = Arc::new(ZbxDb::open(dir.path()).expect("open"));
        let adapter = ZbxDbTrieAdapter::new(db);
        let mut trie = MutableTrie::new(adapter.clone());
        trie.insert(key, value.clone()).expect("trie insert");
        let r = trie.root();
        adapter.commit().expect("commit trie nodes");
        r
    };

    // Re-open with a fresh adapter and prove we can read the value back
    // using the previously-recorded root.
    let db2 = Arc::new(ZbxDb::open(dir.path()).expect("reopen"));
    let adapter2 = ZbxDbTrieAdapter::new(db2);
    let trie2 = MutableTrie::from_root(root, adapter2);
    let got = trie2.get(key).expect("trie get");
    assert_eq!(
        got.as_deref(),
        Some(value.as_slice()),
        "MutableTrie must reload from disk-persisted nodes"
    );
}

// ─── mpt::compute_state_root_with_db — integration ───────────────────────

#[test]
fn compute_state_root_with_db_matches_in_memory_for_full_cache() {
    // When all storage is in cache (no need to read pre-existing slots),
    // the persistent variant must agree with the in-memory variant.
    let (_d, db) = open_db();
    let adapter = ZbxDbTrieAdapter::new(db);

    let mut accounts = HashMap::new();
    accounts.insert(addr(1), nonzero(1, 100));
    accounts.insert(addr(2), nonzero(2, 200));

    let mut storage: HashMap<Address, HashMap<H256, H256>> = HashMap::new();
    let mut a1_slots = HashMap::new();
    a1_slots.insert(slot(1), val(0xAA));
    a1_slots.insert(slot(2), val(0xBB));
    storage.insert(addr(1), a1_slots);

    let in_mem = mpt::compute_state_root(&accounts, &storage);
    let with_db = mpt::compute_state_root_with_db(&accounts, &storage, adapter.clone())
        .expect("with_db");

    assert_eq!(
        in_mem, with_db,
        "_with_db and in-memory variants must agree on full-cache inputs"
    );
    adapter.commit().expect("flush trie nodes");
}

#[test]
fn compute_state_root_with_db_returns_empty_for_no_accounts() {
    let (_d, db) = open_db();
    let adapter = ZbxDbTrieAdapter::new(db);
    let accounts: HashMap<Address, AccountState> = HashMap::new();
    let storage: HashMap<Address, HashMap<H256, H256>> = HashMap::new();
    let r = mpt::compute_state_root_with_db(&accounts, &storage, adapter)
        .expect("empty");
    assert_eq!(r, EMPTY_ROOT, "empty visible-set must produce EMPTY_ROOT");
}

// ─── Partial-overwrite parity (the W2 honest-limitation closure) ─────────

#[test]
fn partial_overwrite_via_from_root_matches_full_rebuild() {
    // This is the W3b headline test: it proves that an incremental
    // storage_root update via `from_root + delta` produces the same root
    // as a full rebuild that has every slot in cache. This was IMPOSSIBLE
    // under the W2/W3a in-memory-only path.
    let dir = TempDir::new().expect("tempdir");
    let db = Arc::new(ZbxDb::open(dir.path()).expect("open"));

    // ─── Phase 1: build the "pre-existing" storage trie with slots 1, 2, 3
    let pre_root = {
        let adapter = ZbxDbTrieAdapter::new(Arc::clone(&db));
        let mut t = MutableTrie::new(adapter.clone());
        let mut s = zbx_rlp::RlpStream::new();
        s.append(&vec![0x11]);
        t.insert(&zbx_crypto::keccak::keccak256(&slot(1).0).0, s.out())
            .expect("insert s1");

        let mut s = zbx_rlp::RlpStream::new();
        s.append(&vec![0x22]);
        t.insert(&zbx_crypto::keccak::keccak256(&slot(2).0).0, s.out())
            .expect("insert s2");

        let mut s = zbx_rlp::RlpStream::new();
        s.append(&vec![0x33]);
        t.insert(&zbx_crypto::keccak::keccak256(&slot(3).0).0, s.out())
            .expect("insert s3");

        let r = t.root();
        adapter.commit().expect("commit phase 1");
        r
    };
    assert_ne!(pre_root, EMPTY_ROOT, "phase-1 trie must be non-empty");

    // ─── Phase 2a: incremental update (the production path) — only
    //              modify slot 2 to 0x99 via from_root, leaving 1+3 alone.
    let incremental_root = {
        let adapter = ZbxDbTrieAdapter::new(Arc::clone(&db));
        let mut dirty = HashMap::new();
        dirty.insert(slot(2), val(0x99));
        mpt::compute_storage_root_with_db(&dirty, pre_root, adapter.clone())
            .expect("incremental compute")
    };

    // ─── Phase 2b: full rebuild (the test-oracle path) — rebuild a
    //              fresh trie with slots 1, 2 (now 0x99), 3 in cache.
    let full_rebuild_root = {
        let mut full = HashMap::new();
        full.insert(slot(1), val(0x11));
        full.insert(slot(2), val(0x99));
        full.insert(slot(3), val(0x33));
        mpt::compute_storage_root(&full)
    };

    assert_eq!(
        incremental_root, full_rebuild_root,
        "incremental from_root update must match full rebuild — closes W2 honest limitation"
    );
}

#[test]
fn partial_overwrite_with_zero_value_deletes_slot() {
    // When a partial update writes zero to an existing slot, the slot
    // must be deleted from the trie (YP §4.1 "absence" rule). The
    // resulting root must match a full rebuild that omits that slot.
    let dir = TempDir::new().expect("tempdir");
    let db = Arc::new(ZbxDb::open(dir.path()).expect("open"));

    // Phase 1: build a trie with slots 1, 2, 3.
    let pre_root = {
        let adapter = ZbxDbTrieAdapter::new(Arc::clone(&db));
        let mut t = MutableTrie::new(adapter.clone());
        for (s, v) in &[(slot(1), 0x11u8), (slot(2), 0x22), (slot(3), 0x33)] {
            let mut rs = zbx_rlp::RlpStream::new();
            rs.append(&vec![*v]);
            t.insert(&zbx_crypto::keccak::keccak256(&s.0).0, rs.out())
                .expect("insert");
        }
        let r = t.root();
        adapter.commit().expect("commit");
        r
    };

    // Phase 2a: zero-out slot 2 incrementally.
    let incr = {
        let adapter = ZbxDbTrieAdapter::new(Arc::clone(&db));
        let mut dirty = HashMap::new();
        dirty.insert(slot(2), H256::zero());
        mpt::compute_storage_root_with_db(&dirty, pre_root, adapter)
            .expect("incremental zero-delete")
    };

    // Phase 2b: full rebuild without slot 2.
    let full = {
        let mut m = HashMap::new();
        m.insert(slot(1), val(0x11));
        m.insert(slot(3), val(0x33));
        mpt::compute_storage_root(&m)
    };

    assert_eq!(
        incr, full,
        "zero-write delete via from_root must equal full rebuild without that slot"
    );
}
