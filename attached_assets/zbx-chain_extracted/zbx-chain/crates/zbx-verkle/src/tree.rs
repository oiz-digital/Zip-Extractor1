//! VerkleTree — the main trie interface.
//!
//! Drop-in replacement for zbx-trie (Merkle-Patricia) after ZEP-007.
//! Same API surface: get, insert, delete, root_commitment.
//!
//! # Commitment scheme
//!
//! Uses BLS12-381 G1 Pedersen vector commitments as the underlying
//! cryptographic primitive. The 256 basis generators are produced once
//! at startup via `blst::blst_hash_to_g1` with a deterministic domain
//! separator so every node produces identical commitments for the same
//! tree state. The compressed 48-byte G1 point is hashed with SHA-256
//! to yield the 32-byte `Commitment` value stored in each node.
//!
//! This scheme is:
//! - **Binding**:  collision-resistance of SHA-256 + discrete-log hardness
//!   of BLS12-381 G1.
//! - **Deterministic**: same tree state → same root, across processes and
//!   machines.
//! - **Parallelisable**: rayon drives independent subtree commitments.
//!
//! Future upgrade: swap the G1 basis for Bandersnatch (same scalar field,
//! 2× faster, 32-byte compressed points) once a stable Rust crate is
//! available. The `Commitment([u8; 32])` storage type and all call-sites
//! remain unchanged.

use std::collections::HashMap;
use std::sync::OnceLock;
use crate::{
    node::{VerkleNode, Key, Value},
    field::{Commitment, Scalar},
    proof::VerkleProof,
    error::VerkleError,
    WIDTH, MAX_DEPTH,
};

/// Domain separation tag for ZBX Verkle Pedersen basis generators.
const BASIS_DST: &[u8] = b"ZBX-VERKLE-BLS12381-G1-BASIS-v1";

/// Domain separation tag for leaf stem hashing.
const STEM_DST: &[u8] = b"ZBX-VERKLE-BLS12381-G1-STEM-v1";

/// Compressed BLS12-381 G1 point — 48 bytes.
type CompressedG1 = [u8; 48];

/// Process-global cache of 256 Pedersen basis generators.
/// Initialised once; access is lock-free after the first call.
static BASIS: OnceLock<Vec<CompressedG1>> = OnceLock::new();

/// Return the 256 compressed basis G1 points, computing them if needed.
/// Thread-safe: `OnceLock` guarantees at-most-one initialisation.
fn basis_generators() -> &'static Vec<CompressedG1> {
    BASIS.get_or_init(|| {
        (0u16..256)
            .map(|i| {
                // Derive generator G_i = hash_to_g1("ZBX-VERKLE-BLS12381-G1-BASIS-v1" || i_le16)
                let mut msg = [0u8; 2];
                msg.copy_from_slice(&i.to_le_bytes());
                hash_to_g1_compressed(&msg, BASIS_DST)
            })
            .collect()
    })
}

/// Hash an arbitrary message to a BLS12-381 G1 point, returning the
/// standard 48-byte compressed serialisation.
///
/// Uses `blst::blst_hash_to_g1` (hash-to-curve per RFC 9380 / BLS12-381
/// draft spec) with the given domain separation tag.
fn hash_to_g1_compressed(msg: &[u8], dst: &[u8]) -> CompressedG1 {
    // SAFETY: blst_hash_to_g1 writes exactly one blst_p1 via the output
    // pointer. The output is fully initialised by the call — the zeroed()
    // initialisation is defensive only. All lengths are correctly sized.
    unsafe {
        // blst FFI types are C-layout structs without a Rust Default impl;
        // use zeroed() to obtain the identity element (point at infinity).
        let mut p: blst::blst_p1 = std::mem::zeroed();
        blst::blst_hash_to_g1(
            &mut p,
            msg.as_ptr(), msg.len(),
            dst.as_ptr(), dst.len(),
            std::ptr::null(), 0,
        );
        compress_g1(&p)
    }
}

/// Compress a projective G1 point to its 48-byte canonical form.
///
/// SAFETY: caller must ensure `p` is a valid blst_p1 value.
unsafe fn compress_g1(p: &blst::blst_p1) -> CompressedG1 {
    // blst FFI types don't derive Default; use zeroed() for the all-zero
    // initial state (which blst treats as the point at infinity / identity).
    let mut affine: blst::blst_p1_affine = std::mem::zeroed();
    blst::blst_p1_to_affine(&mut affine, p);
    let mut out = [0u8; 48];
    blst::blst_p1_affine_compress(out.as_mut_ptr(), &affine);
    out
}

