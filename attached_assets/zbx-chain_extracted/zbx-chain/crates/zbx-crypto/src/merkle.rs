//! Binary Merkle tree with SHA3-256 leaves and inclusion proofs.
//!
//! Used for transaction trie roots, receipt roots, and state proofs.

use crate::keccak::keccak256;
use zbx_types::H256;
use serde::{Deserialize, Serialize};

/// A Merkle inclusion proof: sibling hashes from leaf to root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleProof {
    /// Index of the leaf being proved.
    pub leaf_index: usize,
    /// Total number of leaves in the tree.
    pub leaf_count: usize,
    /// Sibling hashes from leaf level to root (root-exclusive).
    pub siblings: Vec<H256>,
}

impl MerkleProof {
    /// Verify this proof against the given root and leaf hash.
    pub fn verify(&self, root: &H256, leaf_hash: &H256) -> bool {
        let mut current = *leaf_hash;
        let mut index = self.leaf_index;
        for sibling in &self.siblings {
            current = if index % 2 == 0 {
                combine_hashes(&current, sibling)
            } else {
                combine_hashes(sibling, &current)
            };
            index /= 2;
        }
        &current == root
    }
}

/// Binary Merkle tree built from a sequence of leaf data.
pub struct MerkleTree {
    /// All layer hashes; layers[0] = leaves, layers.last() = [root].
    layers: Vec<Vec<H256>>,
}

impl MerkleTree {
    /// Build a Merkle tree from raw leaf data items.
    pub fn build(leaves: &[&[u8]]) -> Self {
        assert!(!leaves.is_empty(), "MerkleTree requires at least one leaf");
        let leaf_hashes: Vec<H256> = leaves.iter().map(|d| hash_leaf(d)).collect();
        Self::build_from_hashes(leaf_hashes)
    }

    /// Build from pre-hashed leaves.
    ///
    /// Audit-2026-05-01 S7-CR3: previously promoted an odd trailing node
    /// as-is (`pair[0]`) without hashing, while the proof/verify path
    /// folded `combine(C, C)` for the same position — so inclusion proofs
    /// for the last leaf in any odd-length layer **always failed** even for
    /// genuinely included leaves. Switched to **duplicate-up** (Bitcoin /
    /// Cosmos style): on odd trailing, `combine(C, C)` is hashed into the
    /// parent so build, proof, and verify all agree. NB: this changes the
    /// numeric root for any tree that previously hit an odd layer; any
    /// persisted historical roots from pre-fix builds will not match
    /// post-fix recomputation.
    pub fn build_from_hashes(mut current: Vec<H256>) -> Self {
        let mut layers = vec![current.clone()];
        while current.len() > 1 {
            let next: Vec<H256> = current
                .chunks(2)
                .map(|pair| {
                    if pair.len() == 2 {
                        combine_hashes(&pair[0], &pair[1])
                    } else {
                        // Odd trailing — duplicate then hash so the build
                        // path matches the verify path's `combine(C, C)` step.
                        combine_hashes(&pair[0], &pair[0])
                    }
                })
                .collect();
            layers.push(next);
            current = layers.last().unwrap().clone();
        }
        MerkleTree { layers }
    }

    /// The Merkle root hash.
    pub fn root(&self) -> H256 {
        self.layers.last()
            .and_then(|l| l.first())
            .copied()
            .unwrap_or_else(H256::zero)
    }

    /// Number of leaves.
    pub fn len(&self) -> usize {
        self.layers.first().map(|l| l.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Generate an inclusion proof for leaf at the given index.
    pub fn proof(&self, leaf_index: usize) -> Option<MerkleProof> {
        let leaf_count = self.len();
        if leaf_index >= leaf_count {
            return None;
        }
        let mut siblings = Vec::new();
        let mut idx = leaf_index;
        for layer in &self.layers[..self.layers.len() - 1] {
            let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
            if sibling_idx < layer.len() {
                siblings.push(layer[sibling_idx]);
            } else {
                siblings.push(layer[idx]); // duplicate odd node
            }
            idx /= 2;
        }
        Some(MerkleProof { leaf_index, leaf_count, siblings })
    }
}

/// Compute a Merkle root from a slice of transaction hashes.
pub fn transactions_root(tx_hashes: &[H256]) -> H256 {
    if tx_hashes.is_empty() {
        return H256::zero();
    }
    let tree = MerkleTree::build_from_hashes(tx_hashes.to_vec());
    tree.root()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Hash a leaf: keccak256(0x00 || data).  Domain-separates leaves from nodes.
fn hash_leaf(data: &[u8]) -> H256 {
    let mut buf = Vec::with_capacity(1 + data.len());
    buf.push(0x00);
    buf.extend_from_slice(data);
    keccak256(&buf)
}

/// Hash two children: keccak256(0x01 || left || right).
fn combine_hashes(left: &H256, right: &H256) -> H256 {
    let mut buf = [0u8; 65];
    buf[0] = 0x01;
    buf[1..33].copy_from_slice(left.as_bytes());
    buf[33..65].copy_from_slice(right.as_bytes());
    keccak256(&buf)
}