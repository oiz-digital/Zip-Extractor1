//! zbx-trace — EVM execution tracing.
//!
//! Provides Geth-compatible tracing for debugging smart contracts.
//!
//! ## Trace types
//!
//! **Opcode trace (`debug_traceTransaction`):**
//!   - Every EVM opcode: PC, opcode name, gas, stack, memory, storage
//!   - Used by: Hardhat, Remix, Tenderly, contract developers
//!
//! **Call trace (`debug_traceCall`):**
//!   - Tree of CALL/DELEGATECALL/STATICCALL/CREATE
//!   - Shows: from, to, value, input, output, gas, revert reason
//!   - Used by: Etherscan, block explorers, front-ends
//!
//! **Prestate trace:**
//!   - Account state BEFORE execution (for debugging state diffs)
//!
//! **State diff trace:**
//!   - Exact changes: which storage slots changed from what to what

pub mod call_trace;
pub mod error;
pub mod opcode_trace;
pub mod tracer;

pub use call_trace::{CallTrace, CallType};
pub use error::TraceError;
pub use opcode_trace::{OpcodeStep, OpcodeTrace};
pub use tracer::{Tracer, TracerConfig};