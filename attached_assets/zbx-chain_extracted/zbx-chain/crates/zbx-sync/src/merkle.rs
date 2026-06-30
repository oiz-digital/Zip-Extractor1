//! Binary Merkle tree over chunk-root hashes for snapshot manifest
//! binding.
//!
//! Internal nodes use `keccak256(left || right)`. Odd layers are
//! padded with `H256::zero()` (CVE-2012-2459 safe; matches modern
//! beacon-chain conventions). Leaves are not domain-prefixed — they
//! are themselves keccak digests of MPT chunk roots, and proof
//! position is authenticated by the bit-reconstruction check in
//! `verify_proof`.

use zbx_crypto::keccak::keccak256;
use zbx_types::H256;

/// Compute the binary Merkle root over `leaves`. Returns `H256::zero()`
/// for an empty input (callers must reject empty chunk lists separately).
pub fn merkle_root(leaves: &[H256]) -> H256 {
    if leaves.is_empty() {
        return H256::zero();
    }
    let mut layer: Vec<H256> = leaves.to_vec();
    while layer.len() > 1 {
        let mut next = Vec::with_capacity((layer.len() + 1) / 2);
        let mut i = 0;
        while i < layer.len() {
            let l = layer[i];
            let r = if i + 1 < layer.len() { layer[i + 1] } else { H256::zero() };
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(l.as_bytes());
            buf[32..].copy_from_slice(r.as_bytes());
            next.push(H256::from(keccak256(&buf)));
            i += 2;
        }
        layer = next;
    }
    layer[0]
}

/// One Merkle authentication-path step: the sibling hash + whether the
/// current node sits on the left (`is_left = true`) or the right
/// (`is_left = false`) at this level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerklePathStep {
    pub sibling: H256,
    /// true ⇒ current node is the left child (so pair is `(cur, sibling)`)
    pub is_left: bool,
}

/// A complete Merkle inclusion proof for one leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    /// Bottom-up path from the leaf to (but not including) the root.
    pub path: Vec<MerklePathStep>,
}

/// Build a Merkle proof for the leaf at `index`. Returns `None` if
/// `index >= leaves.len()`.
pub fn merkle_proof(leaves: &[H256], index: usize) -> Option<MerkleProof> {
    if index >= leaves.len() {
        return None;
    }
    if leaves.len() == 1 {
        // Single-leaf tree: root == leaf, empty proof.
        return Some(MerkleProof { path: vec![] });
    }
    let mut path = Vec::new();
    let mut layer: Vec<H256> = leaves.to_vec();
    let mut idx = index;
    while layer.len() > 1 {
        let is_left = idx % 2 == 0;
        let sibling_idx = if is_left { idx + 1 } else { idx - 1 };
        let sibling = if sibling_idx < layer.len() {
            layer[sibling_idx]
        } else {
            H256::zero() // odd-layer pad
        };
        path.push(MerklePathStep { sibling, is_left });
        // Build the next layer.
        let mut next = Vec::with_capacity((layer.len() + 1) / 2);
        let mut i = 0;
        while i < layer.len() {
            let l = layer[i];
            let r = if i + 1 < layer.len() { layer[i + 1] } else { H256::zero() };
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(l.as_bytes());
            buf[32..].copy_from_slice(r.as_bytes());
            next.push(H256::from(keccak256(&buf)));
            i += 2;
        }
        idx /= 2;
        layer = next;
    }
    Some(MerkleProof { path })
}

/// Verify that `leaf` is the leaf at logical position `index` in a
/// tree rooted at `root`, given the proof. Returns true on success.
///
/// The `index` is checked structurally via the `is_left` flag at each
/// level — a proof generated for index 5 cannot verify against the
/// leaf at index 3 because the left/right traversal differs.
pub fn verify_proof(leaf: H256, index: usize, proof: &MerkleProof, root: H256) -> bool {
    // Reconstruct the index from the path: each step's `is_left`
    // determines bit `i` of the index. Compare against the claimed
    // index — this prevents a tampered-position attack where an
    // attacker re-uses a valid proof for the wrong position.
    let mut reconstructed_idx: usize = 0;
    for (i, step) in proof.path.iter().enumerate() {
        if !step.is_left {
            reconstructed_idx |= 1 << i;
        }
    }
    if reconstructed_idx != index {
        return false;
    }
    let mut cur = leaf;
    for step in &proof.path {
        let mut buf = [0u8; 64];
        if step.is_left {
            buf[..32].copy_from_slice(cur.as_bytes());
            buf[32..].copy_from_slice(step.sibling.as_bytes());
        } else {
            buf[..32].copy_from_slice(step.sibling.as_bytes());
            buf[32..].copy_from_slice(cur.as_bytes());
        }
        cur = H256::from(keccak256(&buf));
    }
    cur == root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> H256 {
        let mut x = [0u8; 32];
        x[31] = b;
        H256(x)
    }

    #[test]
    fn empty_root_is_zero() {
        assert_eq!(merkle_root(&[]), H256::zero());
    }

    #[test]
    fn single_leaf_root_is_leaf_and_proof_empty() {
        let leaf = h(7);
        assert_eq!(merkle_root(&[leaf]), leaf);
        let p = merkle_proof(&[leaf], 0).unwrap();
        assert!(p.path.is_empty());
        assert!(verify_proof(leaf, 0, &p, leaf));
    }

    #[test]
    fn proof_roundtrip_pow2_leaves() {
        let leaves: Vec<H256> = (1..=8u8).map(h).collect();
        let root = merkle_root(&leaves);
        for i in 0..leaves.len() {
            let p = merkle_proof(&leaves, i).unwrap();
            assert!(verify_proof(leaves[i], i, &p, root), "proof {i} must verify");
        }
    }

    #[test]
    fn proof_roundtrip_odd_leaves_with_zero_hash_padding() {
        let leaves: Vec<H256> = (1..=5u8).map(h).collect();
        let root = merkle_root(&leaves);
        for i in 0..leaves.len() {
            let p = merkle_proof(&leaves, i).unwrap();
            assert!(verify_proof(leaves[i], i, &p, root), "odd-tree proof {i}");
        }
    }

    #[test]
    fn rejects_proof_for_wrong_index() {
        let leaves: Vec<H256> = (1..=8u8).map(h).collect();
        let root = merkle_root(&leaves);
        let p = merkle_proof(&leaves, 3).unwrap();
        // Same proof, wrong index → must reject.
        assert!(!verify_proof(leaves[3], 5, &p, root));
        // Same proof, wrong leaf at correct index → must reject.
        assert!(!verify_proof(h(0xFF), 3, &p, root));
        // Same proof, wrong root → must reject.
        assert!(!verify_proof(leaves[3], 3, &p, h(0xAA)));
    }

    #[test]
    fn out_of_range_index_returns_none() {
        let leaves: Vec<H256> = (1..=4u8).map(h).collect();
        assert!(merkle_proof(&leaves, 4).is_none());
        assert!(merkle_proof(&leaves, 99).is_none());
    }

    #[test]
    fn root_is_deterministic_and_order_sensitive() {
        let a = vec![h(1), h(2), h(3)];
        let b = vec![h(3), h(2), h(1)];
        assert_eq!(merkle_root(&a), merkle_root(&a));
        assert_ne!(merkle_root(&a), merkle_root(&b));
    }
}
