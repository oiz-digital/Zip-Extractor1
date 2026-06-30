//! Depth-first trie iterator (yields key-value pairs in sorted order).

use crate::{
    error::TrieError,
    node::{NodeRef, TrieNode},
    trie::TrieDB,
};
use zbx_types::H256;

/// Yields (key_bytes, value) in lexicographic key order.
pub struct TrieIterator<'db, DB: TrieDB> {
    db: &'db DB,
    stack: Vec<IterFrame>,
}

struct IterFrame {
    node_ref: NodeRef,
    prefix: Vec<u8>, // nibbles so far
    child_idx: u8,   // for branch nodes
}

impl<'db, DB: TrieDB> TrieIterator<'db, DB> {
    pub fn new(root: H256, db: &'db DB) -> Self {
        Self {
            db,
            stack: vec![IterFrame {
                node_ref: NodeRef::Hash(root),
                prefix: Vec::new(),
                child_idx: 0,
            }],
        }
    }

    fn resolve(&self, r: &NodeRef) -> Result<TrieNode, TrieError> {
        match r {
            NodeRef::Empty => Ok(TrieNode::Empty),
            NodeRef::Inline(n) => Ok(*n.clone()),
            NodeRef::Hash(h) => {
                let bytes = self.db.get(h)?
                    .ok_or_else(|| TrieError::MissingNode(format!("{:?}", h)))?;
                TrieNode::decode(&bytes)
            }
        }
    }

    fn nibbles_to_bytes(nibbles: &[u8]) -> Vec<u8> {
        nibbles
            .chunks(2)
            .map(|c| if c.len() == 2 { (c[0] << 4) | c[1] } else { c[0] << 4 })
            .collect()
    }
}

impl<'db, DB: TrieDB> Iterator for TrieIterator<'db, DB> {
    type Item = Result<(Vec<u8>, Vec<u8>), TrieError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Simplified DFS: push leaf values onto the stack and yield them.
        //
        // The two-phase loop body avoids the classic E0502 of holding a
        // `&mut` to `self.stack` (via `last_mut()`) while also calling
        // `&self`-receiving helpers like `self.resolve(..)`. We snapshot
        // the frame's `node_ref` and `prefix` under a short immutable
        // borrow, drop that borrow, resolve the node, and only then
        // reacquire the mutable borrow for in-place mutations.
        loop {
            // Phase 1: snapshot under an immutable borrow.
            let (node_ref_clone, prefix_clone, child_idx) = match self.stack.last() {
                Some(frame) => (
                    frame.node_ref.clone(),
                    frame.prefix.clone(),
                    frame.child_idx,
                ),
                None => return None,
            };

            // Phase 2: resolve outside any stack borrow.
            let node = match self.resolve(&node_ref_clone) {
                Ok(n) => n,
                Err(e) => {
                    self.stack.pop();
                    return Some(Err(e));
                }
            };

            // Phase 3: mutate the stack under a fresh `&mut` borrow.
            match node {
                TrieNode::Empty => {
                    self.stack.pop();
                }
                TrieNode::Leaf { partial, value } => {
                    let mut full_prefix = prefix_clone;
                    for i in 0..partial.len() {
                        full_prefix.push(partial.at(i));
                    }
                    let key = Self::nibbles_to_bytes(&full_prefix);
                    self.stack.pop();
                    return Some(Ok((key, value)));
                }
                TrieNode::Extension { partial, child } => {
                    let mut new_prefix = prefix_clone;
                    for i in 0..partial.len() {
                        new_prefix.push(partial.at(i));
                    }
                    self.stack.pop();
                    self.stack.push(IterFrame {
                        node_ref: child,
                        prefix: new_prefix,
                        child_idx: 0,
                    });
                }
                TrieNode::Branch { children, value } => {
                    let idx = child_idx as usize;
                    if idx > 16 {
                        self.stack.pop();
                        continue;
                    }
                    // Bump child_idx for the next iteration.
                    if let Some(frame) = self.stack.last_mut() {
                        frame.child_idx += 1;
                    }
                    if idx == 16 {
                        if let Some(v) = value {
                            let key = Self::nibbles_to_bytes(&prefix_clone);
                            return Some(Ok((key, v)));
                        }
                        continue;
                    }
                    let child = &children[idx];
                    if !matches!(child, NodeRef::Empty) {
                        let mut new_prefix = prefix_clone.clone();
                        new_prefix.push(idx as u8);
                        self.stack.push(IterFrame {
                            node_ref: child.clone(),
                            prefix: new_prefix,
                            child_idx: 0,
                        });
                    }
                }
            }
        }
    }
}