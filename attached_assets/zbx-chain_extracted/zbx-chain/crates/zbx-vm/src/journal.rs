//! EVM journal: tracks state changes for revert support.

use zbx_types::{Address, U256, H256};
use std::collections::HashMap;

/// A single journaled change.
#[derive(Debug, Clone)]
pub enum JournalChange {
    AccountCreated { addr: Address },
    AccountDestroyed { addr: Address, balance: U256 },
    BalanceChanged { addr: Address, old: U256 },
    NonceChanged  { addr: Address, old: u64 },
    CodeChanged   { addr: Address, old_hash: H256 },
    StorageChanged { addr: Address, key: U256, old: U256 },
    TransientStorageChanged { addr: Address, key: U256, old: U256 },
    AccessedAccount { addr: Address },
    AccessedSlot    { addr: Address, key: U256 },
}

/// In-memory journal: tracks all state changes in the current call frame.
pub struct Journal {
    entries: Vec<(usize, JournalChange)>,
    /// Checkpoint stack: each checkpoint is an index into `entries`.
    checkpoints: Vec<usize>,
    /// Transient storage for EIP-1153.
    transient: HashMap<(Address, U256), U256>,
}

impl Journal {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            checkpoints: Vec::new(),
            transient: HashMap::new(),
        }
    }

    pub fn checkpoint(&mut self) -> usize {
        let cp = self.entries.len();
        self.checkpoints.push(cp);
        cp
    }

    pub fn revert_to_checkpoint(&mut self, cp: usize) {
        // Pop entries back to the checkpoint.
        self.entries.truncate(cp);
        self.checkpoints.retain(|&c| c <= cp);
    }

    pub fn commit_checkpoint(&mut self) {
        self.checkpoints.pop();
    }

    pub fn record(&mut self, change: JournalChange) {
        let depth = self.checkpoints.len();
        self.entries.push((depth, change));
    }

    pub fn transient_get(&self, addr: Address, key: U256) -> U256 {
        *self.transient.get(&(addr, key)).unwrap_or(&U256::zero())
    }

    pub fn transient_set(&mut self, addr: Address, key: U256, old: U256, new: U256) {
        self.record(JournalChange::TransientStorageChanged { addr, key, old });
        self.transient.insert((addr, key), new);
    }

    /// Entries count (for debugging).
    pub fn len(&self) -> usize { self.entries.len() }
}

impl Default for Journal {
    fn default() -> Self { Self::new() }
}