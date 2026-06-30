//! Production `ZvmHost` adapter for the executor's `StateView`.
//!
//! The host body itself lives in `zbx-state` ([`zbx_state::host_zvm`]) so
//! the spec's "ProductionZvmHost in zbx-state, generic over live state"
//! requirement is satisfied. This module just implements the
//! [`ZvmStateAccess`] trait for the executor-local `StateView` overlay
//! and re-exports the moved symbols so existing executor call sites
//! keep compiling unchanged.

use crate::executor::StateView;
use zbx_types::{
    account::AccountState,
    address::Address as ZbxAddress,
    receipt::Log,
    H256,
};
use zbx_state::host_zvm::ZvmStateAccess;

pub use zbx_state::host_zvm::{
    ProductionZvmHost,
    TransientScratchpad,
    ZvmBlockEnv,
};

impl ZvmStateAccess for StateView {
    fn get_account(&self, addr: &ZbxAddress) -> AccountState {
        StateView::get_account(self, addr)
    }
    fn set_account(&mut self, addr: ZbxAddress, state: AccountState) {
        StateView::set_account(self, addr, state);
    }
    fn get_storage_word(&self, addr: &ZbxAddress, key: &[u8; 32]) -> [u8; 32] {
        StateView::get_storage(self, addr, key)
    }
    fn set_storage_word(&mut self, addr: ZbxAddress, key: [u8; 32], value: [u8; 32]) {
        StateView::set_storage(self, addr, key, value);
    }
    fn get_code_for(&self, addr: &ZbxAddress) -> Vec<u8> {
        StateView::get_code(self, addr)
    }
    fn seed_code(&mut self, code_hash: H256, code: Vec<u8>) {
        StateView::seed_code(self, code_hash, code);
    }
    fn emit_log(&mut self, log: Log) {
        StateView::emit_log(self, log);
    }
}
