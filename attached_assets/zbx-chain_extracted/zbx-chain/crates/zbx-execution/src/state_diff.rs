//! State diff: changes applied by one transaction or block.

use std::collections::HashMap;
use zbx_types::{account::AccountState, address::Address, H256};

/// Accumulated state changes for one transaction.
#[derive(Debug, Default, Clone)]
pub struct StateDiff {
    /// Modified accounts keyed by address.
    pub accounts: HashMap<Address, AccountState>,
    /// Modified storage slots: address → (slot → new_value).
    pub storage: HashMap<Address, HashMap<H256, H256>>,
    /// New contract bytecode: code_hash → bytecode.
    pub new_code: HashMap<H256, Vec<u8>>,
    /// Accounts marked for deletion (SELFDESTRUCT).
    pub deleted: Vec<Address>,
    /// Logs emitted.
    pub logs: Vec<zbx_types::receipt::Log>,
}

impl StateDiff {
    pub fn new() -> Self { Self::default() }

    pub fn set_account(&mut self, addr: Address, state: AccountState) {
        self.accounts.insert(addr, state);
    }

    pub fn set_storage(&mut self, addr: Address, slot: H256, value: H256) {
        self.storage.entry(addr).or_default().insert(slot, value);
    }

    pub fn add_code(&mut self, code_hash: H256, code: Vec<u8>) {
        self.new_code.insert(code_hash, code);
    }

    pub fn delete_account(&mut self, addr: Address) {
        self.deleted.push(addr);
        self.accounts.remove(&addr);
    }

    pub fn emit_log(&mut self, log: zbx_types::receipt::Log) {
        self.logs.push(log);
    }

    /// Merge another diff into this one (other takes precedence).
    pub fn merge(&mut self, other: StateDiff) {
        self.accounts.extend(other.accounts);
        for (addr, slots) in other.storage {
            self.storage.entry(addr).or_default().extend(slots);
        }
        self.new_code.extend(other.new_code);
        self.deleted.extend(other.deleted);
        self.logs.extend(other.logs);
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
            && self.storage.is_empty()
            && self.new_code.is_empty()
            && self.deleted.is_empty()
    }
}