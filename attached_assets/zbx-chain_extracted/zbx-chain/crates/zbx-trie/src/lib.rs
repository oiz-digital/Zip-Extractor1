//! zbx-trie: Merkle Patricia Trie (MPT) for Zebvix Chain.
//!
//! Implements a fully-persistent, hash-linked trie compatible with the
//! Ethereum MPT specification (EIP-98 / Yellow Paper Appendix D).
//! Used by zbx-state for account state and storage trie management.
//!
//! # Structure
//!
//! - `nibbles`  — 4-bit nibble encoding over byte keys
//! - `node`     — trie node types (blank, leaf, extension, branch)
//! - `trie`     — main Trie struct (insert/get/delete/root)
//! - `proof`    — Merkle proof generation and verification
//! - `iterator` — depth-first trie traversal

pub mod error;
pub mod nibbles;
pub mod node;
pub mod trie;
pub mod proof;
pub mod iterator;

pub use error::TrieError;
pub use trie::{Trie, TrieDB, MutableTrie};
pub use proof::{MerkleProof, verify_proof};
pub use node::TrieNode;
pub use nibbles::Nibbles;

use zbx_types::H256;

/// The canonical empty root (keccak256 of RLP-encoded empty string).
///
/// `H256` is a `primitive_types` newtype wrapping `[u8; 32]`; we construct
/// it via the public tuple-struct field.
pub const EMPTY_ROOT: H256 = H256([
    0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6,
    0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
    0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0,
    0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
]);

/// Build an `H256` from an arbitrary-length byte slice. The slice must
/// be exactly 32 bytes — anything else panics, matching the behaviour
/// of `H256::from_slice`.
#[inline]
pub(crate) fn h256_from_slice(bytes: &[u8]) -> H256 {
    H256::from_slice(bytes)
}