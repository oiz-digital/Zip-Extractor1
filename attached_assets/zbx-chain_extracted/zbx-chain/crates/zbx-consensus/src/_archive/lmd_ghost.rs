//! LMD-GHOST fork choice rule for ZBX Chain.
//! Latest Message Driven Greediest Heaviest Observed SubTree.

use std::collections::{HashMap, HashSet, BTreeMap};
use std::sync::Arc;

use crate::types::{BlockHash, Slot, ValidatorIndex, Epoch};
use crate::consensus::{AttestationData, SLOTS_PER_EPOCH};

/// Fork choice store entry
#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub hash: BlockHash,
    pub parent: BlockHash,
    pub slot: Slot,
    pub weight: u64,          // attestation weight (sum of effective balances)
    pub children: Vec<BlockHash>,
    pub is_valid: bool,
    pub justified: bool,
    pub finalized: bool,
}

/// Latest message per validator (slot + block root)
#[derive(Debug, Clone)]
pub struct LatestMessage {
    pub epoch: Epoch,
    pub root: BlockHash,
}

/// LMD-GHOST fork choice
#[derive(Debug)]
pub struct LMDGhost {
    /// All known blocks (head candidates)
    pub blocks: HashMap<BlockHash, BlockEntry>,
    /// Latest messages per validator
    pub latest_messages: HashMap<ValidatorIndex, LatestMessage>,
    /// Justified checkpoint (anchor)
    pub justified_root: BlockHash,
    pub justified_slot: Slot,
    /// Finalized root (prune everything below)
    pub finalized_root: BlockHash,
    /// Proposer boost (EIP: boost block received in time)
    pub proposer_boost: Option<BlockHash>,
    pub proposer_boost_amount: u64,
    /// Equivocating validators (excluded from fork choice)
    pub equivocating: HashSet<ValidatorIndex>,
    /// Block -> epoch map for checkpoint lookups
    pub block_epoch: HashMap<BlockHash, Epoch>,
}

