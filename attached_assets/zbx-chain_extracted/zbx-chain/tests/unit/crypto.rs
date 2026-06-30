//! Unit tests for zbx-crypto primitives.
//! Run: cargo test --package zbx-crypto

#[cfg(test)]
mod keccak_tests {
    use zbx_crypto::keccak::keccak256;

    #[test]
    fn keccak256_empty() {
        // NIST/Ethereum known vector: keccak256("") =
        // c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        let hash = keccak256(b"");
        assert_eq!(
            hex::encode(hash.0),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
            "keccak256 of empty input must match the known Ethereum vector"
        );
    }

    #[test]
    fn keccak256_known_vector() {
        // keccak256("abc") = 4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45
        let hash = keccak256(b"abc");
        assert_eq!(
            hex::encode(hash.0),
            "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45",
            "keccak256 of 'abc' must match the known Ethereum vector"
        );
    }

    #[test]
    fn keccak256_non_empty_differs_from_empty() {
        let h_empty = keccak256(b"");
        let h_data  = keccak256(b"Zebvix");
        assert_ne!(h_empty, h_data, "distinct inputs must produce distinct hashes");
    }

    #[test]
    fn keccak256_deterministic() {
        let h1 = keccak256(b"hello world");
        let h2 = keccak256(b"hello world");
        assert_eq!(h1, h2, "keccak256 must be deterministic for identical inputs");
    }
}

#[cfg(test)]
mod secp256k1_tests {
    use zbx_crypto::keccak::keccak256;
    use zbx_crypto::secp256k1::{PrivKey, Signature, recover_signer, personal_sign, recover_personal_signer};

    #[test]
    fn address_from_pubkey_deterministic() {
        // Address derivation must be deterministic: same key → same address.
        let priv1 = PrivKey::random();
        let addr1 = priv1.to_address();
        assert_eq!(priv1.to_address(), addr1, "address derivation must be deterministic");
    }

    #[test]
    fn two_different_keys_produce_different_addresses() {
        let pk1 = PrivKey::random();
        let pk2 = PrivKey::random();
        // Collision probability ≈ 2^-160; treat as impossible in tests.
        assert_ne!(
            pk1.to_address(), pk2.to_address(),
            "two independently generated keys must produce distinct addresses"
        );
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        // Sign a message hash, recover signer, compare to expected address.
        let privkey = PrivKey::random();
        let expected_addr = privkey.to_address();
        let msg_hash = keccak256(b"Zebvix Chain test message");
        let sig = privkey.sign(&msg_hash);
        let recovered = recover_signer(&msg_hash, &sig)
            .expect("recover_signer must not fail on a valid signature");
        assert_eq!(
            recovered, expected_addr,
            "recovered address must match the signing key's address"
        );
    }

    #[test]
    fn sign_wrong_hash_does_not_match_address() {
        let privkey = PrivKey::random();
        let expected_addr = privkey.to_address();
        let msg_hash  = keccak256(b"correct message");
        let wrong_hash = keccak256(b"wrong message");
        let sig = privkey.sign(&msg_hash);
        // Recovery over the wrong hash produces a *different* address (or errors).
        match recover_signer(&wrong_hash, &sig) {
            Ok(recovered) => assert_ne!(
                recovered, expected_addr,
                "recovering with wrong hash must not yield the original signer"
            ),
            Err(_) => {}
        }
    }

    #[test]
    fn personal_sign_roundtrip() {
        // personal_sign adds EIP-191 prefix — recovery must use recover_personal_signer.
        let privkey = PrivKey::random();
        let expected_addr = privkey.to_address();
        let msg_hash = keccak256(b"Zebvix paymaster message");
        let sig = personal_sign(&msg_hash, &privkey);
        let recovered = recover_personal_signer(&msg_hash, &sig)
            .expect("recover_personal_signer must succeed on a valid personal-sign signature");
        assert_eq!(
            recovered, expected_addr,
            "personal_sign round-trip: recovered address must match signer"
        );
    }

    #[test]
    fn raw_recover_does_not_match_personal_sign() {
        // Using recover_signer (no EIP-191 prefix) on a personal-signed message
        // must NOT yield the original signer.
        let privkey = PrivKey::random();
        let expected_addr = privkey.to_address();
        let msg_hash = keccak256(b"some message");
        let sig = personal_sign(&msg_hash, &privkey);
        // raw recovery strips the prefix → wrong signing hash → wrong address
        match recover_signer(&msg_hash, &sig) {
            Ok(addr) => assert_ne!(
                addr, expected_addr,
                "raw recover on a personal-signed message must NOT yield the original signer"
            ),
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod merkle_tests {
    use zbx_crypto::keccak::keccak256;
    use zbx_crypto::merkle::MerkleTree;

    #[test]
    fn single_leaf_root_equals_leaf_hash() {
        // Merkle root of one leaf must equal the leaf hash itself.
        // (Binary Merkle: odd nodes are not duplicated; single leaf IS the root.)
        let leaf_data: &[u8] = b"only leaf";
        let leaf_hash = keccak256(leaf_data);
        let tree = MerkleTree::build(&[leaf_data]);
        assert_eq!(
            tree.root(), leaf_hash,
            "single-leaf tree root must equal the leaf hash"
        );
    }

    #[test]
    fn two_leaf_tree_root_is_not_either_leaf() {
        let a: &[u8] = b"leaf-a";
        let b: &[u8] = b"leaf-b";
        let ha = keccak256(a);
        let hb = keccak256(b);
        let tree = MerkleTree::build(&[a, b]);
        let root = tree.root();
        assert_ne!(root, ha, "2-leaf root must not equal first leaf");
        assert_ne!(root, hb, "2-leaf root must not equal second leaf");
    }

    #[test]
    fn proof_verification() {
        // Construct a 4-leaf tree, generate inclusion proofs and verify each.
        let leaves: &[&[u8]] = &[b"tx-0", b"tx-1", b"tx-2", b"tx-3"];
        let tree = MerkleTree::build(leaves);
        let root = tree.root();

        for (i, leaf_data) in leaves.iter().enumerate() {
            let leaf_hash = keccak256(leaf_data);
            let proof = tree.proof(i).unwrap_or_else(|| {
                panic!("proof for leaf {i} must exist in a {}-leaf tree", leaves.len())
            });
            assert!(
                proof.verify(&root, &leaf_hash),
                "inclusion proof for leaf {i} must verify against the tree root"
            );
        }
    }

    #[test]
    fn proof_does_not_verify_wrong_root() {
        let leaves: &[&[u8]] = &[b"a", b"b", b"c", b"d"];
        let tree = MerkleTree::build(leaves);
        let leaf_hash = keccak256(b"a");
        let proof = tree.proof(0).expect("proof for leaf 0 must exist");
        let wrong_root = keccak256(b"not-the-root");
        assert!(
            !proof.verify(&wrong_root, &leaf_hash),
            "inclusion proof must NOT verify against an incorrect root"
        );
    }

    #[test]
    fn different_leaf_sets_produce_different_roots() {
        let tree_a = MerkleTree::build(&[b"x", b"y"]);
        let tree_b = MerkleTree::build(&[b"x", b"z"]);
        assert_ne!(
            tree_a.root(), tree_b.root(),
            "different leaf sets must produce different Merkle roots"
        );
    }
}
