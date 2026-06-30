//! Mutable and read-only Merkle Patricia Trie implementations.
//!
//! W1.5 (S33-state-root sprint) production-harden pass:
//! - **Implements Extension-split** in `insert_node` (was
//!   `Err(TrieError::Inconsistent) // simplified` at the previous
//!   `trie.rs:189-194`). Long-common-prefix inserts now succeed.
//! - **Adds `MutableTrie::delete()`** with Yellow-Paper-Appendix-D
//!   collapse semantics (branch ↘ leaf/extension/branch + extension ↘
//!   merged-leaf/merged-extension).
//! - **Adds `MutableTrie::prove()`** + `Trie::prove()` for EIP-1186
//!   proof generation — bridges and light clients can now produce proofs.
//! - **Fixes the `key.slice(depth).slice(0).slice(0)` placeholder** at
//!   the previous `trie.rs:113` via `Nibbles::sub(start, len)`.
//! - **Fixes the silently-dropped `old_val`** in the leaf-split case when
//!   the existing leaf's path is fully consumed by the common prefix
//!   (it now correctly lands in `branch_value`).
//! - **Switches `cache` to `hashbrown::HashMap`** (drops std HashMap;
//!   hashbrown is faster + HashDoS-resistant by default).
//! - Resolves `EMPTY_ROOT` explicitly without going through the cache /
//!   db (previous code relied on `unwrap_or(Empty)` swallowing a
//!   MissingNode error).

use hashbrown::HashMap;
use crate::{
    error::TrieError,
    nibbles::Nibbles,
    node::{NodeRef, TrieNode},
    proof::MerkleProof,
    EMPTY_ROOT,
    h256_from_slice,
};
use zbx_types::H256;
use sha3::{Digest, Keccak256};

// ---------------------------------------------------------------------------
// TrieDB trait + in-memory impl
// ---------------------------------------------------------------------------

/// A key-value database backing the trie.
pub trait TrieDB: Send + Sync {
    fn get(&self, hash: &H256) -> Result<Option<Vec<u8>>, TrieError>;
    fn insert(&mut self, hash: H256, value: Vec<u8>) -> Result<(), TrieError>;
    fn contains(&self, hash: &H256) -> Result<bool, TrieError>;
}

/// In-memory trie database (for testing and ephemeral state).
#[derive(Default, Clone)]
pub struct MemoryTrieDB {
    store: HashMap<H256, Vec<u8>>,
}

impl TrieDB for MemoryTrieDB {
    fn get(&self, hash: &H256) -> Result<Option<Vec<u8>>, TrieError> {
        Ok(self.store.get(hash).cloned())
    }

    fn insert(&mut self, hash: H256, value: Vec<u8>) -> Result<(), TrieError> {
        self.store.insert(hash, value);
        Ok(())
    }

    fn contains(&self, hash: &H256) -> Result<bool, TrieError> {
        Ok(self.store.contains_key(hash))
    }
}

// ---------------------------------------------------------------------------
// MutableTrie
// ---------------------------------------------------------------------------

/// Mutable Merkle Patricia Trie.
pub struct MutableTrie<DB: TrieDB> {
    root: H256,
    pub(crate) db: DB,
    /// Dirty nodes not yet committed to db.
    pub(crate) cache: HashMap<H256, Vec<u8>>,
}

impl<DB: TrieDB> MutableTrie<DB> {
    /// Create a new empty trie.
    pub fn new(db: DB) -> Self {
        Self {
            root: EMPTY_ROOT,
            db,
            cache: HashMap::new(),
        }
    }

    /// Open an existing trie at `root`.
    pub fn from_root(root: H256, db: DB) -> Self {
        Self { root, db, cache: HashMap::new() }
    }

    /// Current root hash.
    pub fn root(&self) -> H256 {
        self.root
    }

    /// Read-only access to the backing DB.
    pub fn db(&self) -> &DB {
        &self.db
    }

    // ---- internals ----

