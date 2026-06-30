//! Oracle price proof and Merkle commitment.
//!
//! Each oracle round produces a **price commitment** — a Merkle root over all
//! active feed prices at that round. This commitment is:
//!
//! 1. Published in the ZBX block header as `oracle_price_root`
//! 2. Used by light clients to verify a single feed price without downloading
//!    the full oracle state (Merkle inclusion proof)
//! 3. Relayed to external chains as part of ZBX-XCM messages (ZEP-026)
//!
//! ## Merkle construction
//!
//! Leaves are ordered alphabetically by `feed_id` for determinism:
//!
//! ```text
//! leaf = keccak256( feed_id_bytes || price_i128_be || round_id_u64_be || timestamp_u64_be )
//!
//! Tree = standard binary Merkle tree
//!        (odd node count → duplicate last leaf, Ethereum-style)
//!
//! root = keccak256(left_child || right_child)  at each level
//! ```
//!
//! ## Proof format
//!
//! A Merkle inclusion proof for feed `F` is a list of sibling hashes
//! from leaf to root. Verification:
//!
//! ```text
//! current = leaf_hash(F)
//! for sibling in proof.siblings:
//!     if proof.path[i] == Left:
//!         current = keccak256(current || sibling)
//!     else:
//!         current = keccak256(sibling || current)
//! assert current == root
//! ```

use crate::feed::{FeedId, Price};
use zbx_crypto::keccak::keccak256;
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

/// A single price entry committed to the Merkle tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceEntry {
    pub feed_id:   FeedId,
    pub price:     Price,
    pub round_id:  u64,
    pub timestamp: u64,
}

impl PriceEntry {
    pub fn new(feed_id: FeedId, price: Price, round_id: u64, timestamp: u64) -> Self {
        Self { feed_id, price, round_id, timestamp }
    }

    /// Canonical leaf hash.
    /// `keccak256(feed_id_bytes || price_i128_be || round_id_u64_be || timestamp_u64_be)`
    pub fn leaf_hash(&self) -> [u8; 32] {
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.feed_id.0.as_bytes());
        data.extend_from_slice(&self.price.0.to_be_bytes());
        data.extend_from_slice(&self.round_id.to_be_bytes());
        data.extend_from_slice(&self.timestamp.to_be_bytes());
        let h = keccak256(&data);
        let mut out = [0u8; 32];
        out.copy_from_slice(h.as_bytes());
        out
    }
}

// ── Merkle direction ──────────────────────────────────────────────────────────

/// Which side a sibling is on during Merkle proof verification.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MerkleDir {
    Left,
    Right,
}

// ── Merkle inclusion proof ────────────────────────────────────────────────────

/// A Merkle inclusion proof for one price entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceProof {
    /// The entry being proved.
    pub entry:    PriceEntry,
    /// Sibling hashes from leaf to root.
    pub siblings: Vec<[u8; 32]>,
    /// Direction (Left/Right) for each level.
    pub path:     Vec<MerkleDir>,
    /// The root this proof is against.
    pub root:     [u8; 32],
}

impl PriceProof {
    /// Verify this proof against the stored root.
    pub fn verify(&self) -> bool {
        let mut current = self.entry.leaf_hash();
        for (sibling, &dir) in self.siblings.iter().zip(self.path.iter()) {
            current = match dir {
                MerkleDir::Left  => hash_pair(sibling, &current),
                MerkleDir::Right => hash_pair(&current, sibling),
            };
        }
        current == self.root
    }

    /// The feed this proof covers.
    pub fn feed_id(&self) -> &FeedId { &self.entry.feed_id }
    /// The proved price.
    pub fn price(&self) -> Price { self.entry.price }
    /// The proved round ID.
    pub fn round_id(&self) -> u64 { self.entry.round_id }
}

// ── Oracle Merkle commitment ──────────────────────────────────────────────────

/// A complete Merkle commitment over all oracle prices for one round.
///
/// Built once per oracle round after all reporters have submitted and
/// the aggregator has computed the final median prices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OraclePriceCommitment {
    /// The Merkle root of all price leaves.
    pub root:      [u8; 32],
    /// Oracle round ID.
    pub round_id:  u64,
    /// Block number this commitment is for.
    pub block:     u64,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Number of feeds committed.
    pub feed_count: u32,
    /// Ordered leaves (same order as tree construction).
    leaves:        Vec<PriceEntry>,
    /// Precomputed tree (level 0 = leaves, last level = root).
    tree:          Vec<Vec<[u8; 32]>>,
}

