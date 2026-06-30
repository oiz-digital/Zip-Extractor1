//! Trie node RLP encoding for hashing and storage.

use crate::node::TrieNode;
use crate::nibbles::Nibbles;
use zbx_types::H256;
use zbx_crypto::keccak::keccak256;

/// Encode a trie node to RLP and return its hash.
/// Nodes < 32 bytes are returned inline (not hashed) per the MPT spec.
pub fn encode_node(node: &TrieNode) -> NodeEncoding {
    let encoded = rlp_encode_node(node);
    if encoded.len() < 32 {
        NodeEncoding::Inline(encoded)
    } else {
        let hash = H256(keccak256(&encoded));
        NodeEncoding::Hashed(hash, encoded)
    }
}

#[derive(Debug, Clone)]
pub enum NodeEncoding {
    /// Small node embedded inline (not stored separately).
    Inline(Vec<u8>),
    /// Large node stored by hash reference.
    Hashed(H256, Vec<u8>),
}

impl NodeEncoding {
    pub fn hash_or_inline(&self) -> Vec<u8> {
        match self {
            NodeEncoding::Inline(data) => data.clone(),
            NodeEncoding::Hashed(hash, _) => hash.as_bytes().to_vec(),
        }
    }
}

/// RLP-encode a trie node (simplified).
fn rlp_encode_node(node: &TrieNode) -> Vec<u8> {
    match node {
        TrieNode::Empty => vec![0x80], // RLP empty string

        TrieNode::Leaf { path, value } => {
            // RLP: [compact_path, value]
            let compact = Nibbles::encode_compact_leaf(path);
            let mut out = Vec::new();
            rlp_encode_bytes(&compact, &mut out);
            rlp_encode_bytes(value, &mut out);
            rlp_wrap_list(out)
        }

        TrieNode::Extension { path, child } => {
            // RLP: [compact_path, child_hash_or_inline]
            let compact = Nibbles::encode_compact_extension(path);
            let child_enc = match child {
                crate::node::NodeRef::Hash(h) => h.as_bytes().to_vec(),
                crate::node::NodeRef::Inline(data) => data.clone(),
            };
            let mut out = Vec::new();
            rlp_encode_bytes(&compact, &mut out);
            rlp_encode_bytes(&child_enc, &mut out);
            rlp_wrap_list(out)
        }

        TrieNode::Branch { children, value } => {
            // RLP: [c0, c1, ..., c15, value]
            let mut out = Vec::new();
            for child in children {
                match child {
                    crate::node::NodeRef::Hash(h)    => rlp_encode_bytes(h.as_bytes(), &mut out),
                    crate::node::NodeRef::Inline(data)=> rlp_encode_bytes(data, &mut out),
                }
            }
            rlp_encode_bytes(value.as_deref().unwrap_or(&[]), &mut out);
            rlp_wrap_list(out)
        }
    }
}

// Minimal RLP helpers (subset needed for trie encoding).

fn rlp_encode_bytes(data: &[u8], out: &mut Vec<u8>) {
    match data.len() {
        0        => out.push(0x80),
        1 if data[0] < 0x80 => out.push(data[0]),
        len @ 1..=55 => {
            out.push(0x80 + len as u8);
            out.extend_from_slice(data);
        }
        len => {
            let len_bytes = encode_len(len);
            out.push(0xb7 + len_bytes.len() as u8);
            out.extend_from_slice(&len_bytes);
            out.extend_from_slice(data);
        }
    }
}

fn rlp_wrap_list(data: Vec<u8>) -> Vec<u8> {
    let len = data.len();
    let mut out = Vec::new();
    if len <= 55 {
        out.push(0xc0 + len as u8);
    } else {
        let lb = encode_len(len);
        out.push(0xf7 + lb.len() as u8);
        out.extend_from_slice(&lb);
    }
    out.extend_from_slice(&data);
    out
}

fn encode_len(mut n: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    while n > 0 { bytes.push(n as u8); n >>= 8; }
    bytes.reverse();
    bytes
}

impl Nibbles {
    pub fn encode_compact_leaf(nibbles: &[u8]) -> Vec<u8> {
        encode_compact(nibbles, true)
    }
    pub fn encode_compact_extension(nibbles: &[u8]) -> Vec<u8> {
        encode_compact(nibbles, false)
    }
}

/// Compact encoding per Ethereum Yellow Paper Appendix C.
fn encode_compact(nibbles: &[u8], is_leaf: bool) -> Vec<u8> {
    let flag = if is_leaf { 2 } else { 0 };
    let odd  = nibbles.len() % 2 == 1;
    let mut out = Vec::new();
    if odd {
        out.push((flag + 1) * 16 + nibbles[0]);
        for pair in nibbles[1..].chunks(2) {
            out.push(pair[0] * 16 + pair.get(1).copied().unwrap_or(0));
        }
    } else {
        out.push(flag * 16);
        for pair in nibbles.chunks(2) {
            out.push(pair[0] * 16 + pair.get(1).copied().unwrap_or(0));
        }
    }
    out
}