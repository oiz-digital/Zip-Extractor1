//! Merkle proof verification for trustless bridge state verification.

use crate::error::BridgeError;
use zbx_types::H256;
use zbx_crypto::keccak::keccak256;

/// Proof that a bridge deposit event exists in a block's receipt trie.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BridgeProof {
    /// Receipt hash being proved.
    pub receipt_hash: H256,
    /// Block that contains the receipt.
    pub block_hash: H256,
    /// Block number for fast lookup.
    pub block_number: u64,
    /// Merkle sibling hashes from receipt to receipts_root.
    pub siblings: Vec<H256>,
    /// Index of the receipt in the block.
    pub receipt_index: u32,
}

impl BridgeProof {
    /// Verify this proof against the block's receipts root.
    pub fn verify(&self, receipts_root: &H256) -> Result<(), BridgeError> {
        let mut current = self.receipt_hash;
        let mut index = self.receipt_index as usize;
        for sibling in &self.siblings {
            current = if index % 2 == 0 {
                combine(&current, sibling)
            } else {
                combine(sibling, &current)
            };
            index /= 2;
        }
        if &current == receipts_root {
            Ok(())
        } else {
            Err(BridgeError::ProofInvalid(
                format!("computed root {} != expected {}",
                    hex::encode(current), hex::encode(receipts_root))
            ))
        }
    }
}

fn combine(left: &H256, right: &H256) -> H256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(left.as_bytes());
    buf[32..].copy_from_slice(right.as_bytes());
    keccak256(&buf)
}