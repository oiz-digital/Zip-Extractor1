//! Trie node types: blank, leaf, extension, branch.
//!
//! S38-TRIE-REGRESSION fix (Pass-8, 2026-05-09):
//! - **Decoder**: Branch and Extension child slots can be **inline** RLP
//!   nodes (sub-lists embedded directly when encoded < 32 bytes — Yellow
//!   Paper §D). The previous decoder eagerly called `val_at::<Vec<u8>>`
//!   on every slot, which fails with `ExpectedString` when the slot is
//!   a list (inline node). Now we read the slot as an `Rlp` item, branch
//!   on `is_list()`, and recurse into `TrieNode::decode` for inline items.
//! - **Encoder canonicality**: Empty branch slots / empty branch value /
//!   empty extension child were encoded via `s.append(&[0x80u8])`, which
//!   produces `81 80` (a 1-byte string containing 0x80) instead of the
//!   canonical RLP empty string `80`. Two clients producing different
//!   bytes for the same logical state would compute different state
//!   roots → instant chain fork. Now we use `s.append(&[])` which emits
//!   the canonical single byte `0x80`.

use crate::{nibbles::Nibbles, error::TrieError, h256_from_slice};
use zbx_types::H256;

/// The four fundamental MPT node kinds.
#[derive(Debug, Clone)]
pub enum TrieNode {
    /// Empty / null node (used as placeholder in branch arrays).
    Empty,
    /// Leaf: compact-encoded partial path + RLP-encoded value.
    Leaf {
        partial: Nibbles,
        value: Vec<u8>,
    },
    /// Extension: compact-encoded partial path + child hash.
    Extension {
        partial: Nibbles,
        child: NodeRef,
    },
    /// Branch: 16 child pointers + optional value at this node.
    Branch {
        children: Box<[NodeRef; 16]>,
        value: Option<Vec<u8>>,
    },
}

/// A reference to a child node — either an inline node or a hash pointer.
#[derive(Debug, Clone)]
pub enum NodeRef {
    Hash(H256),
    Inline(Box<TrieNode>),
    Empty,
}

impl TrieNode {
    /// RLP-encode this node.
    pub fn encode(&self) -> Vec<u8> {
        use zbx_rlp::RlpStream;
        match self {
            TrieNode::Empty => vec![0x80], // RLP empty string
            TrieNode::Leaf { partial, value } => {
                let mut s = RlpStream::new_list(2);
                s.append(&partial.encode_compact(true));
                s.append(value.as_slice());
                s.out()
            }
            TrieNode::Extension { partial, child } => {
                let mut s = RlpStream::new_list(2);
                s.append(&partial.encode_compact(false));
                match child {
                    // H256 is a [u8; 32] alias — slice directly.
                    NodeRef::Hash(h) => s.append(&h[..]),
                    NodeRef::Inline(n) => s.append_raw(&n.encode()),
                    // SEC-2026-05-09 Pass-8 (S38): canonical empty string `80`,
                    // not `81 80`.
                    NodeRef::Empty => s.append(&[]),
                };
                s.out()
            }
            TrieNode::Branch { children, value } => {
                let mut s = RlpStream::new_list(17);
                for child in children.iter() {
                    match child {
                        NodeRef::Hash(h) => s.append(&h[..]),
                        NodeRef::Inline(n) => s.append_raw(&n.encode()),
                        // SEC-2026-05-09 Pass-8 (S38): canonical empty string.
                        NodeRef::Empty => s.append(&[]),
                    };
                }
                match value {
                    Some(v) => s.append(v.as_slice()),
                    // SEC-2026-05-09 Pass-8 (S38): canonical empty string.
                    None    => s.append(&[]),
                };
                s.out()
            }
        }
    }

    /// Decode an RLP-encoded node.
    pub fn decode(bytes: &[u8]) -> Result<Self, TrieError> {
        use zbx_rlp::{Rlp, Decodable};
        let rlp = Rlp::new(bytes);
        let count = rlp.item_count().map_err(|e| TrieError::RlpDecode(e.to_string()))?;
        match count {
            0 => Ok(TrieNode::Empty),
            2 => {
                let path_bytes: Vec<u8> = rlp.val_at(0)
                    .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                let (partial, is_leaf) = Nibbles::decode_compact(&path_bytes);
                if is_leaf {
                    let value: Vec<u8> = rlp.val_at(1)
                        .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                    Ok(TrieNode::Leaf { partial, value })
                } else {
                    // SEC-2026-05-09 Pass-8 (S38): handle inline (list) child.
                    let child_rlp = rlp.at(1)
                        .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                    let child = if child_rlp.is_list() {
                        NodeRef::Inline(Box::new(TrieNode::decode(child_rlp.as_raw())?))
                    } else {
                        let child_bytes: Vec<u8> = Decodable::decode_from(&child_rlp)
                            .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                        if child_bytes.is_empty() {
                            NodeRef::Empty
                        } else if child_bytes.len() == 32 {
                            NodeRef::Hash(h256_from_slice(&child_bytes))
                        } else {
                            return Err(TrieError::RlpDecode(format!(
                                "extension child has unexpected string length {}",
                                child_bytes.len()
                            )));
                        }
                    };
                    Ok(TrieNode::Extension { partial, child })
                }
            }
            17 => {
                let mut children = Box::new([(); 16].map(|_| NodeRef::Empty));
                for i in 0..16usize {
                    // SEC-2026-05-09 Pass-8 (S38): each branch slot may be
                    // either an RLP string (empty / 32-byte hash) OR an inline
                    // RLP list (child node embedded directly when its
                    // serialization is < 32 bytes — Yellow Paper §D). Read
                    // the slot as an Rlp item and dispatch on `is_list()`.
                    let child_rlp = rlp.at(i)
                        .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                    children[i] = if child_rlp.is_list() {
                        NodeRef::Inline(Box::new(TrieNode::decode(child_rlp.as_raw())?))
                    } else {
                        let raw: Vec<u8> = Decodable::decode_from(&child_rlp)
                            .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                        if raw.is_empty() {
                            NodeRef::Empty
                        } else if raw.len() == 32 {
                            NodeRef::Hash(h256_from_slice(&raw))
                        } else {
                            return Err(TrieError::RlpDecode(format!(
                                "branch slot {} has unexpected string length {}",
                                i, raw.len()
                            )));
                        }
                    };
                }
                let val_bytes: Vec<u8> = rlp.val_at(16)
                    .map_err(|e| TrieError::RlpDecode(e.to_string()))?;
                let value = if val_bytes.is_empty() { None } else { Some(val_bytes) };
                Ok(TrieNode::Branch { children, value })
            }
            _ => Err(TrieError::RlpDecode(format!("unexpected item count: {}", count))),
        }
    }
}
