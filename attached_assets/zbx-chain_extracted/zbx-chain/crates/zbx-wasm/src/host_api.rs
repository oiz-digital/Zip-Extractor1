//! Host API — functions the WASM contract can call into the host (ZBX node).
//!
//! These are the "system calls" available to WASM smart contracts.
//! Each function is registered as a Wasmtime host function import.
//!
//! WASM contracts import these under the `"env"` module:
//!   `(import "env" "zbx_storage_get" ...)`

use crate::WasmError;

/// The host API context for a single WASM contract call.
/// Passed to every host function implementation.
#[derive(Debug, Clone)]
pub struct HostApi {
    pub contract_address: [u8; 20],
    pub caller:           [u8; 20],
    pub value:            u128,
    pub block_number:     u64,
    pub block_timestamp:  u64,
    pub gas_remaining:    u64,
    pub call_depth:       u32,
    /// Events emitted during this call.
    pub events:           Vec<WasmEvent>,
}

/// An event emitted by a WASM contract.
#[derive(Debug, Clone)]
pub struct WasmEvent {
    pub contract:  [u8; 20],
    pub topics:    Vec<[u8; 32]>,
    pub data:      Vec<u8>,
}

impl HostApi {
    pub fn new(
        contract_address: [u8; 20],
        caller:           [u8; 20],
        value:            u128,
        block_number:     u64,
        block_timestamp:  u64,
        gas_remaining:    u64,
    ) -> Self {
        Self {
            contract_address, caller, value,
            block_number, block_timestamp, gas_remaining,
            call_depth: 0, events: vec![],
        }
    }

    // ─── Storage (ZK-compatible key-value store) ──────────────────────────

    /// zbx_storage_get(key_ptr, key_len) → (val_ptr, val_len)
    pub fn storage_get(&self, key: &[u8]) -> Vec<u8> {
        // Real impl: read from zbx-storage using contract_address + key
        let _ = key; vec![]
    }

    /// zbx_storage_set(key_ptr, key_len, val_ptr, val_len)
    pub fn storage_set(&mut self, key: &[u8], value: &[u8]) {
        // Real impl: write to zbx-storage (logged to state diff)
        let _ = (key, value);
    }

    // ─── Token operations ─────────────────────────────────────────────────

    /// zbx_transfer(to_ptr, amount_high, amount_low) → success
    pub fn zbx_transfer(&mut self, to: [u8; 20], amount: u128) -> bool {
        // Real impl: adjust balances in state, check sufficient balance
        let _ = (to, amount); true
    }

    /// zbx_balance(addr_ptr) → (high, low)
    pub fn zbx_balance(&self, addr: [u8; 20]) -> u128 {
        // Real impl: read from state
        let _ = addr; 0
    }

    // ─── Calls ────────────────────────────────────────────────────────────

    /// zbx_call(to_ptr, value, data_ptr, data_len) → (ret_ptr, ret_len, success)
    pub fn call(&mut self, to: [u8; 20], value: u128, data: Vec<u8>) -> (Vec<u8>, bool) {
        if self.call_depth >= 64 {
            return (vec![], false); // call stack depth limit
        }
        // Real impl: dispatch to EVM or WASM executor based on contract type
        let _ = (to, value, data);
        (vec![], true)
    }

    // ─── Crypto precompiles ───────────────────────────────────────────────

    /// zbx_keccak256(data_ptr, data_len) → hash_ptr
    pub fn keccak256(&self, data: &[u8]) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        Keccak256::digest(data).into()
    }

    // ─── Events ───────────────────────────────────────────────────────────

    /// zbx_emit(topics_ptr, topics_count, data_ptr, data_len)
    pub fn emit(&mut self, topics: Vec<[u8; 32]>, data: Vec<u8>) {
        self.events.push(WasmEvent {
            contract: self.contract_address,
            topics,
            data,
        });
    }

    // ─── Gas ──────────────────────────────────────────────────────────────

    pub fn consume_gas(&mut self, amount: u64) -> Result<(), WasmError> {
        if self.gas_remaining < amount {
            return Err(WasmError::OutOfGas {
                limit: self.gas_remaining,
                used:  amount,
            });
        }
        self.gas_remaining -= amount;
        Ok(())
    }
}