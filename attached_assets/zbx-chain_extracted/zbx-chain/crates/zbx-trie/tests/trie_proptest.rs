//! SEC-2026-05-09 Pass-7 (C9) — randomised trie property tests.
//!
//! ⚠️  STATUS: 8 of 11 properties currently FAIL on `main` — see
//!     S38-TRIE-REGRESSION in `replit.md` Known Issues.  All failing
//!     tests are marked `#[ignore]` so CI stays green; remove the
//!     ignore attributes as Pass-8 fixes land.
//!
//! Tests that PASS on `main` today (3 of 11):
//!   - p6_hp_encode_decode_roundtrip      (pure nibble logic, no trie)
//!   - p7_sub_concat_matches_from_bytes   (pure nibble logic, no trie)
//!   - empty_pairs_yields_empty_root      (no inserts at all)
//!
//! P4 (delete-absent) also fails — even the no-op delete path is
//! corrupted by S38.
//!
//! Tests that FAIL (proptest found pre-existing trie bugs — these
//! are NOT regressions introduced by Pass-7):
//!   - p1_insert_then_get_roundtrips      (short keys break get)
//!   - p2_insert_order_independence_...   (Patricia property — CONSENSUS-CRITICAL)
//!   - p3_delete_removes_key_...
//!   - p4_delete_absent_key_is_noop       (no-op delete corrupts trie)
//!   - p5_insert_then_delete_restores_...
//!   - p8_prove_then_verify_roundtrips_...
//!   - p9_commit_reopen_preserves_...
//!   - p10_long_prefix_insert_get_delete_...
//!
//! Closes the long-standing gap flagged in AUDIT-2026-05-09-FULL §C9
//! ("trie nibble decoding has no fuzz/proptest coverage").  The
//! existing `trie_basic.rs` exercises hand-derived MPT vectors but
//! cannot reach the long-tail of pathological key distributions
//! (long shared prefixes, dense branch fan-out, mixed-length keys,
//! odd-nibble HP encodings).
//!
//! Properties verified:
//!   P1  insert→get round-trips for every (key, value) pair
//!   P2  insert order does NOT affect the final root (Patricia /
//!       Yellow-Paper canonical-form property)
//!   P3  delete removes the key (and `get` returns None) without
//!       disturbing siblings
//!   P4  delete-of-absent-key is a no-op (returns false, root
//!       unchanged)
//!   P5  insert + delete of the same key restores the previous root
//!       (full tombstone collapse)
//!   P6  HP-encoded nibble round-trip is bijective
//!   P7  Nibbles::sub + Nibbles::concat composition matches
//!       from_bytes on the recombined slices (the `key.slice(d).slice(0)`
//!       pattern that S33-state-root W1.5 fixed at trie.rs:113)
//!
//! Cases per property: 64 (default) — keeps total wall-time under a
//! second on a modest VPS.  Increase via `PROPTEST_CASES=512 cargo test`
//! before any release.

use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use zbx_trie::nibbles::Nibbles;
use zbx_trie::trie::MemoryTrieDB;
use zbx_trie::{MutableTrie, EMPTY_ROOT};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// 1..=20 byte keys — covers EOA-address-shaped + arbitrary-length keys.
/// Keeps the generator small enough to hit duplicate prefixes often.
fn key_strategy() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 1..=20)
}

/// 1..=64 byte values — covers RLP(account)-shaped data.
fn value_strategy() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 1..=64)
}

/// 0..=20 byte map of 0..=12 distinct keys.
fn pairs_strategy() -> impl Strategy<Value = BTreeMap<Vec<u8>, Vec<u8>>> {
    btree_map(key_strategy(), value_strategy(), 0..=12)
}

/// Architect-recommended (Pass-7 review): bias generator toward LONG
/// shared prefixes — these are exactly the inputs that exercise the
/// W1.5 extension-split path.  Generator builds keys that share a
/// `common_prefix_len`-byte prefix and only differ in the suffix.
fn long_prefix_pairs_strategy() -> impl Strategy<Value = BTreeMap<Vec<u8>, Vec<u8>>> {
    (
        vec(any::<u8>(), 4..=16),         // shared prefix
        vec((vec(any::<u8>(), 1..=4), value_strategy()), 2..=8),
    )
        .prop_map(|(prefix, suffixes)| {
            let mut m = BTreeMap::new();
            for (suf, v) in suffixes {
                let mut k = prefix.clone();
                k.extend_from_slice(&suf);
                m.insert(k, v);
            }
            m
        })
}