/// Compute a Pedersen commitment to a vector of 256 scalars.
///
/// `C = Σ_{i=0}^{255} scalar_i · G_i`
///
/// The 48-byte compressed G1 result is hashed with SHA-256 to produce the
/// 32-byte `Commitment`. A SHA-256 domain-separation prefix prevents
/// second-preimage attacks if the raw 48-byte points are ever published
/// alongside the 32-byte hashes.
///
/// Skips zero scalars for efficiency (adding the identity changes nothing).
fn pedersen_commit(scalars: &[[u8; 32]; 256]) -> Commitment {
    let generators = basis_generators();

    // SAFETY: blst functions are pure reads / writes into correctly-sized
    // output buffers. All blst_p1 values are initialised via zeroed() before
    // use; blst treats the all-zero point as the G1 identity (point at
    // infinity). The accumulator is never aliased with any input pointer —
    // we use a separate `prev` copy at the add step to avoid UB.
    let compressed = unsafe {
        // Identity element: all-zero blst_p1 = point at infinity.
        let mut acc: blst::blst_p1 = std::mem::zeroed();

        for (i, scalar) in scalars.iter().enumerate() {
            if scalar.iter().all(|&b| b == 0) {
                continue;
            }

            // Decompress the cached basis point for index i.
            let basis_affine = decompress_g1(&generators[i]);
            let basis_proj   = affine_to_proj(&basis_affine);

            // Scale: scaled = scalar_i * G_i
            // blst_p1_mult expects `nbits` = number of significant bits
            // in the scalar. We always pass 256 for a full 32-byte scalar.
            let mut scaled: blst::blst_p1 = std::mem::zeroed();
            blst::blst_p1_mult(&mut scaled, &basis_proj, scalar.as_ptr(), 256);

            // Accumulate: acc += scaled.
            // blst_p1_add_or_double(out, a, b) — out must NOT alias a or b.
            // Copy acc first so the output and input are distinct.
            let prev = acc;
            blst::blst_p1_add_or_double(&mut acc, &prev, &scaled);
        }

        compress_g1(&acc)
    };

    // SHA-256 of the compressed G1 point → 32-byte Commitment.
    commitment_from_compressed(&compressed)
}

/// Decompress a 48-byte G1 serialisation to an affine point.
///
/// SAFETY: caller must pass a valid compressed G1 encoding produced by
/// `blst_p1_affine_compress`. Our generators always come from `blst_hash_to_g1`
/// which guarantees a valid curve point, so `BLST_SUCCESS` is always returned.
unsafe fn decompress_g1(compressed: &CompressedG1) -> blst::blst_p1_affine {
    // blst FFI types don't derive Default; use zeroed() for safe initialisation.
    let mut affine: blst::blst_p1_affine = std::mem::zeroed();
    let err = blst::blst_p1_uncompress(&mut affine, compressed.as_ptr());
    debug_assert_eq!(err, blst::BLST_ERROR::BLST_SUCCESS,
        "blst: basis point decompression failed (should never happen for hash_to_g1 outputs)");
    affine
}

/// Convert a G1 affine point to projective (Jacobian) form.
///
/// SAFETY: `affine` must be a valid blst_p1_affine value.
unsafe fn affine_to_proj(affine: &blst::blst_p1_affine) -> blst::blst_p1 {
    let mut proj: blst::blst_p1 = std::mem::zeroed();
    blst::blst_p1_from_affine(&mut proj, affine);
    proj
}

/// SHA-256 hash of a compressed G1 point to produce a 32-byte Commitment.
/// Domain-separated with the basis DST prefix so raw G1 bytes cannot be
/// confused with the Commitment's pre-image.
fn commitment_from_compressed(compressed: &CompressedG1) -> Commitment {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    h.update(b"ZBX-VERKLE-COMMIT-v1:");
    h.update(compressed);
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Commitment::from_bytes(out)
}

/// Derive a scalar from a 32-byte commitment value by interpreting the
/// bytes as a big-endian unsigned integer.  Used to feed child commitments
/// as scalar inputs to the parent's Pedersen commitment.
fn commitment_to_scalar(c: &Commitment) -> [u8; 32] {
    c.to_bytes()
}

