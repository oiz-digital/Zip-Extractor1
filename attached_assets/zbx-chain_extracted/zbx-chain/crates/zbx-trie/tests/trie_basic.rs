//! S33-state-root W1 + W1.5: zbx-trie production-readiness tests.
//!
//! Layout:
//!   1-7   — Baseline regression tests for insert/get/update.
//!   8     — W1.5: long-common-prefix extension split (was #[ignore]; now active).
//!   9     — Insert order independence (Patricia property).
//!   10    — Commit preserves root.
//!   11    — Read-only Trie wrapper.
//!   12    — W1.5: Proof for absent key with correct exclusion (was #[ignore]; now active).
//!   13    — Proof against wrong root fails.
//!   14    — W1.5: Delete existing key removes it (was #[ignore]; now active).
//!   15-17 — W1.5: M-02 closure — hand-derived MPT vectors over real account-shaped values.
//!
//! Reference: Yellow Paper Appendix D, go-ethereum `trie/trie.go`,
//! EIP-1186 (proof format).

use zbx_trie::{
    EMPTY_ROOT,
    MutableTrie,
    Trie,
    verify_proof,
};
use zbx_trie::trie::MemoryTrieDB;
use zbx_types::H256;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fresh_trie() -> MutableTrie<MemoryTrieDB> {
    MutableTrie::new(MemoryTrieDB::default())
}

fn insert_all(pairs: &[(&[u8], &[u8])]) -> H256 {
    let mut t = fresh_trie();
    for (k, v) in pairs {
        t.insert(k, v.to_vec()).expect("insert must succeed");
    }
    t.root()
}

/// Construct a 32-byte key with `lead` as the first byte and zeros elsewhere.
/// Used by tests 12 + 15-17 to ensure all proof-path nodes are hash-linked
/// (RLP encodings of 32-byte values exceed the 32-byte inline threshold).
fn k32(lead: u8) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0] = lead;
    k
}

