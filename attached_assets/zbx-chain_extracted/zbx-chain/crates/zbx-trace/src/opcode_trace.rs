//! Opcode-level EVM trace (one entry per executed instruction).

use serde::{Deserialize, Serialize};

/// A single executed EVM opcode step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpcodeStep {
    /// Program counter (byte offset in bytecode).
    pub pc:        u64,
    /// Opcode name (e.g., "PUSH1", "SLOAD", "CALL").
    pub op:        String,
    /// Gas remaining BEFORE this opcode.
    pub gas:       u64,
    /// Gas cost of this opcode.
    pub gas_cost:  u64,
    /// Current call depth (0 = top-level).
    pub depth:     u32,
    /// Stack contents AFTER this opcode (top = last element).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stack:     Vec<String>,  // hex-encoded 32-byte values
    /// Memory contents (hex-encoded, full dump).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory:    Option<String>,
    /// Storage changes at this step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage:   Option<std::collections::HashMap<String, String>>,
    /// Revert reason if this step caused a revert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error:     Option<String>,
}

/// Full opcode trace for a transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpcodeTrace {
    pub gas:          u64,
    pub failed:       bool,
    pub return_value: String,   // hex
    pub struct_logs:  Vec<OpcodeStep>,
}

impl OpcodeTrace {
    pub fn new(gas: u64) -> Self {
        Self { gas, failed: false, return_value: String::new(), struct_logs: vec![] }
    }

    pub fn push_step(&mut self, step: OpcodeStep) {
        self.struct_logs.push(step);
    }

    pub fn gas_used(&self) -> u64 {
        let last = self.struct_logs.last();
        last.map(|s| self.gas.saturating_sub(s.gas)).unwrap_or(0)
    }
}