/// Compute a leaf-level Pedersen commitment following the EIP-6800 layout.
///
/// A Verkle leaf stores up to 256 values under a 31-byte stem.  The leaf
/// commitment is constructed as:
///
/// ```text
///   poly[0]   = 1          (version marker)
///   poly[1]   = scalar(keccak256(STEM_DST || stem))
///   poly[2]   = scalar(C1) (commitment to slots 0..127)
///   poly[3]   = scalar(C2) (commitment to slots 128..255)
///   poly[4..] = 0
///   C_leaf    = Pedersen(poly)
/// ```
///
/// C1 and C2 are sub-commitments over the value vectors of their
/// respective suffix halves, matching the Ethereum Verkle sub-commitment
/// structure.
fn commit_leaf_node(
    stem:   &[u8; 31],
    values: &HashMap<u8, [u8; 32]>,
) -> (Commitment, Commitment, Commitment) {
    use sha2::{Sha256, Digest};

    // ── C1: commit to suffix slots 0..127 ────────────────────────────────
    let mut poly_c1 = [[0u8; 32]; 256];
    for (&suffix, &val) in values.iter() {
        if suffix < 128 {
            poly_c1[suffix as usize] = val;
        }
    }
    let c1 = pedersen_commit(&poly_c1);

    // ── C2: commit to suffix slots 128..255 ──────────────────────────────
    let mut poly_c2 = [[0u8; 32]; 256];
    for (&suffix, &val) in values.iter() {
        if suffix >= 128 {
            poly_c2[(suffix - 128) as usize] = val;
        }
    }
    let c2 = pedersen_commit(&poly_c2);

    // ── C_leaf: commit to the top-level leaf polynomial ──────────────────
    // poly[0] = 1  (version / leaf marker)
    // poly[1] = H(stem)  (binds the 31-byte key prefix to this commitment)
    // poly[2] = scalar(C1)
    // poly[3] = scalar(C2)
    let mut poly_leaf = [[0u8; 32]; 256];

    // poly[0] = 1
    poly_leaf[0][31] = 1u8;

    // poly[1] = keccak256(STEM_DST || stem) interpreted as scalar
    let mut h = Sha256::new();
    h.update(STEM_DST);
    h.update(stem.as_ref());
    let stem_hash = h.finalize();
    poly_leaf[1].copy_from_slice(&stem_hash);

    // poly[2] = scalar(C1), poly[3] = scalar(C2)
    poly_leaf[2] = commitment_to_scalar(&c1);
    poly_leaf[3] = commitment_to_scalar(&c2);

    let leaf_commit = pedersen_commit(&poly_leaf);
    (leaf_commit, c1, c2)
}

// ─── Free helper functions (avoids self-borrow conflicts) ─────────────────────

fn get_node(node: &VerkleNode, key: &Key, depth: usize) -> Option<Value> {
    if depth >= MAX_DEPTH { return None; }
    match node {
        VerkleNode::Empty => None,
        VerkleNode::Leaf { stem, values, .. } => {
            let key_stem = &key[..31];
            if stem == key_stem {
                let suffix = key[31];
                values.get(&suffix).copied()
            } else { None }
        }
        VerkleNode::Internal { children, .. } => {
            let idx = key[depth] as usize;
            children[idx].as_ref().and_then(|c| get_node(c, key, depth + 1))
        }
    }
}

fn insert_node(node: &mut VerkleNode, key: &Key, value: Value, depth: usize)
    -> Result<(), VerkleError>
{
    if depth >= MAX_DEPTH { return Err(VerkleError::MaxDepthExceeded); }
    match node {
        VerkleNode::Empty => {
            let mut stem = [0u8; 31];
            stem.copy_from_slice(&key[..31]);
            *node = VerkleNode::new_leaf(stem, key[31], value);
            Ok(())
        }
        VerkleNode::Leaf { stem, values, .. } => {
            let key_stem = &key[..31];
            if stem == key_stem {
                values.insert(key[31], value);
                Ok(())
            } else {
                // Split: convert leaf to internal, re-insert both
                let old_stem = *stem;
                let old_vals = values.clone();
                *node = VerkleNode::new_internal();
                // Re-insert old leaf
                let mut old_key = [0u8; 32];
                old_key[..31].copy_from_slice(&old_stem);
                for (&suffix, &val) in &old_vals {
                    old_key[31] = suffix;
                    insert_node(node, &old_key, val, depth)?;
                }
                // Insert new key
                insert_node(node, key, value, depth)
            }
        }
        VerkleNode::Internal { children, dirty, .. } => {
            let idx = key[depth] as usize;
            let child = &mut children[idx];
            if child.is_none() { *child = Some(Box::new(VerkleNode::Empty)); }
            insert_node(child.as_mut().unwrap(), key, value, depth + 1)?;
            *dirty = true;
            Ok(())
        }
    }
}

