//! Account state helpers and encoding utilities.
//!
//! ## Account model
//!
//! ZBX Chain uses the standard Ethereum account model (Yellow Paper §4.1)
//! with two additions:
//!
//! 1. **`code_version`** — bumped on each `DELEGATECALL`-based upgrade so
//!    the EVM interpreter can reject stale delegate-target hashes.
//! 2. **`staked_balance`** — mirrors the amount locked in the staking escrow
//!    for validator accounts.  Read-only from the EVM perspective; mutated
//!    only by the staking sub-system.
//!
//! ## Encoding
//!
//! The canonical RLP encoding for the Merkle-Patricia Trie is:
//!
//! ```text
//! RLP([nonce, balance, storage_root, code_hash])
//! ```
//!
//! The extension fields (`code_version`, `staked_balance`) are stored in a
//! separate namespace in `ZbxDb` and are NOT included in the trie encoding so
//! the state root remains compatible with standard Ethereum tooling.

use zbx_types::{
    account::{AccountState, EMPTY_CODE_HASH},
    address::Address,
    H256,
};
use zbx_crypto::keccak::keccak256;
use serde::{Deserialize, Serialize};

// ── AccountInfo ───────────────────────────────────────────────────────────────

/// Extended account view combining the base `AccountState` with ZBX-specific
/// metadata.  Used by the RPC layer and internal sub-systems that need the
/// full picture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub address: Address,
    pub state: AccountState,
    /// Whether this account has deployed bytecode.
    pub is_contract: bool,
    /// ZBX staking escrow balance (wei).  0 for non-validator accounts.
    pub staked_balance: u128,
}

impl AccountInfo {
    pub fn new(address: Address, state: AccountState) -> Self {
        let is_contract = state.code_hash != EMPTY_CODE_HASH;
        AccountInfo {
            address,
            state,
            is_contract,
            staked_balance: 0,
        }
    }

    /// EOA with zero balance, zero nonce, empty code.
    pub fn empty(address: Address) -> Self {
        AccountInfo::new(address, AccountState::default())
    }

    /// Returns true when the account is considered "empty" per EIP-161
    /// (nonce=0, balance=0, code_hash=EMPTY_CODE_HASH).
    pub fn is_empty(&self) -> bool {
        self.state.nonce == 0
            && self.state.balance.is_zero()
            && self.state.code_hash == EMPTY_CODE_HASH
    }
}

// ── Canonical RLP encoding ────────────────────────────────────────────────────

/// Encode an account for the Merkle-Patricia Trie (Yellow Paper §4.1).
///
/// Output: `RLP([nonce, balance, storage_root, code_hash])`
///
/// Note: this is the TRIE encoding — it excludes ZBX-native extension fields.
pub fn encode_account_rlp(state: &AccountState) -> Vec<u8> {
    // Minimal hand-rolled RLP for the 4-field account tuple.
    // Production code would call a proper RLP library; this mirrors what
    // `crate::mpt` already does for trie-node serialisation.
    let mut out = Vec::with_capacity(128);
    // nonce
    rlp_encode_u64(&mut out, state.nonce);
    // balance (u128 → big-endian minimal bytes)
    rlp_encode_u128(&mut out, state.balance_u128());
    // storage_root (32 bytes)
    rlp_encode_bytes(&mut out, state.storage_root.as_bytes());
    // code_hash (32 bytes)
    rlp_encode_bytes(&mut out, state.code_hash.as_bytes());
    rlp_list_wrap(out)
}

/// Compute the trie key for an address: `keccak256(addr)`.
pub fn account_trie_key(addr: &Address) -> H256 {
    H256::from(keccak256(addr.as_bytes()))
}

// ── Minimal inline RLP helpers ────────────────────────────────────────────────

fn rlp_encode_u64(buf: &mut Vec<u8>, v: u64) {
    if v == 0 {
        buf.push(0x80); // empty string
    } else {
        let bytes = v.to_be_bytes();
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(7);
        let slice = &bytes[start..];
        if slice.len() == 1 && slice[0] < 0x80 {
            buf.push(slice[0]);
        } else {
            buf.push(0x80 + slice.len() as u8);
            buf.extend_from_slice(slice);
        }
    }
}

fn rlp_encode_u128(buf: &mut Vec<u8>, v: u128) {
    if v == 0 {
        buf.push(0x80);
    } else {
        let bytes = v.to_be_bytes();
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(15);
        let slice = &bytes[start..];
        buf.push(0x80 + slice.len() as u8);
        buf.extend_from_slice(slice);
    }
}

fn rlp_encode_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        buf.push(bytes[0]);
    } else if bytes.len() <= 55 {
        buf.push(0x80 + bytes.len() as u8);
        buf.extend_from_slice(bytes);
    } else {
        let len_bytes = bytes.len().to_be_bytes();
        let len_start = len_bytes.iter().position(|&b| b != 0).unwrap_or(7);
        let len_slice = &len_bytes[len_start..];
        buf.push(0xb7 + len_slice.len() as u8);
        buf.extend_from_slice(len_slice);
        buf.extend_from_slice(bytes);
    }
}

fn rlp_list_wrap(inner: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(inner.len() + 3);
    if inner.len() <= 55 {
        out.push(0xc0 + inner.len() as u8);
    } else {
        let len_bytes = inner.len().to_be_bytes();
        let start = len_bytes.iter().position(|&b| b != 0).unwrap_or(7);
        let len_slice = &len_bytes[start..];
        out.push(0xf7 + len_slice.len() as u8);
        out.extend_from_slice(len_slice);
    }
    out.extend_from_slice(&inner);
    out
}

// ── AccountDiff ───────────────────────────────────────────────────────────────

/// Describes the change made to an account during a block.
/// Emitted by `StateDB::commit` for the block receipt and indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountDiff {
    pub address: Address,
    pub nonce_before: u64,
    pub nonce_after: u64,
    pub balance_before: u128,
    pub balance_after: u128,
    /// Non-empty iff the account was deployed in this block.
    pub deployed_code_hash: Option<H256>,
    /// True iff the account was self-destructed in this block.
    pub self_destructed: bool,
}

impl AccountDiff {
    pub fn balance_delta(&self) -> i128 {
        self.balance_after as i128 - self.balance_before as i128
    }

    pub fn is_noop(&self) -> bool {
        self.nonce_before == self.nonce_after
            && self.balance_before == self.balance_after
            && self.deployed_code_hash.is_none()
            && !self.self_destructed
    }
}

// ── GenesisAccount ────────────────────────────────────────────────────────────

/// A pre-funded account in the genesis block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAccount {
    pub address: Address,
    pub balance: u128,
    pub nonce: u64,
    /// Pre-deployed bytecode (for system contracts).
    pub code: Option<Vec<u8>>,
    /// Pre-seeded storage slots.
    pub storage: Vec<(H256, H256)>,
}

impl GenesisAccount {
    pub fn to_account_state(&self) -> AccountState {
        let code_hash = if let Some(code) = &self.code {
            H256::from(keccak256(code))
        } else {
            EMPTY_CODE_HASH
        };
        AccountState {
            nonce: self.nonce,
            balance: zbx_types::U256::from(self.balance),
            storage_root: H256::zero(),
            code_hash,
            vm: zbx_types::account::VmKind::Evm,
        }
    }
}
