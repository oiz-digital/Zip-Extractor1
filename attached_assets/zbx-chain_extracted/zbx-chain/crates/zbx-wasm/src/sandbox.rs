//! WASM sandbox — strict resource limits for contract execution.
//!
//! Each WASM call is isolated in a sandbox with:
//!   - Gas limit (mapped to Wasmtime "fuel")
//!   - Memory limit (max pages)
//!   - Call depth limit
//!   - Wall-clock timeout (secondary safety net)
//!   - Separate address space (no shared memory between calls)

/// Resource limits for a single WASM call.
#[derive(Debug, Clone)]
pub struct SandboxLimits {
    pub gas_limit:       u64,
    pub max_memory_pages: u32,
    pub max_call_depth:  u32,
    /// Wall-clock timeout in milliseconds.
    pub timeout_ms:      u64,
    /// Max number of events emittable in one call.
    pub max_events:      u32,
    /// Max size of a single event's data field (bytes).
    pub max_event_size:  u32,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            gas_limit:        10_000_000,
            max_memory_pages: 256,        // 16 MB
            max_call_depth:   64,
            timeout_ms:       5_000,      // 5 seconds
            max_events:       1_000,
            max_event_size:   16_384,     // 16 KB
        }
    }
}

impl SandboxLimits {
    /// Conservative limits for cheap calls (view functions).
    pub fn view() -> Self {
        Self {
            gas_limit:        1_000_000,
            max_memory_pages: 64,
            max_call_depth:   16,
            timeout_ms:       500,
            max_events:       0,          // view calls cannot emit events
            max_event_size:   0,
        }
    }

    /// Maximum limits for compute-intensive contracts (ZK provers, etc.).
    pub fn high_compute() -> Self {
        Self {
            gas_limit:        100_000_000,
            max_memory_pages: 1024,       // 64 MB
            max_call_depth:   64,
            timeout_ms:       30_000,
            max_events:       1_000,
            max_event_size:   65_536,
        }
    }

    /// Gas limit in Wasmtime "fuel" units (1 fuel ≈ 1 WASM instruction).
    pub fn fuel(&self) -> u64 {
        self.gas_limit / 10 // rough conversion: 1 ZBX gas ≈ 10 WASM instructions
    }
}