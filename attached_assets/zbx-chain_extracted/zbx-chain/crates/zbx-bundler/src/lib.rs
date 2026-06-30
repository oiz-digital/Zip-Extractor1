//! zbx-bundler — ERC-4337 Account Abstraction bundler for Zebvix Chain.
//!
//! The bundler:
//! 1. Receives UserOperations from wallets via JSON-RPC (`eth_sendUserOperation`)
//! 2. Validates each UserOperation off-chain (simulation)
//! 3. Batches valid UserOperations into a bundle transaction
//! 4. Submits the bundle to the ZBX EntryPoint contract (0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789)
//! 5. Monitors for on-chain inclusion and handles reverts

pub mod bundle;
pub mod error;
pub mod mempool;
pub mod relay;
pub mod rpc;
pub mod session_keys;
pub mod simulation;
pub mod validation;

pub use bundle::BundleBuilder;
pub use error::BundlerError;
pub use mempool::{BundlerMempool, UserOperation};
pub use relay::BundleRelay;
pub use session_keys::{
    SessionKeyError, SessionKeyPolicy, SessionKeyValidator,
    DailyUsage, MAX_SESSION_KEYS_PER_WALLET, MAX_SESSION_KEY_DURATION_SECS,
};
pub use simulation::UserOpSimulator;

/// ZBX mainnet EntryPoint contract address (ERC-4337 v0.6 compatible)
pub const ENTRY_POINT_ADDRESS: &str = "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789";

/// Maximum gas per UserOperation
pub const MAX_USER_OP_GAS: u64 = 5_000_000;

/// Maximum bundle size (UserOps per bundle)
pub const MAX_BUNDLE_SIZE: usize = 50;

// ── High-level runner (ZEP-017 node wiring) ───────────────────────────────────
pub mod service;
pub use service::BundlerService;