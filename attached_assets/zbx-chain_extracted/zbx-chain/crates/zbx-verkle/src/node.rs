//! Verkle trie node types.
//!
//! Three node kinds:
//!   InternalNode — 256 children, holds a Pedersen commitment
//!   LeafNode     — key/value pair + extension stem
//!   Empty        — null child slot

use std::collections::HashMap;
use crate::field::{Commitment, Scalar};

/// Key type: 32-byte (256-bit) key derived from address + storage slot.
pub type Key   = [u8; 32];
/// Value type: 32-byte EVM word.
pub type Value = [u8; 32];

/// A single Verkle trie node.
#[derive(Clone, Debug)]
pub enum VerkleNode {
    Empty,
    Leaf {
        /// Key extension (first depth bytes of the key that reach here)
        stem:       [u8; 31],
        /// Values at each suffix (up to 256 slots per stem)
        values:     HashMap<u8, Value>,
        /// Commitment to the suffix values
        commitment: Commitment,
        /// Low-order polynomial coefficients (for proof)
        c1:         Commitment,
        c2:         Commitment,
    },
    Internal {
        /// One child per byte index (0..=255)
        children:   [Option<Box<VerkleNode>>; 256],
        /// Pedersen commitment to children commitments
        commitment: Commitment,
        /// Cache: was this node modified since last commit?
        dirty:      bool,
    },
}

impl VerkleNode {
    /// Create an empty internal node.
    pub fn new_internal() -> Self {
        const NONE: Option<Box<VerkleNode>> = None;
        VerkleNode::Internal {
            children:   [NONE; 256],
            commitment: Commitment::IDENTITY,
            dirty:      false,
        }
    }

    /// Create a leaf node for a single key.
    pub fn new_leaf(stem: [u8; 31], suffix: u8, value: Value) -> Self {
        let mut values = HashMap::new();
        values.insert(suffix, value);
        VerkleNode::Leaf {
            stem,
            values,
            commitment: Commitment::IDENTITY,
            c1: Commitment::IDENTITY,
            c2: Commitment::IDENTITY,
        }
    }

    /// Get the commitment of this node.
    pub fn commitment(&self) -> Commitment {
        match self {
            VerkleNode::Empty => Commitment::IDENTITY,
            VerkleNode::Leaf  { commitment, .. } => *commitment,
            VerkleNode::Internal { commitment, .. } => *commitment,
        }
    }

    /// Is this node empty (null child)?
    pub fn is_empty(&self) -> bool {
        matches!(self, VerkleNode::Empty)
    }
}