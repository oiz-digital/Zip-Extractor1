//! State cache — in-memory cache for hot accounts during block execution.
//!
//! During block execution, the same accounts are accessed repeatedly.
//! Without a cache, every account read would require a trie lookup (expensive).
//!
//! The cache is:
//!   - Populated on first read from the trie (or pending changes).
//!   - Written to atomically at block end (batch trie update).
//!   - Cleared between blocks (no cross-block state sharing).

use crate::account::Account;
use std::collections::HashMap;

/// Cache entry — tracks whether the account was modified.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub account:  Account,
    pub dirty:    bool,   // true if modified during current block
    pub created:  bool,   // true if account was created in current block
}

/// Block-scoped state cache.
pub struct StateCache {
    accounts: HashMap<[u8; 20], CacheEntry>,
    /// Contract code cache: code_hash → bytecode.
    code:     HashMap<[u8; 32], Vec<u8>>,
    /// Contract storage cache: (address, slot) → value.
    storage:  HashMap<([u8; 20], [u8; 32]), [u8; 32]>,
    /// Number of storage writes this block (for gas metering).
    storage_writes: u64,
}

impl StateCache {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            code:     HashMap::new(),
            storage:  HashMap::new(),
            storage_writes: 0,
        }
    }

    pub fn get_account(&self, addr: &[u8; 20]) -> Option<&Account> {
        self.accounts.get(addr).map(|e| &e.account)
    }

    pub fn set_account(&mut self, addr: [u8; 20], account: Account, created: bool) {
        let dirty = self.accounts.get(&addr)
            .map(|e| e.account != account)
            .unwrap_or(true);
        self.accounts.insert(addr, CacheEntry { account, dirty, created });
    }

    pub fn get_storage(&self, addr: &[u8; 20], slot: &[u8; 32]) -> Option<[u8; 32]> {
        self.storage.get(&(*addr, *slot)).copied()
    }

    pub fn set_storage(&mut self, addr: [u8; 20], slot: [u8; 32], value: [u8; 32]) {
        self.storage.insert((addr, slot), value);
        self.storage_writes += 1;
    }

    pub fn get_code(&self, code_hash: &[u8; 32]) -> Option<&Vec<u8>> {
        self.code.get(code_hash)
    }

    pub fn set_code(&mut self, code_hash: [u8; 32], code: Vec<u8>) {
        self.code.insert(code_hash, code);
    }

    /// All dirty (modified) accounts — to be flushed to the state trie.
    pub fn dirty_accounts(&self) -> impl Iterator<Item = (&[u8; 20], &Account)> {
        self.accounts.iter()
            .filter(|(_, e)| e.dirty)
            .map(|(addr, e)| (addr, &e.account))
    }

    pub fn account_count(&self) -> usize { self.accounts.len() }
    pub fn storage_write_count(&self) -> u64 { self.storage_writes }
    pub fn clear(&mut self) { *self = Self::new(); }
}

impl Default for StateCache {
    fn default() -> Self { Self::new() }
}