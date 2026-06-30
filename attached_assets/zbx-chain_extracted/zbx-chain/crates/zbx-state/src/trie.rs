//! Simplified Merkle-Patricia Trie for state root computation.
//!
//! A production implementation uses a full MPT with hex-prefix encoding.
//! This module provides the canonical API used by StateDB and storage.

use zbx_crypto::keccak::keccak256;
use zbx_types::H256;
use std::collections::BTreeMap;

/// A simple key-value store that produces a deterministic Merkle root.
pub struct MerkleTrie {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MerkleTrie {
    pub fn new() -> Self { MerkleTrie { entries: BTreeMap::new() } }

    pub fn put(&mut self, key: &[u8], value: &[u8]) {
        if value.is_empty() {
            self.entries.remove(key);
        } else {
            self.entries.insert(key.to_vec(), value.to_vec());
        }
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.entries.get(key).map(Vec::as_slice)
    }

    pub fn delete(&mut self, key: &[u8]) {
        self.entries.remove(key);
    }

    /// Compute the trie root: keccak256 of all sorted (key, value) pairs.
    pub fn root(&self) -> H256 {
        if self.entries.is_empty() {
            // Empty trie root (standard Ethereum value)
            return H256([
                0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6,
                0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
                0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0,
                0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
            ]);
        }
        // Merkle root via successive hashing of sorted pairs
        let mut leaves: Vec<H256> = self.entries
            .iter()
            .map(|(k, v)| {
                let mut buf = k.clone();
                buf.extend_from_slice(v);
                keccak256(&buf)
            })
            .collect();
        while leaves.len() > 1 {
            let next: Vec<H256> = leaves.chunks(2).map(|pair| {
                if pair.len() == 2 {
                    let mut buf = [0u8; 64];
                    buf[..32].copy_from_slice(&pair[0].0);
                    buf[32..].copy_from_slice(&pair[1].0);
                    keccak256(&buf)
                } else {
                    pair[0]
                }
            }).collect();
            leaves = next;
        }
        leaves[0]
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

impl Default for MerkleTrie { fn default() -> Self { Self::new() } }