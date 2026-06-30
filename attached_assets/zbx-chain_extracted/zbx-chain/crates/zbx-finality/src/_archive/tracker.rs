//! Tracks block finality state.

use std::collections::HashMap;
use zbx_primitives::{H256, Address};
use crate::checkpoint::Checkpoint;

pub struct FinalityTracker {
    pub checkpoints:    HashMap<u64, Checkpoint>,
    pub last_finalized: u64,
    pub finalized_hash: H256,
    pub epoch_length:   u64,
    pub validator_count: u32,
}

impl FinalityTracker {
    pub fn new(epoch_length: u64, validator_count: u32) -> Self {
        Self { checkpoints: HashMap::new(), last_finalized: 0, finalized_hash: H256::ZERO, epoch_length, validator_count }
    }

    pub fn required_votes(&self) -> u32 { 2 * ((self.validator_count.saturating_sub(1)) / 3) + 1 }

    pub fn on_block(&mut self, number: u64, hash: H256) {
        let epoch = number / self.epoch_length;
        self.checkpoints.insert(number, Checkpoint::new(number, hash, epoch, self.required_votes()));
    }

    pub fn on_vote(&mut self, block: u64, signer: Address) -> bool {
        if let Some(cp) = self.checkpoints.get_mut(&block) {
            if cp.add_vote(signer) && block > self.last_finalized {
                self.last_finalized = block;
                self.finalized_hash = cp.block_hash;
                self.checkpoints.retain(|&n, _| n >= block);
                return true;
            }
        }
        false
    }

    pub fn is_finalized(&self, block: u64) -> bool { block <= self.last_finalized }
    pub fn finality_lag(&self, head: u64)  -> u64  { head.saturating_sub(self.last_finalized) }
}