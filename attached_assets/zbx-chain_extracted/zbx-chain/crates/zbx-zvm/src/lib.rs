//! zbx-zvm — Zebvix Virtual Machine.
//!
//! ZVM is a superset of the Ethereum Virtual Machine (EVM).
//! All existing Ethereum/Solidity contracts run unchanged.
//! ZVM adds ZBX-native opcodes for chain-specific features:
//!
//! | Opcode   | Hex  | Description                                   |
//! |----------|------|-----------------------------------------------|
//! | PAYID    | 0xC0 | Resolve ali@zbx Pay ID → wallet address       |
//! | ZUSDBAL  | 0xC1 | Get ZUSD balance of an address                |
//! | ZBXPRICE | 0xC2 | Get current ZBX/USD price (oracle)            |
//! | ZBXTIME  | 0xC3 | Get ZBX block time (5000 ms = 5 seconds)      |
//! | AASENDER | 0xC4 | Get AA UserOperation original sender          |
//! | CHAINVER | 0xC5 | Get ZVM version number                        |
//! | BLOBFEE  | 0xC6 | Get current blob base fee (DA layer)          |
//! | PAYIDSET | 0xC7 | Check if address has a Pay ID registered      |
//! | ZBXBURN  | 0xC8 | Burn ZBX from caller (deflationary mechanism) |
//! | ZVMLOG   | 0xC9 | Emit structured ZVM log (richer than LOG4)    |
//!
//! ## Backwards Compatibility
//!
//! - All EVM opcodes (0x00–0xFF) behave identically to EIP-3855 / Shanghai spec,
//!   including the system-call range 0xF0=CREATE, 0xF1=CALL, 0xF2=CALLCODE,
//!   0xF3=RETURN, 0xF4=DELEGATECALL, 0xF5=CREATE2, 0xFA=STATICCALL,
//!   0xFD=REVERT, 0xFE=INVALID, 0xFF=SELFDESTRUCT.
//! - ZVM-native opcodes occupy 0xC0–0xC9 (previously INVALID/unused in EVM —
//!   chosen specifically to avoid collision with EVM system opcodes).
//! - Contracts not using ZVM opcodes run exactly as on Ethereum.
//! - ZVM bytecode is identified by magic prefix `0xEF 0x5A 0x42` (optional).
//!
//! Audit-2026-05-01 S7-ZVM-DOC1: prior versions of this header doc-block
//! claimed ZBX-native opcodes lived at 0xF0–0xF9, which would have collided
//! with EVM CREATE/CALL/CALLCODE/RETURN/DELEGATECALL/CREATE2. The actual
//! implementation has always used 0xC0–0xC9 (see `opcodes.rs`); the doc
//! table was stale aspirational text from an early design draft.

pub mod opcodes;
pub mod interpreter;
pub mod stack;
pub mod memory;
pub mod gas;
pub mod precompiles;
pub mod context;
pub mod host;
pub mod executor;
pub mod tracer;
pub mod error;

pub use interpreter::ZvmInterpreter;
pub use executor::ZvmExecutor;
pub use context::{ZvmContext, ZvmResult, ExecutionStatus};
pub use opcodes::{Opcode, ZVM_OPCODES};
pub use host::ZvmHost;
pub use error::ZvmError;

/// ZVM version — bumped on each spec change.
pub const ZVM_VERSION: u32 = 1;

/// ZVM magic prefix for ZVM-native contracts (optional, for tooling).
pub const ZVM_MAGIC: [u8; 3] = [0xEF, 0x5A, 0x42]; // EF ZB (Zebvix)

/// ZBX mainnet chain ID. Re-exported for backward compatibility — new code
/// should import `zbx_types::CHAIN_ID_MAINNET` directly.
pub const ZBX_CHAIN_ID: u64 = zbx_types::CHAIN_ID_MAINNET;