    /// Store `node`. Inline nodes (encoded < 32 B) are returned as
    /// `NodeRef::Inline`; larger nodes are hashed and inserted into
    /// `cache`. The root is hashed unconditionally by `commit_root`,
    /// so callers building the root should not rely on this returning
    /// a hash-linked ref for tiny tries.
    fn store_node(&mut self, node: &TrieNode) -> Result<NodeRef, TrieError> {
        let encoded = node.encode();
        if encoded.len() < 32 {
            return Ok(NodeRef::Inline(Box::new(node.clone())));
        }
        let hash = h256_from_slice(&Keccak256::digest(&encoded));
        self.cache.insert(hash, encoded);
        Ok(NodeRef::Hash(hash))
    }

    /// Resolve a `NodeRef` to a concrete `TrieNode`. Handles `EMPTY_ROOT`
    /// without consulting the cache/db.
    fn resolve(&self, r: &NodeRef) -> Result<TrieNode, TrieError> {
        match r {
            NodeRef::Empty => Ok(TrieNode::Empty),
            NodeRef::Inline(n) => Ok(*n.clone()),
            NodeRef::Hash(h) => {
                if *h == EMPTY_ROOT {
                    return Ok(TrieNode::Empty);
                }
                let bytes = self.cache.get(h)
                    .cloned()
                    .or_else(|| self.db.get(h).ok().flatten())
                    .ok_or_else(|| TrieError::MissingNode(format!("{:?}", h)))?;
                TrieNode::decode(&bytes)
            }
        }
    }

    /// Commit a candidate root node, returning its hash.
    /// Empty trees collapse to `EMPTY_ROOT`. Non-empty roots are always
    /// hashed (per Yellow Paper §D the root commitment is always
    /// `keccak256(rlp(root_node))`, regardless of size).
    fn commit_root(&mut self, node: &TrieNode) -> Result<H256, TrieError> {
        if matches!(node, TrieNode::Empty) {
            return Ok(EMPTY_ROOT);
        }
        let encoded = node.encode();
        let hash = h256_from_slice(&Keccak256::digest(&encoded));
        self.cache.insert(hash, encoded);
        Ok(hash)
    }

    // ---- insert ----

    /// Insert `key` → `value` into the trie.
    pub fn insert(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), TrieError> {
        let nibbles = Nibbles::from_bytes(key);
        let old_root = if self.root == EMPTY_ROOT {
            TrieNode::Empty
        } else {
            self.resolve(&NodeRef::Hash(self.root))?
        };
        let new_root_node = self.insert_node(old_root, &nibbles, 0, value)?;
        self.root = self.commit_root(&new_root_node)?;
        Ok(())
    }