impl OraclePriceCommitment {
    /// Build a commitment from a set of price entries.
    ///
    /// Entries are sorted by `feed_id` alphabetically for determinism.
    pub fn build(
        mut entries: Vec<PriceEntry>,
        round_id:    u64,
        block:       u64,
        timestamp:   u64,
    ) -> Self {
        // Sort by feed_id for deterministic ordering
        entries.sort_by(|a, b| a.feed_id.0.cmp(&b.feed_id.0));

        let feed_count = entries.len() as u32;
        let leaf_hashes: Vec<[u8; 32]> = entries.iter().map(|e| e.leaf_hash()).collect();
        let tree = build_merkle_tree(&leaf_hashes);
        let root = *tree.last().and_then(|r| r.first()).unwrap_or(&[0u8; 32]);

        Self { root, round_id, block, timestamp, feed_count, leaves: entries, tree }
    }

    /// Generate a Merkle inclusion proof for the given feed.
    pub fn proof_for(&self, feed_id: &FeedId) -> Option<PriceProof> {
        // Find leaf index
        let idx = self.leaves.iter().position(|e| &e.feed_id == feed_id)?;
        let entry = self.leaves[idx].clone();

        let (siblings, path) = generate_proof(&self.tree, idx);

        Some(PriceProof {
            entry,
            siblings,
            path,
            root: self.root,
        })
    }

    /// Get the price for a specific feed from this commitment.
    pub fn price_for(&self, feed_id: &FeedId) -> Option<Price> {
        self.leaves.iter()
            .find(|e| &e.feed_id == feed_id)
            .map(|e| e.price)
    }

    /// All feed IDs committed to in this round.
    pub fn feeds(&self) -> Vec<&FeedId> {
        self.leaves.iter().map(|e| &e.feed_id).collect()
    }
}

// ── Merkle helpers ────────────────────────────────────────────────────────────

/// Hash two 32-byte values: `keccak256(left || right)`.
pub fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(left);
    data[32..].copy_from_slice(right);
    let h = keccak256(&data);
    let mut out = [0u8; 32];
    out.copy_from_slice(h.as_bytes());
    out
}

/// Build a full binary Merkle tree from leaf hashes.
///
/// Returns a vector of levels where `tree[0]` is the leaf level and
/// `tree[last]` is a single-element root level.
fn build_merkle_tree(leaves: &[[u8; 32]]) -> Vec<Vec<[u8; 32]>> {
    if leaves.is_empty() {
        return vec![vec![[0u8; 32]]];
    }

    let mut tree = vec![leaves.to_vec()];
    let mut current = leaves.to_vec();

    while current.len() > 1 {
        // Duplicate last node if odd count (Ethereum-style)
        if current.len() % 2 == 1 {
            let last = *current.last().unwrap();
            current.push(last);
        }

        let mut next_level = Vec::with_capacity(current.len() / 2);
        for chunk in current.chunks(2) {
            next_level.push(hash_pair(&chunk[0], &chunk[1]));
        }
        tree.push(next_level.clone());
        current = next_level;
    }

    tree
}

/// Generate a Merkle inclusion proof for a leaf at `idx`.
///
/// Returns `(siblings, path)` where `path[i]` is the direction of the
/// sibling at level `i` (Left = sibling is on the left, current node is right).
fn generate_proof(tree: &[Vec<[u8; 32]>], mut idx: usize) -> (Vec<[u8; 32]>, Vec<MerkleDir>) {
    let mut siblings = Vec::new();
    let mut path     = Vec::new();

    for level in &tree[..tree.len().saturating_sub(1)] {
        let sibling_idx = if idx % 2 == 0 {
            // Current is left child — sibling is right
            (idx + 1).min(level.len() - 1)
        } else {
            // Current is right child — sibling is left
            idx - 1
        };

        let dir = if idx % 2 == 0 { MerkleDir::Right } else { MerkleDir::Left };
        siblings.push(level[sibling_idx]);
        path.push(dir);
        idx /= 2;
    }

    (siblings, path)
}

// ── Commitment registry ───────────────────────────────────────────────────────

/// Rolling history of oracle price commitments (one per round).
pub struct CommitmentRegistry {
    /// Most recent N commitments, keyed by round_id.
    history: BTreeMap<u64, OraclePriceCommitment>,
    max_history: usize,
}

impl CommitmentRegistry {
    pub fn new(max_history: usize) -> Self {
        Self { history: BTreeMap::new(), max_history }
    }

