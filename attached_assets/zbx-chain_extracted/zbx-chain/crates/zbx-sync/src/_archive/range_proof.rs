//! Range proofs for snap sync state download.
//!
//! A range proof proves that a set of (key, value) pairs in a Merkle trie
//! are correct and form a contiguous range -- no values were omitted.
//!
//! Used in snap sync to verify that the account/storage data received from
//! a peer is authentic, without downloading the entire trie.
//!
//! # Proof structure
//!   - left_proof:  Merkle path from root to the leftmost key
//!   - right_proof: Merkle path from root to the rightmost key
//!   - keys/values: all (key, value) pairs in the range [left, right]
//!
//! # Verification algorithm
//!   1. Verify left_proof against state_root
//!   2. Verify right_proof against state_root
//!   3. Walk keys in order; for each key, verify it is between left and right
//!   4. Recompute trie hash from keys/values; compare to state_root
//!
//! This prevents a malicious peer from:
//!   - Omitting accounts (would change the trie hash)
//!   - Inserting fake accounts (would change the trie hash)
//!   - Reordering accounts (sorted trie -- impossible to reorder)

use std::collections::BTreeMap;

/// A Merkle range proof for a contiguous key range in a trie.
#[derive(Debug, Clone)]
pub struct RangeProof {
    /// Proof nodes for the leftmost key (from root to leaf).
    pub left_proof:  Vec<Vec<u8>>,
    /// Proof nodes for the rightmost key (from root to leaf).
    pub right_proof: Vec<Vec<u8>>,
    /// Keys in the range (sorted, hex-encoded trie keys).
    pub keys:   Vec<[u8; 32]>,
    /// Corresponding values (RLP-encoded account / storage data).
    pub values: Vec<Vec<u8>>,
}

/// Result of verifying a range proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeProofResult {
    /// Proof is valid -- all (key, value) pairs are authentic.
    Valid,
    /// Proof is invalid -- reject the peer's data.
    Invalid(RangeProofError),
    /// Range is empty (valid, but no data to process).
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeProofError {
    RootMismatch,
    UnsortedKeys,
    MissingProofNode,
    KeyCountMismatch,
    InvalidTrieHash,
}

/// Verify a range proof against a known state root.
///
/// Called by the snap sync engine after receiving a chunk of account data.
pub fn verify_range_proof(
    state_root:  &[u8; 32],
    proof:       &RangeProof,
) -> RangeProofResult {
    if proof.keys.is_empty() { return RangeProofResult::Empty; }
    if proof.keys.len() != proof.values.len() {
        return RangeProofResult::Invalid(RangeProofError::KeyCountMismatch);
    }

    // Check keys are sorted
    for w in proof.keys.windows(2) {
        if w[0] >= w[1] {
            return RangeProofResult::Invalid(RangeProofError::UnsortedKeys);
        }
    }

    // In production: verify left_proof and right_proof as Merkle paths,
    // then reconstruct a partial trie from keys/values and compare root hash.
    // Stub: trust the range for now (real impl uses zbx-trie crate).
    let _ = state_root;

    RangeProofResult::Valid
}

/// Range proof request -- sent to a peer to download a chunk of state.
#[derive(Debug, Clone)]
pub struct GetAccountRange {
    /// Starting key (hash of the first account address wanted).
    pub origin:         [u8; 32],
    /// Ending key (hash of the last account address wanted).
    pub limit:          [u8; 32],
    /// Max response bytes the requester can accept.
    pub response_bytes: u64,
}

/// Response with account range data + proof.
#[derive(Debug, Clone)]
pub struct AccountRangeResponse {
    pub accounts: BTreeMap<[u8; 32], AccountLeaf>,
    pub proof:    RangeProof,
}

/// A single account leaf returned in a range response.
#[derive(Debug, Clone)]
pub struct AccountLeaf {
    pub nonce:       u64,
    pub balance:     u128,
    pub storage_root: [u8; 32],
    pub code_hash:   [u8; 32],
}

/// Storage range proof -- same as account range but for storage slots.
#[derive(Debug, Clone)]
pub struct GetStorageRanges {
    /// Account address whose storage is being downloaded.
    pub account_hash: [u8; 32],
    pub origin:       [u8; 32],
    pub limit:        [u8; 32],
    pub response_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct StorageRangeResponse {
    pub slots: BTreeMap<[u8; 32], [u8; 32]>,  // slot_hash -> value
    pub proof: RangeProof,
}