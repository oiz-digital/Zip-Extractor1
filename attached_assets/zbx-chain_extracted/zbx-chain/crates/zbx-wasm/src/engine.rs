//! WASM execution engine — wraps Wasmtime with ZBX-specific config.

use crate::{error::WasmError, host_api::HostApi, instance::WasmInstance};

/// WASM engine configuration.
#[derive(Debug, Clone)]
pub struct WasmConfig {
    /// Maximum gas per WASM call (prevents infinite loops).
    pub gas_limit:       u64,
    /// Maximum memory pages (1 page = 64 KB). Default: 256 = 16 MB.
    pub max_memory_pages: u32,
    /// Maximum call stack depth.
    pub max_stack_depth: usize,
    /// Whether to enable WASM SIMD instructions.
    pub enable_simd:     bool,
    /// Whether to enable WASM threads (shared memory).
    pub enable_threads:  bool,
    /// Cache compiled modules on disk.
    pub module_cache:    bool,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            gas_limit:        10_000_000,
            max_memory_pages: 256,      // 16 MB max
            max_stack_depth:  512,
            enable_simd:      true,
            enable_threads:   false,    // disabled for determinism
            module_cache:     true,
        }
    }
}

/// The WASM execution engine (one per node, shared across calls).
pub struct WasmEngine {
    config: WasmConfig,
    // engine: wasmtime::Engine  ← real impl wraps Wasmtime
}

impl WasmEngine {
    pub fn new(config: WasmConfig) -> Result<Self, WasmError> {
        // Real impl: configure wasmtime::Engine with our settings.
        // - Disable WASM threads (non-deterministic)
        // - Enable fuel metering (gas accounting)
        // - Enable module caching
        Ok(Self { config })
    }

    pub fn default() -> Result<Self, WasmError> {
        Self::new(WasmConfig::default())
    }

    /// Compile and cache a WASM module from bytecode.
    pub fn compile(&self, wasm_bytes: &[u8]) -> Result<CompiledModule, WasmError> {
        if wasm_bytes.len() < 4 || &wasm_bytes[..4] != b"\0asm" {
            return Err(WasmError::InvalidModule("not a valid WASM binary".into()));
        }
        // Real impl: wasmtime::Module::new(&engine, wasm_bytes)
        Ok(CompiledModule { bytecode: wasm_bytes.to_vec() })
    }

    /// Create a new execution instance from a compiled module.
    pub fn instantiate(
        &self,
        module:   &CompiledModule,
        host_api: HostApi,
        gas:      u64,
    ) -> Result<WasmInstance, WasmError> {
        WasmInstance::new(module.bytecode.clone(), host_api, gas)
    }

    pub fn config(&self) -> &WasmConfig { &self.config }
}

/// A compiled (but not yet instantiated) WASM module.
pub struct CompiledModule {
    pub bytecode: Vec<u8>,
    // module: wasmtime::Module  ← real impl
}