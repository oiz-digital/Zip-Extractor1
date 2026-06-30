//! Account state — Ethereum-compatible account structure.
//!
//! Each ZBX Chain account has:
//!   - balance:     ZBX (in wei, 18 decimals)
//!   - nonce:       transaction counter (prevents replay)
//!   - code_hash:   keccak256 of contract bytecode (0 for EOAs)
//!   - storage_root: Merkle root of contract storage trie

use serde::{Deserialize, Serialize};

/// ZBX Chain account (EVM-compatible).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub balance:      u128,
    pub nonce:        u64,
    pub code_hash:    [u8; 32],
    pub storage_root: [u8; 32],
}

impl Account {
    /// Keccak256 hash of empty code (EOA code hash).
    pub const EMPTY_CODE_HASH: [u8; 32] = [
        0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c,
        0x92, 0x7e, 0x7d, 0xb2, 0xdc, 0xc7, 0x03, 0xc0,
        0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b,
        0x7b, 0xfa, 0xd8, 0x04, 0x5d, 0x85, 0xa4, 0x70,
    ];

    /// Keccak256 hash of empty storage trie.
    pub const EMPTY_STORAGE_ROOT: [u8; 32] = [
        0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6,
        0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
        0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0,
        0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
    ];

    /// Create a new Externally Owned Account (no code, no storage).
    pub fn new_eoa(balance: u128, nonce: u64) -> Self {
        Self {
            balance,
            nonce,
            code_hash:    Self::EMPTY_CODE_HASH,
            storage_root: Self::EMPTY_STORAGE_ROOT,
        }
    }

    pub fn is_eoa(&self) -> bool {
        self.code_hash == Self::EMPTY_CODE_HASH
    }

    pub fn is_contract(&self) -> bool {
        !self.is_eoa()
    }

    pub fn is_empty(&self) -> bool {
        self.balance == 0 && self.nonce == 0 && self.is_eoa()
    }

    /// RLP-encode the account for the state trie (Ethereum-compatible).
    pub fn rlp_encode(&self) -> Vec<u8> {
        // In production: use zbx_rlp::encode
        let mut out = Vec::with_capacity(100);
        out.extend_from_slice(&self.nonce.to_be_bytes());
        out.extend_from_slice(&self.balance.to_be_bytes());
        out.extend_from_slice(&self.storage_root);
        out.extend_from_slice(&self.code_hash);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_eoa_is_eoa() {
        let acc = Account::new_eoa(1000, 0);
        assert!(acc.is_eoa());
        assert!(!acc.is_contract());
    }

    #[test]
    fn zero_account_is_empty() {
        let acc = Account::new_eoa(0, 0);
        assert!(acc.is_empty());
    }

    #[test]
    fn nonzero_balance_not_empty() {
        let acc = Account::new_eoa(1, 0);
        assert!(!acc.is_empty());
    }
}