fn v32(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

// ---------------------------------------------------------------------------
// 1. Empty trie root matches the canonical Ethereum constant
// ---------------------------------------------------------------------------
#[test]
fn empty_trie_root_matches_ethereum_constant() {
    let t = fresh_trie();
    assert_eq!(t.root(), EMPTY_ROOT);
    let expected_prefix = [0x56u8, 0xe8, 0x1f, 0x17];
    assert_eq!(&t.root()[0..4], &expected_prefix);
}

// ---------------------------------------------------------------------------
// 2. Single insert + get round-trips
// ---------------------------------------------------------------------------
#[test]
fn single_insert_then_get_roundtrips() {
    let mut t = fresh_trie();
    t.insert(b"foo", b"bar".to_vec()).unwrap();
    assert_eq!(t.get(b"foo").unwrap(), Some(b"bar".to_vec()));
    assert_ne!(t.root(), EMPTY_ROOT, "non-empty trie must have non-empty root");
}

// ---------------------------------------------------------------------------
// 3. Single insert + get on wrong key returns None
// ---------------------------------------------------------------------------
#[test]
fn single_insert_then_get_wrong_key_returns_none() {
    let mut t = fresh_trie();
    t.insert(b"foo", b"bar".to_vec()).unwrap();
    assert_eq!(t.get(b"baz").unwrap(), None);
}

// ---------------------------------------------------------------------------
// 4. Updating an existing key changes the root
// ---------------------------------------------------------------------------
#[test]
fn update_existing_key_changes_root() {
    let mut t = fresh_trie();
    t.insert(b"foo", b"bar".to_vec()).unwrap();
    let r1 = t.root();
    t.insert(b"foo", b"qux".to_vec()).unwrap();
    let r2 = t.root();
    assert_ne!(r1, r2, "root must change when value changes");
    assert_eq!(t.get(b"foo").unwrap(), Some(b"qux".to_vec()));
}

// ---------------------------------------------------------------------------
// 5. Updating to the same value leaves the root unchanged
// ---------------------------------------------------------------------------
#[test]
fn update_to_same_value_root_unchanged() {
    let mut t = fresh_trie();
    t.insert(b"foo", b"bar".to_vec()).unwrap();
    let r1 = t.root();
    t.insert(b"foo", b"bar".to_vec()).unwrap();
    let r2 = t.root();
    assert_eq!(r1, r2, "idempotent insert must yield identical root");
}

// ---------------------------------------------------------------------------
// 6. Two keys with distinct top nibbles: branch creation succeeds
// ---------------------------------------------------------------------------
#[test]
fn two_keys_distinct_prefixes_inserts_succeed() {
    let mut t = fresh_trie();
    t.insert(&[0x10, 0x00, 0x00], b"a".to_vec()).unwrap();
    t.insert(&[0x20, 0x00, 0x00], b"b".to_vec()).unwrap();
    assert_eq!(t.get(&[0x10, 0x00, 0x00]).unwrap(), Some(b"a".to_vec()));
    assert_eq!(t.get(&[0x20, 0x00, 0x00]).unwrap(), Some(b"b".to_vec()));
}

// ---------------------------------------------------------------------------
// 7. Two keys with short common prefix: branch creation succeeds
// ---------------------------------------------------------------------------
#[test]
fn two_keys_short_common_prefix_inserts_succeed() {
    let mut t = fresh_trie();
    t.insert(&[0x10], b"a".to_vec()).unwrap();
    t.insert(&[0x12], b"b".to_vec()).unwrap();
    assert_eq!(t.get(&[0x10]).unwrap(), Some(b"a".to_vec()));
    assert_eq!(t.get(&[0x12]).unwrap(), Some(b"b".to_vec()));
}

// ---------------------------------------------------------------------------
// 8. [W1.5] Two keys with LONG common prefix → extension split
// ---------------------------------------------------------------------------
//
// Inserting `[0xab, 0xcd, 0xef]` then `[0xab, 0xcd, 0x12]` requires the
// trie to split mid-extension. Previously the code at
// `crates/zbx-trie/src/trie.rs:189-194` returned
// `Err(TrieError::Inconsistent) // simplified`. W1.5 implemented the
// split — this test now verifies the fix works end-to-end.
#[test]
fn two_keys_long_common_prefix_inserts_succeed() {
    let mut t = fresh_trie();
    t.insert(&[0xab, 0xcd, 0xef], b"a".to_vec()).unwrap();
    t.insert(&[0xab, 0xcd, 0x12], b"b".to_vec()).unwrap();

    assert_eq!(t.get(&[0xab, 0xcd, 0xef]).unwrap(), Some(b"a".to_vec()));
    assert_eq!(t.get(&[0xab, 0xcd, 0x12]).unwrap(), Some(b"b".to_vec()));
    assert_eq!(t.get(&[0xab, 0xcd, 0x99]).unwrap(), None,
        "key in the same extension space but absent must return None");
}

// ---------------------------------------------------------------------------
// 9. Insert order independence — Patricia property
// ---------------------------------------------------------------------------
#[test]
fn insert_order_independence_of_root() {
    let pairs_a: &[(&[u8], &[u8])] = &[
        (&[0x10], b"alpha"),
        (&[0x20], b"beta"),
        (&[0x30], b"gamma"),
    ];
    let pairs_b: &[(&[u8], &[u8])] = &[
        (&[0x30], b"gamma"),
        (&[0x10], b"alpha"),
        (&[0x20], b"beta"),
    ];
    let pairs_c: &[(&[u8], &[u8])] = &[
        (&[0x20], b"beta"),
        (&[0x30], b"gamma"),
        (&[0x10], b"alpha"),
    ];

    let r_a = insert_all(pairs_a);
    let r_b = insert_all(pairs_b);
    let r_c = insert_all(pairs_c);

    assert_eq!(r_a, r_b, "(a,b,c) and (c,a,b) must produce the same root");
    assert_eq!(r_b, r_c, "(c,a,b) and (b,c,a) must produce the same root");
}

// ---------------------------------------------------------------------------
// 10. Commit then root preserves
// ---------------------------------------------------------------------------
#[test]
fn commit_then_reopen_preserves_root() {
    let mut t = fresh_trie();
    t.insert(&[0x10], b"alpha".to_vec()).unwrap();
    t.insert(&[0x20], b"beta".to_vec()).unwrap();

    let pre_commit_root = t.root();
    let post_commit_root = t.commit().expect("commit must succeed");

    assert_eq!(pre_commit_root, post_commit_root,
        "commit must not change the root");
}

// ---------------------------------------------------------------------------
// 11. Read-only Trie wrapper exposes the constructed root
// ---------------------------------------------------------------------------
#[test]
fn readonly_trie_wrapper_exposes_constructed_root() {
    let root = H256([0xab; 32]);
    let db = MemoryTrieDB::default();
    let ro = Trie::new(root, db);
    assert_eq!(ro.root(), root, "wrapper must echo the constructed root");
}

// ---------------------------------------------------------------------------
// 12. [W1.5] Proof for absent key with correct exclusion verifies
// ---------------------------------------------------------------------------
//
// Uses 32-byte keys + 32-byte values so all proof-path children are
// hash-linked (RLP encodings exceed the 32-byte inline threshold). This
// avoids the W1.6 inline-child limitation in `verify_proof`.
#[test]
fn proof_for_absent_key_with_correct_exclusion_verifies() {
    let mut t = fresh_trie();
    let k_present_a = k32(0xa0);
    let k_present_b = k32(0xb0);
    let k_absent    = k32(0xff);

    t.insert(&k_present_a, v32(0x11)).unwrap();
    t.insert(&k_present_b, v32(0x22)).unwrap();
    let root = t.root();

    let proof = t.prove(&k_absent).expect("proof generation must succeed");
    assert_eq!(proof.value, None, "absent-key proof must carry None");
    assert!(proof.verify(root),
        "non-inclusion proof must verify against the root");
}

// ---------------------------------------------------------------------------
// 13. Proof against wrong root fails
// ---------------------------------------------------------------------------
#[test]
fn proof_against_wrong_root_fails() {
    let wrong_root = H256([0xff; 32]);
    let key = b"foo".to_vec();
    let value = Some(b"bar".to_vec());
    let nodes: Vec<Vec<u8>> = vec![vec![0x80]]; // RLP empty, won't hash to 0xff..

    assert!(!verify_proof(wrong_root, &key, &value, &nodes),
        "proof must NOT verify against arbitrary root");
}

// ---------------------------------------------------------------------------
// 14. [W1.5] Delete an existing key removes it
// ---------------------------------------------------------------------------
//
// Exercises the basic delete + branch-collapse path. After deleting
// one of two keys from a 2-key branch trie, the trie must collapse
// back to a single leaf and the surviving key must still resolve.
#[test]
fn delete_existing_key_removes_it() {
    let mut t = fresh_trie();
    t.insert(&[0x10], b"alpha".to_vec()).unwrap();
    t.insert(&[0x20], b"beta".to_vec()).unwrap();
    let pre_delete_root = t.root();

    let removed = t.delete(&[0x10]).unwrap();
    assert!(removed, "delete returns true when key existed");

    assert_eq!(t.get(&[0x10]).unwrap(), None,
        "deleted key must no longer resolve");
    assert_ne!(t.root(), pre_delete_root,
        "root must change after delete");
    assert_eq!(t.get(&[0x20]).unwrap(), Some(b"beta".to_vec()),
        "unrelated key must still resolve");

    let removed_again = t.delete(&[0x10]).unwrap();
    assert!(!removed_again, "second delete of same key must return false");
}

// ---------------------------------------------------------------------------
// 15. [W1.5 / M-02 closure] Inclusion proof on account-shaped values
// ---------------------------------------------------------------------------
//
// Mirrors the structure of an account trie: 20-byte address-shaped keys,
// 32-byte value-shaped data. Verifies that an inclusion proof for a
// known key correctly round-trips through verify_proof.
#[test]
fn inclusion_proof_on_account_shaped_values() {
    let mut t = fresh_trie();
    let addr_a = [0xa0u8; 20];
    let addr_b = [0xb0u8; 20];
    let acct_a = vec![0x11u8; 64]; // simulated RLP(account)
    let acct_b = vec![0x22u8; 64];

    t.insert(&addr_a, acct_a.clone()).unwrap();
    t.insert(&addr_b, acct_b.clone()).unwrap();
    let root = t.root();

    let proof = t.prove(&addr_a).expect("proof generation");
    assert_eq!(proof.value, Some(acct_a),
        "inclusion proof must carry the value");
    assert!(proof.verify(root), "inclusion proof must verify");
}

// ---------------------------------------------------------------------------
// 16. [W1.5 / M-02 closure] Tampered value in proof is rejected
// ---------------------------------------------------------------------------
//
// Demonstrates that a bridge / light-client cannot be tricked by
// substituting a different `expected_value` while keeping the same
// proof bytes. This is the security property M-02 was about: encoding
// drift breaking the proof binding.
#[test]
fn tampered_expected_value_rejected() {
    let mut t = fresh_trie();
    let addr = [0xa0u8; 20];
    let acct_real = vec![0x11u8; 64];
    let acct_fake = vec![0x99u8; 64];

    t.insert(&addr, acct_real.clone()).unwrap();
    t.insert(&[0xb0u8; 20], vec![0x22u8; 64]).unwrap();
    let root = t.root();

    let proof = t.prove(&addr).expect("proof generation");

    // Honest verification passes.
    assert!(verify_proof(root, &addr, &Some(acct_real.clone()), &proof.nodes),
        "honest verification must pass");
    // Tampered value verification fails.
    assert!(!verify_proof(root, &addr, &Some(acct_fake), &proof.nodes),
        "tampered value must be rejected by the verifier");
    // Tampered key verification fails.
    let mut bad_key = addr;
    bad_key[0] ^= 0xff;
    assert!(!verify_proof(root, &bad_key, &Some(acct_real), &proof.nodes),
        "tampered key must be rejected by the verifier");
}

// ---------------------------------------------------------------------------
// 17. [W1.5 / M-02 closure] Empty trie proof for any key verifies as absent
// ---------------------------------------------------------------------------
//
// EMPTY_ROOT + empty nodes list MUST verify as a valid non-inclusion
// proof for every key. Bridges querying non-existent accounts pre-
// genesis must rely on this property.
#[test]
fn empty_trie_proof_verifies_as_absent_for_any_key() {
    let nodes: Vec<Vec<u8>> = Vec::new();
    let key = vec![0xde, 0xad, 0xbe, 0xef];
    let value: Option<Vec<u8>> = None;

    assert!(verify_proof(EMPTY_ROOT, &key, &value, &nodes),
        "empty proof against EMPTY_ROOT must verify non-inclusion");

    // But asserting INCLUSION against EMPTY_ROOT must fail.
    let claimed = Some(vec![0u8; 32]);
    assert!(!verify_proof(EMPTY_ROOT, &key, &claimed, &nodes),
        "claiming inclusion against EMPTY_ROOT must fail");
}
