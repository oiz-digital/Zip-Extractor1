//! zbx-bridge: Multi-token cross-chain bridge protocol for Zebvix.
//!
//! ## Architecture
//!
//! ```text
//!                         ZBX Chain
//!  ┌──────────────────────────────────────────────────────┐
//!  │  User locks/burns token  →  BridgeVault.sol emits    │
//!  │  BridgeEvent             →  Relayers observe         │
//!  │  3-of-5 multisig sigs    →  execute() on dest chain  │
//!  └──────────────────────────────────────────────────────┘
//!           │ Deposit: lock + mint wrapped on target
//!           │ Withdrawal: burn wrapped + unlock on ZBX
//! ```
//!
//! ## Supported token models
//!
//! | Model         | Source action | Dest action  | Example     |
//! |---------------|---------------|--------------|-------------|
//! | Lock-and-Mint | lock tokens   | mint wrapped | ZBX → WZBX |
//! | Burn-and-Mint | burn tokens   | mint tokens  | ZUSD        |
//!
//! ## Security
//!
//! - **OUT1 (nonce-collision / replay protection)**: deposit IDs include source
//!   chain + sequence so they never collide across chains or redeployments.
//!   `spent_operations` is now backed by a RocksDB column family
//!   (`Column::BridgeSpentOps`) via `BridgeSpentOpsStore`.  The 4-step atomic
//!   flow in `BridgeRelayer::execute()` (is_spent → verify_threshold →
//!   persist_one fsync → mark_spent) guarantees replay-blocking survives
//!   process restarts and power-loss.  Wire `attach_storage()` on startup.
//!   **MAINNET-BLOCKER: FIXED** — see `persistence::BridgeSpentOpsStore`.
//! - **OUT2 (source-chain binding)**: `BridgeRelayer::own_chain_id` is checked
//!   at every stage (`submit`, `confirm`, `execute`).  Requests originating from
//!   a different chain are rejected immediately.
//! - **MS1 (tally-griefing)**: `confirm()` calls `auth.verify_single()` BEFORE
//!   appending to the confirmation list.  An invalid or unauthorised signature
//!   can never poison the list and block threshold execution.
//! - **H-03 (replay protection)**: `verify_and_consume` atomically records
//!   executed operation hashes, preventing relay/replay of the same signed batch.
//! - Per-token daily limits (`DailyLimitTracker`)
//! - Per-transaction max amounts (`TokenWhitelist`)
//! - 24-hour request TTL (`expire_stale`)
//! - Global pause switch (`set_paused`)
//! - Duplicate-request rejection via content-hash ID

pub mod error;
pub mod multisig;
pub mod persistence;
pub mod proofs;
pub mod relayer;
pub mod token;

pub use error::BridgeError;
pub use multisig::{MultisigAuth, MultisigKey};
pub use persistence::{BridgeSpentOpsStore, MemSpentOpsStore, SpentOpsStore};
pub use relayer::{
    BridgeAction, BridgeRelayer, BridgeRequest, BridgeRequestType,
    ZBX_CHAIN_ID_MAINNET, ZBX_CHAIN_ID_TESTNET,
};
pub use token::{BridgeToken, DailyLimitTracker, TokenWhitelist, NATIVE_ZBX_SENTINEL};

/// Minimum bridge amount for native ZBX: 1 ZBX.
pub const MIN_BRIDGE_AMOUNT: u128 = 1 * 10u128.pow(18);
/// Bridge fee: 0.1% (10 bps) — applies to all tokens.
pub const BRIDGE_FEE_BPS: u128 = 10;
/// Required multisig confirmations (3-of-5).
pub const MULTISIG_THRESHOLD: usize = 3;
pub const MULTISIG_SIZE: usize = 5;
/// Pending request TTL: 24 hours.
pub const REQUEST_EXPIRY_SECS: u64 = 86_400;

/// Required finality confirmations before trusting a foreign-chain event.
pub const CONFIRMATIONS_ETH:     u64 = 12;
pub const CONFIRMATIONS_BSC:     u64 = 20;
pub const CONFIRMATIONS_POLYGON: u64 = 128;

/// Supported target chains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChainId {
    Ethereum = 1,
    BSC      = 56,
    Polygon  = 137,
}

impl ChainId {
    pub fn name(&self) -> &'static str {
        match self {
            ChainId::Ethereum => "Ethereum",
            ChainId::BSC      => "BSC",
            ChainId::Polygon  => "Polygon",
        }
    }

    /// Required block confirmations before trusting this chain's events.
    pub fn required_confirmations(&self) -> u64 {
        match self {
            ChainId::Ethereum => CONFIRMATIONS_ETH,
            ChainId::BSC      => CONFIRMATIONS_BSC,
            ChainId::Polygon  => CONFIRMATIONS_POLYGON,
        }
    }

    /// L-8: RPC endpoint for this chain.
    ///
    /// Returns the URL from environment variable, falling back to the
    /// well-known public endpoint for the chain. In production, always
    /// set ZBX_ETH_RPC_URL / ZBX_BSC_RPC_URL / ZBX_POLYGON_RPC_URL to
    /// a private or authenticated endpoint — public endpoints have rate
    /// limits and may lag.
    pub fn rpc_url(&self) -> String {
        match self {
            ChainId::Ethereum => std::env::var("ZBX_ETH_RPC_URL")
                .unwrap_or_else(|_| "https://ethereum.publicnode.com".into()),
            ChainId::BSC => std::env::var("ZBX_BSC_RPC_URL")
                .unwrap_or_else(|_| "https://bsc-dataseed.binance.org".into()),
            ChainId::Polygon => std::env::var("ZBX_POLYGON_RPC_URL")
                .unwrap_or_else(|_| "https://polygon-rpc.com".into()),
        }
    }
}

impl TryFrom<u64> for ChainId {
    type Error = BridgeError;

    fn try_from(id: u64) -> Result<Self, Self::Error> {
        match id {
            1   => Ok(ChainId::Ethereum),
            56  => Ok(ChainId::BSC),
            137 => Ok(ChainId::Polygon),
            _   => Err(BridgeError::UnsupportedChain(id)),
        }
    }
}
