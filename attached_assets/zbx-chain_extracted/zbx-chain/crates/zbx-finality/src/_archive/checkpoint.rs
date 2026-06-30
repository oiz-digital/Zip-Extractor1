//! Finality checkpoint — a block that has been finalized by 2f+1 validators.

use serde::{Deserialize, Serialize};
use zbx_primitives::{H256, Address};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub block_number:          u64,
    pub block_hash:            H256,
    pub epoch:                 u64,
    pub finalized:             bool,
    pub votes:                 u32,
    pub required:              u32,
    pub signers:               Vec<Address>,
}

impl Checkpoint {
    pub fn new(block: u64, hash: H256, epoch: u64, required: u32) -> Self {
        Self { block_number: block, block_hash: hash, epoch, finalized: false, votes: 0, required, signers: vec![] }
    }

    pub fn add_vote(&mut self, signer: Address) -> bool {
        if self.signers.contains(&signer) { return false; }
        self.signers.push(signer);
        self.votes += 1;
        if self.votes >= self.required {
            self.finalized = true;
            tracing::info!(block = self.block_number, "Block FINALIZED");
        }
        self.finalized
    }
}