//! Legacy transaction format (type 0, pre-EIP-1559).
//!
//! ZBX supports all four Ethereum transaction types:
//!   Type 0: Legacy (gas_price only)           — this file
//!   Type 1: Access list (EIP-2930)
//!   Type 2: EIP-1559 (max_fee + priority_fee) — primary type on ZBX
//!   Type 3: Blob transaction (EIP-4844)
//!
//! Legacy transactions still work on ZBX for backwards compatibility with
//! Ethereum tooling (MetaMask defaults, older dApps).
//! They are treated as EIP-1559 type 2 internally:
//!   max_fee_per_gas    = gas_price
//!   max_priority_fee   = gas_price - base_fee (clamped to 0)

use serde::{Serialize, Deserialize};
use crate::signature::Signature;

/// Transaction type discriminant (EIP-2718 envelope).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TxType {
    /// Type 0: Legacy (no EIP-2718 envelope byte)
    Legacy      = 0,
    /// Type 1: EIP-2930 (access list)
    AccessList  = 1,
    /// Type 2: EIP-1559 (dynamic fee)
    Eip1559     = 2,
    /// Type 3: EIP-4844 (blob transaction)
    Blob        = 3,
}

impl TxType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Legacy),
            1 => Some(Self::AccessList),
            2 => Some(Self::Eip1559),
            3 => Some(Self::Blob),
            _ => None,
        }
    }
}

/// A legacy (type 0) transaction.
///
/// Format: RLP([nonce, gasPrice, gasLimit, to, value, data, v, r, s])
/// Signed: H256(keccak256(RLP([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0])))
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyTransaction {
    /// Sender's nonce (anti-replay)
    pub nonce:     u64,
    /// Gas price (wei) — replaces max_fee + priority_fee in EIP-1559
    pub gas_price: u128,
    /// Maximum gas this tx may consume
    pub gas_limit: u64,
    /// Recipient (None for contract creation)
    pub to:        Option<[u8; 20]>,
    /// Value in wei
    pub value:     u128,
    /// Input data (calldata)
    pub data:      Vec<u8>,
    /// Signature
    pub sig:       Signature,
    /// Chain ID (EIP-155 replay protection)
    pub chain_id:  u64,
}

impl LegacyTransaction {
    /// Compute the message hash for signing (EIP-155).
    pub fn signing_hash(&self) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(&self.nonce.to_le_bytes());
        h.update(&self.gas_price.to_le_bytes());
        h.update(&self.gas_limit.to_le_bytes());
        if let Some(to) = &self.to { h.update(to); }
        h.update(&self.value.to_le_bytes());
        h.update(&self.data);
        h.update(&self.chain_id.to_le_bytes());
        h.finalize().into()
    }

    /// Convert to EIP-1559 equivalent for execution.
    pub fn as_eip1559_params(&self, base_fee: u128) -> (u128, u128) {
        let max_fee = self.gas_price;
        let priority = self.gas_price.saturating_sub(base_fee);
        (max_fee, priority)
    }

    /// Effective gas tip (how much goes to validator after base_fee burned).
    pub fn effective_gas_tip(&self, base_fee: u128) -> u128 {
        self.gas_price.saturating_sub(base_fee)
    }

    pub fn tx_type(&self) -> TxType { TxType::Legacy }

    pub fn is_contract_creation(&self) -> bool { self.to.is_none() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_tx() -> LegacyTransaction {
        LegacyTransaction {
            nonce:     5,
            gas_price: 2_000_000_000, // 2 Gwei
            gas_limit: 21_000,
            to:        Some([0xAA; 20]),
            value:     1_000_000_000_000_000_000, // 1 ZBX
            data:      vec![],
            sig:       Signature { v: 0, r: [0u8; 32], s: [0u8; 32] },
            chain_id:  zbx_types::CHAIN_ID_MAINNET,
        }
    }

    #[test]
    fn legacy_tx_type_is_zero() {
        assert_eq!(legacy_tx().tx_type() as u8, 0);
    }

    #[test]
    fn gas_tip_above_base_fee() {
        let tx = legacy_tx();
        let base = 1_000_000_000u128; // 1 Gwei
        assert_eq!(tx.effective_gas_tip(base), 1_000_000_000u128); // 2-1 = 1 Gwei
    }

    #[test]
    fn gas_tip_zero_when_base_equals_gas_price() {
        let tx = legacy_tx();
        let tip = tx.effective_gas_tip(tx.gas_price);
        assert_eq!(tip, 0);
    }

    #[test]
    fn chain_id_matches_zbx_mainnet() {
        assert_eq!(legacy_tx().chain_id, zbx_types::CHAIN_ID_MAINNET);
    }

    #[test]
    fn tx_type_from_byte() {
        assert_eq!(TxType::from_byte(0), Some(TxType::Legacy));
        assert_eq!(TxType::from_byte(2), Some(TxType::Eip1559));
        assert_eq!(TxType::from_byte(3), Some(TxType::Blob));
        assert_eq!(TxType::from_byte(255), None);
    }
}