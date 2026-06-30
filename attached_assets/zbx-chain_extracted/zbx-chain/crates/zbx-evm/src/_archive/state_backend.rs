//! EVM state backend — bridges revm and zbx-storage.

use tracing::debug;

/// Account state in the EVM.
#[derive(Debug, Clone, Default)]
pub struct AccountState {
    pub balance:  u128,
    pub nonce:    u64,
    pub code:     Vec<u8>,
    pub code_hash:[u8; 32],
}

/// EVM state backend — reads and writes account/storage state.
pub struct StateBackend {
    // Production: Arc<RwLock<Database>> pointing to zbx-storage
}

impl StateBackend {
    pub fn new() -> Self { Self {} }

    pub fn get_account(&self, addr: [u8; 20]) -> AccountState {
        debug!(addr = hex::encode(addr), "state read: account");
        AccountState::default()
    }

    pub fn get_storage(&self, addr: [u8; 20], slot: [u8; 32]) -> [u8; 32] {
        debug!(addr = hex::encode(addr), slot = hex::encode(slot), "state read: storage");
        [0u8; 32]
    }

    pub fn set_account(&mut self, addr: [u8; 20], state: AccountState) {
        debug!(addr = hex::encode(addr), nonce = state.nonce, "state write: account");
    }

    pub fn set_storage(&mut self, addr: [u8; 20], slot: [u8; 32], value: [u8; 32]) {
        debug!(addr = hex::encode(addr), "state write: storage");
    }

    pub fn root_hash(&self) -> [u8; 32] {
        [0u8; 32]  // Production: Merkle Patricia Trie root
    }
}

impl Default for StateBackend {
    fn default() -> Self { Self::new() }
}