    fn insert_node(
        &mut self,
        node: TrieNode,
        key: &Nibbles,
        depth: usize,
        value: Vec<u8>,
    ) -> Result<TrieNode, TrieError> {
        let key_remaining = key.slice(depth);
        match node {
            TrieNode::Empty => Ok(TrieNode::Leaf {
                partial: key_remaining,
                value,
            }),

            TrieNode::Leaf { partial, value: old_val } => {
                let cp = key_remaining.common_prefix_len(&partial);

                // Exact match → in-place value update.
                if cp == partial.len() && cp == key_remaining.len() {
                    return Ok(TrieNode::Leaf { partial, value });
                }

                let mut children: Box<[NodeRef; 16]> =
                    Box::new([(); 16].map(|_| NodeRef::Empty));
                let mut branch_value: Option<Vec<u8>> = None;

                // Place the existing leaf.
                if cp == partial.len() {
                    // Existing path fully consumed by common prefix → goes
                    // in branch.value. (Previous code dropped old_val here.)
                    branch_value = Some(old_val);
                } else {
                    let old_nibble = partial.at(cp) as usize;
                    let old_leaf = TrieNode::Leaf {
                        partial: partial.sub(cp + 1, partial.len() - cp - 1),
                        value: old_val,
                    };
                    children[old_nibble] = self.store_node(&old_leaf)?;
                }

                // Place the new key.
                if cp == key_remaining.len() {
                    branch_value = Some(value);
                } else {
                    let new_nibble = key_remaining.at(cp) as usize;
                    let new_leaf = TrieNode::Leaf {
                        partial: key_remaining.sub(cp + 1, key_remaining.len() - cp - 1),
                        value,
                    };
                    children[new_nibble] = self.store_node(&new_leaf)?;
                }

                let branch = TrieNode::Branch { children, value: branch_value };
                if cp > 0 {
                    Ok(TrieNode::Extension {
                        partial: key_remaining.sub(0, cp),
                        child: self.store_node(&branch)?,
                    })
                } else {
                    Ok(branch)
                }
            }

            TrieNode::Extension { partial, child } => {
                let cp = key_remaining.common_prefix_len(&partial);

                // Full match → descend into child.
                if cp == partial.len() {
                    let child_node = self.resolve(&child)?;
                    let new_child = self.insert_node(child_node, key, depth + cp, value)?;
                    return Ok(TrieNode::Extension {
                        partial,
                        child: self.store_node(&new_child)?,
                    });
                }

                // Split: build a branch hosting both the existing
                // child path AND the new key.
                let mut children: Box<[NodeRef; 16]> =
                    Box::new([(); 16].map(|_| NodeRef::Empty));
                let mut branch_value: Option<Vec<u8>> = None;

                // Existing child placement: take partial.at(cp) as branch
                // index. The remainder of partial (cp+1..) becomes a new
                // Extension wrapping the original child; if the remainder
                // is empty, the original child sits directly under the slot.
                let ext_nibble = partial.at(cp) as usize;
                let ext_remaining = if cp + 1 < partial.len() {
                    partial.sub(cp + 1, partial.len() - cp - 1)
                } else {
                    Nibbles::empty()
                };
                let ext_branch_child: NodeRef = if ext_remaining.is_empty() {
                    child
                } else {
                    let new_ext = TrieNode::Extension {
                        partial: ext_remaining,
                        child,
                    };
                    self.store_node(&new_ext)?
                };
                children[ext_nibble] = ext_branch_child;

                // New key placement.
                if cp == key_remaining.len() {
                    branch_value = Some(value);
                } else {
                    let new_nibble = key_remaining.at(cp) as usize;
                    let new_leaf = TrieNode::Leaf {
                        partial: key_remaining.sub(cp + 1, key_remaining.len() - cp - 1),
                        value,
                    };
                    children[new_nibble] = self.store_node(&new_leaf)?;
                }

                let branch = TrieNode::Branch { children, value: branch_value };
                if cp > 0 {
                    Ok(TrieNode::Extension {
                        partial: key_remaining.sub(0, cp),
                        child: self.store_node(&branch)?,
                    })
                } else {
                    Ok(branch)
                }
            }

            TrieNode::Branch { mut children, value: branch_val } => {
                if depth == key.len() {
                    return Ok(TrieNode::Branch { children, value: Some(value) });
                }
                let idx = key.at(depth) as usize;
                let child_node = self.resolve(&children[idx])?;
                let new_child = self.insert_node(child_node, key, depth + 1, value)?;
                children[idx] = self.store_node(&new_child)?;
                Ok(TrieNode::Branch { children, value: branch_val })
            }
        }
    }

    // ---- get ----

