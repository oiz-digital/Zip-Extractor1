//! EVM host interface: state queries and mutations from the interpreter.

use zbx_types::{Address, U256, H256};
use crate::context::Log;

/// The host interface: connects the EVM interpreter to the node's state.
pub trait Host {
    /// Get the balance of `addr`.
    fn balance(&self, addr: Address) -> (U256, bool); // (balance, is_warm)

    /// Get the nonce of `addr`.
    fn nonce(&self, addr: Address) -> u64;

    /// Get the code at `addr`.
    fn code(&self, addr: Address) -> &[u8];

    /// Get the code hash of `addr`.
    fn code_hash(&self, addr: Address) -> H256;

    /// Read a storage slot (returns value + warm status).
    fn storage(&self, addr: Address, key: U256) -> (U256, bool);

    /// Read a transient storage slot (EIP-1153).
    fn transient_storage(&self, addr: Address, key: U256) -> U256;

    /// Write a storage slot. Returns (original, current, new).
    fn set_storage(&mut self, addr: Address, key: U256, value: U256) -> StorageOutcome;

    /// Write a transient storage slot.
    fn set_transient_storage(&mut self, addr: Address, key: U256, value: U256);

    /// Get the code of an arbitrary account (for EXTCODECOPY etc).
    fn ext_code(&self, addr: Address) -> &[u8];

    /// Emit an EVM log.
    fn emit_log(&mut self, log: Log);

    /// Mark an address as accessed in the access list.
    fn access_address(&mut self, addr: Address) -> bool; // true = was warm

    /// Mark a storage slot as accessed.
    fn access_slot(&mut self, addr: Address, key: U256) -> bool; // true = was warm

    /// Selfdestructed account.
    fn selfdestruct(&mut self, addr: Address, beneficiary: Address);

    /// Check if `addr` is empty (zero balance, zero nonce, no code).
    fn is_empty(&self, addr: Address) -> bool;

    /// Get the block hash for block `number` (for the last 256 blocks).
    fn block_hash(&self, number: u64) -> H256;
}

/// Outcome of writing to a storage slot (used for SSTORE gas calculation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOutcome {
    /// Value is the same as current.
    Unchanged,
    /// Slot was clean (current == original).
    SlotClean,
    /// Slot was dirty (current != original).
    SlotDirty,
    /// Slot was cleared (current goes to zero).
    Cleared,
    /// Slot was reset to original (refund applies).
    Reset,
}