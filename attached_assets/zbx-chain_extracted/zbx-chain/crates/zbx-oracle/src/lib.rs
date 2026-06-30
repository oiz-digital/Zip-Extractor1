//! ZBX Decentralized Price Oracle (ZEP-011).
//!
//! # Architecture
//!
//! Similar to Chainlink's decentralized oracle network, but native to ZBX:
//!
//! ```
//!                    ┌──────────────────────────────────────┐
//!                    │          ZBX Oracle Network          │
//!                    │                                      │
//!  ┌──────────┐      │  ┌─────────┐  ┌─────────┐           │
//!  │ Binance  │─────►│  │ Node 1  │  │ Node 2  │  ...      │
//!  │ Coinbase │─────►│  │(reporter│  │(reporter│           │
//!  │ Kraken   │─────►│  │)        │  │)        │           │
//!  └──────────┘      │  └────┬────┘  └────┬────┘           │
//!                    │       │             │                │
//!                    │  ┌────▼─────────────▼────┐           │
//!                    │  │   Aggregator (median)  │           │
//!                    │  └────────────┬───────────┘           │
//!                    └──────────────┼───────────────────────┘
//!                                   │
//!                    ┌──────────────▼───────────────────────┐
//!                    │      ZbxOracle.sol (on-chain)         │
//!                    │  latestRoundData() → price, timestamp │
//!                    └──────────────────────────────────────┘
//! ```
//!
//! # Supported Price Feeds
//!
//! | Feed       | Decimals | Update Threshold | Heartbeat | Sources |
//! |:---|:---|:---|:---|:---|
//! | ZBX/USD    | 8        | 0.5%             | 1 hour    | Binance, Coinbase, Kraken, OKX |
//! | ZUSD/USD   | 8        | 0.1%             | 30 min    | Binance, Coinbase, Kraken |
//! | ETH/USD    | 8        | 0.5%             | 1 hour    | Binance, Coinbase, Kraken |
//! | BTC/USD    | 8        | 0.5%             | 1 hour    | Binance, Coinbase, Kraken |
//! | BNB/USD    | 8        | 0.5%             | 1 hour    | Binance, Coinbase, Kraken |
//! | **USD/INR**  | 8      | 0.2%             | 1 hour    | RBI, ExchangeRate-API, Fixer.io, WazirX, CoinDCX |
//!
//! # Reporter Incentives
//!
//! Approved reporters earn oracle fees per round:
//!   - ZBX/USD: 0.001 ZBX/round (paid from oracle treasury)
//!   - Slashing: reporter slashed if deviation > 5× median for 3 rounds
//!
//! # Chainlink Compatibility
//!
//! ZbxAggregatorV3.sol implements the standard AggregatorV3Interface.
//! Any contract written for Chainlink price feeds works on ZBX without changes.

pub mod feed;
pub mod aggregator;
pub mod aggregator_reader;
pub mod reporter;
pub mod round;
pub mod fetcher;
pub mod scheduler;
pub mod error;
pub mod inr_fetcher;
// ── Session 40: Advanced oracle modules ──────────────────────────────────────
pub mod twap;
pub mod circuit_breaker;
pub mod multi_chain;
pub mod dex_fetcher;
pub mod slasher;
pub mod heartbeat;
pub mod proof;

pub use feed::{PriceFeed, FeedId, Price, DECIMALS};
pub use aggregator::{OracleAggregator, AggregateResult};
pub use reporter::{OracleReporter, PriceReport};
pub use round::{OracleRound, RoundId};
pub use error::OracleError;
pub use inr_fetcher::{
    InrPrice,
    fetch_usd_inr_vwap,
    usd_inr_cache_age_secs,
    usd_inr_cached_price,
    MAX_CACHE_AGE_SECS,
};
pub use twap::{
    TwapAccumulator, TwapRegistry, TwapResult, PriceObservation,
    TWAP_5MIN, TWAP_30MIN, TWAP_2H, TWAP_24H,
    MAX_OBSERVATIONS, MIN_TWAP_OBSERVATIONS,
};
pub use circuit_breaker::{
    CircuitBreaker, BreakerRegistry, BreakerConfig, BreakerState, TripReason,
    DEFAULT_MAX_VELOCITY_PCT, STABLECOIN_MAX_VELOCITY_PCT, COOLDOWN_SECS,
};
pub use multi_chain::{
    MultiChainRegistry, NetworkDescriptor, NetworkPrice, RelayMessage,
    ChainId, FinalityModel,
    CHAIN_ZBX_MAINNET, CHAIN_ZBX_TESTNET, CHAIN_ETHEREUM, CHAIN_BSC,
    CHAIN_POLYGON, CHAIN_ARBITRUM, CHAIN_OPTIMISM, CHAIN_AVALANCHE,
    ALL_CHAIN_IDS,
};
pub use dex_fetcher::{
    DexProtocol, DexPool, DexPrice, FeeTier,
    aggregate_dex_prices, fetch_dex_price_aggregate,
    sqrt_price_x96_to_price,
};
pub use slasher::{
    OracleSlasher, OracleSlashEvent, SlashSeverity, ReporterSlashState,
    SLASH_THRESHOLD, APPEAL_WINDOW_BLOCKS,
    SLASH_BPS_MINOR, SLASH_BPS_MAJOR, SLASH_BPS_CRITICAL,
};
pub use heartbeat::{
    HeartbeatMonitor, FeedHealth, HeartbeatAlert, HeartbeatAlertLevel,
    HEARTBEAT_ZUSD_USD, HEARTBEAT_STD, HEARTBEAT_MEDIUM, HEARTBEAT_LOW,
    HEARTBEAT_GRACE_SECS,
};
pub use proof::{
    PriceEntry, PriceProof, MerkleDir,
    OraclePriceCommitment, CommitmentRegistry,
    hash_pair,
};

// ── High-level runner (ZEP-011 node wiring) ───────────────────────────────────
pub mod service;
pub use service::OracleScheduler;