    /// Get the value for `key`, returning None if absent.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, TrieError> {
        if self.root == EMPTY_ROOT {
            return Ok(None);
        }
        let nibbles = Nibbles::from_bytes(key);
        self.get_node(&NodeRef::Hash(self.root), &nibbles, 0)
    }

    fn get_node(&self, r: &NodeRef, key: &Nibbles, depth: usize) -> Result<Option<Vec<u8>>, TrieError> {
        let node = self.resolve(r)?;
        match node {
            TrieNode::Empty => Ok(None),
            TrieNode::Leaf { partial, value } => {
                let key_remaining = key.slice(depth);
                if key_remaining.common_prefix_len(&partial) == partial.len()
                    && partial.len() == key_remaining.len()
                {
                    Ok(Some(value))
                } else {
                    Ok(None)
                }
            }
            TrieNode::Extension { partial, child } => {
                let key_remaining = key.slice(depth);
                let cp = key_remaining.common_prefix_len(&partial);
                if cp < partial.len() {
                    Ok(None)
                } else {
                    self.get_node(&child, key, depth + cp)
                }
            }
            TrieNode::Branch { children, value } => {
                if depth == key.len() {
                    return Ok(value);
                }
                let idx = key.at(depth) as usize;
                self.get_node(&children[idx], key, depth + 1)
            }
        }
    }

    // ---- delete ----

    /// Delete `key`. Returns `true` iff the key existed.
    /// Implements Yellow-Paper Appendix-D collapse semantics:
    /// - Branch{0 ch, None} → Empty
    /// - Branch{0 ch, Some(v)} → Leaf{empty, v}
    /// - Branch{1 ch, None} → merge with single child (Leaf/Ext/Branch)
    /// - Extension whose child becomes Empty → Empty
    /// - Extension whose child becomes Leaf/Ext → merged with concatenated partials
    pub fn delete(&mut self, key: &[u8]) -> Result<bool, TrieError> {
        if self.root == EMPTY_ROOT {
            return Ok(false);
        }
        let nibbles = Nibbles::from_bytes(key);
        let old_root = self.resolve(&NodeRef::Hash(self.root))?;
        let (new_root_node, removed) = self.delete_node(old_root, &nibbles, 0)?;
        if !removed {
            return Ok(false);
        }
        self.root = self.commit_root(&new_root_node)?;
        Ok(true)
    }

    fn delete_node(
        &mut self,
        node: TrieNode,
        key: &Nibbles,
        depth: usize,
    ) -> Result<(TrieNode, bool), TrieError> {
        let key_remaining = key.slice(depth);
        match node {
            TrieNode::Empty => Ok((TrieNode::Empty, false)),

            TrieNode::Leaf { partial, value } => {
                if key_remaining.len() == partial.len()
                    && key_remaining.common_prefix_len(&partial) == partial.len()
                {
                    Ok((TrieNode::Empty, true))
                } else {
                    Ok((TrieNode::Leaf { partial, value }, false))
                }
            }

            TrieNode::Extension { partial, child } => {
                let cp = key_remaining.common_prefix_len(&partial);
                if cp != partial.len() {
                    return Ok((TrieNode::Extension { partial, child }, false));
                }
                let child_node = self.resolve(&child)?;
                let (new_child, removed) =
                    self.delete_node(child_node, key, depth + partial.len())?;
                if !removed {
                    return Ok((TrieNode::Extension { partial, child }, false));
                }
                let collapsed = match new_child {
                    TrieNode::Empty => TrieNode::Empty,
                    TrieNode::Leaf { partial: cp2, value } => TrieNode::Leaf {
                        partial: partial.concat(&cp2),
                        value,
                    },
                    TrieNode::Extension { partial: cp2, child: gc } => TrieNode::Extension {
                        partial: partial.concat(&cp2),
                        child: gc,
                    },
                    branch @ TrieNode::Branch { .. } => TrieNode::Extension {
                        partial,
                        child: self.store_node(&branch)?,
                    },
                };
                Ok((collapsed, true))
            }

            TrieNode::Branch { mut children, value: branch_val } => {
                if depth == key.len() {
                    if branch_val.is_some() {
                        let collapsed = self.maybe_collapse_branch(children, None)?;
                        return Ok((collapsed, true));
                    }
                    return Ok((TrieNode::Branch { children, value: None }, false));
                }
                let idx = key.at(depth) as usize;
                let child_node = self.resolve(&children[idx])?;
                let (new_child, removed) = self.delete_node(child_node, key, depth + 1)?;
                if !removed {
                    return Ok((TrieNode::Branch { children, value: branch_val }, false));
                }
                children[idx] = match new_child {
                    TrieNode::Empty => NodeRef::Empty,
                    other => self.store_node(&other)?,
                };
                let collapsed = self.maybe_collapse_branch(children, branch_val)?;
                Ok((collapsed, true))
            }
        }
    }

    fn maybe_collapse_branch(
        &mut self,
        children: Box<[NodeRef; 16]>,
        value: Option<Vec<u8>>,
    ) -> Result<TrieNode, TrieError> {
        let non_empty: Vec<usize> = (0..16)
            .filter(|&i| !matches!(children[i], NodeRef::Empty))
            .collect();
        match (non_empty.len(), &value) {
            (0, None) => Ok(TrieNode::Empty),
            (0, Some(v)) => Ok(TrieNode::Leaf {
                partial: Nibbles::empty(),
                value: v.clone(),
            }),
            (1, None) => {
                let idx = non_empty[0];
                let child_node = self.resolve(&children[idx])?;
                let prefix = Nibbles::single(idx as u8);
                match child_node {
                    TrieNode::Leaf { partial, value: v } => Ok(TrieNode::Leaf {
                        partial: prefix.concat(&partial),
                        value: v,
                    }),
                    TrieNode::Extension { partial, child } => Ok(TrieNode::Extension {
                        partial: prefix.concat(&partial),
                        child,
                    }),
                    branch @ TrieNode::Branch { .. } => Ok(TrieNode::Extension {
                        partial: prefix,
                        child: self.store_node(&branch)?,
                    }),
                    TrieNode::Empty => Ok(TrieNode::Empty),
                }
            }
            _ => Ok(TrieNode::Branch { children, value }),
        }
    }

    // ---- prove ----

    /// Generate an EIP-1186 Merkle proof for `key`.
    /// For absent keys, the returned proof's `value` is `None` and its
    /// `nodes` list ends at the divergence node. `verify_proof` accepts
    /// this as a valid non-inclusion claim (see proof.rs W1.5 fix).
    ///
    /// Limitation (W1.6 follow-up): inline children on the proof path
    /// are not yet supported by the verifier. For production state
    /// tries this never occurs because RLP-encoded `AccountState` is
    /// always > 32 bytes (storage_root + code_hash alone are 64 bytes).
    pub fn prove(&self, key: &[u8]) -> Result<MerkleProof, TrieError> {
        let nibbles = Nibbles::from_bytes(key);
        let mut nodes: Vec<Vec<u8>> = Vec::new();
        let mut current = NodeRef::Hash(self.root);
        let mut depth = 0usize;
        let mut value: Option<Vec<u8>> = None;

        loop {
            let decoded = match &current {
                NodeRef::Empty => break,
                NodeRef::Hash(h) => {
                    if *h == EMPTY_ROOT {
                        break;
                    }
                    let bytes = self.cache.get(h)
                        .cloned()
                        .or_else(|| self.db.get(h).ok().flatten())
                        .ok_or_else(|| TrieError::MissingNode(format!("{:?}", h)))?;
                    let node = TrieNode::decode(&bytes)?;
                    nodes.push(bytes);
                    node
                }
                NodeRef::Inline(n) => *n.clone(),
            };

            match decoded {
                TrieNode::Empty => break,
                TrieNode::Leaf { partial, value: v } => {
                    let key_remaining = nibbles.slice(depth);
                    if key_remaining.len() == partial.len()
                        && key_remaining.common_prefix_len(&partial) == partial.len()
                    {
                        value = Some(v);
                    }
                    break;
                }
                TrieNode::Extension { partial, child } => {
                    let key_remaining = nibbles.slice(depth);
                    let cp = key_remaining.common_prefix_len(&partial);
                    if cp != partial.len() {
                        break;
                    }
                    depth += cp;
                    current = child;
                }
                TrieNode::Branch { children, value: v } => {
                    if depth == nibbles.len() {
                        value = v;
                        break;
                    }
                    let idx = nibbles.at(depth) as usize;
                    depth += 1;
                    current = children[idx].clone();
                }
            }
        }

        Ok(MerkleProof {
            key: key.to_vec(),
            value,
            nodes,
        })
    }

    // ---- commit ----

    /// Flush dirty cache entries to the backing DB.
    pub fn commit(&mut self) -> Result<H256, TrieError> {
        for (hash, bytes) in self.cache.drain() {
            self.db.insert(hash, bytes)?;
        }
        Ok(self.root)
    }
}

