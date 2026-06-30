//! Merkle proof generation and verification.
//!
//! W1.5 (S33-state-root sprint): `verify_proof` now correctly accepts
//! NON-inclusion proofs. Previously, when the proof's terminal node was
//! a Leaf/Extension/Branch whose path diverged from the requested key,
//! the verifier returned `false` — conflating "proof structure is bad"
//! with "proof says the key is absent." After W1.5 the divergence point
//! is treated as a valid non-inclusion proof iff `expected_value == None`.
//!
//! Proof generation lives on `MutableTrie::prove(&self, key)` and
//! `Trie::prove(&self, key)` (see `trie.rs`).
//!
//! **Known limitation (W1.6 follow-up):** inline children on the proof
//! path return `false` from the verifier. For production state tries
//! this never occurs in practice because RLP-encoded `AccountState` is
//! always > 32 bytes (storage_root + code_hash alone are 64 bytes), so
//! every proof-path child is hash-linked.

use crate::{
    nibbles::Nibbles,
    node::{NodeRef, TrieNode},
    h256_from_slice,
};
use zbx_types::H256;
use sha3::{Digest, Keccak256};

/// A Merkle proof: the sequence of RLP-encoded trie nodes from root to leaf.
#[derive(Debug, Clone)]
pub struct MerkleProof {
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
    /// Ordered list of RLP-encoded trie nodes.
    pub nodes: Vec<Vec<u8>>,
}

impl MerkleProof {
    /// Verify this proof against `root`.
    pub fn verify(&self, root: H256) -> bool {
        verify_proof(root, &self.key, &self.value, &self.nodes)
    }
}

/// Verify a Merkle proof.
/// Returns `true` iff the proof is valid and consistent with the claimed value
/// (including non-inclusion when `expected_value == None`).
pub fn verify_proof(
    root: H256,
    key: &[u8],
    expected_value: &Option<Vec<u8>>,
    nodes: &[Vec<u8>],
) -> bool {
    if nodes.is_empty() {
        return root == crate::EMPTY_ROOT && expected_value.is_none();
    }

    let nibbles = Nibbles::from_bytes(key);
    let mut expected_hash = root;
    let mut depth = 0usize;

    for (i, encoded) in nodes.iter().enumerate() {
        // Verify hash linkage.
        let node_hash = h256_from_slice(&Keccak256::digest(encoded));
        if node_hash != expected_hash {
            return false;
        }

        let node = match TrieNode::decode(encoded) {
            Ok(n) => n,
            Err(_) => return false,
        };

        match node {
            TrieNode::Leaf { partial, value } => {
                let cp = nibbles.slice(depth).common_prefix_len(&partial);
                if cp == partial.len() && depth + cp == nibbles.len() {
                    // Inclusion claim
                    return *expected_value == Some(value);
                }
                // Path diverges at the leaf → valid non-inclusion proof
                // iff the caller is asserting absence.
                return expected_value.is_none();
            }
            TrieNode::Extension { partial, child } => {
                let cp = nibbles.slice(depth).common_prefix_len(&partial);
                if cp != partial.len() {
                    // Path diverges at the extension → non-inclusion.
                    return expected_value.is_none();
                }
                depth += cp;
                match child {
                    NodeRef::Hash(h) => expected_hash = h,
                    NodeRef::Empty => return expected_value.is_none(),
                    NodeRef::Inline(_) => {
                        // W1.6 follow-up: inline children on the proof
                        // path are not yet supported. Production state
                        // tries don't produce them (see file header).
                        return false;
                    }
                }
            }
            TrieNode::Branch { children, value } => {
                if depth == nibbles.len() {
                    if i != nodes.len() - 1 {
                        return false;
                    }
                    return *expected_value == value;
                }
                let idx = nibbles.at(depth) as usize;
                depth += 1;
                match &children[idx] {
                    NodeRef::Hash(h) => expected_hash = *h,
                    NodeRef::Empty => return expected_value.is_none(),
                    NodeRef::Inline(_) => {
                        // See above — W1.6 follow-up.
                        return false;
                    }
                }
            }
            TrieNode::Empty => return expected_value.is_none(),
        }
    }
    false
}
