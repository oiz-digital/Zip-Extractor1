//! Justification vote signed by validator.

use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use zbx_primitives::{H256, Address};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Justification {
    pub block_number: u64,
    pub block_hash:   H256,
    pub epoch:        u64,
    pub validator:    Address,
    #[serde(with = "BigArray")]
    pub signature:    [u8; 65],
}

impl Justification {
    pub fn sign_payload(block: u64, hash: H256, epoch: u64) -> Vec<u8> {
        let mut m = b"ZBX_FINALITY_V1:".to_vec();
        m.extend_from_slice(&block.to_be_bytes());
        m.extend_from_slice(&hash.0);
        m.extend_from_slice(&epoch.to_be_bytes());
        m
    }
    pub fn is_valid(&self) -> bool { !self.block_hash.is_zero() && self.block_number > 0 }
}