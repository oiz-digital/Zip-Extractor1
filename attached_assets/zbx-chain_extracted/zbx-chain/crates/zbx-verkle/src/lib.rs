//! Verkle trie implementation for ZBX chain.
//!
//! # Why Verkle over Merkle-Patricia?
//!
//! | Property              | Merkle-Patricia (current) | Verkle (ZEP-007)     |
//! |:---|:---|:---|
//! | Proof size            | ~3 KB                     | ~100–200 bytes       |
//! | Stateless client      | Impractical               | Practical            |
//! | Node count for proof  | O(log₁₆ n) nodes         | O(1) polynomial      |
//! | State witness         | Megabytes                 | Kilobytes            |
//! | Crypto primitive      | Keccak-256                | Pedersen / IPA       |
//!
//! # ZBX Verkle Tree Design
//!
//! Uses a 256-ary tree (width=256) with Inner Product Argument (IPA) commitments.
//! Each internal node commits to 256 children using a Pedersen vector commitment.
//! Proofs are multi-point polynomial evaluations — compact and aggregatable.
//!
//! # ZEP-007 Migration Plan
//! Block 150,000: Dual-mode (Merkle + Verkle, read from Merkle, write to Verkle)
//! Block 200,000: Full Verkle-only (Merkle dropped)

pub mod field;
pub mod node;
pub mod tree;
pub mod proof;
pub mod error;

pub use tree::VerkleTree;
pub use proof::{VerkleProof, MultiProof};
pub use error::VerkleError;

/// Tree width — 256 children per internal node (byte-indexed).
pub const WIDTH: usize = 256;

/// Depth of the tree for 32-byte keys.
pub const MAX_DEPTH: usize = 32;