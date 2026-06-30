//! In-memory pending block store with LRU eviction.

use zbx_types::{block::Block, H256};
use std::collections::{HashMap, VecDeque};

/// Stores proposed but not yet committed blocks.
pub struct BlockStore {
    blocks: HashMap<H256, Block>,
    insertion_order: VecDeque<H256>,
    capacity: usize,
}

impl BlockStore {
    pub fn new(capacity: usize) -> Self {
        BlockStore {
            blocks: HashMap::new(),
            insertion_order: VecDeque::new(),
            capacity,
        }
    }

    pub fn add(&mut self, block: Block) {
        let hash = block.hash();
        if self.blocks.contains_key(&hash) {
            return;
        }
        if self.blocks.len() >= self.capacity {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.blocks.remove(&oldest);
            }
        }
        self.insertion_order.push_back(hash);
        self.blocks.insert(hash, block);
    }

    pub fn get(&self, hash: &H256) -> Option<&Block> {
        self.blocks.get(hash)
    }

    pub fn remove(&mut self, hash: &H256) -> Option<Block> {
        self.blocks.remove(hash)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }
}