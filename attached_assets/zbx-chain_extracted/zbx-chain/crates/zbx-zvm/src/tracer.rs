//! ZVM execution tracer — records every opcode step for debugging.

use crate::{opcodes::Opcode, context::ExecutionStatus};
use serde::{Deserialize, Serialize};

/// A single traced execution step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    /// Program counter.
    pub pc:     usize,
    /// Opcode executed.
    pub op:     String,
    /// Opcode byte value.
    pub op_byte: u8,
    /// Whether this is a ZVM-native opcode.
    pub is_zvm: bool,
    /// Gas remaining before this step.
    pub gas:    u64,
    /// Gas cost of this step.
    pub gas_cost: u64,
    /// Stack depth after this step.
    pub stack_depth: usize,
}

/// Full execution trace.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ZvmTrace {
    pub steps: Vec<TraceStep>,
    pub final_status: Option<String>,
    pub gas_used: u64,
}

impl ZvmTrace {
    pub fn new() -> Self {
        ZvmTrace::default()
    }

    pub fn add_step(&mut self, step: TraceStep) {
        self.steps.push(step);
    }

    pub fn finish(&mut self, status: &ExecutionStatus, gas_used: u64) {
        self.final_status = Some(format!("{:?}", status));
        self.gas_used = gas_used;
    }

    /// Filter trace to only ZVM-native opcode steps.
    pub fn zvm_steps(&self) -> Vec<&TraceStep> {
        self.steps.iter().filter(|s| s.is_zvm).collect()
    }
}