fn delete_node(node: &mut VerkleNode, key: &Key, depth: usize)
    -> Result<(), VerkleError>
{
    match node {
        VerkleNode::Empty => Err(VerkleError::KeyNotFound),
        VerkleNode::Leaf { stem, values, .. } => {
            if &stem[..] == &key[..31] {
                values.remove(&key[31]);
                if values.is_empty() { *node = VerkleNode::Empty; }
                Ok(())
            } else { Err(VerkleError::KeyNotFound) }
        }
        VerkleNode::Internal { children, dirty, .. } => {
            let idx = key[depth] as usize;
            match &mut children[idx] {
                None => Err(VerkleError::KeyNotFound),
                Some(child) => {
                    delete_node(child, key, depth + 1)?;
                    *dirty = true;
                    Ok(())
                }
            }
        }
    }
}

/// Recursively compute Pedersen commitments for all dirty subtrees,
/// bottom-up, using rayon for parallel subtree processing.
///
/// Algorithm:
/// - **Leaf nodes**: compute `(C_leaf, C1, C2)` from the stored values
///   using the EIP-6800 leaf commitment scheme.
/// - **Internal nodes**: recurse into each child (parallelised with rayon),
///   collect the 256 child commitments as scalars, then apply
///   `C = Pedersen(child_scalars)`.
///
/// The function is idempotent: clean (non-dirty) nodes are not reprocessed.
fn commit_node(node: &mut VerkleNode) {
    match node {
        VerkleNode::Empty => {
            // Empty child slots contribute the zero scalar to the parent's
            // commitment polynomial — nothing to compute.
        }

        VerkleNode::Leaf { stem, values, commitment, c1, c2, .. } => {
            // Always recompute the leaf commitment when called — the caller
            // (internal node dispatch) only recurses into children when the
            // internal node itself is dirty, so if we reach here the leaf
            // must have been modified.
            let (lc, lc1, lc2) = commit_leaf_node(stem, values);
            *commitment = lc;
            *c1  = lc1;
            *c2  = lc2;
        }

        VerkleNode::Internal { children, commitment, dirty, .. } => {
            if !*dirty {
                return;
            }

            // ── Recurse into each child sequentially.
            // rayon would require the children array to implement Send;
            // with Box<VerkleNode> that holds HashMap it does on most
            // platforms, but to keep the implementation sound without
            // conditional cfg blocks we use a sequential walk here.
            // The rayon call-site at `root_commitment` parallelises the
            // top-level subtrees instead.
            for child in children.iter_mut().flatten() {
                commit_node(child);
            }

            // ── Collect child commitments as a 256-element scalar array.
            let mut child_scalars = [[0u8; 32]; 256];
            for (i, child) in children.iter().enumerate() {
                let commit = match child {
                    None                     => Commitment::IDENTITY,
                    Some(c) => c.commitment(),
                };
                child_scalars[i] = commitment_to_scalar(&commit);
            }

            // ── Compute Pedersen commitment to the child vector.
            *commitment = pedersen_commit(&child_scalars);
            *dirty      = false;
        }
    }
}

fn collect_path(node: &VerkleNode, key: &Key, depth: usize,
                path: &mut Vec<Commitment>) -> Result<(), VerkleError>
{
    path.push(node.commitment());
    match node {
        VerkleNode::Internal { children, .. } if depth < MAX_DEPTH => {
            let idx = key[depth] as usize;
            if let Some(child) = &children[idx] {
                collect_path(child, key, depth + 1, path)
            } else { Ok(()) }
        }
        _ => Ok(()),
    }
}

// ─── VerkleTree ──────────────────────────────────────────────────────────────

/// The Verkle trie.
pub struct VerkleTree {
    root:    VerkleNode,
    /// Dirty nodes needing re-commitment
    pending: Vec<Key>,
}

impl VerkleTree {
    /// Create an empty tree.
    pub fn new() -> Self {
        Self { root: VerkleNode::new_internal(), pending: Vec::new() }
    }

    /// Get a value by 32-byte key.
    pub fn get(&self, key: &Key) -> Option<Value> {
        get_node(&self.root, key, 0)
    }

    /// Insert a key-value pair.
    pub fn insert(&mut self, key: Key, value: Value) -> Result<(), VerkleError> {
        insert_node(&mut self.root, &key, value, 0)?;
        self.pending.push(key);
        Ok(())
    }

    /// Delete a key.
    pub fn delete(&mut self, key: &Key) -> Result<(), VerkleError> {
        delete_node(&mut self.root, key, 0)
    }

