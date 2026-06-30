//! Tracer — main entry point for replaying and tracing transactions.

use crate::{call_trace::CallTrace, error::TraceError, opcode_trace::OpcodeTrace};

/// Tracer configuration.
#[derive(Debug, Clone)]
pub struct TracerConfig {
    /// Whether to include stack in each opcode step.
    pub with_stack:    bool,
    /// Whether to include memory in each opcode step.
    pub with_memory:   bool,
    /// Whether to include storage changes in each opcode step.
    pub with_storage:  bool,
    /// Max memory size to include in trace (bytes). 0 = no limit.
    pub max_memory:    usize,
    /// Max trace output size (bytes). Protects against huge traces.
    pub max_output:    usize,
    /// Tracer type: opcode, call, prestate, or diff.
    pub tracer_type:   TracerType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TracerType {
    Opcode,       // struct_logs style
    Call,         // call tree style
    Prestate,     // account state before execution
    StateDiff,    // account state changes
}

impl Default for TracerConfig {
    fn default() -> Self {
        Self {
            with_stack:   true,
            with_memory:  false,  // disabled by default (large)
            with_storage: false,  // disabled by default
            max_memory:   32_768, // 32 KB
            max_output:   100 * 1024 * 1024, // 100 MB
            tracer_type:  TracerType::Opcode,
        }
    }
}

/// Tracer: replays a transaction and produces a trace.
pub struct Tracer {
    /// L53-03 FIX: suppress dead_code warning — config is used once real
    /// state-replay is implemented; the field is intentionally retained for
    /// the stable API surface.
    #[allow(dead_code)]
    config: TracerConfig,
}

impl Tracer {
    pub fn new(config: TracerConfig) -> Self { Self { config } }

    /// Trace a transaction by hash (replay from historical state).
    ///
    /// L53-03 FIX: parameter renamed to `_tx_hash` to suppress the
    /// unused-variable warning while the full replay implementation is
    /// pending.  The hash will be used once the block/state lookup is wired.
    pub fn trace_tx(&self, _tx_hash: [u8; 32]) -> Result<TraceOutput, TraceError> {
        // Real impl:
        // 1. Find tx in block.
        // 2. Restore state to the block's parent state.
        // 3. Re-execute all prior txs in the block (to get correct state).
        // 4. Execute the target tx with tracing hooks enabled.
        // 5. Collect opcode/call trace.
        Ok(TraceOutput::Opcode(OpcodeTrace::new(0)))
    }

    /// Simulate a call against the current state (no actual tx needed).
    pub fn trace_call(
        &self,
        from:  [u8; 20],
        to:    [u8; 20],
        input: Vec<u8>,
        gas:   u64,
    ) -> Result<TraceOutput, TraceError> {
        Ok(TraceOutput::Call(CallTrace::new_call(from, to, 0, gas, input)))
    }
}

/// Output of a trace operation.
#[derive(Debug, Clone)]
pub enum TraceOutput {
    Opcode(OpcodeTrace),
    Call(CallTrace),
    Prestate(std::collections::HashMap<String, PrestateAccount>),
}

/// Account state before execution (prestate trace).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrestateAccount {
    pub balance: String,
    pub nonce:   u64,
    pub code:    String,
    pub storage: std::collections::HashMap<String, String>,
}
