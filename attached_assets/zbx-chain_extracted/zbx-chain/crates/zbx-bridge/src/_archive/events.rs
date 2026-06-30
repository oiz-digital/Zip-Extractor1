//! Bridge event types emitted on-chain by `BridgeVault.sol`.

use zbx_types::{address::Address, U256, H256};
use serde::{Deserialize, Serialize};
use zbx_crypto::keccak::keccak256;

/// Keccak256 event selectors (topic[0]).
pub mod selectors {
    use super::*;
    pub fn lock_initiated()   -> H256 { H256(keccak256(b"LockInitiated(address,address,uint256,uint64,bytes32)")) }
    pub fn release_completed()-> H256 { H256(keccak256(b"ReleaseCompleted(bytes32,address,uint256)")) }
    pub fn guardian_signed()  -> H256 { H256(keccak256(b"GuardianSigned(bytes32,address)")) }
    pub fn quorum_reached()   -> H256 { H256(keccak256(b"QuorumReached(bytes32)")) }
    pub fn committee_updated()-> H256 { H256(keccak256(b"CommitteeUpdated(address[],uint256)")) }
}

/// Emitted when a user locks tokens on the origin chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInitiated {
    pub user:         Address,
    pub token:        Address,
    pub amount:       U256,
    pub dest_chain:   u64,
    pub nonce:        H256,
    pub block_number: u64,
    pub tx_hash:      H256,
}

impl LockInitiated {
    /// The bridge message hash (signed by guardians).
    pub fn message_hash(&self) -> H256 {
        let mut data = Vec::with_capacity(116);
        data.extend_from_slice(self.user.as_bytes());
        data.extend_from_slice(self.token.as_bytes());
        let amount_bytes = self.amount.to_be_bytes();
        data.extend_from_slice(&amount_bytes);
        data.extend_from_slice(&self.dest_chain.to_be_bytes());
        data.extend_from_slice(self.nonce.as_bytes());
        H256(keccak256(&data))
    }
}

/// Emitted when tokens are released on the destination chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCompleted {
    pub nonce:        H256,
    pub recipient:    Address,
    pub amount:       U256,
    pub block_number: u64,
    pub tx_hash:      H256,
}

/// Emitted when a guardian signs a bridge message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardianSigned {
    pub message_hash: H256,
    pub guardian:     Address,
}

/// Emitted when quorum is reached for a bridge message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumReached {
    pub message_hash: H256,
}