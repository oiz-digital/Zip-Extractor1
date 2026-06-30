//! zbx-vm — complete Ethereum Virtual Machine implementation.
//!
//! Implements the full EVM specification including:
//! - All 150+ opcodes (Cancun-era)
//! - EIP-1559 base fee mechanics
//! - EIP-2929 / EIP-2930 access lists and gas repricing
//! - EIP-3529 gas refund limits
//! - EIP-3855 PUSH0 opcode
//! - EIP-3860 initcode size limits
//! - EIP-4895 beacon withdrawals
//! - Built-in precompiles (ecrecover, sha256, bn128, blake2f, ...)
//! - Memory expansion gas
//! - Storage cold/warm access tracking
//!
//! # Usage
//! ```rust
//! use zbx_vm::{Evm, EvmConfig, Context};
//!
//! let mut evm = Evm::new(EvmConfig::mainnet());
//! let result  = evm.transact(context)?;
//! ```

#![warn(missing_docs, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod opcode;
pub mod stack;
pub mod memory;
pub mod gas;
pub mod interpreter;
pub mod precompiles;
pub mod host;
pub mod context;
pub mod journal;

pub use context::{Context, CallContext, TxEnv};
pub use interpreter::{Evm, EvmConfig, ExecutionResult, ExitReason};
pub use host::Host;
pub use journal::Journal;