// ---------------------------------------------------------------------------
// Trie (read-only wrapper)
// ---------------------------------------------------------------------------

/// Read-only Merkle Patricia Trie wrapper.
pub struct Trie<DB: TrieDB> {
    inner: MutableTrie<DB>,
}

impl<DB: TrieDB> Trie<DB> {
    pub fn new(root: H256, db: DB) -> Self {
        Self { inner: MutableTrie::from_root(root, db) }
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, TrieError> {
        self.inner.get(key)
    }

    pub fn root(&self) -> H256 {
        self.inner.root()
    }

    /// Generate an EIP-1186 Merkle proof for `key`.
    pub fn prove(&self, key: &[u8]) -> Result<MerkleProof, TrieError> {
        self.inner.prove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trie() -> MutableTrie<MemoryTrieDB> {
        MutableTrie::new(MemoryTrieDB::default())
    }

    #[test]
    fn empty_trie_get_returns_none() {
        let trie = make_trie();
        assert_eq!(trie.get(&[0x01]).unwrap(), None);
    }

    #[test]
    fn insert_then_get() {
        let mut trie = make_trie();
        trie.insert(&[0xaa, 0xbb], b"hello".to_vec()).unwrap();
        let val = trie.get(&[0xaa, 0xbb]).unwrap();
        assert_eq!(val.as_deref(), Some(b"hello".as_ref()));
    }

    #[test]
    fn insert_multiple_keys() {
        let mut trie = make_trie();
        trie.insert(&[0x01], b"one".to_vec()).unwrap();
        trie.insert(&[0x02], b"two".to_vec()).unwrap();
        trie.insert(&[0x03], b"three".to_vec()).unwrap();
        assert_eq!(trie.get(&[0x01]).unwrap().as_deref(), Some(b"one".as_ref()));
        assert_eq!(trie.get(&[0x02]).unwrap().as_deref(), Some(b"two".as_ref()));
        assert_eq!(trie.get(&[0x03]).unwrap().as_deref(), Some(b"three".as_ref()));
    }

    #[test]
    fn missing_key_returns_none() {
        let mut trie = make_trie();
        trie.insert(&[0x01], b"val".to_vec()).unwrap();
        assert_eq!(trie.get(&[0x02]).unwrap(), None);
    }

    #[test]
    fn delete_existing_key() {
        let mut trie = make_trie();
        trie.insert(&[0xcc], b"data".to_vec()).unwrap();
        let removed = trie.delete(&[0xcc]).unwrap();
        assert!(removed);
        assert_eq!(trie.get(&[0xcc]).unwrap(), None);
    }

    #[test]
    fn delete_absent_key_returns_false() {
        let mut trie = make_trie();
        let removed = trie.delete(&[0xaa]).unwrap();
        assert!(!removed);
    }

    #[test]
    fn commit_changes_root() {
        let mut trie = make_trie();
        let root_before = trie.root();
        trie.insert(&[0xde, 0xad], b"beef".to_vec()).unwrap();
        let root_after = trie.commit().unwrap();
        assert_ne!(root_before, root_after);
    }

    #[test]
    fn same_data_same_root() {
        let mut a = make_trie();
        let mut b = make_trie();
        for (k, v) in [(&[0x01u8][..], &b"foo"[..]), (&[0x02], &b"bar"[..])] {
            a.insert(k, v.to_vec()).unwrap();
            b.insert(k, v.to_vec()).unwrap();
        }
        let ra = a.commit().unwrap();
        let rb = b.commit().unwrap();
        assert_eq!(ra, rb);
    }

    #[test]
    fn prove_existing_key_and_verify() {
        let mut trie = make_trie();
        trie.insert(&[0xab, 0xcd], b"proof_value".to_vec()).unwrap();
        trie.commit().unwrap();
        let root = trie.root();
        let proof = trie.prove(&[0xab, 0xcd]).unwrap();
        assert!(proof.verify(root));
    }

    #[test]
    fn prove_absent_key_non_inclusion() {
        let mut trie = make_trie();
        trie.insert(&[0x01], b"exists".to_vec()).unwrap();
        trie.commit().unwrap();
        let root = trie.root();
        let proof = trie.prove(&[0x99]).unwrap();
        assert!(proof.verify(root));
        assert!(proof.value.is_none());
    }
}
