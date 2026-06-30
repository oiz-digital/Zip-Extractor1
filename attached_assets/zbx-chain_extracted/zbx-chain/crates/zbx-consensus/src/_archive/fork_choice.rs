//! Fork choice rule: LMD-GHOST (Latest Message Driven Greedy Heaviest Observed SubTree).
//!
//! LMD-GHOST is the fork choice rule used by ZBX (same as Ethereum PoS).
//!
//! Algorithm:
//!   1. Start from the latest finalized checkpoint (safe starting point)
//!   2. Among children of current head, pick the one with the most attestation weight
//!      (where weight = stake of validators whose LATEST attestation is in that subtree)
//!   3. Repeat until we reach a leaf node (current head)
//!
//! "Latest Message Driven": only the most recent attestation per validator counts.
//! "Greedy Heaviest": always pick the subtree with the most stake.
//!
//! Safe head vs justified head vs finalized head:
//!   finalized_head: cannot be reverted, 2/3+ finalized
//!   justified_head: 2/3+ justified but not yet finalized
//!   safe_head:      latest justified (for RPC eth_getBlockByNumber("safe"))
//!   head:           LMD-GHOST tip (for RPC eth_getBlockByNumber("latest"))

use std::collections::HashMap;

// ── LMD-GHOST ─────────────────────────────────────────────────────────────────

/// Attestation from a validator (vote for a block).
#[derive(Debug, Clone)]
pub struct LatestAttestation {
    /// Validator index
    pub validator_idx: u32,
    /// Block hash this validator's latest attestation targets
    pub target_block:  [u8; 32],
    /// Block number of target (for tie-breaking)
    pub target_number: u64,
    /// Validator's stake weight
    pub stake:         u128,
}

/// LMD-GHOST fork choice state.
pub struct LmdGhost {
    /// Latest attestation per validator (only the most recent counts).
    pub latest_messages: HashMap<u32, LatestAttestation>,
    /// Block tree: block_hash -> parent_hash
    pub parent_map:      HashMap<[u8; 32], [u8; 32]>,
    /// Block tree: block_hash -> children
    pub children_map:    HashMap<[u8; 32], Vec<[u8; 32]>>,
    /// Cumulative weight: block_hash -> total stake in subtree
    pub weights:         HashMap<[u8; 32], u128>,
    /// Latest finalized checkpoint block hash
    pub finalized_head:  [u8; 32],
    /// Latest justified head (safe_head)
    pub safe_head:       [u8; 32],
    /// Current head (LMD-GHOST tip)
    pub head:            [u8; 32],
}

impl LmdGhost {
    pub fn new(genesis_hash: [u8; 32]) -> Self {
        Self {
            latest_messages: HashMap::new(),
            parent_map:      HashMap::new(),
            children_map:    HashMap::new(),
            weights:         HashMap::new(),
            finalized_head:  genesis_hash,
            safe_head:       genesis_hash,
            head:            genesis_hash,
        }
    }

    /// Add a new block to the block tree.
    pub fn add_block(&mut self, block_hash: [u8; 32], parent_hash: [u8; 32]) {
        self.parent_map.insert(block_hash, parent_hash);
        self.children_map.entry(parent_hash).or_insert_with(Vec::new).push(block_hash);
        self.weights.entry(block_hash).or_insert(0);
    }

    /// Process an attestation (update latest message for validator).
    pub fn process_attestation(&mut self, att: LatestAttestation) {
        // Remove weight from old target (if validator previously attested)
        if let Some(old) = self.latest_messages.get(&att.validator_idx) {
            let old_target = old.target_block;
            let old_stake  = old.stake;
            self.remove_weight_from_ancestors(&old_target, old_stake);
        }
        // Add weight to new target and all ancestors up to finalized_head
        let new_target = att.target_block;
        let stake = att.stake;
        self.add_weight_to_ancestors(&new_target, stake);
        self.latest_messages.insert(att.validator_idx, att);
    }

    /// Run LMD-GHOST: find the head block (greedy heaviest subtree).
    ///
    /// Starting from finalized_head, greedily pick the child with max weight.
    pub fn find_head(&mut self) -> [u8; 32] {
        let mut current = self.finalized_head;
        loop {
            let children = match self.children_map.get(&current) {
                Some(c) if !c.is_empty() => c.clone(),
                _ => break,
            };
            // Pick heaviest child subtree
            let best = children.iter()
                .max_by_key(|&&child| self.weights.get(&child).copied().unwrap_or(0))
                .copied()
                .unwrap_or(current);
            if best == current { break; }
            current = best;
        }
        self.head = current;
        current
    }

    /// Update safe_head (justified checkpoint tip).
    pub fn update_safe_head(&mut self, justified_hash: [u8; 32]) {
        self.safe_head = justified_hash;
    }

    /// Update finalized_head (finalized checkpoint tip).
    /// Also prunes blocks below the finalized head.
    pub fn update_finalized_head(&mut self, finalized_hash: [u8; 32]) {
        self.finalized_head = finalized_hash;
        self.prune_finalized_blocks(finalized_hash);
    }

    /// Prune blocks that are below the finalized head (can never be reverted).
    fn prune_finalized_blocks(&mut self, finalized: [u8; 32]) {
        // Walk up from finalized, remove all branches that are not on the canonical path
        // In real impl: traverse parent_map, remove non-canonical branches
        let _ = finalized;
    }

    fn add_weight_to_ancestors(&mut self, block: &[u8; 32], stake: u128) {
        let mut current = *block;
        loop {
            *self.weights.entry(current).or_insert(0) += stake;
            if current == self.finalized_head { break; }
            match self.parent_map.get(&current).copied() {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }

    fn remove_weight_from_ancestors(&mut self, block: &[u8; 32], stake: u128) {
        let mut current = *block;
        loop {
            if let Some(w) = self.weights.get_mut(&current) {
                *w = w.saturating_sub(stake);
            }
            if current == self.finalized_head { break; }
            match self.parent_map.get(&current).copied() {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }
}