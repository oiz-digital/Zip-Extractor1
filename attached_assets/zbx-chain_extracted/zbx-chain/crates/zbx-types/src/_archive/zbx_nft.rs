//! ZbxNFT Rust bindings — ABI types for interacting with ZbxNFT.sol.

use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};

/// ZbxNFT token metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZbxNftToken {
    pub token_id:   u256_stub,
    pub owner:      [u8; 20],
    pub uri:        String,
    pub is_soulbound: bool,
    pub royalty_bps: u16,
}

/// Placeholder for U256.
pub type u256_stub = u128;

/// ZbxNFT transfer event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NftTransferEvent {
    pub from:     Option<[u8; 20]>, // None = mint
    pub to:       [u8; 20],
    pub token_id: u128,
    pub block:    u64,
    pub tx_hash:  [u8; 32],
}

/// Lazy mint voucher (signed off-chain by collection owner).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LazyMintVoucher {
    pub to:        [u8; 20],
    pub uri:       String,
    pub price_wei: u128,
    #[serde(with = "BigArray")]
    pub signature: [u8; 65], // ECDSA sig over keccak256(to || uri || price || contract)
}

impl LazyMintVoucher {
    /// ABI-encode for on-chain submission.
    pub fn encode_calldata(&self) -> Vec<u8> {
        // In production: ABI encode to bytes for ZbxNFT.lazyMint()
        let mut data = Vec::new();
        data.extend_from_slice(&self.to);
        data.extend_from_slice(self.uri.as_bytes());
        data.extend_from_slice(&self.price_wei.to_be_bytes());
        data.extend_from_slice(&self.signature);
        data
    }
}