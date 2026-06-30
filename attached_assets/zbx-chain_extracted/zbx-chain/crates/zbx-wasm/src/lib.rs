//! zbx-wasm — WebAssembly smart contract runtime.
//!
//! ZBX Chain supports TWO smart contract runtimes:
//!
//! ```
//! ┌─────────────────────────────────────────────┐
//! │              ZBX Chain                      │
//! │                                             │
//! │   ┌──────────────┐  ┌──────────────────┐   │
//! │   │  EVM Runtime │  │  WASM Runtime    │   │
//! │   │  (zbx-evm)   │  │  (zbx-wasm)      │   │
//! │   │              │  │                  │   │
//! │   │  Solidity    │  │  Rust / C / C++  │   │
//! │   │  Vyper       │  │  AssemblyScript  │   │
//! │   │  Yul         │  │  TinyGo          │   │
//! │   └──────────────┘  └──────────────────┘   │
//! │         Both share the same state layer     │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! WASM contracts can:
//!   - Read and write contract storage (key-value, same as EVM)
//!   - Transfer ZBX and ZRC-20 tokens
//!   - Call other contracts (both EVM and WASM)
//!   - Emit events
//!   - Use precompiles (crypto, oracle, bridge)
//!
//! # Example WASM contract (Rust)
//! ```rust,ignore
//! #[zbx_contract]
//! pub mod counter {
//!     use zbx_sdk::prelude::*;
//!
//!     #[storage]
//!     struct Counter { count: u64 }
//!
//!     pub fn increment(env: Env) {
//!         Counter::count(env.storage()).add(1);
//!         env.emit("Incremented", count);
//!     }
//! }
//! ```

pub mod engine;
pub mod error;
pub mod host_api;
pub mod instance;
pub mod loader;
pub mod sandbox;

pub use engine::{WasmEngine, WasmConfig};
pub use error::WasmError;
pub use host_api::HostApi;
pub use instance::{WasmInstance, WasmOutput};