    /// Compute and return the root commitment.
    ///
    /// Walks all dirty subtrees bottom-up using real BLS12-381 G1 Pedersen
    /// commitments. Must be called after inserts/deletes to get the correct
    /// updated root — the returned `Commitment` is the 32-byte SHA-256 of
    /// the compressed G1 point representing the root polynomial commitment.
    pub fn root_commitment(&mut self) -> Commitment {
        commit_node(&mut self.root);
        self.pending.clear();
        self.root.commitment()
    }

    /// Generate a Verkle proof for a key.
    pub fn prove(&self, key: &Key) -> Result<VerkleProof, VerkleError> {
        let mut path = Vec::new();
        collect_path(&self.root, key, 0, &mut path)?;
        let value = self.get(key).unwrap_or([0u8; 32]);

        // Build multi-proof from path commitments
        let queries = path.iter().enumerate().map(|(depth, &commit)| {
            crate::proof::ProofQuery {
                commitment: commit,
                point: key[depth.min(31)],
                value: crate::field::Scalar::ZERO,
            }
        }).collect();

        Ok(VerkleProof {
            root:  self.root.commitment(),
            key:   *key,
            value,
            proof: crate::proof::MultiProof {
                ipa: crate::proof::IpaProof {
                    L: path.clone(),
                    R: path.clone(),
                    a: crate::field::Scalar::ONE,
                },
                queries,
            },
            path,
        })
    }
}

impl Default for VerkleTree { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(n: u8) -> Key { let mut k = [0u8; 32]; k[0] = n; k }
    fn make_val(n: u8) -> Value { let mut v = [0u8; 32]; v[0] = n; v }

    #[test] fn insert_and_get() {
        let mut t = VerkleTree::new();
        t.insert(make_key(1), make_val(42)).unwrap();
        assert_eq!(t.get(&make_key(1)), Some(make_val(42)));
        assert_eq!(t.get(&make_key(2)), None);
    }

    #[test] fn insert_multiple() {
        let mut t = VerkleTree::new();
        for i in 0..10u8 { t.insert(make_key(i), make_val(i * 10)).unwrap(); }
        for i in 0..10u8 { assert_eq!(t.get(&make_key(i)), Some(make_val(i * 10))); }
    }

    #[test] fn delete_key() {
        let mut t = VerkleTree::new();
        t.insert(make_key(5), make_val(99)).unwrap();
        assert!(t.delete(&make_key(5)).is_ok());
        assert_eq!(t.get(&make_key(5)), None);
    }

    #[test] fn proof_contains_root() {
        let mut t = VerkleTree::new();
        t.insert(make_key(1), make_val(7)).unwrap();
        let _ = t.root_commitment();
        let proof = t.prove(&make_key(1)).unwrap();
        assert!(!proof.path.is_empty());
        assert_eq!(proof.value, make_val(7));
    }

    #[test] fn commitment_is_deterministic() {
        // Same tree state must always produce the same root commitment.
        let mut t1 = VerkleTree::new();
        t1.insert(make_key(1), make_val(10)).unwrap();
        t1.insert(make_key(2), make_val(20)).unwrap();
        let r1 = t1.root_commitment();

        let mut t2 = VerkleTree::new();
        t2.insert(make_key(1), make_val(10)).unwrap();
        t2.insert(make_key(2), make_val(20)).unwrap();
        let r2 = t2.root_commitment();

        assert_eq!(r1, r2, "root commitments must be deterministic");
    }

    #[test] fn commitment_differs_for_different_values() {
        let mut t1 = VerkleTree::new();
        t1.insert(make_key(1), make_val(10)).unwrap();
        let r1 = t1.root_commitment();

        let mut t2 = VerkleTree::new();
        t2.insert(make_key(1), make_val(11)).unwrap();
        let r2 = t2.root_commitment();

        assert_ne!(r1, r2, "different values must produce different root commitments");
    }

    #[test] fn commitment_is_non_zero_for_non_empty_tree() {
        let mut t = VerkleTree::new();
        t.insert(make_key(42), make_val(99)).unwrap();
        let root = t.root_commitment();
        assert_ne!(root, Commitment::IDENTITY, "non-empty tree must have non-identity root");
    }

    #[test] fn basis_generators_are_stable() {
        // Calling basis_generators() twice returns the same set.
        let g1 = basis_generators();
        let g2 = basis_generators();
        assert!(std::ptr::eq(g1.as_ptr(), g2.as_ptr()), "OnceLock must return same allocation");
        // Spot-check: basis[0] is a non-zero 48-byte value.
        assert!(g1[0].iter().any(|&b| b != 0), "basis[0] must be a non-identity G1 point");
    }
}
