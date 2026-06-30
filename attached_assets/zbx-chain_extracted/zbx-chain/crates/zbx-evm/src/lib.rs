//! zbx-evm: EVM-compatible bytecode interpreter for Zebvix.
//!
//! Implements the Ethereum Virtual Machine specification up to Shanghai
//! (EIP-3855: PUSH0, EIP-3860: initcode size limit).
//! Uses secp256k1 / keccak256 throughout (EVM-native).
//!
//! ## S7-ARCH1 — VM Architecture Notice (2026-05-08)
//!
//! The workspace contains **three** VM crates:
//!
//! | Crate | Purpose | Status |
//! |-------|---------|--------|
//! | `zbx-vm` | Full Cancun EVM (150+ opcodes, EIP-1559, access lists) | **CANONICAL — use for new code** |
//! | `zbx-evm` | Shanghai EVM used by the execution pipeline | Active — do not add new features |
//! | `zbx-zvm` | ZBX-native superset (0xC0–0xC9 opcodes) | Active — ZBX-specific contract execution |
//!
//! **New EVM code should target `zbx-vm`.** `zbx-evm` is retained because it
//! is wired into `zbx-execution` and changing that integration is deferred to
//! the 3-VM consolidation project (S7-ARCH1, assigned to the node team).
//!
//! Until consolidation is complete the rule is:
//! - `zbx-execution` → keeps using `zbx-evm` (no change)
//! - New consensus-layer EVM work → use `zbx-vm`
//! - ZBX-native opcode contracts → use `zbx-zvm`

pub mod error;
pub mod gas;
pub mod host;
pub mod interpreter;
pub mod memory;
pub mod opcodes;
pub mod precompiles;
pub mod stack;
pub mod state;

pub use error::EvmError;
pub use host::{Host, MockHost, SnapshotId};
pub use interpreter::{CallFrame, EVMContext, EVMInterpreter, ExitStatus};
pub use opcodes::Opcode;