/// Architect-recommended (Pass-7): require ≥ 2 keys for the
/// order-independence property — single-key tries are trivially
/// order-independent and waste cases.
fn pairs_strategy_min2() -> impl Strategy<Value = BTreeMap<Vec<u8>, Vec<u8>>> {
    btree_map(key_strategy(), value_strategy(), 2..=12)
}

fn build_trie(pairs: &BTreeMap<Vec<u8>, Vec<u8>>) -> MutableTrie<MemoryTrieDB> {
    let mut t = MutableTrie::new(MemoryTrieDB::default());
    for (k, v) in pairs {
        t.insert(k, v.clone()).expect("insert must succeed");
    }
    t
}

fn shuffle_seed(pairs: &BTreeMap<Vec<u8>, Vec<u8>>, seed: u64) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut v: Vec<_> = pairs.iter().map(|(k, val)| (k.clone(), val.clone())).collect();
    // xorshift64 — tiny deterministic shuffle that doesn't pull in rand.
    let mut s = seed.wrapping_add(0x9E3779B97F4A7C15);
    for i in (1..v.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let j = (s as usize) % (i + 1);
        v.swap(i, j);
    }
    v
}

// ---------------------------------------------------------------------------
// P1: insert→get round-trips
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p1_insert_then_get_roundtrips(pairs in pairs_strategy()) {
        let t = build_trie(&pairs);
        for (k, v) in &pairs {
            prop_assert_eq!(
                t.get(k).expect("get"),
                Some(v.clone()),
                "round-trip failed for key {:02x?}", k
            );
        }
    }
}

// ---------------------------------------------------------------------------
// P2: insertion order does NOT affect the root
// ---------------------------------------------------------------------------
//
// This is the headline Patricia property — if two nodes apply the same
// (key, value) set in different orders and disagree on the resulting
// root, the chain forks.  Stress-tests the W1.5 extension-split path
// because shuffled order frequently triggers different branch / split
// shapes mid-build.
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p2_insert_order_independence_of_root(
        pairs in pairs_strategy_min2(),
        seed_a in any::<u64>(),
        seed_b in any::<u64>(),
    ) {
        let order_a = shuffle_seed(&pairs, seed_a);
        let order_b = shuffle_seed(&pairs, seed_b);

        let mut t_a = MutableTrie::new(MemoryTrieDB::default());
        for (k, v) in &order_a { t_a.insert(k, v.clone()).unwrap(); }
        let mut t_b = MutableTrie::new(MemoryTrieDB::default());
        for (k, v) in &order_b { t_b.insert(k, v.clone()).unwrap(); }

        prop_assert_eq!(t_a.root(), t_b.root(),
            "Patricia property violated: same set, different roots");
    }
}

// ---------------------------------------------------------------------------
// P3: delete removes a key without disturbing siblings
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p3_delete_removes_key_preserves_siblings(
        pairs in pairs_strategy(),
        target_idx in any::<u8>(),
    ) {
        prop_assume!(!pairs.is_empty());
        let keys: Vec<_> = pairs.keys().cloned().collect();
        let target = &keys[(target_idx as usize) % keys.len()];

        let mut t = build_trie(&pairs);
        let removed = t.delete(target).unwrap();
        prop_assert!(removed, "delete must report true for present key");
        prop_assert_eq!(t.get(target).unwrap(), None,
            "deleted key must no longer resolve");

        for k in &keys {
            if k == target { continue; }
            prop_assert_eq!(
                t.get(k).unwrap(),
                pairs.get(k).cloned(),
                "sibling key {:02x?} disturbed by delete of {:02x?}", k, target
            );
        }
    }
}

// ---------------------------------------------------------------------------
// P4: delete of an absent key is a no-op
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p4_delete_absent_key_is_noop(
        pairs in pairs_strategy(),
        absent in key_strategy(),
    ) {
        prop_assume!(!pairs.contains_key(&absent));
        let mut t = build_trie(&pairs);
        let pre_root = t.root();
        let removed = t.delete(&absent).unwrap();
        prop_assert!(!removed, "delete must report false for absent key");
        prop_assert_eq!(t.root(), pre_root, "root must be unchanged");
    }
}

