//! Unit tests for zbx-trie (Merkle Patricia Trie).
//! Run: cargo test --package zbx-trie

#[cfg(test)]
mod mpt_tests {
    use zbx_trie::{Trie, TrieDB, MutableTrie, verify_proof};

    // Canonical Ethereum empty-trie root:
    // keccak256(RLP("")) = 56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421
    const EMPTY_ROOT_HEX: &str =
        "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421";

    #[test]
    fn insert_and_get() {
        // Insert a key-value pair into the trie, then retrieve it.
        // The retrieved value must equal the inserted value.
        let mut trie = TrieDB::new_empty();
        let key   = b"account:0xdeadbeef";
        let value = b"balance:1000000000000000000";
        trie.insert(key, value).expect("insert must succeed");
        let got = trie.get(key).expect("get must not fail");
        assert_eq!(
            got.as_deref(), Some(value.as_slice()),
            "retrieved value must equal inserted value"
        );
    }

    #[test]
    fn get_missing_key_returns_none() {
        let trie = TrieDB::new_empty();
        let got = trie.get(b"nonexistent").expect("get on empty trie must not error");
        assert!(got.is_none(), "missing key must return None, not Some");
    }

    #[test]
    fn empty_trie_root_matches_known_vector() {
        // An empty MPT must produce the Ethereum-canonical empty root.
        let trie = TrieDB::new_empty();
        let root = trie.root_hex();
        assert_eq!(
            root, EMPTY_ROOT_HEX,
            "empty trie root must match keccak256(RLP(\"\"))"
        );
    }

    #[test]
    fn single_entry_root_deterministic() {
        // Two tries built with identical inserts must produce identical roots.
        let mut trie1 = TrieDB::new_empty();
        let mut trie2 = TrieDB::new_empty();
        trie1.insert(b"k", b"v").unwrap();
        trie2.insert(b"k", b"v").unwrap();
        assert_eq!(
            trie1.root(), trie2.root(),
            "identical trie inserts must produce identical roots"
        );
    }

    #[test]
    fn proof_verify_after_insert() {
        // Insert a key, generate an inclusion proof, and verify it against
        // the trie root. This exercises the full Merkle-Patricia path:
        //   insert → root commitment → proof generation → proof verification.
        let mut trie = TrieDB::new_empty();
        let key   = b"alice";
        let value = b"1000";
        trie.insert(key, value).unwrap();
        let root  = trie.root();
        let proof = trie.generate_proof(key)
            .expect("proof generation must succeed for a key that was inserted");
        assert!(
            verify_proof(&root, key, &proof).is_ok(),
            "inclusion proof for an inserted key must verify against the committed root"
        );
    }

    #[test]
    fn deletion_updates_root() {
        // Insert two keys, capture root, delete one — root must change.
        let mut trie = TrieDB::new_empty();
        trie.insert(b"foo", b"1").unwrap();
        trie.insert(b"bar", b"2").unwrap();
        let root_before = trie.root();

        trie.remove(b"bar").unwrap();
        let root_after = trie.root();

        assert_ne!(
            root_before, root_after,
            "removing a key must change the trie root"
        );
    }

    #[test]
    fn different_values_produce_different_roots() {
        let mut trie_a = TrieDB::new_empty();
        let mut trie_b = TrieDB::new_empty();
        trie_a.insert(b"key", b"value-a").unwrap();
        trie_b.insert(b"key", b"value-b").unwrap();
        assert_ne!(
            trie_a.root(), trie_b.root(),
            "different values under the same key must produce different roots"
        );
    }
}