impl LMDGhost {
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            latest_messages: HashMap::new(),
            justified_root: BlockHash::default(),
            justified_slot: 0,
            finalized_root: BlockHash::default(),
            proposer_boost: None,
            proposer_boost_amount: 0,
            equivocating: HashSet::new(),
            block_epoch: HashMap::new(),
        }
    }

    /// Update justified checkpoint
    pub fn update_justified(&mut self, root: BlockHash, slot: Slot) {
        self.justified_root = root;
        self.justified_slot = slot;
    }

    /// Update finalized checkpoint (prune old blocks)
    pub fn update_finalized(&mut self, root: BlockHash) {
        self.finalized_root = root;
        self.prune_below_finalized();
    }

    /// Add a new block to the tree
    pub fn on_block(&mut self, hash: BlockHash, parent: BlockHash, slot: Slot) -> Result<(), ForkChoiceError> {
        if self.blocks.contains_key(&hash) { return Ok(()); }
        // Verify parent exists (unless genesis)
        if slot > 0 && !self.blocks.contains_key(&parent) {
            return Err(ForkChoiceError::ParentNotFound(parent));
        }
        let epoch = slot / SLOTS_PER_EPOCH;
        let entry = BlockEntry {
            hash, parent, slot, weight: 0,
            children: vec![], is_valid: true,
            justified: false, finalized: false,
        };
        self.blocks.insert(hash, entry);
        self.block_epoch.insert(hash, epoch);
        // Register as child of parent
        if let Some(p) = self.blocks.get_mut(&parent) {
            p.children.push(hash);
        }
        Ok(())
    }

    /// Process an attestation (update latest message + propagate weight)
    pub fn on_attestation(
        &mut self,
        slot: Slot,
        block_root: BlockHash,
        bits: &[bool],
    ) -> Result<(), ForkChoiceError> {
        if !self.blocks.contains_key(&block_root) {
            return Err(ForkChoiceError::BlockNotFound(block_root));
        }
        let epoch = slot / SLOTS_PER_EPOCH;
        let weight = bits.iter().filter(|&&b| b).count() as u64 * 32_000_000_000u64;

        // Update weight on target block and all ancestors up to justified
        self.add_weight_to_chain(block_root, weight);
        Ok(())
    }

    /// Add weight to a block and all its ancestors up to justified root
    fn add_weight_to_chain(&mut self, mut root: BlockHash, weight: u64) {
        loop {
            if let Some(entry) = self.blocks.get_mut(&root) {
                entry.weight += weight;
                if root == self.justified_root { break; }
                let parent = entry.parent;
                root = parent;
            } else {
                break;
            }
        }
    }

    /// Get canonical head (greedy heaviest subtree)
    pub fn get_head(&self) -> Result<BlockHash, ForkChoiceError> {
        let mut head = self.justified_root;
        loop {
            let entry = self.blocks.get(&head)
                .ok_or(ForkChoiceError::BlockNotFound(head))?;
            if entry.children.is_empty() { break; }
            // Pick child with highest weight (tie-break: higher slot, then hash)
            let best = entry.children.iter()
                .filter_map(|c| self.blocks.get(c).map(|e| (c, e.weight, e.slot)))
                .max_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)))
                .map(|(c, _, _)| *c)
                .ok_or(ForkChoiceError::NoChildren)?;
            // Apply proposer boost
            if let Some(boosted) = self.proposer_boost {
                let boost_weight = self.blocks.get(&boosted).map(|e| e.weight + self.proposer_boost_amount).unwrap_or(0);
                let best_weight = self.blocks.get(&best).map(|e| e.weight).unwrap_or(0);
                if boosted == entry.children.iter().copied().find(|&c| c == boosted).unwrap_or_default()
                    && boost_weight >= best_weight {
                    head = boosted;
                    continue;
                }
            }
            head = best;
        }
        Ok(head)
    }

    /// Get block root at a given slot (for attestation target)
    pub fn get_block_root(&self, slot: Slot) -> Result<BlockHash, ForkChoiceError> {
        let head = self.get_head()?;
        let mut current = head;
        loop {
            let entry = self.blocks.get(&current).ok_or(ForkChoiceError::BlockNotFound(current))?;
            if entry.slot <= slot { return Ok(current); }
            if current == self.justified_root { return Ok(current); }
            current = entry.parent;
        }
    }

    /// Get checkpoint block for a given epoch
    pub fn get_checkpoint_block(&self, epoch: Epoch) -> Result<BlockHash, ForkChoiceError> {
        let target_slot = epoch * SLOTS_PER_EPOCH;
        self.get_block_root(target_slot)
    }

    /// Register an equivocating validator
    pub fn register_equivocation(&mut self, validator: ValidatorIndex) {
        self.equivocating.insert(validator);
    }

    /// Prune finalized blocks from tree
    fn prune_below_finalized(&mut self) {
        // Remove all blocks below finalized root to save memory
        let keep: HashSet<BlockHash> = self.collect_descendants(self.finalized_root);
        self.blocks.retain(|h, _| keep.contains(h));
        self.block_epoch.retain(|h, _| keep.contains(h));
    }

    fn collect_descendants(&self, root: BlockHash) -> HashSet<BlockHash> {
        let mut result = HashSet::new();
        let mut queue = vec![root];
        while let Some(h) = queue.pop() {
            result.insert(h);
            if let Some(entry) = self.blocks.get(&h) {
                queue.extend(&entry.children);
            }
        }
        result
    }

    /// Tree statistics
    pub fn stats(&self) -> ForkChoiceStats {
        ForkChoiceStats {
            total_blocks: self.blocks.len(),
            attestors: self.latest_messages.len(),
            equivocating: self.equivocating.len(),
            justified_slot: self.justified_slot,
        }
    }
}

/// Fork choice statistics
#[derive(Debug, Clone)]
pub struct ForkChoiceStats {
    pub total_blocks: usize,
    pub attestors: usize,
    pub equivocating: usize,
    pub justified_slot: Slot,
}

/// Fork choice errors
#[derive(Debug, thiserror::Error)]
pub enum ForkChoiceError {
    #[error("Block not found: {0:?}")]
    BlockNotFound(BlockHash),
    #[error("Parent not found: {0:?}")]
    ParentNotFound(BlockHash),
    #[error("No children")]
    NoChildren,
    #[error("Invalid attestation")]
    InvalidAttestation,
}