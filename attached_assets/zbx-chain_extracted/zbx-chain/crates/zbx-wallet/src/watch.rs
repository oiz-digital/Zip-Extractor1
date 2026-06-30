//! Watch-only wallet for ZBX Chain.
//!
//! A watch-only wallet tracks an address without holding any private key.
//! It can build unsigned transactions for signing on a hardware wallet or
//! cold storage device.
//!
//! ## Use cases
//! - Hardware wallet companion (Ledger, Trezor): build txs in software, sign on device
//! - Cold storage observer: monitor balance without exposing keys
//! - Portfolio tracker: watch multiple ZBX addresses simultaneously
//! - Multi-sig coordinator: track M-of-N wallet state without being an owner
//!
//! ## Capabilities
//! - Display address (with EIP-55 checksum)
//! - Build unsigned transactions for external signing
//! - Verify incoming/outgoing transactions by address
//!
//! ## Cannot
//! - Sign any message or transaction (no private key stored)

use sha3::{Keccak256, Digest};
use serde::{Serialize, Deserialize};
use crate::signer::eip55_checksum;

/// A watch-only ZBX wallet — address tracking without private key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchWallet {
    /// EVM address bytes (20 bytes)
    pub address:          [u8; 20],
    /// EIP-55 checksum address (0x-prefixed mixed-case hex)
    pub checksum_address: String,
    /// Chain ID: 8989 mainnet / 8990 testnet+devnet
    pub chain_id:         u64,
    /// Optional human-readable label for UI display
    pub label:            Option<String>,
    /// Optional stored public key — enables key verification proofs
    /// Stored as Vec<u8> (not [u8; 65]) for serde compatibility (arrays >32 not auto-derived).
    pub public_key:       Option<Vec<u8>>,
}

/// An unsigned transaction ready for hardware wallet or multi-sig signing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsignedTx {
    /// Sender (EIP-55 checksum address)
    pub from:      String,
    /// Recipient (raw or checksum hex)
    pub to:        String,
    /// Transfer amount in wei (u128 covers full EVM range)
    pub value_wei: u128,
    /// Account nonce (fetched from chain before building)
    pub nonce:     u64,
    /// Gas limit
    pub gas_limit: u64,
    /// Gas price in wei
    pub gas_price: u64,
    /// Chain ID (8989 / 8990) for EIP-155 replay protection
    pub chain_id:  u64,
    /// ABI-encoded call data (empty for plain ZBX transfers)
    pub data:      Vec<u8>,
}

/// Watch wallet errors.
#[derive(Debug)]
pub enum WatchError {
    InvalidAddress,
    InvalidPublicKey,
}

impl WatchWallet {
    /// Create a watch-only wallet from a hex address string.
    ///
    /// Accepts addresses with or without 0x prefix, any case.
    pub fn from_address(address_hex: &str, chain_id: u64) -> Result<Self, WatchError> {
        let clean = address_hex.trim_start_matches("0x");
        if clean.len() != 40 {
            return Err(WatchError::InvalidAddress);
        }
        let bytes = hex::decode(clean).map_err(|_| WatchError::InvalidAddress)?;
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&bytes);
        Ok(Self {
            checksum_address: eip55_checksum(&addr),
            address: addr,
            chain_id,
            label: None,
            public_key: None,
        })
    }

    /// Create a watch-only wallet from an uncompressed secp256k1 public key (65 bytes).
    ///
    /// Derives the EVM address as keccak256(pubkey[1..])[12..].
    /// The public key must start with 0x04 (uncompressed point marker).
    pub fn from_public_key(pubkey: &[u8; 65], chain_id: u64) -> Result<Self, WatchError> {
        if pubkey[0] != 0x04 {
            return Err(WatchError::InvalidPublicKey);
        }
        let hash = Keccak256::digest(&pubkey[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..]);
        Ok(Self {
            checksum_address: eip55_checksum(&addr),
            address: addr,
            chain_id,
            label: None,
            public_key: Some(pubkey.to_vec()),
        })
    }

    /// Attach a human-readable label (builder-pattern).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Build an unsigned transaction for external signing.
    ///
    /// The returned `UnsignedTx` can be serialized and sent to a hardware device.
    pub fn build_tx(
        &self,
        to:        &str,
        value_wei: u128,
        nonce:     u64,
        gas_limit: u64,
        gas_price: u64,
        data:      Vec<u8>,
    ) -> UnsignedTx {
        UnsignedTx {
            from:      self.checksum_address.clone(),
            to:        to.to_string(),
            value_wei, nonce, gas_limit, gas_price,
            chain_id:  self.chain_id,
            data,
        }
    }

    /// Convenience: build a plain ZBX transfer (no call data, standard 21 000 gas).
    pub fn build_transfer(
        &self,
        to:        &str,
        value_wei: u128,
        nonce:     u64,
        gas_price: u64,
    ) -> UnsignedTx {
        self.build_tx(to, value_wei, nonce, 21_000, gas_price, vec![])
    }

    /// Return the EIP-55 checksum address string.
    pub fn address_str(&self) -> &str {
        &self.checksum_address
    }
}
