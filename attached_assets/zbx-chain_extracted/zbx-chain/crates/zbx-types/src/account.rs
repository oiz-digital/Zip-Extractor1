//! Account state stored in the world-state trie.

use crate::{H256, U256};
use serde::{Deserialize, Serialize};

/// Task #2 (2026-05-09) — execution-VM tag for an account.
///
/// Determines which interpreter the executor dispatches to when this
/// account is the *callee* of a transaction (or a top-level CREATE).
///
/// `Evm` is the default: matches every account ever deployed before
/// Task #2 landed, plus all EOAs (which never execute code anyway).
/// `Zvm` is set when an account is deployed via CREATE/CREATE2 whose
/// init-code begins with the `0x5A` discriminator byte (see executor
/// `execute_tx` deploy branch). The discriminator is stripped from
/// the stored runtime code, so `code_hash` already commits to the
/// post-strip bytes — `vm` is a derived classification cache and does
/// not need to enter the world-state trie root for deterministic
/// replay (deploying the same init-code on every node yields the
/// same `vm` independently).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VmKind {
    /// Ethereum-compatible interpreter (`zbx-evm`). Default for every
    /// EOA and every account deployed without the ZVM discriminator.
    #[default]
    Evm = 0,
    /// Zebvix VM interpreter (`zbx-zvm`) — superset of EVM with
    /// ZBX-native opcodes (PAYID, ZUSDBAL, ZBXPRICE, …).
    Zvm = 1,
}

/// Keccak256 of empty bytes — used as default code_hash for EOAs.
pub const EMPTY_CODE_HASH: H256 = H256([
    0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c,
    0x92, 0x7e, 0x7d, 0xb2, 0xdc, 0xc7, 0x03, 0xc0,
    0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b,
    0x7b, 0xfa, 0xd8, 0x04, 0x5d, 0x85, 0xa4, 0x70,
]);

/// Keccak256 of the RLP of an empty list — the empty storage trie root.
pub const EMPTY_STORAGE_ROOT: H256 = H256([
    0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6,
    0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
    0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0,
    0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
]);

/// Per-account state stored in the global Merkle-Patricia Trie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountState {
    /// Monotonically increasing counter preventing replay attacks.
    pub nonce: u64,
    /// ZBX balance in wei (1 ZBX = 10^18 wei).
    pub balance: U256,
    /// Keccak256 of the account's EVM bytecode (EMPTY_CODE_HASH for EOAs).
    pub code_hash: H256,
    /// Root of the account's storage Patricia trie.
    pub storage_root: H256,
    /// Execution-VM tag set at deploy time. Persisted in canonical
    /// account RLP as a 5th list element **only** when `vm == Zvm`
    /// (4-element list for Evm preserves pre-existing state roots).
    /// `#[serde(default)]` keeps every JSON-encoded account written
    /// before this field landed loadable as `Evm`.
    #[serde(default)]
    pub vm: VmKind,
}

impl Default for AccountState {
    fn default() -> Self {
        AccountState {
            nonce: 0,
            balance: U256::zero(),
            code_hash: EMPTY_CODE_HASH,
            storage_root: EMPTY_STORAGE_ROOT,
            vm: VmKind::Evm,
        }
    }
}

impl AccountState {
    /// True for externally-owned accounts (no contract code).
    pub fn is_eoa(&self) -> bool {
        self.code_hash == EMPTY_CODE_HASH
    }

    /// True for deployed smart contracts.
    pub fn is_contract(&self) -> bool {
        !self.is_eoa()
    }

    /// True if the account has never been touched (default state).
    pub fn is_empty(&self) -> bool {
        self.nonce == 0 && self.balance.is_zero() && self.is_eoa()
    }

    /// Balance as a u128 (sufficient for ZBX total supply).
    /// Saturates at u128::MAX if the on-chain balance overflows u128.
    pub fn balance_u128(&self) -> u128 {
        // primitive_types::U256 stores 4 little-endian u64 words; the low
        // 128 bits are exactly the u128 value.
        if self.balance.bits() > 128 {
            u128::MAX
        } else {
            self.balance.low_u128()
        }
    }

    /// Set balance from u128 wei value.
    pub fn set_balance_u128(&mut self, wei: u128) {
        self.balance = U256::from(wei);
    }

    /// Increment nonce; returns overflow error if nonce wraps.
    pub fn increment_nonce(&mut self) -> Result<(), crate::ZbxError> {
        self.nonce = self.nonce.checked_add(1).ok_or(crate::ZbxError::Overflow)?;
        Ok(())
    }
}