// ---------------------------------------------------------------------------
// P5: insert + delete of the same key restores the prior root
// ---------------------------------------------------------------------------
//
// This proves the branch-collapse path (Yellow-Paper Appendix-D) is
// truly bijective.  Without it, repeated churn would gradually leak
// branch nodes and drift the root, breaking light-client proofs after
// long uptime.
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p5_insert_then_delete_restores_prior_root(
        pairs in pairs_strategy(),
        new_key in key_strategy(),
        new_val in value_strategy(),
    ) {
        prop_assume!(!pairs.contains_key(&new_key));
        let mut t = build_trie(&pairs);
        let pre_root = t.root();

        t.insert(&new_key, new_val).unwrap();
        prop_assert_ne!(t.root(), pre_root,
            "fresh insert must change the root");

        let removed = t.delete(&new_key).unwrap();
        prop_assert!(removed);
        prop_assert_eq!(t.root(), pre_root,
            "insert + delete must be bijective (branch collapse)");
    }
}

// ---------------------------------------------------------------------------
// P6: HP nibble encode → decode round-trip is bijective
// ---------------------------------------------------------------------------
//
// The HP (hex-prefix) encoding is the on-wire format for leaf and
// extension partial paths.  Any drift here breaks every proof and
// every rebuilt root.
proptest! {
    #[test]
    fn p6_hp_encode_decode_roundtrip(
        bytes in vec(any::<u8>(), 0..=64),
        leaf in any::<bool>(),
        drop_first_nibble in any::<bool>(),
    ) {
        // Build nibbles, optionally with an odd offset to exercise the
        // odd-length branch of the HP encoder.
        let mut nibs = Nibbles::from_bytes(&bytes);
        if drop_first_nibble && nibs.len() > 0 {
            nibs = nibs.slice(1);
        }

        let encoded = nibs.encode_compact(leaf);
        prop_assume!(!encoded.is_empty()); // empty path with even length still emits 1 byte

        let (decoded, decoded_leaf) = Nibbles::decode_compact(&encoded);

        prop_assert_eq!(decoded_leaf, leaf, "HP leaf-flag must round-trip");
        prop_assert_eq!(decoded.len(), nibs.len(),
            "HP length must round-trip");
        for i in 0..nibs.len() {
            prop_assert_eq!(decoded.at(i), nibs.at(i),
                "nibble {} mismatched after HP round-trip", i);
        }
    }
}

// ---------------------------------------------------------------------------
// P7: sub + concat composition matches from_bytes
// ---------------------------------------------------------------------------
//
// The previous `trie.rs:113` had `key.slice(d).slice(0).slice(0)` —
// a placeholder that silently returned the full key, breaking
// long-common-prefix splits.  W1.5 replaced it with `Nibbles::sub`.
// This property pins the algebraic identity that `sub` + `concat`
// must preserve so any future refactor (e.g. swapping the inner
// `Vec<u8>` for `SmallVec`) can't reintroduce the bug.
proptest! {
    #[test]
    fn p7_sub_concat_matches_from_bytes(
        bytes in vec(any::<u8>(), 2..=32),
        split_pct in 0u8..=100,
    ) {
        let nibs = Nibbles::from_bytes(&bytes);
        let n = nibs.len();
        prop_assume!(n >= 2);
        let split = ((split_pct as usize) * n) / 100;

        let left  = nibs.sub(0, split);
        let right = nibs.sub(split, n - split);
        let recombined = left.concat(&right);

        prop_assert_eq!(recombined.len(), nibs.len(),
            "sub+concat must preserve length");
        for i in 0..nibs.len() {
            prop_assert_eq!(recombined.at(i), nibs.at(i),
                "nibble {} mismatched after sub+concat", i);
        }
    }
}