    /// Store a new commitment.
    pub fn insert(&mut self, commitment: OraclePriceCommitment) {
        let round_id = commitment.round_id;
        self.history.insert(round_id, commitment);
        // Evict oldest if over limit
        while self.history.len() > self.max_history {
            let oldest = *self.history.keys().next().unwrap();
            self.history.remove(&oldest);
        }
    }

    /// Get the commitment for a given round.
    pub fn get(&self, round_id: u64) -> Option<&OraclePriceCommitment> {
        self.history.get(&round_id)
    }

    /// Latest commitment (highest round_id).
    pub fn latest(&self) -> Option<&OraclePriceCommitment> {
        self.history.values().last()
    }

    /// Generate a proof for a feed in the latest round.
    pub fn latest_proof_for(&self, feed_id: &FeedId) -> Option<PriceProof> {
        self.latest()?.proof_for(feed_id)
    }

    pub fn len(&self) -> usize { self.history.len() }
    pub fn is_empty(&self) -> bool { self.history.is_empty() }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(feed: &str, price: f64, round: u64) -> PriceEntry {
        PriceEntry::new(FeedId(feed.into()), Price::from_f64(price), round, 9_999)
    }

    fn sample_commitment() -> OraclePriceCommitment {
        let entries = vec![
            entry("BTC/USD", 68_000.0, 1),
            entry("ETH/USD", 3_500.0, 1),
            entry("ZBX/USD", 2.50, 1),
        ];
        OraclePriceCommitment::build(entries, 1, 100_000, 9_999)
    }

    #[test]
    fn commitment_has_non_zero_root() {
        let c = sample_commitment();
        assert_ne!(c.root, [0u8; 32], "root must be non-zero");
        assert_eq!(c.feed_count, 3);
    }

    #[test]
    fn feeds_sorted_alphabetically() {
        let c = sample_commitment();
        let feeds: Vec<_> = c.feeds().iter().map(|f| f.0.clone()).collect();
        assert_eq!(feeds, vec!["BTC/USD", "ETH/USD", "ZBX/USD"],
            "feeds must be sorted alphabetically");
    }

    #[test]
    fn proof_verifies() {
        let c = sample_commitment();
        for feed in [FeedId("BTC/USD".into()), FeedId("ETH/USD".into()), FeedId("ZBX/USD".into())] {
            let proof = c.proof_for(&feed).unwrap();
            assert!(proof.verify(), "proof for {} must verify against root", feed);
        }
    }

    #[test]
    fn proof_for_unknown_feed_is_none() {
        let c = sample_commitment();
        assert!(c.proof_for(&FeedId("UNKNOWN".into())).is_none());
    }

    #[test]
    fn tampered_price_fails_verification() {
        let c = sample_commitment();
        let mut proof = c.proof_for(&FeedId("ZBX/USD".into())).unwrap();
        // Tamper with the price in the entry
        proof.entry.price = Price::from_f64(999.0);
        assert!(!proof.verify(), "tampered price should fail verification");
    }

    #[test]
    fn single_feed_commitment() {
        let entries = vec![entry("ZBX/USD", 2.50, 42)];
        let c = OraclePriceCommitment::build(entries, 42, 1_000, 9_999);
        assert_ne!(c.root, [0u8; 32]);
        let proof = c.proof_for(&FeedId("ZBX/USD".into())).unwrap();
        assert!(proof.verify());
    }

    #[test]
    fn registry_retains_latest() {
        let mut reg = CommitmentRegistry::new(5);
        for round in 1..=3u64 {
            let entries = vec![entry("ZBX/USD", round as f64, round)];
            let c = OraclePriceCommitment::build(entries, round, round * 1000, 9_999);
            reg.insert(c);
        }
        assert_eq!(reg.len(), 3);
        assert_eq!(reg.latest().unwrap().round_id, 3);
    }

    #[test]
    fn registry_evicts_old_on_overflow() {
        let mut reg = CommitmentRegistry::new(3);
        for round in 1..=5u64 {
            let entries = vec![entry("ZBX/USD", round as f64, round)];
            let c = OraclePriceCommitment::build(entries, round, round * 1000, 9_999);
            reg.insert(c);
        }
        // Only 3 most recent should remain
        assert_eq!(reg.len(), 3);
        assert!(reg.get(1).is_none(), "round 1 should be evicted");
        assert!(reg.get(2).is_none(), "round 2 should be evicted");
        assert!(reg.get(5).is_some(), "round 5 should be present");
    }
}
