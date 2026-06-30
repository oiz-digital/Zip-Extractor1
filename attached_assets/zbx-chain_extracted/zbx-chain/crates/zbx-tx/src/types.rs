//! Transaction type definitions.
//!
//! ## Gas token
//!
//! ZBX Chain supports two native gas tokens. Any of the two can be used
//! to pay transaction fees — set `gas_token` in the transaction.
//!
//! | GasToken | Value | Payment mechanism |
//! |----------|-------|-------------------|
//! | Zbx      | 0     | Deducted from native ZBX balance (default, EVM-compatible) |
//! | Zusd     | 1     | Deducted from ZUSD genesis-contract balance |
//!
//! `gas_token` defaults to `Zbx` (serde default) so existing Ethereum tooling
//! and legacy transactions continue to work without modification.

use serde::{Deserialize, Serialize};

// ── Gas token ─────────────────────────────────────────────────────────────────

/// Which token the sender uses to pay gas fees.
///
/// Both are first-class native tokens on ZBX Chain.
/// `Zbx` is the protocol default and fully EVM-backward-compatible.
/// `Zusd` is a genesis-deployed stablecoin contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum GasToken {
    /// Native ZBX — default. Deducted from Account.balance.
    #[default]
    Zbx  = 0,
    /// ZUSD stablecoin. Deducted from the sender's ZUSD contract balance.
    Zusd = 1,
}

impl GasToken {
    /// Parse from a u8 byte (wire encoding).
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Zbx),
            1 => Some(Self::Zusd),
            _ => None,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Zbx  => "ZBX",
            Self::Zusd => "ZUSD",
        }
    }

    pub fn is_native_zbx(self) -> bool { matches!(self, Self::Zbx) }
}

// ── Transaction types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxType {
    Legacy    = 0,
    Eip2930   = 1,
    Eip1559   = 2,   // ← default on ZBX Chain
}

/// An unsigned EIP-1559 transaction (with ZBX-native multi-gas-token extension).
///
/// The `gas_token` field is a ZBX Chain extension — it specifies which native
/// token (ZBX / ZUSD) covers the gas fee. It is always included in the
/// signing hash so it cannot be changed after signing.
///
/// `gas_token` defaults to `GasToken::Zbx` so existing Ethereum tooling
/// remains fully compatible when not using stablecoin gas.
///
/// `Default` produces a zero-nonce EIP-1559 transaction with no recipient
/// (contract creation), no value, empty calldata, and ZBX gas. Callers
/// MUST set `chain_id`, `gas_limit`, and any other required fields before
/// signing — the default chain_id of 0 is not a valid ZBX network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub tx_type:           TxType,
    pub chain_id:          u64,      // 8989 mainnet, 8990 testnet+devnet
    pub nonce:             u64,
    pub max_fee_per_gas:   u128,     // wei-equivalent in the gas token
    pub max_priority_fee:  u128,     // wei-equivalent in the gas token
    pub gas_limit:         u64,
    pub to:                Option<[u8; 20]>,
    pub value:             u128,     // wei (always ZBX regardless of gas_token)
    pub data:              Vec<u8>,
    pub access_list:       Vec<AccessListEntry>,
    /// Gas payment token. Defaults to ZBX (EVM-compatible).
    /// Included in the signing hash — changing this invalidates the signature.
    #[serde(default)]
    pub gas_token:         GasToken,
}

impl Default for Transaction {
    fn default() -> Self {
        Transaction {
            tx_type:          TxType::Eip1559,
            chain_id:         0,    // MUST be overridden before signing
            nonce:            0,
            max_fee_per_gas:  0,
            max_priority_fee: 0,
            gas_limit:        21_000,
            to:               None,
            value:            0,
            data:             Vec::new(),
            access_list:      Vec::new(),
            gas_token:        GasToken::Zbx,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessListEntry {
    pub address:      [u8; 20],
    pub storage_keys: Vec<[u8; 32]>,
}

/// A signed transaction (transaction + v, r, s signature).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTx {
    pub tx:   Transaction,
    pub v:    u64,
    pub r:    [u8; 32],
    pub s:    [u8; 32],
    /// Recovered sender address (cached after verification).
    pub from: Option<[u8; 20]>,
    /// Transaction hash (keccak256 of RLP-encoded signed tx).
    pub hash: [u8; 32],
}