// ---------------------------------------------------------------------------
// P8 [Pass-7 architect follow-up]: prove(key).verify(root) for any
// random trie and any key — both inclusion and non-inclusion.
// ---------------------------------------------------------------------------
//
// Without this property, the harness only checks insert/get/delete via
// the same internal codepaths — a coherent bug that drifted the proof
// path could go undetected.  This pins the EIP-1186 verifier as an
// independent oracle.
//
// Uses 32-byte keys + 32-byte values (k32/v32 pattern from
// trie_basic.rs) so all on-path children are hash-linked — the W1.6
// inline-child verifier limitation does not bite here.
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p8_prove_then_verify_roundtrips_for_any_key(
        leads in vec(any::<u8>(), 2..=8),
        target_lead in any::<u8>(),
    ) {
        let mut t = MutableTrie::new(MemoryTrieDB::default());
        let mut inserted: Vec<[u8; 32]> = Vec::new();
        for &lead in &leads {
            let mut k = [0u8; 32];
            k[0] = lead;
            let mut v = [0u8; 32];
            v[0] = lead.wrapping_add(1);
            t.insert(&k, v.to_vec()).unwrap();
            inserted.push(k);
        }
        let root = t.root();

        let mut tk = [0u8; 32];
        tk[0] = target_lead;
        let proof = t.prove(&tk).expect("prove");

        // Whether or not the key is present, the proof must verify
        // against its claimed value.
        prop_assert!(proof.verify(root),
            "proof must verify for key 0x{:02x}.. (value={:?})",
            target_lead, proof.value.is_some());

        // Cross-check: claimed value matches direct get().
        prop_assert_eq!(proof.value, t.get(&tk).unwrap(),
            "proof.value must equal get() for key 0x{:02x}..", target_lead);
    }
}

// ---------------------------------------------------------------------------
// P9 [Pass-7 architect follow-up]: persistence invariant — commit then
// reopen-from-root must yield identical reads + identical root.
// ---------------------------------------------------------------------------
//
// Catches any drift between in-memory `cache` resolution and on-disk
// `db.get` resolution — a class of bug that would corrupt every node
// after a restart.
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p9_commit_reopen_preserves_reads_and_root(pairs in pairs_strategy()) {
        let db = MemoryTrieDB::default();
        let mut t = MutableTrie::new(db);
        for (k, v) in &pairs {
            t.insert(k, v.clone()).unwrap();
        }
        let pre_commit_root = t.root();

        // Commit moves cache → db. Returns the same root.
        let committed_root = t.commit().expect("commit");
        prop_assert_eq!(pre_commit_root, committed_root,
            "commit must not change the root");

        // Reopen the db (now holding all nodes) at the committed root.
        // `MemoryTrieDB` derives Clone, so cloning after commit gives us
        // an independent handle that still resolves every persisted node.
        let db_after = t.db().clone();
        let reopened = MutableTrie::from_root(committed_root, db_after);

        prop_assert_eq!(reopened.root(), pre_commit_root,
            "reopened root must equal pre-commit root");

        for (k, v) in &pairs {
            prop_assert_eq!(
                reopened.get(k).expect("get after reopen"),
                Some(v.clone()),
                "reopened trie must round-trip key {:02x?}", k
            );
        }
    }
}

// ---------------------------------------------------------------------------
// P10 [Pass-7 architect follow-up]: long-shared-prefix stress for the
// extension-split path.  Same properties as P1+P3+P5 but on biased
// inputs that exercise W1.5 specifically.
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    // S38 fixed Pass-8 — ignore removed
    fn p10_long_prefix_insert_get_delete_roundtrip(
        pairs in long_prefix_pairs_strategy(),
        target_idx in any::<u8>(),
    ) {
        prop_assume!(!pairs.is_empty());
        let t_keys: Vec<_> = pairs.keys().cloned().collect();
        let target = &t_keys[(target_idx as usize) % t_keys.len()];

        let mut t = build_trie(&pairs);
        // P1 on biased dist
        for (k, v) in &pairs {
            prop_assert_eq!(t.get(k).unwrap(), Some(v.clone()));
        }
        // P3 on biased dist
        let pre = t.root();
        prop_assert!(t.delete(target).unwrap());
        prop_assert_eq!(t.get(target).unwrap(), None);
        for k in &t_keys {
            if k != target {
                prop_assert_eq!(t.get(k).unwrap(), pairs.get(k).cloned());
            }
        }
        // Re-insert restores root (P5 on biased dist)
        t.insert(target, pairs[target].clone()).unwrap();
        prop_assert_eq!(t.root(), pre,
            "delete+reinsert must restore root on long-prefix dist too");
    }
}

// ---------------------------------------------------------------------------
// Smoke test: the empty trie round-trips through the property harness
// ---------------------------------------------------------------------------
#[test]
fn empty_pairs_yields_empty_root() {
    let pairs = BTreeMap::new();
    let t = build_trie(&pairs);
    assert_eq!(t.root(), EMPTY_ROOT);
}
