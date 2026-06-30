//! WASM contract instance — represents a single running WASM execution.

use crate::{error::WasmError, host_api::HostApi};

/// The output of a WASM contract call.
#[derive(Debug, Clone)]
pub struct WasmOutput {
    pub return_data:  Vec<u8>,
    pub gas_used:     u64,
    pub success:      bool,
    pub revert_reason: Option<String>,
    pub events:       Vec<crate::host_api::WasmEvent>,
}

/// A single WASM contract instance (one per call).
pub struct WasmInstance {
    bytecode:  Vec<u8>,
    host_api:  HostApi,
    gas_limit: u64,
}

impl WasmInstance {
    pub fn new(bytecode: Vec<u8>, host_api: HostApi, gas_limit: u64) -> Result<Self, WasmError> {
        Ok(Self { bytecode, host_api, gas_limit })
    }

    /// Call a named function in the WASM module.
    pub fn call(&mut self, func: &str, args: &[u8]) -> Result<WasmOutput, WasmError> {
        // Real impl:
        // 1. Instantiate wasmtime Store with fuel = gas_limit
        // 2. Link host functions (storage_get, zbx_transfer, etc.)
        // 3. Instantiate the module
        // 4. Find the exported function `func`
        // 5. Call it with `args` (ABI-encoded)
        // 6. Collect return data and remaining fuel (gas)
        let gas_used = self.gas_limit.min(1_000);
        Ok(WasmOutput {
            return_data:   vec![],
            gas_used,
            success:       true,
            revert_reason: None,
            events:        self.host_api.events.clone(),
        })
    }

    /// Deploy a new WASM contract (call the constructor).
    pub fn deploy(&mut self, constructor_args: &[u8]) -> Result<WasmOutput, WasmError> {
        self.call("constructor", constructor_args)
    }
}